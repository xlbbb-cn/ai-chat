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
//! The composer also *writes*: a file pasted with `Ctrl+V` or dropped onto the
//! input box lands in `<workspace>/.data/tmp/` and becomes an `@.data/tmp/<name>`
//! mention ([`save_pasted_file`], [`import_workspace_paths`]). That is the app's
//! scratch area ([`crate::workspace::tmp_root`]), so a paste never shows up
//! among the user's tracked files — and [`resolve_in_workspace`] opens exactly
//! that one subdirectory of `.data`, so the mention can still be read back while
//! the database, skills and scripts stay unreachable from the composer. Every
//! file this module writes there is prefixed [`PASTE_PREFIX`], so the stale sweep
//! never touches a sub-agent mission batch or a cached PDF conversion.
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
/// workspace or inside the app's own `.data` state — except `.data/tmp`.
///
/// `.data/tmp` is the one writable/readable corner of `.data`: composer pastes
/// land there ([`save_pasted_file`]) and a PDF mention is cached there, so a
/// mention has to be able to read its own paste back. Everything else under
/// `.data` (the database, skills, scripts, memory) stays unreachable from the
/// composer, and the comparison is component-wise so a sibling such as
/// `.data/tmpx` is *not* waved through.
///
/// Shared with [`crate::pdf_mentions`], which converts a mentioned PDF: the
/// rule is the same one (`@` can only *name* a path inside the workspace), so
/// it must not be re-implemented next to it.
pub(crate) fn resolve_in_workspace(root: &Path, relative: &str) -> Result<PathBuf, String> {
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
    let mut parts = rel.components();
    if parts.next().map(|c| c.as_os_str() == ".data").unwrap_or(false)
        && parts.next().map(|c| c.as_os_str() == "tmp") != Some(true)
    {
        return Err("only .data/tmp is readable from the composer".to_string());
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

// ─── Pasted / dropped files ──────────────────────────────────────────────────

/// Workspace-relative folder that receives composer pastes.
///
/// It is the app's own scratch area ([`crate::workspace::tmp_root`]) rather than
/// a top-level `tmp/`, so nothing appears among the user's tracked files.
/// [`resolve_in_workspace`] opens exactly this subdirectory of `.data` and
/// nothing else, and the inserted `@.data/tmp/<name>` then reads back through
/// the very same path a hand-typed mention uses.
const COMPOSER_TMP_REL: &str = ".data/tmp";

/// Absolute folder that receives files pasted (`Ctrl+V`) or dropped onto the
/// composer.
pub(crate) fn composer_tmp_root(workspace: &Path) -> PathBuf {
    crate::workspace::tmp_root(workspace)
}

/// Prefix of every file this module writes into `.data/tmp`, a folder it shares
/// with the sub-agent exchange and the PDF cache. The sweep keys on the full
/// [`is_paste_file_name`] shape rather than this prefix, so neither of those can
/// ever be deleted by accident.
const PASTE_PREFIX: &str = "paste-";

/// A pasted file travels through the mention pipeline, so it is capped like any
/// other binary mention (24 MiB before base64).
const MAX_PASTE_BYTES: u64 = MAX_BINARY_BYTES;

/// base64 inflates by 4/3; checked before decoding so a bogus payload never
/// reaches the decoder.
const MAX_PASTE_BASE64_LEN: usize = (MAX_BINARY_BYTES as usize) / 3 * 4 + 8;

/// Longest file stem kept from a paste.
const MAX_PASTE_STEM_LEN: usize = 80;

/// Longest extension kept from a paste (`tar.gz` is `gz`; this is generous).
const MAX_PASTE_EXT_LEN: usize = 12;

/// A pasted file nobody sent within a day is garbage: the mention in the
/// composer is what keeps it alive, and after that the folder would only grow.
const PASTE_TTL_SECS: i64 = 24 * 60 * 60;

/// Most files accepted from one drop (dragging a folder names thousands).
const MAX_IMPORTED_PATHS: usize = 20;

/// Split a file name into (stem, extension) on the last dot, ignoring a leading
/// dot (`.env` has no extension) and an over-long tail.
fn split_paste_name(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= MAX_PASTE_EXT_LEN
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            (stem.to_string(), ext.to_ascii_lowercase())
        }
        _ => (name.to_string(), String::new()),
    }
}

/// Replace everything the mention grammar (or Windows) cannot carry with `-`.
fn squish_name_part(value: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in value.chars() {
        // Letters/digits only, which keeps CJK names readable; `-`/`_` are the
        // two safe separators. Everything else (` `, `#`, `@`, `<`, `>`, `%`,
        // control chars, path separators) collapses into a single dash.
        if c.is_alphanumeric() || matches!(c, '-' | '_') {
            out.push(c);
            pending_dash = false;
        } else if !pending_dash && !out.is_empty() {
            out.push('-');
            pending_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Build the stored name for a pasted file: `<PASTE_PREFIX><stamp>-<stem>.<ext>`.
///
/// The name must survive the `@` mention grammar, which ends a token at
/// whitespace, `#`, `@`, `<` or `>` — so it is rebuilt from a whitelist instead
/// of being escaped. The timestamp prefix keeps two pastes of the same
/// screenshot apart without a collision check for the common case.
pub(crate) fn sanitize_paste_name(raw: &str, stamp: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let (raw_stem, ext) = split_paste_name(base);
    let mut stem = squish_name_part(&raw_stem);
    if stem.is_empty() {
        stem = "file".to_string();
    }
    if stem.chars().count() > MAX_PASTE_STEM_LEN {
        stem = stem.chars().take(MAX_PASTE_STEM_LEN).collect();
    }
    let stem = stem.trim_matches('-');
    if ext.is_empty() {
        format!("{PASTE_PREFIX}{stamp}-{stem}")
    } else {
        format!("{PASTE_PREFIX}{stamp}-{stem}.{ext}")
    }
}

/// First free name in `dir`: the name itself, else `-2`, `-3`… before the dot.
fn next_free_paste_name(dir: &Path, file_name: &str) -> String {
    if !dir.join(file_name).exists() {
        return file_name.to_string();
    }
    let (stem, ext) = split_paste_name(file_name);
    for n in 2..=99u32 {
        let candidate = if ext.is_empty() {
            format!("{stem}-{n}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    // 99 pastes inside one second: overwrite the last slot instead of failing.
    file_name.to_string()
}

/// Write `bytes` as `.data/tmp/<file_name>` and return the workspace-relative
/// path.
fn write_composer_paste(
    workspace: &Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let root = composer_tmp_root(workspace);
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let file_name = next_free_paste_name(&root, file_name);
    let relative = format!("{COMPOSER_TMP_REL}/{file_name}");
    // The name was rebuilt from a whitelist and carries no separator, but the
    // write still goes through the workspace sandbox every other write uses.
    let target = crate::tools::paths::resolve_safe_path(workspace, &relative)?;
    std::fs::write(&target, bytes).map_err(|e| format!("{relative}: {e}"))?;
    Ok(relative)
}

/// Drop `.data/tmp/paste-*` files older than [`PASTE_TTL_SECS`].
///
/// Best effort: a locked or vanished file is skipped, never an error — this runs
/// on the way into an unrelated paste.
///
/// `.data/tmp` is shared with the sub-agent deliverable exchange
/// ([`crate::agents::exchange`]) and the PDF conversion cache
/// ([`crate::pdf_mentions`]), which are permanent. The sweep therefore deletes
/// on [`is_paste_file_name`] — a `<PASTE_PREFIX><YYYYMMDD>-<HHMMSS>-` shape this
/// module alone produces — not on the bare prefix, so no batch directory and no
/// hand-made file can ever match.
fn sweep_composer_tmp(workspace: &Path) {
    let root = composer_tmp_root(workspace);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_paste_file_name(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let age = modified
            .elapsed()
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0);
        if age > PASTE_TTL_SECS {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Does `name` look like a stored paste (`paste-20261111-155012-shot.png`)?
///
/// The shape is what [`sweep_composer_tmp`] deletes on: eight digits, a dash,
/// six digits, a dash — the stamp [`paste_stamp`] writes. A file that merely
/// starts with `paste-` (hand-made, or a future writer) is left alone.
fn is_paste_file_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(PASTE_PREFIX) else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() > 16
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'-'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'-'
}

/// Workspace-relative path of a dropped file that can be mentioned *in place*.
///
/// Dropping a file that already lives in the workspace must not duplicate it
/// into `.data/tmp` (and must not put a project file on the paste expiry clock),
/// so it is referenced where it is. Two cases still have to be copied: `.data`
/// outside `tmp/` (the composer cannot read it back) and a path the mention
/// grammar cannot carry (whitespace, `#`, `@`, `<`, `>` — the token would end in
/// the middle of the name).
fn mentionable_workspace_path(workspace: &Path, path: &Path) -> Option<String> {
    let root = std::fs::canonicalize(workspace).ok()?;
    let target = std::fs::canonicalize(path).ok()?;
    if !target.is_file() {
        return None;
    }
    let relative = target.strip_prefix(&root).ok()?;
    let text = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if text.is_empty() {
        return None;
    }
    // `.data` is mention-readable only under `tmp/` (see `resolve_in_workspace`).
    if text.starts_with(".data/") && !text.starts_with(".data/tmp/") {
        return None;
    }
    // The mention grammar ends a token at these characters; such a path is
    // copied instead, which rebuilds the name from a whitelist.
    if text.chars().any(|c| {
        c.is_whitespace() || matches!(c, '#' | '@' | '<' | '>' | '`' | '|' | '"' | '\'')
    }) {
        return None;
    }
    Some(text)
}

/// Decode a clipboard payload into bytes, tolerating the `data:<mime>;base64,`
/// prefix the webview's `FileReader` may leave on it.
pub(crate) fn decode_paste(payload: &str) -> Result<Vec<u8>, String> {
    let payload = payload.trim();
    let payload = match payload.find(";base64,") {
        Some(index) => &payload[index + ";base64,".len()..],
        None => payload,
    };
    if payload.len() > MAX_PASTE_BASE64_LEN {
        return Err(format!(
            "file is too large to attach ({} characters of base64, limit {MAX_PASTE_BASE64_LEN})",
            payload.len()
        ));
    }
    let bytes = STANDARD
        .decode(payload)
        .map_err(|e| format!("pasted data is not valid base64: {e}"))?;
    if bytes.is_empty() {
        return Err("the pasted file is empty".to_string());
    }
    if bytes.len() as u64 > MAX_PASTE_BYTES {
        return Err(format!(
            "file is too large to attach ({} bytes, limit {MAX_PASTE_BYTES})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Timestamp prefix for a stored paste (`20261111-155012`), local time.
fn paste_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Store the bytes of one file pasted into the composer (`Ctrl+V`).
///
/// Returns the workspace-relative path (`.data/tmp/paste-….png`), which the composer
/// inserts as an `@` mention — so the file is attached by the normal pipeline
/// when the turn is sent, exactly like a workspace file that was already there.
#[tauri::command]
pub async fn save_pasted_file(
    state: State<'_, AppState>,
    name: String,
    data_base64: String,
) -> Result<String, String> {
    let root = workspace_root(&state)?;
    let bytes = decode_paste(&data_base64)?;
    let file_name = sanitize_paste_name(&name, &paste_stamp());
    sweep_composer_tmp(&root);
    write_composer_paste(&root, &file_name, &bytes)
}

/// What [`import_workspace_paths`] managed to copy out of one drop.
#[derive(Debug, Clone, Serialize)]
pub struct ImportedPaths {
    /// Workspace-relative paths to attach: a dropped file that already lives in
    /// the workspace is referenced where it is, everything else arrives as a
    /// copy under `.data/tmp/`.
    pub imported: Vec<String>,
    /// One line per refused path (folder, oversized, unreadable).
    pub problems: Vec<String>,
}

/// Attach files dropped onto the composer.
///
/// The native drag-drop event hands over real filesystem paths, so the bytes
/// never travel through the webview. A file from inside the workspace is
/// mentioned in place (see [`mentionable_workspace_path`]); anything else —
/// desktop, another drive, `.data` outside `tmp/` — is copied into
/// `<workspace>/.data/tmp/` exactly like a clipboard paste, so the turn reads a
/// stable workspace path. A path that is a folder, unreadable or oversized is
/// reported without discarding the rest; the caller attaches what arrived and
/// shows the leftovers as a warning.
#[tauri::command]
pub async fn import_workspace_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<ImportedPaths, String> {
    let root = workspace_root(&state)?;
    if paths.is_empty() {
        return Ok(ImportedPaths {
            imported: Vec::new(),
            problems: Vec::new(),
        });
    }
    sweep_composer_tmp(&root);
    let stamp = paste_stamp();
    let mut imported = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for path in paths.iter().take(MAX_IMPORTED_PATHS) {
        let source = PathBuf::from(path);
        // Referenced where it is: copying would duplicate a project file and
        // put it on the paste expiry clock.
        if let Some(relative) = mentionable_workspace_path(&root, &source) {
            imported.push(relative);
            continue;
        }
        let meta = match std::fs::metadata(&source) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => {
                problems.push(format!("{path}: not a file"));
                continue;
            }
            Err(e) => {
                problems.push(format!("{path}: {e}"));
                continue;
            }
        };
        if meta.len() > MAX_PASTE_BYTES {
            problems.push(format!(
                "{path}: too large to attach ({} bytes, limit {MAX_PASTE_BYTES})",
                meta.len()
            ));
            continue;
        }
        let bytes = match std::fs::read(&source) {
            Ok(bytes) => bytes,
            Err(e) => {
                problems.push(format!("{path}: {e}"));
                continue;
            }
        };
        let file_name = sanitize_paste_name(&file_name_of(&source), &stamp);
        match write_composer_paste(&root, &file_name, &bytes) {
            Ok(relative) => imported.push(relative),
            Err(e) => problems.push(e),
        }
    }

    if imported.is_empty() && problems.is_empty() {
        problems.push("no files to import".to_string());
    }
    Ok(ImportedPaths { imported, problems })
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
        // Only `.data/tmp` is opened; the rest of the state folder stays shut,
        // and the comparison is component-wise, so a lookalike sibling is out.
        assert!(resolve_in_workspace(&root, ".data/skills/demo/skill.md").is_err());
        assert!(resolve_in_workspace(&root, ".data/tmpx/a.png").is_err());
        assert!(resolve_in_workspace(&root, ".data").is_err());
        assert!(resolve_in_workspace(&root, "").is_err());
        // A file that really is inside the crate root still resolves.
        assert!(resolve_in_workspace(&root, "Cargo.toml").is_ok());
    }

    #[test]
    fn paste_names_survive_the_mention_grammar() {
        // The token ends at whitespace, `#`, `@`, `<` and `>`, so none of them
        // may reach the stored name.
        let name = sanitize_paste_name("my screenshot #2 (final).png", "20261111-155012");
        assert_eq!(name, "paste-20261111-155012-my-screenshot-2-final.png");
        assert!(!name.contains([' ', '#', '@', '<', '>']));

        // CJK names stay readable, a missing extension stays missing, and a
        // name without letters still gets a usable stem.
        assert_eq!(
            sanitize_paste_name("会议记录.md", "s"),
            "paste-s-会议记录.md"
        );
        // Only the last segment is an extension; a dot inside the stem is
        // collapsed like any other separator, so the stored name has one
        // unambiguous extension.
        assert_eq!(sanitize_paste_name("archive.tar.gz", "s"), "paste-s-archive-tar.gz");
        assert_eq!(sanitize_paste_name("noext", "s"), "paste-s-noext");
        assert_eq!(sanitize_paste_name("  ???  ", "s"), "paste-s-file");
        // Only the basename is used: a pasted name can never carry a directory.
        assert_eq!(sanitize_paste_name("../../etc/passwd", "s"), "paste-s-passwd");
        assert_eq!(sanitize_paste_name("C:\\Windows\\x.txt", "s"), "paste-s-x.txt");
    }

    #[test]
    fn paste_names_are_length_capped() {
        let long = "a".repeat(300);
        let name = sanitize_paste_name(&format!("{long}.png"), "s");
        let (stem, ext) = split_paste_name(&name);
        assert_eq!(ext, "png");
        assert!(stem.chars().count() <= MAX_PASTE_STEM_LEN + "paste-s-".len());
    }

    #[test]
    fn decodes_a_clipboard_payload_and_refuses_junk() {
        let encoded = STANDARD.encode(b"hello");
        assert_eq!(decode_paste(&encoded).unwrap(), b"hello");
        // A `data:` URL prefix is tolerated.
        assert_eq!(
            decode_paste(&format!("data:image/png;base64,{encoded}")).unwrap(),
            b"hello"
        );
        assert!(decode_paste("").is_err());
        assert!(decode_paste("not base64 !!").is_err());
        assert!(decode_paste(&"A".repeat(MAX_PASTE_BASE64_LEN + 4)).is_err());
    }

    #[test]
    fn writes_a_paste_into_tmp_and_never_overwrites() {
        let workspace =
            std::env::temp_dir().join(format!("pasted-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).unwrap();

        let first = write_composer_paste(&workspace, "paste-s-shot.png", b"one").unwrap();
        assert_eq!(first, ".data/tmp/paste-s-shot.png");
        assert_eq!(
            std::fs::read(workspace.join(&first)).unwrap(),
            b"one".to_vec()
        );

        let second = write_composer_paste(&workspace, "paste-s-shot.png", b"two").unwrap();
        assert_eq!(second, ".data/tmp/paste-s-shot-2.png");

        // The written path is readable through the `@` pipeline: `.data/tmp` is
        // the single subdirectory of `.data` the composer may touch, while the
        // state next to it stays unreachable.
        assert!(resolve_in_workspace(&workspace, &first).is_ok());
        assert!(resolve_in_workspace(&workspace, ".data/tmp/paste-s-shot.png").is_ok());
        std::fs::write(workspace.join(".data/chat.db"), b"db").unwrap();
        assert!(resolve_in_workspace(&workspace, ".data/chat.db").is_err());

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn only_the_paste_shape_is_swept() {
        // Stamped by `paste_stamp`, so the sweep can recognise it.
        assert!(is_paste_file_name("paste-20261111-155012-shot.png"));
        assert!(is_paste_file_name("paste-20261111-155012-shot-2.png"));
        // A mission batch directory, the PDF cache directory, and anything a
        // user or a future writer put into `.data/tmp` themselves.
        assert!(!is_paste_file_name("20261111-155012-0f3a91bc"));
        assert!(!is_paste_file_name("pdf"));
        assert!(!is_paste_file_name("paste-notes.md"));
        assert!(!is_paste_file_name("paste-2026111-155012-x.png"));
        assert!(!is_paste_file_name("paste-20261111-15501-x.png"));
        assert!(!is_paste_file_name("Paste-20261111-155012-x.png"));
    }

    #[test]
    fn a_dropped_workspace_file_is_referenced_in_place() {
        let workspace =
            std::env::temp_dir().join(format!("dropped-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::create_dir_all(workspace.join(".data/skills/demo")).unwrap();
        std::fs::create_dir_all(workspace.join(".data/tmp")).unwrap();
        std::fs::write(workspace.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(workspace.join("my notes.md"), b"# notes").unwrap();
        std::fs::write(workspace.join(".data/skills/demo/skill.md"), b"# skill").unwrap();
        std::fs::write(workspace.join(".data/tmp/paste-s-shot.png"), b"shot").unwrap();

        // Already part of the workspace: no copy, no expiry clock.
        assert_eq!(
            mentionable_workspace_path(&workspace, &workspace.join("src/main.rs")),
            Some("src/main.rs".to_string())
        );
        assert_eq!(
            mentionable_workspace_path(&workspace, &workspace.join(".data/tmp/paste-s-shot.png")),
            Some(".data/tmp/paste-s-shot.png".to_string())
        );
        // The mention grammar would end the token at the space, so this one is
        // copied instead — the copy rebuilds the name from a whitelist.
        assert_eq!(
            mentionable_workspace_path(&workspace, &workspace.join("my notes.md")),
            None
        );
        // `.data` outside `tmp/` is not mention-readable.
        assert_eq!(
            mentionable_workspace_path(&workspace, &workspace.join(".data/skills/demo/skill.md")),
            None
        );
        // Outside the workspace entirely.
        assert_eq!(
            mentionable_workspace_path(&workspace, Path::new("Cargo.toml")),
            None
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }
}
