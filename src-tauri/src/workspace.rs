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
//!     todos/<session>/...      todos tool
//!     skills/<name>/skill.md   workspace skills (the only skills root)
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

/// Workspace skill status file (`<workspace>/.data/skills.status`).
pub fn skills_status_path(workspace_dir: &Path) -> PathBuf {
    data_root(workspace_dir).join("skills.status")
}

/// Create the workspace state directory and mark it hidden.
///
/// Called at startup and whenever the active workspace changes. Individual
/// sub-directories (`memory`, `todos`, `skills`) are still created on demand
/// by the feature that owns them, except `memory`, which the memory tool and
/// the automatic conclusion recorder create before writing.
pub fn ensure_data_layout(workspace_dir: &Path) -> std::io::Result<()> {
    let root = data_root(workspace_dir);
    std::fs::create_dir_all(&root)?;
    mark_hidden(&root);
    Ok(())
}

#[cfg(windows)]
fn mark_hidden(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN,
    };

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
        assert_eq!(skills_status_path(workspace), data.join("skills.status"));
    }

    #[test]
    fn there_is_exactly_one_skills_root_per_workspace() {
        let workspace = Path::new("/workspace");
        // The one and only skills directory: `<workspace>/.data/skills`.
        assert_eq!(skills_root(workspace), workspace.join(".data").join("skills"));
        // It is not mirrored anywhere else in the workspace layout.
        assert_ne!(skills_root(workspace), memory_root(workspace));
        assert_ne!(skills_root(workspace), todos_root(workspace));
        // The skill status file lives next to it, not inside it.
        assert_eq!(skills_status_path(workspace), workspace.join(".data").join("skills.status"));
    }
}
