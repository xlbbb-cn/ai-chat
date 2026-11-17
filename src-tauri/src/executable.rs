//! Executable resolution shared by `run_shell` and the `cli` sub-agent backend.

use std::path::{Path, PathBuf};

/// Resolve a configured `command` to a real file. Bare names are searched on
/// `PATH` (with Windows `PATHEXT` extensions), paths are used as given. Windows
/// package-manager CLIs (npm, bun, pnpm) are `.cmd` shims, which `CreateProcess`
/// cannot start directly — the caller wraps those in `cmd.exe`.
pub(crate) fn resolve_executable(command: &str) -> Result<PathBuf, String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err("The command is empty.".to_string());
    }

    let candidate = PathBuf::from(trimmed);
    let looks_like_path =
        candidate.is_absolute() || trimmed.contains('/') || trimmed.contains('\\');
    if looks_like_path {
        return find_with_extensions(&candidate)
            .ok_or_else(|| format!("'{}' was not found.", trimmed));
    }

    let path_var = std::env::var_os("PATH")
        .ok_or_else(|| "PATH is not set in this environment.".to_string())?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if let Some(found) = find_with_extensions(&dir.join(trimmed)) {
            return Ok(found);
        }
    }

    Err(format!(
        "'{}' was not found on PATH. Put the full path to the executable in the agent's command field if it is installed elsewhere.",
        trimmed
    ))
}

fn find_with_extensions(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(candidate.to_path_buf());
    }

    #[cfg(windows)]
    {
        let ext = candidate
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        let already_has_extension =
            matches!(ext.as_deref(), Some("exe" | "cmd" | "bat" | "com" | "ps1"));
        if !already_has_extension {
            for extension in windows_path_extensions() {
                let with_ext = PathBuf::from(format!("{}{}", candidate.display(), extension));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }

    None
}

#[cfg(windows)]
fn windows_path_extensions() -> Vec<String> {
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut extensions: Vec<String> = raw
        .split(';')
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_lowercase())
        .collect();
    if !extensions.iter().any(|value| value == ".ps1") {
        extensions.push(".ps1".to_string());
    }
    if extensions.is_empty() {
        extensions.push(".exe".to_string());
    }
    extensions
}
