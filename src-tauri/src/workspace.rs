//! Workspace-scoped runtime state: the hidden `.data` directory.
//!
//! Everything that belongs to the *current workspace* lives in one hidden
//! directory inside the workspace root, so switching the workspace switches
//! the whole state (database, memory, todos, skills) in one step:
//!
//! ```text
//! <workspace>/
//!   .data/
//!     chat.db                  SQLite: history / API monitor / missions
//!     memory/{user,repo}/...   memory tool file backend
//!     memory/evolution/...     self-evolution entries (front matter) + archive/
//!     todos/<session>/...      todos tool
//!     skills/<name>/skill.md   workspace skills (the only skills root)
//!     scripts/                 run_cmd script library (on the child PATH)
//!     tmp/<batch-id>/...       sub-agent deliverable exchange area
//!     mcp/<slug>/server.*      self-built MCP servers (evolution mode)
//!     mcp.conf                 self-built server roster (merged with the
//!                              app-data mcp_servers.json)
//!     skills.status            per-workspace skill enablement + metadata
//!     timers.json              pending "continue the task" timers
//! ```
//!
//! `.data` is part of the workspace: `file_actions` lists, searches, reads and
//! writes it like any other folder (the SQLite database excepted — see
//! `tools::paths`). On Windows the directory carries the file-system hidden
//! attribute so it stays out of Explorer. Application-level files
//! (config.json, app.log, MCP/sub-agent definitions, backups) remain in the OS
//! app data directory; timers live here because a timer resumes a chat session
//! of *this* workspace.

use std::path::{Path, PathBuf};

/// Name of the hidden workspace state directory.
pub const DATA_DIR: &str = ".data";

/// Hidden workspace state root (`<workspace>/.data`).
pub fn data_root(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(DATA_DIR)
}

/// Workspace SQLite database (`<workspace>/.data/chat.db`).
pub fn db_path(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("chat.db")
}

/// Workspace memory store root (`<workspace>/.data/memory`).
pub fn memory_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("memory")
}

/// Workspace todo list root (`<workspace>/.data/todos`).
pub fn todos_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("todos")
}

/// Workspace skills root (`<workspace>/.data/skills`).
pub fn skills_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("skills")
}

/// Workspace script library root (`<workspace>/.data/scripts`).
///
/// `run_cmd` prepends this directory to the child's `PATH` and also resolves
/// script names itself, so `.ps1` / `.py` files work on Windows without
/// `PATHEXT`. Metadata is read from the script header; see
/// `tools::scripts`.
pub fn scripts_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("scripts")
}

/// The workspace scratch root (`<workspace>/.data/tmp`).
///
/// Three writers share it, and its top level is the only namespace they have in
/// common: every mission (or direct `call_subagent` call) gets one batch
/// directory here (`<stamp>-<8hex>/`, see `agents::exchange`), a PDF mention
/// caches its Markdown under `pdf/` (`pdf_mentions`), and a file pasted or
/// dropped into the composer lands as `<name>` and is addressed as `@@<name>`
/// (`workspace_files`). Each of the three owns its own lifetime: a composer paste
/// is a plain file in this folder and expires after 24 h (swept on the way into
/// the next paste), the PDF cache drops entries older than a week when the next
/// conversion is written, and batch directories are permanent — the user cleans
/// those manually through "open the exchange folder".
pub fn tmp_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("tmp")
}

/// Root of the self-built MCP servers (`<workspace>/.data/mcp`).
///
/// One directory per server (`<slug>/server.py`), written by the `mcp_self`
/// tool in self-evolution mode. The registry entry in the app-data
/// `mcp_servers.json` points at the script with a workspace-relative path, so
/// the assets travel with the workspace (and with a backup archive) while the
/// stdio child keeps starting in the workspace root.
pub fn mcp_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("mcp")
}

/// Workspace side of the MCP roster (`<workspace>/.data/mcp.conf`, YAML).
///
/// The assistant's self-evolution mode keeps the servers it built here, next
/// to their scripts, and never writes the app-data `mcp_servers.json`. The two
/// files are merged into the effective roster (`mcp::load_effective_servers`),
/// so a self-built server still moves with the workspace (and with a backup
/// archive) while the panel-owned entries stay application-wide.
pub fn mcp_conf_path(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("mcp.conf")
}

/// Workspace skill status file (`<workspace>/.data/skills.status`).
pub fn skills_status_path(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("skills.status")
}

/// Pending `timer_set` entries (`<workspace>/.data/timers.json`).
///
/// A timer re-injects a message into a chat session, and sessions live in this
/// workspace's database — so the mirror belongs here, not in the app data
/// directory (where every workspace shared one file and a timer could resume a
/// session id that does not exist in the workspace that happened to be open).
pub fn timers_path(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("timers.json")
}

/// Create the workspace state directory and mark it hidden.
///
/// Called at startup and whenever the active workspace changes. Individual
/// sub-directories (`memory`, `todos`, `skills`) are still created on demand
/// by the feature that owns them, except `memory`, which the memory tool and
/// the automatic conclusion recorder create before writing, and `scripts` /
/// `tmp`, which are created here so the script library and the deliverable
/// exchange root exist from the first launch.
pub fn ensure_data_layout(workspace_dir: &Path) -> std::io::Result<()> {
    let root = data_root(workspace_dir);
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(scripts_root(workspace_dir))?;
    // The exchange root exists from the first launch so the main agent can
    // always open/inspect it; batch directories are created on demand.
    std::fs::create_dir_all(tmp_root(workspace_dir))?;
    mark_hidden(&root);
    Ok(())
}

/// Whether `dir` can be opened as the workspace right now.
///
/// Deliberately stricter than [`prepare_workspace`]: nothing is created, so a
/// workspace whose drive was unplugged or whose folder was moved is reported
/// instead of being rebuilt as an empty workspace in its place. The `.data`
/// layout is probed too, because on a read-only volume that is the first thing
/// to fail and the failure would otherwise abort the startup.
pub fn check_startup_workspace(dir: &Path) -> Result<(), String> {
    if dir.as_os_str().is_empty() {
        return Err("the path is empty".to_string());
    }
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(format!("'{}' is not a directory", dir.display())),
        Err(err) => return Err(describe_missing(dir, &err)),
    }
    ensure_data_layout(dir).map_err(|err| {
        format!(
            "the data directory '{}' cannot be written: {err}",
            data_root(dir).display()
        )
    })
}

/// Make `dir` usable as a workspace, creating it when it is not there.
///
/// This is the explicit choice path (`set_workspace_dir`): the user picked that
/// folder, so an absent directory is created instead of rejected. Startup never
/// uses it; it goes through [`check_startup_workspace`] so a vanished workspace
/// falls back to the default one.
pub fn prepare_workspace(dir: &Path) -> Result<(), String> {
    if dir.as_os_str().is_empty() {
        return Err("the path is empty".to_string());
    }
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(format!("'{}' is not a directory", dir.display())),
        Err(_) => std::fs::create_dir_all(dir)
            .map_err(|err| format!("'{}' cannot be created ({err})", dir.display()))?,
    }
    ensure_data_layout(dir).map_err(|err| {
        format!(
            "the data directory '{}' cannot be written: {err}",
            data_root(dir).display()
        )
    })
}

/// Why a workspace path is not there, in terms of what the user has to fix.
///
/// A gap in the middle of the path means the volume is gone (`E:\gone\ws` with
/// `E:` unplugged), a gap at the end means only the folder is missing.
fn describe_missing(dir: &Path, err: &std::io::Error) -> String {
    let nearest = dir
        .ancestors()
        .skip(1)
        .find(|ancestor| !ancestor.as_os_str().is_empty() && ancestor.exists());
    // A relative path is never a volume problem.
    if nearest.is_none() && !dir.is_relative() {
        return format!(
            "the volume holding '{}' is not available ({err})",
            dir.display()
        );
    }
    format!("'{}' does not exist ({err})", dir.display())
}

/// Windows paths compare case- and separator-insensitively.
fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/').to_string();
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    normalize(a) == normalize(b)
}

/// The workspace a window opens at startup.
pub struct StartupWorkspace {
    /// The directory to open: the configured one, or the default one.
    pub path: PathBuf,
    /// `Some(reason)` when the configured workspace was unusable and `path` is
    /// the default workspace, for `app.log`.
    pub fallback: Option<String>,
}

/// Pick the workspace a window opens: the configured one when it is still
/// there, the default one otherwise.
///
/// The configuration keeps the configured path either way — a drive that is
/// only temporarily gone comes back, and the next start returns to it — so this
/// decides what this start opens and never rewrites the choice.
pub fn resolve_startup_workspace(configured: Option<&Path>, default: PathBuf) -> StartupWorkspace {
    let mut fallback: Option<String> = None;
    // Configuring the default workspace explicitly is not a fallback: the same
    // directory opens either way, and it may legitimately not exist yet.
    let candidate = configured
        .filter(|dir| !dir.as_os_str().is_empty())
        .filter(|dir| !same_path(dir, &default));
    if let Some(dir) = candidate {
        match check_startup_workspace(dir) {
            Ok(()) => {
                return StartupWorkspace {
                    path: dir.to_path_buf(),
                    fallback: None,
                }
            }
            Err(reason) => {
                fallback = Some(format!(
                    "workspace '{}' is unavailable: {reason}",
                    dir.display()
                ));
            }
        }
    }
    // The default workspace is ours to create (first launch, or the fallback
    // above). A failure is only reported: opening the database then fails the
    // same way it did before this fallback existed.
    if let Err(err) = prepare_workspace(&default) {
        let note = format!(
            "the default workspace '{}' could not be prepared: {err}",
            default.display()
        );
        fallback = Some(match fallback {
            Some(reason) => format!("{reason}; {note}"),
            None => note,
        });
    }
    StartupWorkspace {
        path: default,
        fallback,
    }
}

#[cfg(windows)]
fn mark_hidden(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    // Best effort: hiding is cosmetic, a failure must never abort startup.
    unsafe {
        SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN);
    }
}

#[cfg(not(windows))]
fn mark_hidden(_path: &Path) {
    // A dot-prefixed name is already hidden on Unix-like systems.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_paths_live_under_the_hidden_data_dir() {
        let workspace = Path::new("/workspace");
        let data = workspace.join(".data");
        assert_eq!(data_root(workspace), data);
        assert_eq!(db_path(workspace), data.join("chat.db"));
        assert_eq!(memory_root(workspace), data.join("memory"));
        assert_eq!(todos_root(workspace), data.join("todos"));
        assert_eq!(skills_root(workspace), data.join("skills"));
        assert_eq!(scripts_root(workspace), data.join("scripts"));
        assert_eq!(tmp_root(workspace), data.join("tmp"));
        assert_eq!(mcp_root(workspace), data.join("mcp"));
        assert_eq!(skills_status_path(workspace), data.join("skills.status"));
        assert_eq!(timers_path(workspace), data.join("timers.json"));
    }

    #[test]
    fn there_is_exactly_one_skills_root_per_workspace() {
        let workspace = Path::new("/workspace");
        // The one and only skills directory: `<workspace>/.data/skills`.
        assert_eq!(
            skills_root(workspace),
            workspace.join(".data").join("skills")
        );
        // It is not mirrored anywhere else in the workspace layout.
        assert_ne!(skills_root(workspace), memory_root(workspace));
        assert_ne!(skills_root(workspace), todos_root(workspace));
        // The skill status file lives next to it, not inside it.
        assert_eq!(
            skills_status_path(workspace),
            workspace.join(".data").join("skills.status")
        );
    }

    /// A private scratch root per test; removed again by the test itself.
    fn scratch(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "rabbit-workspace-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        base
    }

    #[test]
    fn an_existing_configured_workspace_is_opened_as_is() {
        let root = scratch("existing");
        let configured = root.join("mine");
        std::fs::create_dir_all(&configured).unwrap();
        let resolved = resolve_startup_workspace(Some(&configured), root.join("default"));
        assert_eq!(resolved.path, configured);
        assert!(resolved.fallback.is_none());
        // The database can be opened right after: the layout is there.
        assert!(data_root(&configured).is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_vanished_configured_workspace_falls_back_without_recreating_it() {
        let root = scratch("vanished");
        std::fs::create_dir_all(&root).unwrap();
        let gone = root.join("gone");
        let default = root.join("default");
        let resolved = resolve_startup_workspace(Some(&gone), default.clone());
        assert_eq!(resolved.path, default);
        assert!(resolved.fallback.unwrap().contains("does not exist"));
        // The rule: a workspace that is not there is not rebuilt in its place.
        assert!(!gone.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_at_the_configured_path_falls_back() {
        let root = scratch("file");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        let default = root.join("default");
        let resolved = resolve_startup_workspace(Some(&file), default.clone());
        assert_eq!(resolved.path, default);
        assert!(resolved.fallback.unwrap().contains("not a directory"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_configured_workspace_prepares_the_default_one_without_a_fallback() {
        let root = scratch("default");
        let default = root.join("workspace");
        let resolved = resolve_startup_workspace(None, default.clone());
        assert_eq!(resolved.path, default);
        assert!(resolved.fallback.is_none());
        assert!(data_root(&default).is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_default_workspace_is_not_a_fallback_even_when_missing() {
        let root = scratch("same");
        let default = root.join("workspace");
        let resolved = resolve_startup_workspace(Some(&default), default.clone());
        assert_eq!(resolved.path, default);
        assert!(resolved.fallback.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_explicit_choice_is_created_but_a_blocked_one_is_an_error() {
        let root = scratch("prepare");
        let picked = root.join("picked");
        prepare_workspace(&picked).unwrap();
        assert!(data_root(&picked).is_dir());
        let blocked = root.join("notes.txt");
        std::fs::write(&blocked, "x").unwrap();
        assert!(prepare_workspace(&blocked).is_err());
        assert!(prepare_workspace(&blocked.join("child")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
