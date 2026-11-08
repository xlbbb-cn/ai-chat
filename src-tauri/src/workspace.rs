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
//! ```
//!
//! `.data` is part of the workspace: `file_actions` lists, searches, reads and
//! writes it like any other folder (the SQLite database excepted — see
//! `tools::paths`). On Windows the directory carries the file-system hidden
//! attribute so it stays out of Explorer. Application-level files
//! (config.json, app.log, MCP/sub-agent definitions, profiles, timers) remain
//! in the OS app data directory.

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

/// Sub-agent deliverable exchange root (`<workspace>/.data/tmp`).
///
/// Every mission (or direct `call_subagent` call) gets one batch directory
/// below it; see `agents::exchange`. The area is permanent and the user
/// cleans it manually, so it is deliberately not pruned at startup.
pub fn tmp_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("tmp")
}

/// Root of the self-built MCP servers (`<workspace>/.data/mcp`).
///
/// One directory per server (`<slug>/server.py`), written by the `mcp_self`
/// tool in self-evolution mode. The registry entry in the app-data
/// `mcp_servers.json` points at the script with a workspace-relative path, so
/// the assets travel with the workspace (and with a profile backup) while the
/// stdio child keeps starting in the workspace root.
pub fn mcp_root(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("mcp")
}

/// Workspace side of the MCP roster (`<workspace>/.data/mcp.conf`, YAML).
///
/// The assistant's self-evolution mode keeps the servers it built here, next
/// to their scripts, and never writes the app-data `mcp_servers.json`. The two
/// files are merged into the effective roster (`mcp::load_effective_servers`),
/// so a self-built server still moves with the workspace (and with a profile
/// backup) while the panel-owned entries stay application-wide.
pub fn mcp_conf_path(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("mcp.conf")
}

/// Workspace skill status file (`<workspace>/.data/skills.status`).
pub fn skills_status_path(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("skills.status")
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
}
