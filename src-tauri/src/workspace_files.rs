//! Workspace file listing and reading for the composer's `@file` mentions.
//!
//! The chat input box offers an inline file picker: typing `@` opens a popup
//! that filters workspace files by name, and a mention may carry a line range
//! (`@src/App.tsx#12`, `@src/App.tsx#12-24`, `@src/App.tsx#12-`). The webview
//! has no filesystem access of its own, so these three commands expose exactly
//! what that picker needs:
//!
//! * [`list_workspace_files`] — relative paths of the files the model could be
//!   shown (hidden/gitignore aware, capped);
//! * [`read_workspace_file_range`] — a text file, optionally sliced to a line
//!   range;
//! * [`read_workspace_file_bytes`] — base64 payload for image/binary mentions,
//!   which are attached whole (a line range is meaningless there).
//!
//! Every path is resolved against the workspace root and must stay inside it:
//! the UI can only *name* a path, so this check is the only thing between a
//! typo (or `../`) and reading arbitrary files.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ignore::WalkBuilder;
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use tauri::State;

use crate::AppState;

/// Cap on the popup's file list. A larger tree is not navigable in a keyboard
/// popup anyway, and the walk would stall the command.
const MAX_LISTED_FILES: usize = 20_000;

/// Directories never offered as `@` mentions: VCS metadata, build output and
/// this application's own state (`.data` holds the database, skills, scripts).
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    ".data",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
];

/// Largest text file offered whole or sliced (8 MiB).
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

/// Largest binary file attached through a mention (24 MiB before base64).
const MAX_BINARY_BYTES: u64 = 24 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceFileEntry {
    /// Workspace-relative path with `/` separators.
    pub path: String,
    /// File name (basename) — the "extern name" the popup filters on.
    pub name: String,
    /// Lowercase extension without the dot (`""` when there is none).
    pub ext: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceFileRange {
    pub text: String,
    pub total_lines: u64,
    /// 1-based first line held by `text`.
    pub start_line: u64,
    /// 1-based last line held by `text`.
    pub end_line: u64,
    /// The requested range ran past the end of the file and was clamped.
    pub truncated: bool,
}

fn workspace_root(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .workspace_dir
        .lock()
        .map(|dir| dir.clone())
        .map_err(|_| "workspace directory lock poisoned".to_string())
}

/// Resolve a UI-provided relative path, refusing anything outside the
/// workspace or inside the app's own `.data` directory.
fn resolve_in_workspace(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let trimmed = relative.trim();
    if trimmed.is_empty() {
        return Err("empty path".to_string());
    }
    let rel = Path::new(trimmed);
    // Only plain components: rejects absolute paths, `..`, drive prefixes and
    // `.`-padded tricks before anything touches the filesystem.
    if !rel
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("path escapes the workspace: {trimmed}"));
    }
    if rel
        .components()
        .next()
        .map(|c| c.as_os_str() == ".data")
        .unwrap_or(false)
    {
        return Err("the .data directory is not readable from the composer".to_string());
    }

    let canonical = root
        .join(rel)
        .canonicalize()
        .map_err(|e| format!("{trimmed}: {e}"))?;
    let canonical_root = root.canonicalize().map_err(|e| format!("workspace: {e}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(format!("path escapes the workspace: {trimmed}"));
    }
    Ok(canonical)
}

/// Slice `text` to the requested 1-based line range.
///
/// * `start = None` → from line 1
/// * `end = None` → to the last line (`#12-`)
/// * `start = end` → a single line (`#12`)
fn slice_lines(
    text: &str,
    start: Option<u64>,
    end: Option<u64>,
) -> Result<WorkspaceFileRange, String> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len() as u64;
    if total == 0 {
        return Err("file is empty".to_string());
    }

    let start_line = start.unwrap_or(1).max(1);
    if start_line > total {
        return Err(format!(
            "line {start_line} is past the end of the file ({total} lines)"
        ));
    }
    let requested_end = end.unwrap_or(total);
    let truncated = requested_end > total;
    let end_line = requested_end.min(total).max(start_line);

    let text = lines[(start_line - 1) as usize..end_line as usize].join("\n");
    Ok(WorkspaceFileRange {
        text,
        total_lines: total,
        start_line,
        end_line,
        truncated,
    })
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn extension_of_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Relative path with `/` separators, so the frontend never has to care about
/// the platform separator.
fn relative_path_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|dir| name.eq_ignore_ascii_case(dir))
}

/// List the workspace's files for the `@` mention popup.
///
/// Hidden files and everything `.gitignore` excludes are skipped, as are the
/// directories in [`SKIP_DIRS`]. Sorted by path so the popup order is stable.
#[tauri::command]
pub async fn list_workspace_files(
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceFileEntry>, String> {
    let root = workspace_root(&state)?;
    if !root.is_dir() {
        return Err(format!("workspace is not a directory: {}", root.display()));
    }

    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(true)
        .follow_links(false)
        .filter_entry(|entry| {
            entry.depth() == 0 || !is_skipped_dir(&entry.file_name().to_string_lossy())
        });

    let mut files = Vec::new();
    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            // Unreadable directories are not a reason to fail the popup.
            Err(_) => continue,
        };
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let Some(path) = relative_path_of(&root, entry.path()) else {
            continue;
        };
        let name = file_name_of(entry.path());
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        files.push(WorkspaceFileEntry {
            ext: extension_of_name(&name),
            path,
            name,
            size,
        });
        if files.len() >= MAX_LISTED_FILES {
            break;
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Read a text file, optionally sliced to a 1-based line range.
#[tauri::command]
pub async fn read_workspace_file_range(
    state: State<'_, AppState>,
    path: String,
    start_line: Option<u64>,
    end_line: Option<u64>,
) -> Result<WorkspaceFileRange, String> {
    let root = workspace_root(&state)?;
    let file = resolve_in_workspace(&root, &path)?;
    let meta = std::fs::metadata(&file).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("not a file: {path}"));
    }
    if meta.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "{path} is too large to attach ({} bytes, limit {MAX_TEXT_BYTES})",
            meta.len()
        ));
    }

    let bytes = std::fs::read(&file).map_err(|e| format!("{path}: {e}"))?;
    let text = String::from_utf8_lossy(&bytes);
    slice_lines(&text, start_line, end_line).map_err(|e| format!("{path}: {e}"))
}

/// Read an image/binary file as a base64 payload (the UI builds the `data:`
/// URL from the MIME type it derived from the extension).
#[tauri::command]
pub async fn read_workspace_file_bytes(
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let root = workspace_root(&state)?;
    let file = resolve_in_workspace(&root, &path)?;
    let meta = std::fs::metadata(&file).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("not a file: {path}"));
    }
    if meta.len() > MAX_BINARY_BYTES {
        return Err(format!(
            "{path} is too large to attach ({} bytes, limit {MAX_BINARY_BYTES})",
            meta.len()
        ));
    }

    let bytes = std::fs::read(&file).map_err(|e| format!("{path}: {e}"))?;
    Ok(STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(text: &str, start: Option<u64>, end: Option<u64>) -> WorkspaceFileRange {
        slice_lines(text, start, end).expect("range should be valid")
    }

    #[test]
    fn slices_a_line_range() {
        let text = "a\nb\nc\nd\ne\n";
        let got = range(text, Some(2), Some(4));
        assert_eq!(got.text, "b\nc\nd");
        assert_eq!((got.start_line, got.end_line, got.total_lines), (2, 4, 5));
        assert!(!got.truncated);
    }

    #[test]
    fn single_line_and_open_end() {
        assert_eq!(range("a\nb\nc", Some(2), Some(2)).text, "b");
        let to_eof = range("a\nb\nc", Some(2), None);
        assert_eq!(to_eof.text, "b\nc");
        assert_eq!(to_eof.end_line, 3);
    }

    #[test]
    fn clamps_a_range_past_the_end() {
        let got = range("a\nb\nc", Some(2), Some(99));
        assert_eq!((got.start_line, got.end_line), (2, 3));
        assert!(got.truncated);
    }

    #[test]
    fn rejects_a_start_past_the_end() {
        assert!(slice_lines("a\nb", Some(5), None).is_err());
    }

    #[test]
    fn rejects_paths_outside_the_workspace() {
        let root = std::env::current_dir().expect("cwd");
        assert!(resolve_in_workspace(&root, "../outside.txt").is_err());
        assert!(resolve_in_workspace(&root, "/etc/passwd").is_err());
        assert!(resolve_in_workspace(&root, ".data/chat.db").is_err());
        assert!(resolve_in_workspace(&root, "").is_err());
        // A file that really is inside the crate root still resolves.
        assert!(resolve_in_workspace(&root, "Cargo.toml").is_ok());
    }
}
