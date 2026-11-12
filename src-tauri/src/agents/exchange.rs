//! Deliverable exchange area for sub-agent missions.
//!
//! Every planner-dispatched mission (and every direct `call_subagent` call)
//! gets one batch directory under `<workspace>/.data/tmp`:
//!
//! ```text
//! <workspace>/.data/tmp/<batch-id>/
//!   shared/          # files every task may read and write
//!   t1/              # deliverables produced by task t1
//!   t2/
//! ```
//!
//! The directory is intentionally *outside* the task's working directory so a
//! producer and its consumers can exchange large artifacts without pasting
//! them into prompts. It is permanent (the user cleans it manually) and is not
//! part of the profile backup.
//!
//! The top level of `.data/tmp` is shared with `pdf/` (the PDF mention cache) and
//! with composer pastes, which land there as plain files under the file's own
//! name (`@@<name>` in a message). The paste sweep only deletes *files* that sit
//! directly in the folder and are older than a day, so a batch id — always a
//! directory, `<stamp>-<8hex>` — can neither collide with those names nor be
//! swept as an expired paste.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// Files listed per task in a mission report / tool result. The cap keeps a
/// runaway build directory from turning the report into a file tree.
const MAX_LISTED_FILES: usize = 50;
/// Directory levels walked when listing deliverables.
const MAX_LIST_DEPTH: usize = 3;
/// Directory names that are never descended into when listing deliverables.
const SKIPPED_DIRS: [&str; 8] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    "__pycache__",
    ".venv",
    "venv",
    "build",
];

/// One file inside a task's exchange directory.
#[derive(Debug, Clone)]
pub struct ExchangeFile {
    /// Path relative to the task directory, always with `/` separators.
    pub relative: String,
    pub size: u64,
}

/// One mission's deliverable exchange area.
#[derive(Debug, Clone)]
pub struct ExchangeArea {
    root: PathBuf,
    workspace_dir: PathBuf,
    task_dirs: HashMap<String, PathBuf>,
}

impl ExchangeArea {
    /// A fresh, unique batch root under `<workspace>/.data/tmp`. The directory
    /// is not created until `create_at` runs, so a declined plan leaves no
    /// empty folder behind.
    pub fn new_batch_root(workspace_dir: &Path) -> PathBuf {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let short = Uuid::new_v4().simple().to_string();
        crate::workspace::tmp_root(workspace_dir).join(format!("{stamp}-{}", &short[..8]))
    }

    /// Creates a batch area for `task_ids` with a generated root.
    pub fn create(workspace_dir: &Path, task_ids: &[String]) -> Result<Self, String> {
        Self::create_at(Self::new_batch_root(workspace_dir), workspace_dir, task_ids)
    }

    /// Creates the shared directory and one directory per task. Task ids are
    /// sanitized into filesystem-safe names; colliding sanitized names get a
    /// numeric suffix so two tasks can never share a directory.
    pub fn create_at(
        root: PathBuf,
        workspace_dir: &Path,
        task_ids: &[String],
    ) -> Result<Self, String> {
        let shared = root.join("shared");
        fs::create_dir_all(&shared).map_err(|err| {
            format!(
                "could not create the deliverable exchange directory '{}': {err}",
                shared.display()
            )
        })?;

        let mut task_dirs: HashMap<String, PathBuf> = HashMap::new();
        let mut used: HashSet<String> = HashSet::new();
        for task_id in task_ids {
            let base = safe_component(task_id);
            let mut name = base.clone();
            let mut suffix = 1usize;
            while !used.insert(name.clone()) {
                suffix += 1;
                name = format!("{base}-{suffix}");
            }
            let dir = root.join(&name);
            fs::create_dir_all(&dir).map_err(|err| {
                format!(
                    "could not create the task deliverable directory '{}': {err}",
                    dir.display()
                )
            })?;
            task_dirs.insert(task_id.clone(), dir);
        }

        Ok(Self {
            root,
            workspace_dir: workspace_dir.to_path_buf(),
            task_dirs,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn shared_dir(&self) -> PathBuf {
        self.root.join("shared")
    }

    pub fn task_dir(&self, task_id: &str) -> Option<&Path> {
        self.task_dirs.get(task_id).map(PathBuf::as_path)
    }

    /// Workspace-relative form of the batch root (`.data/tmp/<batch-id>`),
    /// which is what `file_actions` paths use.
    pub fn relative_root(&self) -> String {
        self.root
            .strip_prefix(&self.workspace_dir)
            .unwrap_or(&self.root)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Prompt block injected into a sub-agent's static system prompt. The
    /// static prompt is re-sent on every request and survives context
    /// compression, so the paths stay authoritative for the whole task.
    pub fn prompt_block(&self, task_id: &str, can_write_files: bool) -> String {
        let task_dir = self
            .task_dir(task_id)
            .map(|path| path.to_path_buf())
            .unwrap_or_else(|| self.root.join(safe_component(task_id)));
        let mut siblings: Vec<String> = self
            .task_dirs
            .iter()
            .filter(|(id, _)| id.as_str() != task_id)
            .filter_map(|(_, path)| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .collect();
        siblings.sort();

        let sibling_line = if siblings.is_empty() {
            String::new()
        } else {
            format!(
                "Other task directories (read a dependency's output here): {}\n",
                siblings
                    .iter()
                    .map(|name| format!("{name}/"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let write_rule = if can_write_files {
            "Write large deliverables (reports, datasets, patches, generated files) into your \
             task directory; use the shared directory for inputs/outputs meant for every task. \
             Reference files by path in your answer instead of pasting their contents.\n"
        } else {
            "You have no file or shell tool, so you cannot write to this area. Return the \
             deliverable inline and mention that it was not written to the exchange area.\n"
        };

        format!(
            "\n\nDELIVERABLE EXCHANGE AREA (this mission):\n\
             Root: {} (workspace-relative: {})\n\
             Your task directory: {}\n\
             Shared directory: {}\n\
             {}\
             {}\
             Do not write deliverables outside this area (the workspace itself is still writable \
             for code changes).",
            self.root.display(),
            self.relative_root(),
            task_dir.display(),
            self.shared_dir().display(),
            sibling_line,
            write_rule,
        )
    }

    /// Files written by one task, newest listing order (sorted by path).
    pub fn list_files(&self, task_id: &str) -> Vec<ExchangeFile> {
        let Some(dir) = self.task_dir(task_id) else {
            return Vec::new();
        };
        let mut files = Vec::new();
        walk_files(dir, dir, 0, &mut files);
        files.sort_by(|a, b| a.relative.cmp(&b.relative));
        files
    }
}

fn walk_files(dir: &Path, base: &Path, depth: usize, out: &mut Vec<ExchangeFile>) {
    if out.len() >= MAX_LISTED_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if out.len() >= MAX_LISTED_FILES {
            break;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        // Never follow symlinks: a link could point outside the exchange area
        // (or create a cycle) and make the listing lie about what is there.
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if depth >= MAX_LIST_DEPTH {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if SKIPPED_DIRS.iter().any(|skip| *skip == name) {
                continue;
            }
            walk_files(&path, base, depth + 1, out);
        } else if meta.is_file() {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(ExchangeFile {
                relative,
                size: meta.len(),
            });
        }
    }
}

/// A task id is planner/agent-controlled, so it must never become a path
/// component verbatim. Anything outside the safe set becomes `_`; empty and
/// dot-only names fall back to `task`.
fn safe_component(value: &str) -> String {
    let cleaned: String = value
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned: String = cleaned.chars().take(64).collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "task".to_string()
    } else {
        cleaned
    }
}

/// Renders a compact, size-aware listing for a mission report or tool result.
pub fn render_files(files: &[ExchangeFile]) -> String {
    if files.is_empty() {
        return "No files were written to the exchange area.".to_string();
    }
    let mut text = String::new();
    for file in files {
        text.push_str(&format!(
            "- {} ({})\n",
            file.relative,
            format_size(file.size)
        ));
    }
    if files.len() >= MAX_LISTED_FILES {
        text.push_str("- … more files not listed\n");
    }
    text
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let value = bytes as f64;
    if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-exchange-{tag}-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn creates_shared_and_per_task_directories_under_tmp() {
        let workspace = temp_workspace("layout");
        let area = ExchangeArea::create(&workspace, &["t1".to_string(), "t2".to_string()]).unwrap();

        assert!(area
            .root()
            .starts_with(crate::workspace::tmp_root(&workspace)));
        assert!(area.shared_dir().is_dir());
        assert!(area.task_dir("t1").unwrap().is_dir());
        assert!(area.task_dir("t2").unwrap().is_dir());
        assert!(area.relative_root().starts_with(".data/tmp/"));

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn sanitizes_task_ids_and_keeps_them_unique() {
        let workspace = temp_workspace("safe");
        let area = ExchangeArea::create(
            &workspace,
            &[
                "../evil".to_string(),
                "..\\evil".to_string(),
                ".".to_string(),
            ],
        )
        .unwrap();

        for id in ["../evil", "..\\evil", "."] {
            let dir = area.task_dir(id).expect("task directory");
            assert!(dir.starts_with(area.root()), "{dir:?}");
            assert!(
                !dir.components()
                    .any(|component| matches!(component, std::path::Component::ParentDir)),
                "path escapes the exchange root: {dir:?}"
            );
        }
        let dirs: HashSet<PathBuf> = area.task_dirs.values().map(|path| path.clone()).collect();
        assert_eq!(dirs.len(), 3, "every task must own a distinct directory");

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn lists_files_and_skips_heavy_directories() {
        let workspace = temp_workspace("list");
        let area = ExchangeArea::create(&workspace, &["t1".to_string()]).unwrap();
        let task_dir = area.task_dir("t1").unwrap();
        fs::write(task_dir.join("report.md"), b"hello").unwrap();
        fs::create_dir_all(task_dir.join("node_modules/pkg")).unwrap();
        fs::write(task_dir.join("node_modules/pkg/index.js"), b"x").unwrap();
        fs::create_dir_all(task_dir.join("nested")).unwrap();
        fs::write(task_dir.join("nested/data.csv"), b"a,b").unwrap();

        let files = area.list_files("t1");
        let names: Vec<&str> = files.iter().map(|file| file.relative.as_str()).collect();
        assert!(names.contains(&"report.md"));
        assert!(names.contains(&"nested/data.csv"));
        assert!(!names.iter().any(|name| name.contains("node_modules")));

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn prompt_block_contains_paths_and_sibling_directories() {
        let workspace = temp_workspace("prompt");
        let area = ExchangeArea::create(&workspace, &["t1".to_string(), "t2".to_string()]).unwrap();
        let block = area.prompt_block("t1", true);
        assert!(block.contains("DELIVERABLE EXCHANGE AREA"));
        assert!(block.contains(&area.relative_root()));
        assert!(block.contains("t2/"));
        assert!(block.contains("Shared directory"));

        fs::remove_dir_all(&workspace).ok();
    }
}
