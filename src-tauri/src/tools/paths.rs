// Path safety: workspace sandbox resolution, mutation guards and the shell
// working-directory validation quoted for `run_shell`.

use std::path::{Path, PathBuf};

use super::split_command_line;

pub(super) fn resolve_safe_path(root_dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
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
    workspace_dir.join("skills")
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

    // The persistent memory directory is managed by the memory tool; direct
    // mutation would wipe user/repo memories.
    let memory_root = workspace_dir.join("memory");
    if path_is_within(path, &memory_root) || path.starts_with(&memory_root) {
        return Err(
            "The memory directory is managed by the memory tool and cannot be modified directly."
                .to_string(),
        );
    }

    let workspace_skills = workspace_skills_root(workspace_dir);
    if path_is_within(path, &workspace_skills)
        && !path_is_within_any_root(path, writable_skill_roots)
    {
        return Err(
            "Skill directories are read-only. Only workspace/skills is writable in self-evolution mode."
                .to_string(),
        );
    }

    Ok(())
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
        {
            return &value[1..value.len() - 1];
        }
    }

    value
}

fn contains_dynamic_shell_path_syntax(value: &str) -> bool {
    // `~` expands to $HOME, `-` means "previous directory", `$`/`%`/backtick
    // are variable/command substitution. None of these are literal paths.
    value.contains('$')
        || value.contains('%')
        || value.contains('`')
        || value.contains('~')
        || value == "-"
}

fn validate_directory_change_target(workspace_dir: &Path, raw_target: &str) -> Result<(), String> {
    let trimmed = strip_matching_quotes(raw_target.trim()).trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if contains_dynamic_shell_path_syntax(trimmed) {
        return Err(
            "Shell directory changes must use literal paths that stay within the workspace root."
                .to_string(),
        );
    }
    // Reject `..` path components using either `/` or `\` as the separator.
    // This catches `..\..` (PowerShell on Windows) even when the test suite
    // runs on macOS, where `..\..` would otherwise be a literal filename.
    if trimmed
        .split(['/', '\\'])
        .any(|component| component == "..")
    {
        return Err(format!(
            "Shell directory changes must stay within the workspace root '{}'.",
            workspace_dir.display()
        ));
    }

    resolve_safe_path(workspace_dir, trimmed)
        .map(|_| ())
        .map_err(|_| {
            format!(
                "Shell directory changes must stay within the workspace root '{}'.",
                workspace_dir.display()
            )
        })
}

/// True if `statement` contains a `cd` / `pushd` / `set-location` command as a
/// standalone token anywhere (start, after `&&`/`||`/`;`, in a subshell, ...).
fn contains_directory_change_command(shell_type: &str, statement: &str) -> bool {
    let lower = statement.to_ascii_lowercase();
    let commands: &[&str] = if shell_type == "powershell" {
        &["set-location", "push-location", "pushd", "cd", "sl"]
    } else {
        &["cd", "pushd"]
    };

    commands.iter().any(|cmd| {
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find(cmd) {
            let abs = search_from + pos;
            let before_ok = abs == 0
                || lower.as_bytes()[abs - 1].is_ascii_whitespace()
                || matches!(lower.as_bytes()[abs - 1], b';' | b'&' | b'|' | b'(' | b'{');
            let after = abs + cmd.len();
            let after_ok = after >= lower.len() || lower.as_bytes()[after].is_ascii_whitespace();
            if before_ok && after_ok {
                return true;
            }
            search_from = abs + cmd.len();
        }
        false
    })
}

fn extract_directory_change_target(shell_type: &str, statement: &str) -> Option<String> {
    let lower = statement.to_ascii_lowercase();
    let commands: &[&str] = if shell_type == "powershell" {
        &["set-location", "push-location", "pushd", "cd", "sl"]
    } else {
        &["cd", "pushd"]
    };

    // Scan for the command anywhere in the statement, so `echo hi && cd /etc`
    // and `(cd /etc && ls)` are caught too — not just statements that START
    // with `cd`.
    for cmd in commands {
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find(cmd) {
            let abs = search_from + pos;
            let before_ok = abs == 0
                || lower.as_bytes()[abs - 1].is_ascii_whitespace()
                || matches!(lower.as_bytes()[abs - 1], b';' | b'&' | b'|' | b'(' | b'{');
            let after = abs + cmd.len();
            let after_ok = after >= lower.len() || lower.as_bytes()[after].is_ascii_whitespace();
            if before_ok && after_ok {
                let rest = statement[after..].trim_start();
                if rest.is_empty() {
                    return None;
                }
                let mut parts = split_command_line(rest);
                if shell_type == "powershell" {
                    parts.retain(|part| {
                        !part.eq_ignore_ascii_case("-path")
                            && !part.eq_ignore_ascii_case("-literalpath")
                    });
                }
                return parts.into_iter().next();
            }
            search_from = abs + cmd.len();
        }
    }

    None
}

/// Strip `#` comments (outside quotes) so commented-out `cd` lines do not
/// cause false rejections.
fn strip_shell_comments(code: &str) -> String {
    let mut result = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = code.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '#' if !in_single && !in_double => {
                // Skip to end of line.
                for c in chars.by_ref() {
                    if c == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            }
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\n' => {
                in_single = false;
                in_double = false;
                result.push('\n');
            }
            _ => result.push(ch),
        }
    }
    result
}

pub(super) fn validate_shell_working_directory_changes(
    shell_type: &str,
    code: &str,
    workspace_dir: &Path,
) -> Result<(), String> {
    let code = strip_shell_comments(code);
    for statement in code.replace(';', "\n").lines() {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }

        if contains_directory_change_command(shell_type, trimmed) {
            match extract_directory_change_target(shell_type, trimmed) {
                Some(target) => validate_directory_change_target(workspace_dir, &target)?,
                None => {
                    // Bare `cd` / `pushd` with no argument changes to $HOME.
                    return Err(
                        "Shell directory changes must specify a literal path within the workspace root."
                            .to_string(),
                    );
                }
            }
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
