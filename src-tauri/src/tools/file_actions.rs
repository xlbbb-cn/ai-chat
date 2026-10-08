// `file_actions` tool: read/write/list/search/mkdir/patch/diff/rename/move/
// delete on workspace files, plus the backup, move and diff helpers only this
// tool needs.

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

use super::patch::apply_unified_patch;
use super::paths::{ensure_mutation_target_allowed, resolve_safe_path, with_root_header};
use super::request_tool_confirmation;
use super::search::{
    is_probably_binary, run_integrated_search, SearchOptions, SEARCH_MAX_MATCHES,
};
use super::truncate_tool_output;

pub(super) async fn handle_file_actions(
    app: &AppHandle,
    args: &Value,
    workspace_dir: &Path,
    protected_skill_roots: &[PathBuf],
) -> String {
    let action = args["action"].as_str().unwrap_or("");
    let path_str = args["path"].as_str().unwrap_or("");
    let root_dir = workspace_dir.to_path_buf();
    match action {
        "read" => {
            let _ = app.emit("tool-call", format!("📄 *Reading {}*\n\n", path_str));
            let start_line = args["start_line"].as_i64();
            let end_line = args["end_line"].as_i64();
            match resolve_file_action_path(app, action, path_str, &root_dir, true).await {
                Ok(p) => {
                    match fs::metadata(&p) {
                        Ok(metadata) if metadata.is_dir() => {
                            return format!("Error: {} is a directory", path_str);
                        }
                        Err(e) => {
                            return format!("Error reading file metadata: {}", e);
                        }
                        _ => {}
                    }

                    if start_line.is_none() && end_line.is_none() {
                        return fs::read_to_string(&p)
                            .map(truncate_tool_output)
                            .unwrap_or_else(|e| format!("Error reading file: {}", e));
                    }

                    let start = start_line.unwrap_or(1);
                    let end = end_line.unwrap_or(i64::MAX);
                    if start <= 0 || end < start {
                        return "Error: invalid line range. start_line must be >= 1 and end_line must be >= start_line.".to_string();
                    }

                    match fs::File::open(&p) {
                        Ok(file) => {
                            let reader = BufReader::new(file);
                            let mut result = String::new();
                            for (index, line) in reader.lines().enumerate() {
                                let line_num = (index + 1) as i64;
                                if line_num < start {
                                    continue;
                                }
                                if line_num > end {
                                    break;
                                }
                                match line {
                                    Ok(text) => {
                                        if !result.is_empty() {
                                            result.push('\n');
                                        }
                                        result.push_str(&text);
                                    }
                                    Err(e) => {
                                        return format!("Error reading file: {}", e);
                                    }
                                }
                            }
                            if result.is_empty() {
                                "(no matching lines)".to_string()
                            } else {
                                result
                            }
                        }
                        Err(e) => format!("Error opening file: {}", e),
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        "write" => {
            let content_str = args["content"].as_str().unwrap_or("");
            let _ = app.emit("tool-call", format!("💾 *Writing {}*\n\n", path_str));
            match resolve_file_action_path(app, action, path_str, &root_dir, false).await {
                Ok(p) => {
                    if let Err(e) =
                        ensure_mutation_target_allowed(&p, &root_dir, protected_skill_roots)
                    {
                        return format_file_action_error(e);
                    }
                    let backup = match backup_protected_file(&p, protected_skill_roots) {
                        Ok(backup) => backup,
                        Err(e) => return format!("Error: {}", e),
                    };
                    if let Some(parent) = p.parent() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            return format!(
                                "Error creating parent directory '{}': {}",
                                parent.display(),
                                e
                            );
                        }
                    }
                    match fs::write(&p, content_str) {
                        Ok(_) => {
                            if let Some(backup) = backup {
                                format!(
                                    "Successfully backed up to {} and wrote to {}",
                                    backup.display(),
                                    path_str
                                )
                            } else {
                                format!("Successfully wrote to {}", path_str)
                            }
                        }
                        Err(e) => format!("Error writing file: {}", e),
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        "list" => {
            let _ = app.emit("tool-call", format!("📂 *Listing {}*\n\n", path_str));
            match resolve_file_action_path(app, action, path_str, &root_dir, true).await {
                Ok(p) => {
                    match fs::metadata(&p) {
                        Ok(metadata) => {
                            if metadata.is_file() {
                                let name = p
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(path_str)
                                    .to_string();
                                return with_root_header(format!(
                                    "{} ({} bytes)",
                                    name,
                                    metadata.len()
                                ));
                            }
                        }
                        Err(_) => {}
                    }

                    match fs::read_dir(&p) {
                        Ok(entries) => {
                            let mut res = Vec::new();
                            for entry in entries.flatten() {
                                if let Ok(name) = entry.file_name().into_string() {
                                    let is_dir = entry
                                        .file_type()
                                        .map(|t| t.is_dir())
                                        .unwrap_or(false);
                                    if is_dir {
                                        res.push(format!("{}/", name));
                                    } else {
                                        let size =
                                            entry.metadata().map(|m| m.len()).unwrap_or(0);
                                        res.push(format!("{} ({} bytes)", name, size));
                                    }
                                }
                            }
                            let body = if res.is_empty() {
                                "(empty directory)".to_string()
                            } else {
                                res.join("\n")
                            };
                            with_root_header(body)
                        }
                        Err(e) => format!("Error listing directory: {}", e),
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        "search" => {
            let query = args["query"].as_str().unwrap_or("").to_string();
            let options = SearchOptions {
                recursive: args["recursive"].as_bool().unwrap_or(true),
                case_sensitive: args["case_sensitive"].as_bool().unwrap_or(false),
                smart_case: args["smart_case"].as_bool().unwrap_or(false),
                use_regex: args["use_regex"].as_bool().unwrap_or(false),
                include_hidden: args["include_hidden"].as_bool().unwrap_or(true),
                respect_gitignore: args["respect_gitignore"].as_bool().unwrap_or(false),
                glob: args["glob"].as_str().map(str::to_string),
                max_matches: args["max_results"]
                    .as_u64()
                    .map(|value| value.clamp(1, 20_000) as usize)
                    .unwrap_or(SEARCH_MAX_MATCHES),
            };

            if query.is_empty() {
                return "Error: query is required for search.".to_string();
            }

            let _ = app.emit(
                "tool-call",
                format!(
                    "🔎 *Searching {} for {}*\n\n",
                    path_str,
                    serde_json::to_string(&query).unwrap_or_else(|_| query.to_string())
                ),
            );

            match resolve_file_action_path(app, action, path_str, &root_dir, true).await {
                Ok(p) => {
                    // Fan-out over a large tree plus per-file IO is
                    // long-running blocking work — keep it off the
                    // async runtime so streaming keeps flowing.
                    let search_root = root_dir.clone();
                    let outcome = tauri::async_runtime::spawn_blocking(move || {
                        run_integrated_search(&query, &p, &search_root, &options)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("Search task failed: {}", e)));

                    match outcome {
                        Ok(output) => with_root_header(output),
                        Err(e) => format!("Error: {}", e),
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        "rename" | "move" => {
            let new_path = args["new_path"].as_str().unwrap_or("");
            let _ = app.emit(
                "tool-call",
                format!("🔁 *Moving {} -> {}*\n\n", path_str, new_path),
            );
            if new_path.is_empty() {
                return "Error: new_path is required for move/rename.".to_string();
            }
            match resolve_file_action_path(app, action, path_str, &root_dir, true).await {
                Ok(src) => {
                    if let Err(e) =
                        ensure_mutation_target_allowed(&src, &root_dir, protected_skill_roots)
                    {
                        return format_file_action_error(e);
                    }
                    match resolve_file_action_path(app, action, new_path, &root_dir, false).await {
                        Ok(dst) => {
                            if let Err(e) = ensure_mutation_target_allowed(
                                &dst,
                                &root_dir,
                                protected_skill_roots,
                            ) {
                                return format_file_action_error(e);
                            }
                            let backup = match backup_protected_file(&src, protected_skill_roots) {
                                Ok(backup) => backup,
                                Err(e) => return format!("Error: {}", e),
                            };
                            // If the destination already exists and is a
                            // protected file, back it up before overwriting.
                            let dst_backup =
                                match backup_protected_file(&dst, protected_skill_roots) {
                                    Ok(backup) => backup,
                                    Err(e) => return format!("Error: {}", e),
                                };
                            if let Some(parent) = dst.parent() {
                                if let Err(e) = fs::create_dir_all(parent) {
                                    return format!("Error creating destination directory: {}", e);
                                }
                            }
                            match move_path(&src, &dst) {
                                Ok(_) => {
                                    let backup_note = match (backup, dst_backup) {
                                        (Some(b), _) => {
                                            format!(" (source backed up to {})", b.display())
                                        }
                                        (None, Some(b)) => format!(
                                            " (destination backed up to {})",
                                            b.display()
                                        ),
                                        (None, None) => String::new(),
                                    };
                                    format!(
                                        "Successfully moved {} to {}{}",
                                        path_str, new_path, backup_note
                                    )
                                }
                                Err(e) => format!(
                                    "Error moving {} to {}: {}",
                                    path_str, new_path, e
                                ),
                            }
                        }
                        Err(e) => format_file_action_error(e),
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        "patch" => {
            // The model may send the diff as one string or as an array
            // of chunks; join chunks so both shapes work.
            let patch_str = match args["patch"].as_str() {
                Some(value) => value.to_string(),
                None => args["patch"]
                    .as_array()
                    .map(|chunks| {
                        chunks
                            .iter()
                            .filter_map(|chunk| chunk.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
            };
            let _ = app.emit("tool-call", format!("🩹 *Patching {}*\n\n", path_str));
            match resolve_file_action_path(app, action, path_str, &root_dir, true).await {
                Ok(p) => {
                    if let Err(e) =
                        ensure_mutation_target_allowed(&p, &root_dir, protected_skill_roots)
                    {
                        return format_file_action_error(e);
                    }
                    let backup = match backup_protected_file(&p, protected_skill_roots) {
                        Ok(backup) => backup,
                        Err(e) => return format!("Error: {}", e),
                    };
                    match fs::read_to_string(&p) {
                        Ok(original) => match apply_unified_patch(&original, &patch_str) {
                            Ok((patched, notes)) => match fs::write(&p, &patched) {
                                Ok(_) => {
                                    let notes = if notes.is_empty() {
                                        String::new()
                                    } else {
                                        format!(
                                            "\n\nPatch notes:\n{}",
                                            notes
                                                .iter()
                                                .map(|note| format!("- {note}"))
                                                .collect::<Vec<_>>()
                                                .join("\n")
                                        )
                                    };
                                    if let Some(backup) = backup {
                                        format!(
                                            "Successfully backed up to {} and patched {}{}",
                                            backup.display(),
                                            path_str,
                                            notes
                                        )
                                    } else {
                                        format!("Successfully patched {}{}", path_str, notes)
                                    }
                                }
                                Err(e) => format!("Error writing patched file: {}", e),
                            },
                            Err(report) => format!(
                                "⛔ Error applying patch to {}:\n{}",
                                path_str, report
                            ),
                        },
                        Err(e) => format!("Error reading file: {}", e),
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        "diff" => {
            let new_path = args["new_path"].as_str().unwrap_or("");
            if new_path.is_empty() {
                return "Error: new_path is required for diff (the file to compare against)."
                    .to_string();
            }
            let context_lines = args["context_lines"]
                .as_u64()
                .map(|value| value.clamp(0, 20) as usize)
                .unwrap_or(3);
            let ignore_whitespace = args["ignore_whitespace"].as_bool().unwrap_or(false);
            let _ = app.emit(
                "tool-call",
                format!("🔀 *Comparing {} → {}*\n\n", path_str, new_path),
            );

            let original_path = match resolve_file_action_path(
                app, action, path_str, &root_dir, true,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return format_file_action_error(e),
            };
            let modified_path = match resolve_file_action_path(
                app, action, new_path, &root_dir, true,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return format_file_action_error(e),
            };

            let original = match read_diff_input(&original_path) {
                Ok(text) => text,
                Err(e) => return e,
            };
            let modified = match read_diff_input(&modified_path) {
                Ok(text) => text,
                Err(e) => return e,
            };

            match build_file_diff(
                path_str,
                new_path,
                &original,
                &modified,
                context_lines,
                ignore_whitespace,
            ) {
                None => format!("No differences between {} and {}.", path_str, new_path),
                // The summary goes first: tool output keeps a head+tail
                // slice, so a truncation marker mid-diff never hides it.
                Some(diff) => format!(
                    "{} vs {}: +{} / -{} lines across {} hunk(s)\n\n{}",
                    path_str, new_path, diff.added, diff.removed, diff.hunks, diff.text
                ),
            }
        }
        "mkdir" => {
            let _ = app.emit(
                "tool-call",
                format!("📁 *Creating directory {}*\n\n", path_str),
            );
            match resolve_file_action_path(app, action, path_str, &root_dir, false).await {
                Ok(p) => {
                    if let Err(e) =
                        ensure_mutation_target_allowed(&p, &root_dir, protected_skill_roots)
                    {
                        return format_file_action_error(e);
                    }
                    if p.exists() && p.is_file() {
                        return format!("Error: {} is an existing file", path_str);
                    }
                    match fs::create_dir_all(&p) {
                        Ok(_) => format!("Successfully created directory {}", path_str),
                        Err(e) => format!("Error creating directory: {}", e),
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        "delete" => {
            let _ = app.emit("tool-call", format!("🗑️ *Deleting {}*\n\n", path_str));
            match resolve_file_action_path(app, action, path_str, &root_dir, true).await {
                Ok(p) => {
                    if let Err(e) =
                        ensure_mutation_target_allowed(&p, &root_dir, protected_skill_roots)
                    {
                        return format_file_action_error(e);
                    }
                    let backup = match backup_protected_file(&p, protected_skill_roots) {
                        Ok(backup) => backup,
                        Err(e) => return format!("Error: {}", e),
                    };
                    if p.is_dir() {
                        match fs::remove_dir_all(&p) {
                            Ok(_) => format!("Successfully deleted directory {}", path_str),
                            Err(e) => format!("Error deleting directory: {}", e),
                        }
                    } else {
                        match fs::remove_file(&p) {
                            Ok(_) => {
                                if let Some(backup) = backup {
                                    format!(
                                        "Successfully backed up to {} and deleted file {}",
                                        backup.display(),
                                        path_str
                                    )
                                } else {
                                    format!("Successfully deleted file {}", path_str)
                                }
                            }
                            Err(e) => format!("Error deleting file: {}", e),
                        }
                    }
                }
                Err(e) => format_file_action_error(e),
            }
        }
        _ => format!("Unknown action '{}' for file_actions tool.", action),
    }
}

pub(super) fn external_absolute_path_candidate(workspace_dir: &Path, input: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(input);
    if !candidate.is_absolute() {
        return None;
    }

    let normalized = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.clone());
    let canonical_root = workspace_dir
        .canonicalize()
        .unwrap_or_else(|_| workspace_dir.to_path_buf());

    (!normalized.starts_with(&canonical_root)).then_some(normalized)
}

async fn resolve_file_action_path(
    app: &AppHandle,
    action: &str,
    input: &str,
    workspace_dir: &Path,
    require_exists: bool,
) -> Result<PathBuf, String> {
    match resolve_safe_path(workspace_dir, input) {
        Ok(path) => {
            if require_exists && !path.exists() {
                return Err(format!(
                    "Path '{}' was not found under workspace root '{}'",
                    input,
                    workspace_dir.display()
                ));
            }
            Ok(path)
        }
        Err(_) => {
            let Some(path) = external_absolute_path_candidate(workspace_dir, input) else {
                return Err(format!(
                    "Path '{}' is outside the workspace root '{}'",
                    input,
                    workspace_dir.display()
                ));
            };

            let confirm = request_tool_confirmation(
                app,
                format!(
                    "external absolute path access requested outside workspace root '{}'",
                    workspace_dir.display()
                ),
                "file_actions".to_string(),
                format!("{}: {}", action, path.display()),
                "external_path",
                "none",
            )
            .await;

            if !confirm.confirmed {
                return Err("⛔ External absolute path access denied by user.".to_string());
            }

            if require_exists && !path.exists() {
                return Err(format!("Path '{}' does not exist", path.display()));
            }

            Ok(path)
        }
    }
}

fn format_file_action_error(err: String) -> String {
    if err.starts_with('⛔') {
        err
    } else {
        format!("Error: {}", err)
    }
}

fn is_path_protected(path: &Path, protected_roots: &[PathBuf]) -> bool {
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    protected_roots.iter().any(|root| {
        let normalized_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        normalized.starts_with(&normalized_root)
    })
}

fn next_backup_path(path: &Path) -> Result<PathBuf, String> {
    let Some(parent) = path.parent() else {
        return Err(format!(
            "Cannot create backup for '{}': missing parent directory",
            path.display()
        ));
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(format!(
            "Cannot create backup for '{}': invalid file name",
            path.display()
        ));
    };

    for index in 1.. {
        let candidate = parent.join(format!("{file_name}.bak.{index}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(format!("Cannot create backup for '{}'", path.display()))
}

fn backup_protected_file(
    path: &Path,
    protected_roots: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    if !path.exists() || !path.is_file() || !is_path_protected(path, protected_roots) {
        return Ok(None);
    }

    let backup_path = next_backup_path(path)?;
    fs::copy(path, &backup_path).map_err(|e| {
        format!(
            "Failed to back up '{}' to '{}': {}",
            path.display(),
            backup_path.display(),
            e
        )
    })?;
    Ok(Some(backup_path))
}

/// Maximum size of a single file accepted by the `diff` action. The Myers diff
/// costs roughly O(N·D) in the number of changed lines, so a pair of
/// multi-megabyte files (minified bundles, huge logs) would block the async
/// runtime for minutes while producing a diff nobody can read.
const DIFF_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// A unified diff between two files plus the counts shown in the summary line.
pub(super) struct FileDiff {
    /// Rendered `---` / `+++` / `@@` text, ready to hand back to the model.
    pub(super) text: String,
    pub(super) added: usize,
    pub(super) removed: usize,
    pub(super) hunks: usize,
}

/// Read one side of a `diff` comparison. Directories, binary and oversized
/// files are rejected with an actionable message instead of producing a
/// mojibake diff the model then has to interpret.
fn read_diff_input(path: &Path) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|e| format!("Error reading '{}': {}", path.display(), e))?;
    if metadata.is_dir() {
        return Err(format!(
            "Error: '{}' is a directory; the diff action compares two files.",
            path.display()
        ));
    }
    if metadata.len() > DIFF_MAX_FILE_BYTES {
        return Err(format!(
            "Error: '{}' is {} bytes, above the {} byte diff limit. Compare a smaller file, or read a line range with the 'read' action and diff the excerpts.",
            path.display(),
            metadata.len(),
            DIFF_MAX_FILE_BYTES
        ));
    }

    let bytes = fs::read(path).map_err(|e| format!("Error reading '{}': {}", path.display(), e))?;
    if is_probably_binary(&bytes) {
        return Err(format!(
            "Error: '{}' looks like a binary file; the diff action only supports text files.",
            path.display()
        ));
    }

    String::from_utf8(bytes).map_err(|_| {
        format!(
            "Error: '{}' is not valid UTF-8 text; the diff action only supports text files.",
            path.display()
        )
    })
}

/// Drop a UTF-8 BOM and normalize CRLF to LF, so a pair that only differs in
/// line endings (or was saved by an editor that injects a BOM) does not show up
/// as a whole-file rewrite — the same tolerance `apply_unified_patch` applies.
fn normalize_diff_input(text: &str) -> String {
    let without_bom = text.strip_prefix('\u{feff}').unwrap_or(text);
    without_bom.replace("\r\n", "\n")
}

/// `git diff -w` style normalization: trim each line and collapse internal
/// whitespace runs down to a single space.
fn collapse_whitespace(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a git-style unified diff between two file contents. Returns `None`
/// when the normalized contents are identical so callers can report "no
/// differences" without materializing a patch.
pub(super) fn build_file_diff(
    original_label: &str,
    modified_label: &str,
    original: &str,
    modified: &str,
    context_lines: usize,
    ignore_whitespace: bool,
) -> Option<FileDiff> {
    let normalize = |text: &str| {
        let normalized = normalize_diff_input(text);
        if ignore_whitespace {
            collapse_whitespace(&normalized)
        } else {
            normalized
        }
    };

    let original = normalize(original);
    let modified = normalize(modified);
    if original == modified {
        return None;
    }

    let patch = diffy::DiffOptions::new()
        .set_context_len(context_lines)
        .set_original_filename(original_label.to_string())
        .set_modified_filename(modified_label.to_string())
        .create_patch(&original, &modified);

    let mut added = 0usize;
    let mut removed = 0usize;
    for line in patch.hunks().iter().flat_map(|hunk| hunk.lines()) {
        match line {
            diffy::Line::Insert(_) => added += 1,
            diffy::Line::Delete(_) => removed += 1,
            diffy::Line::Context(_) => {}
        }
    }

    Some(FileDiff {
        text: patch.to_string(),
        added,
        removed,
        hunks: patch.hunks().len(),
    })
}

/// Recursively copy a directory tree from `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create '{}': {}", dst.display(), e))?;
    for entry in
        fs::read_dir(src).map_err(|e| format!("Failed to read '{}': {}", src.display(), e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|e| format!("Failed to get file type for '{}': {}", from.display(), e))?
            .is_dir()
        {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|e| format!("Failed to copy '{}': {}", from.display(), e))?;
        }
    }
    Ok(())
}

/// Move `src` to `dst`, falling back to copy + delete when `fs::rename` fails
/// (e.g. cross-device `EXDEV` when src and dst are on different filesystems).
fn move_path(src: &Path, dst: &Path) -> Result<(), String> {
    match fs::rename(src, dst) {
        Ok(_) => Ok(()),
        Err(_) => {
            if src.is_dir() {
                copy_dir_recursive(src, dst)?;
                fs::remove_dir_all(src).map_err(|e| e.to_string())
            } else {
                fs::copy(src, dst).map_err(|e| e.to_string())?;
                fs::remove_file(src).map_err(|e| e.to_string())
            }
        }
    }
}
