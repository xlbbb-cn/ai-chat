//! The per-host Python environment (app data) and the switch in the prompt.
//!
//! The `create-skill` / `create-mcp-server` / `create-plugin` manuals recommend
//! Python, and third-party packages need a home: not the system interpreter
//! (other apps must keep working), and not the workspace either — a workspace
//! is copied to other machines while a venv carries absolute host paths and a
//! per-OS layout. So the app keeps exactly one 3.13 venv per host under
//! `<app_data>/env`, with `<app_data>/env/ready.json` as the completion flag.
//!
//! That root is not only a venv: anything the assistant installs for its own
//! use — a CLI tool, a second venv, a downloaded binary — belongs under
//! `<app_data>/env` too. Never the workspace (it travels to other machines),
//! never the system interpreter or `/usr/local` (other apps share it). The
//! `description` field of `ready.json` records what is installed there, and the
//! prompt quotes it back — but only while a shell tool is enabled, because a
//! model that cannot run anything has no use for the list.
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
use tauri::{AppHandle, Manager};

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
    /// What lives in `<env root>` besides the venv — free-form, one entry per
    /// line (e.g. `- <env root>/bin/rg (ripgrep 14.1)\n- <env root>/py313
    /// (CPython 3.13.13 + httpx, pillow)`). Quoted in the prompt while a shell
    /// tool is enabled, so a later turn knows it does not have to install the
    /// same thing twice.
    #[serde(default)]
    pub description: String,
}

/// What the prompt should say about this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No flag, an unreadable flag, or a flag pointing at a missing file.
    Pending(String),
    /// Flag parsed and the interpreter it names exists.
    Ready(Ready),
}

impl Status {
    /// True when the environment counts as prepared.
    pub fn is_ready(&self) -> bool {
        matches!(self, Status::Ready(_))
    }
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

/// The `bin/` (`Scripts/` on Windows) directory of a venv — the one that goes
/// on a child's `PATH` when the venv is mounted.
pub fn venv_bin_dir(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts")
    } else {
        venv.join("bin")
    }
}

/// The interpreter a venv exposes on this platform.
pub fn venv_python(env_root: &Path) -> PathBuf {
    let name = if cfg!(windows) { "python.exe" } else { "python" };
    venv_bin_dir(&venv_dir(env_root)).join(name)
}

/// The version recorded in the venv's `pyvenv.cfg` (`version_info = 3.13.13`).
///
/// A cheap file read instead of spawning the interpreter. It decides whether
/// the venv may be mounted into `run_cmd` children and whether [`auto_flag`]
/// may vouch for it: a venv whose base Python is not 3.13 must not silently
/// become the `python` of every `run_cmd` child.
pub fn venv_version(env_root: &Path) -> Option<String> {
    let cfg = fs::read_to_string(venv_dir(env_root).join("pyvenv.cfg")).ok()?;
    let mut version = None;
    for line in cfg.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            // `version_info` is the precise one; `version` is the fallback that
            // older virtualenv writers produce without the dotted `final` tag.
            "version_info" => return Some(value.trim().to_string()),
            "version" => version = Some(value.trim().to_string()),
            _ => {}
        }
    }
    version
}

/// Ask the interpreter directly — for a venv whose `pyvenv.cfg` is unreadable.
fn probe_version(python: &Path) -> Option<String> {
    let out = Command::new(python).arg("-V").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let version = text.trim().trim_start_matches("Python ").trim().to_string();
    (!version.is_empty()).then_some(version)
}

/// The venv `run_cmd` mounts into its children, when this host has a usable
/// 3.13 environment: the interpreter exists and `pyvenv.cfg` reports 3.13.
///
/// The completion flag is deliberately **not** required here — a workspace that
/// just gained a venv should already get it inside `run_cmd`, before the flag
/// (and with it the prompt note) is written. The version check keeps a stray
/// 3.14 venv from quietly taking over every `run_cmd` child's `python`.
pub fn mounted_venv(data_dir: &Path) -> Option<PathBuf> {
    let env_root = root(data_dir);
    if !venv_python(&env_root).is_file() {
        return None;
    }
    let version = venv_version(&env_root)?;
    version.starts_with("3.13").then(|| venv_dir(&env_root))
}

/// The app-data directory as a tool sees it: `app.path().app_data_dir()`, with
/// the state's config path as the fallback when Tauri cannot resolve it. One
/// helper so the venv mount and the experience store agree on the location.
pub fn data_dir_from_app(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(dir) = app.path().app_data_dir() {
        return Some(dir);
    }
    app.state::<crate::AppState>()
        .config_path
        .parent()
        .map(Path::to_path_buf)
}

/// Read the flag and decide whether the environment counts as complete.
///
/// Complete means: the flag parses **and** the interpreter it names is a real
/// file. Everything else keeps the `setup-env` manual in the prompt, which is
/// what makes a broken flag self-healing instead of silently accepted.
pub fn status(data_dir: &Path) -> Status {
    let flag = ready_path(&root(data_dir));
    if !flag.is_file() {
        return Status::Pending("the completion flag is missing".to_string());
    }
    let Some(ready) = read_ready(data_dir) else {
        return Status::Pending("the completion flag is unreadable".to_string());
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

/// Parse `<env root>/ready.json`, if it exists and is valid JSON.
pub fn read_ready(data_dir: &Path) -> Option<Ready> {
    let raw = fs::read_to_string(ready_path(&root(data_dir))).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Export the environment root to child processes (scripts, plugins, MCP
/// servers). Called once at startup: the flag, not this variable, decides what
/// the prompt says, so exporting it before the venv exists is harmless.
pub fn publish(data_dir: &Path) {
    std::env::set_var(ENV_ROOT_VAR, root(data_dir).as_os_str());
}

/// Self-check: when the venv is there and reports 3.13 (`pyvenv.cfg`, or `-V`
/// as a fallback), write the completion flag instead of insisting on a manual
/// step.
///
/// Called from [`prompt_block`], so it costs one `-V` spawn at most — and only
/// while the environment is pending **and** the interpreter file exists. An
/// interpreter that reports anything but 3.13 (a 3.14 system venv, a broken
/// wrapper) leaves the flag alone, which keeps the `setup-env` manual in the
/// prompt instead of silently answering "ready" to a mismatch.
pub fn auto_flag(data_dir: &Path) -> bool {
    if status(data_dir).is_ready() {
        return false;
    }
    let root = root(data_dir);
    let python = venv_python(&root);
    if !python.is_file() {
        return false;
    }
    let Some(version) = venv_version(&root).or_else(|| probe_version(&python)) else {
        return false;
    };
    if !version.starts_with("3.13") {
        return false;
    }
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs().to_string())
        .unwrap_or_default();
    // Rewriting the flag must not throw away what an earlier turn recorded
    // about the software installed here.
    let previous = read_ready(data_dir).unwrap_or_default();
    let ready = Ready {
        python: python.display().to_string(),
        version,
        uv: previous.uv,
        created,
        description: previous.description,
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
/// environment is pending, the one-line note plus the installed-software list
/// once the flag is valid.
///
/// `shells` is whether `run_cmd` / `run_shell` are enabled: the whole block is
/// about running things, so it stays out of a prompt whose model cannot.
pub fn prompt_block(data_dir: &Path, workspace: &Path, shells: bool) -> String {
    if !shells {
        return String::new();
    }
    let root = root(data_dir);
    auto_flag(data_dir);
    match status(data_dir) {
        Status::Ready(ready) => prompt::env_ready_block(
            &ready.python,
            &ready.version,
            &root.display().to_string(),
            &ready.description,
        ),
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

/// Directories from the host environment that `run_cmd` mounts on a child's
/// `PATH`, in order: `<env root>/bin` (tools the assistant installed there) and
/// the venv's `bin/` (`Scripts/` on Windows) when it really is a 3.13 one.
pub fn tool_path_dirs(data_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let bin = root(data_dir).join("bin");
    if bin.is_dir() {
        dirs.push(bin);
    }
    if let Some(venv) = mounted_venv(data_dir) {
        dirs.push(venv_bin_dir(&venv));
    }
    dirs
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
                description: String::new(),
            },
        );
        assert!(matches!(status(&data), Status::Ready(_)));

        let block = prompt_block(&data, &workspace, true);
        assert!(block.starts_with("ENVIRONMENT:"), "{block}");
        assert!(block.contains("3.13.13"), "{block}");
        assert!(!block.contains("MANUALS"), "ready must not announce a manual: {block}");
        // Nothing is seeded while the environment is complete.
        assert!(!workspace.join(prompt::MANUALS_DIR).exists());
        // An empty inventory still says where installed software belongs.
        assert!(block.contains("only the venv so far"), "{block}");
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn pending_block_points_at_the_manual_and_seeds_it() {
        let data = scratch("announce");
        let workspace = data.join("workspace");
        let block = prompt_block(&data, &workspace, true);
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

    /// `pyvenv.cfg` is the cheap source of truth for the venv's base version.
    #[test]
    fn pyvenv_cfg_reports_the_version() {
        let data = scratch("cfg");
        let env_root = root(&data);
        let venv = venv_dir(&env_root);
        fs::create_dir_all(&venv).unwrap();
        fs::write(
            venv.join("pyvenv.cfg"),
            "home = /uv/python/bin\nimplementation = CPython\nversion_info = 3.13.13\n",
        )
        .unwrap();
        assert_eq!(venv_version(&env_root).as_deref(), Some("3.13.13"));

        fs::write(venv.join("pyvenv.cfg"), "version = 3.12.9\n").unwrap();
        assert_eq!(venv_version(&env_root).as_deref(), Some("3.12.9"));
        let _ = fs::remove_dir_all(&data);
    }

    /// `run_cmd` mounts the venv only when it is a 3.13 one: a stray 3.14 venv
    /// must not become every `run_cmd` child's `python`.
    #[test]
    fn only_a_3_13_venv_is_mounted() {
        let data = scratch("mount");
        let env_root = root(&data);
        let venv = venv_dir(&env_root);
        fs::create_dir_all(venv_bin_dir(&venv)).unwrap();
        fs::write(venv_python(&env_root), "").unwrap();
        fs::write(venv.join("pyvenv.cfg"), "version_info = 3.13.13\n").unwrap();
        assert_eq!(mounted_venv(&data).as_deref(), Some(venv.as_path()));

        fs::write(venv.join("pyvenv.cfg"), "version_info = 3.14.1\n").unwrap();
        assert_eq!(mounted_venv(&data), None);

        fs::remove_file(venv_python(&env_root)).unwrap();
        assert_eq!(mounted_venv(&data), None);
        let _ = fs::remove_dir_all(&data);
    }

    /// The self-check prefers `pyvenv.cfg` over spawning the interpreter.
    #[test]
    fn self_check_reads_the_config_first() {
        let data = scratch("cfgflag");
        let env_root = root(&data);
        let venv = venv_dir(&env_root);
        fs::create_dir_all(venv_bin_dir(&venv)).unwrap();
        fs::write(venv_python(&env_root), "").unwrap();
        fs::write(venv.join("pyvenv.cfg"), "version_info = 3.13.13\n").unwrap();

        assert!(auto_flag(&data));
        match status(&data) {
            Status::Ready(ready) => assert_eq!(ready.version, "3.13.13"),
            Status::Pending(reason) => panic!("the flag should be ready: {reason}"),
        }
        let _ = fs::remove_dir_all(&data);
    }

    /// Without `run_cmd` / `run_shell` the whole block is prompt noise: nothing
    /// can be run, and the inventory would only cost tokens.
    #[test]
    fn nothing_is_injected_without_a_shell_tool() {
        let data = scratch("nogate");
        let workspace = scratch("nogate-ws");
        assert_eq!(prompt_block(&data, &workspace, false), "");
        assert!(prompt_block(&data, &workspace, true).contains("ENVIRONMENT (pending)"));
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&workspace);
    }

    /// The flag's `description` is the inventory the prompt quotes back — and
    /// only then, once a shell tool is enabled.
    #[test]
    fn the_inventory_is_quoted_in_the_ready_note() {
        let data = scratch("inventory");
        let workspace = scratch("inventory-ws");
        let env_root = root(&data);
        let venv = venv_dir(&env_root);
        fs::create_dir_all(venv_bin_dir(&venv)).unwrap();
        let python = venv_python(&env_root);
        fs::write(&python, "").unwrap();
        let ready = Ready {
            python: python.display().to_string(),
            version: "3.13.13".to_string(),
            uv: "/usr/local/bin/uv".to_string(),
            created: "0".to_string(),
            description: "- <env root>/bin/rg — ripgrep 14.1".to_string(),
        };
        fs::write(ready_path(&env_root), serde_json::to_string(&ready).unwrap()).unwrap();

        let block = prompt_block(&data, &workspace, true);
        assert!(block.contains("ENVIRONMENT:"), "unexpected block: {block}");
        assert!(
            block.contains("ripgrep 14.1"),
            "the inventory must reach the prompt: {block}"
        );
        assert_eq!(prompt_block(&data, &workspace, false), "");
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&workspace);
    }

    /// `<env root>/bin` is mounted for installed tools, ahead of the venv, and
    /// only when it exists.
    #[test]
    fn installed_tools_get_their_bin_on_the_path() {
        let data = scratch("bindir");
        let env_root = root(&data);
        let venv = venv_dir(&env_root);
        fs::create_dir_all(venv_bin_dir(&venv)).unwrap();
        fs::write(venv_python(&env_root), "").unwrap();
        fs::write(venv.join("pyvenv.cfg"), "version_info = 3.13.13\n").unwrap();
        assert_eq!(tool_path_dirs(&data), vec![venv_bin_dir(&venv)]);

        fs::create_dir_all(env_root.join("bin")).unwrap();
        assert_eq!(
            tool_path_dirs(&data),
            vec![env_root.join("bin"), venv_bin_dir(&venv)]
        );
        let _ = fs::remove_dir_all(&data);
    }

    /// Rewriting the flag (the self-check path) keeps what an earlier turn
    /// recorded about the installed software.
    #[test]
    fn the_self_check_keeps_the_inventory() {
        let data = scratch("keepinv");
        let env_root = root(&data);
        let venv = venv_dir(&env_root);
        fs::create_dir_all(venv_bin_dir(&venv)).unwrap();
        let python = venv_python(&env_root);
        fs::write(&python, "").unwrap();
        fs::write(venv.join("pyvenv.cfg"), "version_info = 3.13.13\n").unwrap();
        let stale = Ready {
            python: "/gone/python".to_string(),
            version: "3.13.13".to_string(),
            uv: "/usr/local/bin/uv".to_string(),
            created: "0".to_string(),
            description: "- <env root>/bin/rg — ripgrep".to_string(),
        };
        fs::write(ready_path(&env_root), serde_json::to_string(&stale).unwrap()).unwrap();

        assert!(auto_flag(&data), "a broken python path must be re-flagged");
        let ready = read_ready(&data).expect("flag parses");
        assert!(ready.description.contains("ripgrep"));
        assert!(ready.python.contains("py313"));
        let _ = fs::remove_dir_all(&data);
    }

}
