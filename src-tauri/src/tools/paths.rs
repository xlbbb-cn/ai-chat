// Path safety: workspace sandbox resolution and mutation guards.

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
        // actually resolving outside the workspace. Canonicalize the parent
        // directory (which exists) to resolve any symlinks, verify it is still
        // inside root_dir, then rebuild the path from the canonical parent.
        Err(_) => {
            if let Some(parent) = resolved.parent() {
                if let Ok(canonical_parent) = parent.canonicalize() {
                    let canonical_root = root_dir
                        .canonicalize()
                        .unwrap_or_else(|_| root_dir.to_path_buf());
                    if !canonical_parent.starts_with(&canonical_root) {
                        return Err("Path escapes workspace directory".to_string());
                    }
                    if let Some(file_name) = resolved.file_name() {
                        return Ok(canonical_parent.join(file_name));
                    }
                }
            }
            if !resolved.starts_with(root_dir) {
                return Err("Path escapes workspace directory".to_string());
            }
            Ok(resolved)
        }
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path.canonicalize()
        .map(|p| p.starts_with(root.canonicalize().unwrap_or_else(|_| root.to_path_buf())))
        .unwrap_or(false)
}

fn path_is_within_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path_is_within(path, root))
}

fn workspace_skills_root(workspace_dir: &Path) -> PathBuf {
    crate::workspace::skills_root(workspace_dir)
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
    writable_skill_roots: &[PathBuf],
) -> Result<(), String> {
    // Never allow mutating the workspace root itself (e.g. `delete` on `.` or
    // `./` would otherwise wipe the entire workspace via remove_dir_all).
    let canonical_workspace = workspace_dir
        .canonicalize()
        .unwrap_or_else(|_| workspace_dir.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if canonical_path == canonical_workspace {
        return Err("The workspace root itself cannot be modified or deleted.".to_string());
    }

    // The hidden `.data` directory holds application-managed workspace state:
    // the SQLite database, the memory store, todo lists and the skill status
    // file. Only the skills root is writable, and only when the caller passes
    // it as a writable root (self-evolution mode) — direct mutation of the
    // database or the memory/todo files would corrupt or wipe user data.
    let data_root = crate::workspace::data_root(workspace_dir);
    if path_is_within(path, &data_root) || path.starts_with(&data_root) {
        if !path_is_within_any_root(path, writable_skill_roots) {
            let skills_root = workspace_skills_root(workspace_dir);
            if path_is_within(path, &skills_root) || path.starts_with(&skills_root) {
                return Err(
                    "Skill directories are read-only. Only .data/skills is writable in self-evolution mode."
                        .to_string(),
                );
            }
            return Err(
                "The workspace .data directory is managed by the application and cannot be modified directly."
                    .to_string(),
            );
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
