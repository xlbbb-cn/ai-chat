//! The per-host Python environment (app data) and the switch in the prompt.
//!
//! The `create-skill` / `create-mcp-server` / `create-plugin` manuals recommend
//! Python, and third-party packages need a home: not the system interpreter
//! (other apps must keep working), and not the workspace either — a workspace
//! is copied to other machines while a venv carries absolute host paths and a
//! per-OS layout. So the app keeps exactly one 3.13 venv per host under
//! `<app_data>/env`, with `<app_data>/env/ready.json` as the completion flag.
//!
//! That flag is the single switch in the system prompt: while it is missing,
//! unreadable, or points at a file that is gone, the pointer block announces
//! the `setup-env` manual (the model reads it, builds the venv, verifies, and
//! writes the flag); once it is valid only the one-line `ENVIRONMENT` note is
//! injected. Preparing the environment is therefore a one-time cost per host.
//! The model is not the only writer: [`auto_flag`] verifies a venv that already
//! exists (`-V` must answer 3.13) and writes the flag itself, so building the
//! venv is enough and a forgotten flag is not a dead end.
//!
//! The absolute root is exported to every child process as
//! [`ENV_ROOT_VAR`] (`env::publish`), so a script can find the venv at
//! runtime instead of baking a host-specific path into an artifact.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::manuals::{self, Manual};
use crate::prompt;

/// Folder under app data that holds the environment.
pub const ENV_DIR: &str = "env";
/// The venv inside the environment root (uv-managed CPython 3.13).
pub const VENV_DIR: &str = "py313";
/// Completion flag written by whoever prepared the environment.
pub const READY_FILE: &str = "ready.json";
/// Exported to every child process so scripts can locate the venv.
pub const ENV_ROOT_VAR: &str = "AI_CHAT_ENV_ROOT";

/// Contents of `<env root>/ready.json` — the flag and the facts the prompt
/// quotes back once the environment is ready.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// Absolute path of the venv interpreter this flag vouches for.
    pub python: String,
    /// Version the interpreter printed when the flag was written (`3.13.13`).
    pub version: String,
    /// Absolute path of `uv`, empty when it was not used.
    #[serde(default)]
    pub uv: String,
    /// ISO-8601 timestamp of the verification.
    #[serde(default)]
    pub created: String,
}

/// What the prompt should say about this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No flag, an unreadable flag, or a flag pointing at a missing file.
    Pending(String),
    /// Flag parsed and the interpreter it names exists.
    Ready(Ready),
}

/// `<app_data>/env`.
pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join(ENV_DIR)
}

/// `<env root>/py313`.
pub fn venv_dir(root: &Path) -> PathBuf {
    root.join(VENV_DIR)
}

/// `<env root>/ready.json`.
pub fn ready_path(root: &Path) -> PathBuf {
    root.join(READY_FILE)
}

/// The interpreter a venv exposes on this platform.
pub fn venv_python(root: &Path) -> PathBuf {
    let dir = venv_dir(root);
    if cfg!(windows) {
        dir.join("Scripts").join("python.exe")
    } else {
        dir.join("bin").join("python")
    }
}

/// Read the flag and decide whether the environment counts as complete.
///
/// Complete means: the flag parses **and** the interpreter it names is a real
/// file. Everything else keeps the `setup-env` manual in the prompt, which is
/// what makes a broken flag self-healing instead of silently accepted.
pub fn status(data_dir: &Path) -> Status {
    let flag = ready_path(&root(data_dir));
    let raw = match fs::read_to_string(&flag) {
        Ok(raw) => raw,
        Err(_) => return Status::Pending("the completion flag is missing".to_string()),
    };
    let ready: Ready = match serde_json::from_str(&raw) {
        Ok(ready) => ready,
        Err(_) => return Status::Pending("the completion flag is unreadable".to_string()),
    };
    // A flag that omits the interpreter still counts when the conventional
    // `<env root>/py313` interpreter is there.
    let python = if ready.python.trim().is_empty() {
        venv_python(&root(data_dir)).display().to_string()
    } else {
        ready.python.clone()
    };
    if !Path::new(&python).is_file() {
        return Status::Pending("the flag names an interpreter that is not there".to_string());
    }
    Status::Ready(Ready { python, ..ready })
}

/// Export the environment root to child processes (scripts, plugins, MCP
/// servers). Called once at startup: the flag, not this variable, decides what
/// the prompt says, so exporting it before the venv exists is harmless.
pub fn publish(data_dir: &Path) {
    std::env::set_var(ENV_ROOT_VAR, root(data_dir).as_os_str());
}

/// Self-check: when the venv is there and its interpreter reports 3.13, write
/// the completion flag instead of insisting on a manual step.
///
/// Called from [`prompt_block`], so it costs one `-V` spawn at most — and only
/// while the environment is pending **and** the interpreter file exists. An
/// interpreter that reports anything but 3.13 (a 3.14 system venv, a broken
/// wrapper) leaves the flag alone, which keeps the `setup-env` manual in the
/// prompt instead of silently answering "ready" to a mismatch.
pub fn auto_flag(data_dir: &Path) -> bool {
    if matches!(status(data_dir), Status::Ready(_)) {
        return false;
    }
    let root = root(data_dir);
    let python = venv_python(&root);
    if !python.is_file() {
        return false;
    }
    let out = match Command::new(&python).arg("-V").output() {
        Ok(out) if out.status.success() => out,
        _ => return false,
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let version = text.trim().trim_start_matches("Python ").trim().to_string();
    if !version.starts_with("3.13") {
        return false;
    }
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs().to_string())
        .unwrap_or_default();
    let ready = Ready {
        python: python.display().to_string(),
        version,
        uv: String::new(),
        created,
    };
    let Ok(body) = serde_json::to_string_pretty(&ready) else {
        return false;
    };
    if fs::create_dir_all(&root).is_err() {
        return false;
    }
    fs::write(ready_path(&root), body).is_ok()
}

/// The prompt block for this host: the `setup-env` manual while the
/// environment is pending, the one-line note once the flag is valid.
pub fn prompt_block(data_dir: &Path, workspace: &Path) -> String {
    let root = root(data_dir);
    auto_flag(data_dir);
    match status(data_dir) {
        Status::Ready(ready) => prompt::env_ready_block(&ready.python, &ready.version),
        Status::Pending(reason) => {
            let lead = prompt::env_pending_block(&root.display().to_string(), &reason);
            let pointer = manuals::pointer_for(workspace, &[Manual::SetupEnv]);
            if pointer.is_empty() {
                lead
            } else {
                format!("{lead}\n\n{pointer}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-chat-env-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_flag(data_dir: &Path, ready: &Ready) {
        let root = root(data_dir);
        fs::create_dir_all(&root).unwrap();
        fs::write(ready_path(&root), serde_json::to_string(ready).unwrap()).unwrap();
    }

    #[test]
    fn paths_sit_under_app_data() {
        let data = Path::new("/tmp/app-data");
        assert_eq!(root(data), Path::new("/tmp/app-data/env"));
        assert_eq!(venv_dir(&root(data)), Path::new("/tmp/app-data/env/py313"));
        assert_eq!(
            ready_path(&root(data)),
            Path::new("/tmp/app-data/env/ready.json")
        );
        let python = venv_python(&root(data));
        let wanted = if cfg!(windows) {
            Path::new("/tmp/app-data/env/py313/Scripts/python.exe")
        } else {
            Path::new("/tmp/app-data/env/py313/bin/python")
        };
        assert_eq!(python, wanted);
    }

    #[test]
    fn missing_flag_is_pending() {
        let data = scratch("pending");
        match status(&data) {
            Status::Pending(reason) => assert!(reason.contains("missing"), "{reason}"),
            Status::Ready(_) => panic!("an empty app data dir cannot be ready"),
        }
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn flag_pointing_nowhere_stays_pending() {
        let data = scratch("stale");
        write_flag(
            &data,
            &Ready {
                python: "/nonexistent/py313/bin/python".to_string(),
                version: "3.13.13".to_string(),
                ..Ready::default()
            },
        );
        match status(&data) {
            Status::Pending(reason) => assert!(reason.contains("not there"), "{reason}"),
            Status::Ready(_) => panic!("a flag to a missing file is not complete"),
        }
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn valid_flag_is_ready_and_the_block_is_one_line() {
        let data = scratch("ready");
        let workspace = data.join("workspace");
        let python = venv_python(&root(&data));
        fs::create_dir_all(python.parent().unwrap()).unwrap();
        fs::write(&python, b"").unwrap();
        write_flag(
            &data,
            &Ready {
                python: python.display().to_string(),
                version: "3.13.13".to_string(),
                uv: String::new(),
                created: "2026-11-28T00:00:00Z".to_string(),
            },
        );
        assert!(matches!(status(&data), Status::Ready(_)));

        let block = prompt_block(&data, &workspace);
        assert!(block.starts_with("ENVIRONMENT:"), "{block}");
        assert!(block.contains("3.13.13"), "{block}");
        assert!(!block.contains("MANUALS"), "ready must not announce a manual: {block}");
        // Nothing is seeded while the environment is complete.
        assert!(!workspace.join(prompt::MANUALS_DIR).exists());
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn pending_block_points_at_the_manual_and_seeds_it() {
        let data = scratch("announce");
        let workspace = data.join("workspace");
        let block = prompt_block(&data, &workspace);
        assert!(block.contains("(pending)"), "{block}");
        assert!(
            block.contains(".data/manuals/setup-env.md"),
            "{block}"
        );
        assert!(
            workspace.join(".data/manuals/setup-env.md").is_file(),
            "the announced manual must exist as a file"
        );
        let _ = fs::remove_dir_all(&data);
    }

    /// A venv nobody flagged still counts once its interpreter answers 3.13.
    #[cfg(unix)]
    #[test]
    fn self_check_writes_the_flag_for_a_working_venv() {
        let data = scratch("selfcheck");
        let python = venv_python(&root(&data));
        fs::create_dir_all(python.parent().unwrap()).unwrap();
        fs::write(&python, "#!/bin/sh\necho 'Python 3.13.9'\n").unwrap();
        let mut perms = fs::metadata(&python).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&python, perms).unwrap();

        assert!(auto_flag(&data), "a 3.13 venv must get a flag");
        match status(&data) {
            Status::Ready(ready) => {
                assert_eq!(ready.version, "3.13.9");
                assert_eq!(ready.python, python.display().to_string());
            }
            Status::Pending(reason) => panic!("the flag should be ready: {reason}"),
        }
        let _ = fs::remove_dir_all(&data);
    }

    /// An older interpreter must not be auto-flagged — the manual terms are
    /// still true, so the `setup-env` pointer has to stay.
    #[cfg(unix)]
    #[test]
    fn self_check_refuses_another_version() {
        let data = scratch("wrongver");
        let python = venv_python(&root(&data));
        fs::create_dir_all(python.parent().unwrap()).unwrap();
        fs::write(&python, "#!/bin/sh\necho 'Python 3.14.2'\n").unwrap();
        let mut perms = fs::metadata(&python).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&python, perms).unwrap();

        assert!(!auto_flag(&data), "3.14 must not be flagged as the 3.13 env");
        assert!(matches!(status(&data), Status::Pending(_)));
        let _ = fs::remove_dir_all(&data);
    }
}
