// Path safety: workspace sandbox resolution and mutation guards.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

pub(crate) fn resolve_safe_path(root_dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
    // If an absolute path is given and it starts with root_dir, strip the prefix
    // so the AI can pass either relative or absolute paths within the workspace.
    let stripped;
    let rel_path_str = if Path::new(rel_path).is_absolute() {
        let canonical_root = root_dir
            .canonicalize()
            .unwrap_or_else(|_| root_dir.to_path_buf());
        let abs = Path::new(rel_path);
        let abs_canonical = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
        if let Ok(suffix) = abs_canonical.strip_prefix(&canonical_root) {
            stripped = suffix.to_string_lossy().into_owned();
            &stripped as &str
        } else {
            return Err(format!(
                "Absolute path '{}' is outside the workspace root '{}'",
                rel_path,
                root_dir.display()
            ));
        }
    } else {
        rel_path
    };

    let rel_path = Path::new(rel_path_str);

    // Rooted paths (`/etc`, `\foo` on Windows) and drive-relative prefixes
    // (`C:foo`) ignore the current directory, so they are not
    // workspace-relative. `Path::is_absolute()` is false for the first group on
    // Windows, and the component loop below would otherwise swallow the
    // `RootDir` component — `cd /etc` would "resolve" to `<workspace>/etc` and
    // slip past the shell working-directory guard.
    if rel_path
        .components()
        .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return Err(format!(
            "Path '{}' is rooted and cannot be resolved inside the workspace root '{}'",
            rel_path.display(),
            root_dir.display()
        ));
    }

    let mut resolved = root_dir.to_path_buf();
    for comp in rel_path.components() {
        match comp {
            std::path::Component::ParentDir => {
                resolved.pop();
                if !resolved.starts_with(root_dir) {
                    return Err("Path escapes workspace directory".to_string());
                }
            }
            std::path::Component::Normal(c) => {
                resolved.push(c);
            }
            _ => {}
        }
    }

    // Final guard: canonicalize the resolved path and verify it is still inside root_dir
    match resolved.canonicalize() {
        Ok(canonical) => {
            let canonical_root = root_dir
                .canonicalize()
                .unwrap_or_else(|_| root_dir.to_path_buf());
            if !canonical.starts_with(&canonical_root) {
                return Err("Path escapes workspace directory".to_string());
            }
            Ok(canonical)
        }
        // File does not exist yet (e.g. write to a new file). A lexical
        // `starts_with` check alone is unsafe: a symlinked parent (e.g.
        // `./link/newfile` where `link -> /etc`) would pass lexically while
        // actually resolving outside the workspace. Canonicalize the deepest
        // existing ancestor (which exists and resolves symlinks), verify it
        // is still inside root_dir, then rebuild the path from it.
        Err(_) => {
            if let Some(canonical_resolved) = canonicalize_lenient(&resolved) {
                let canonical_root = root_dir
                    .canonicalize()
                    .unwrap_or_else(|_| root_dir.to_path_buf());
                if !canonical_resolved.starts_with(&canonical_root) {
                    return Err("Path escapes workspace directory".to_string());
                }
                return Ok(canonical_resolved);
            }
            if !resolved.starts_with(root_dir) {
                return Err("Path escapes workspace directory".to_string());
            }
            Ok(resolved)
        }
    }
}

/// Canonicalize `path` even when it does not exist yet.
///
/// `Path::canonicalize` fails for a missing path, so a plain containment check
/// would reject every *new* file — exactly the operation self-evolution needs
/// (creating a skill folder). This canonicalizes the deepest existing
/// ancestor (which resolves symlinks) and re-appends the remaining
/// components, so a symlinked parent that escapes the root is still caught.
fn canonicalize_lenient(path: &Path) -> Option<PathBuf> {
    let mut ancestor = path.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    loop {
        if let Ok(canonical) = ancestor.canonicalize() {
            let mut rebuilt = canonical;
            for name in tail.iter().rev() {
                rebuilt.push(name);
            }
            return Some(rebuilt);
        }
        let name = ancestor.file_name().map(|n| n.to_os_string())?;
        tail.push(name);
        if !ancestor.pop() {
            return None;
        }
    }
}

/// Containment check that also holds for paths that do not exist yet.
fn path_is_within_lenient(path: &Path, root: &Path) -> bool {
    match (canonicalize_lenient(path), canonicalize_lenient(root)) {
        (Some(path), Some(root)) => path.starts_with(&root),
        _ => false,
    }
}

/// True for the workspace SQLite database and its journal sidecars.
///
/// `file_actions` can only truncate or delete these — it can never rewrite
/// them — so they stay protected while the rest of `.data` is reachable.
fn is_workspace_database(path: &Path, workspace_dir: &Path) -> bool {
    let db_path = crate::workspace::db_path(workspace_dir);
    let canonical_path =
        canonicalize_lenient(path).unwrap_or_else(|| path.to_path_buf());
    let canonical_db =
        canonicalize_lenient(&db_path).unwrap_or_else(|| db_path.to_path_buf());
    if canonical_path == canonical_db {
        return true;
    }

    // SQLite sidecars: `chat.db-journal`, `chat.db-wal`, `chat.db-shm`.
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let db_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("chat.db");
    file_name.len() > db_name.len()
        && file_name.starts_with(db_name)
        && file_name.as_bytes()[db_name.len()] == b'-'
}

pub(super) fn with_root_header(body: String) -> String {
    if body.is_empty() {
        "ROOT: ./".to_string()
    } else {
        format!("ROOT: ./\n{body}")
    }
}

pub(super) fn ensure_mutation_target_allowed(
    path: &Path,
    workspace_dir: &Path,
    writable_skill_root: Option<&Path>,
    action: &str,
) -> Result<(), String> {
    // Never allow mutating the workspace root itself (e.g. `delete` on `.` or
    // `./` would otherwise wipe the entire workspace via remove_dir_all).
    let canonical_workspace = workspace_dir
        .canonicalize()
        .unwrap_or_else(|_| workspace_dir.to_path_buf());
    let canonical_path =
        canonicalize_lenient(path).unwrap_or_else(|| path.to_path_buf());
    if canonical_path == canonical_workspace {
        return Err("The workspace root itself cannot be modified or deleted.".to_string());
    }

    // The SQLite database is the one file inside `.data` that `file_actions`
    // cannot recreate: `write` truncates it and `delete` removes it, which
    // would destroy the chat history, the API monitor, the interaction log and
    // the agent missions. Everything else under `.data` (memory, todos, skill
    // status, skills) is part of the workspace and stays reachable.
    if is_workspace_database(path, workspace_dir) {
        return Err(
            "The workspace database (.data/chat.db) is managed by the application and cannot be modified or deleted."
                .to_string(),
        );
    }

    // The one writable skill root wins over every other rule: in self-evolution mode
    // the assistant owns `<workspace>/.data/skills` and may create, edit, move
    // and delete skills there. The check runs before the `.data` guard below so
    // a *new* skill folder (a path that does not exist yet) is accepted too.
    if let Some(root) = writable_skill_root {
        if path_is_within_lenient(path, root) {
            // An *existing* skills root is never a mutation target: wiping it
            // would destroy every skill in the workspace. Only its children
            // are. The root itself may still be created — it is created on
            // demand, and `mkdir` on an existing folder is a harmless no-op.
            if path.exists()
                && canonical_path == canonicalize_lenient(root).unwrap_or_default()
                && action != "mkdir"
            {
                return Err(
                    "The skills root itself cannot be modified or deleted; target a skill folder instead."
                        .to_string(),
                );
            }
            return Ok(());
        }
    }

    Ok(())
}

#[cfg(windows)]
fn quote_cmd_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\"\""))
}

fn quote_powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn quote_posix_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(super) fn build_workspace_scoped_shell_code(
    shell_type: &str,
    workspace_dir: &Path,
    code: &str,
) -> String {
    match shell_type {
        "powershell" => format!(
            "Set-Location -LiteralPath {};\n{}",
            quote_powershell_literal(&workspace_dir.display().to_string()),
            code
        ),
        _ => {
            #[cfg(windows)]
            {
                format!("cd /d {} && {}", quote_cmd_path(workspace_dir), code)
            }

            #[cfg(not(windows))]
            {
                format!(
                    "cd {}\n{}",
                    quote_posix_literal(&workspace_dir.display().to_string()),
                    code
                )
            }
        }
    }
}
