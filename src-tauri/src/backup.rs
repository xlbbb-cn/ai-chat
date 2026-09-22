//! Profile backup: export / import of the application state.
//!
//! An exported profile is a zip archive laid out as:
//!
//! ```text
//! config.json             AppConfig
//! profiles.json           named profiles (copy of the app-data profiles.json)
//! mcp_servers.json        MCP server definitions
//! sub_agents.json         sub-agents + orchestration
//! skills/...              app-managed skills root
//! workspace/memory/...    workspace-scoped memory notes
//! workspace/todos/...     workspace-scoped todo lists
//! chat.db                 SQLite snapshot (VACUUM INTO)
//! ```
//!
//! Import applies `config.json` first (which may switch the active workspace)
//! and then restores the workspace-scoped folders into the *newly active*
//! workspace directory.

use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;
use zip::{write::FileOptions, ZipArchive, ZipWriter};

use crate::{agents, apply_config, mcp, AppConfig, AppState};

const PROFILE_EXPORT_START_EVENT: &str = "profile-export-start";
const PROFILE_EXPORT_STATUS_EVENT: &str = "profile-export-status";
const PROFILE_EXPORT_DONE_EVENT: &str = "profile-export-done";
const PROFILE_EXPORT_ERROR_EVENT: &str = "profile-export-error";
pub(crate) const PROFILE_RESTORED_EVENT: &str = "profile-restored";

/// Workspace sub-directories that are part of a profile backup.
const WORKSPACE_BACKUP_DIRS: [&str; 2] = ["memory", "todos"];

// ─── Zip helpers ─────────────────────────────────────────────────────────────

fn zip_options() -> FileOptions {
    FileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

/// Write a text entry into the archive.
fn write_text_entry(
    zip: &mut ZipWriter<fs::File>,
    name: &str,
    contents: &str,
) -> Result<(), String> {
    zip.start_file(name, zip_options())
        .map_err(|e| e.to_string())?;
    zip.write_all(contents.as_bytes())
        .map_err(|e| e.to_string())
}

/// Append `path` (file, or directory recursively) to `zip` as
/// `<zip_prefix>/<path relative to base_dir>`.
fn add_path_to_zip(
    zip: &mut ZipWriter<fs::File>,
    base_dir: &Path,
    path: &Path,
    zip_prefix: &str,
) -> Result<(), String> {
    if path.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            add_path_to_zip(zip, base_dir, &entry.path(), zip_prefix)?;
        }
        return Ok(());
    }

    if path.is_file() {
        let relative_path = path.strip_prefix(base_dir).map_err(|e| e.to_string())?;
        let entry_name = format!(
            "{zip_prefix}/{}",
            relative_path.to_string_lossy().replace('\\', "/")
        );
        zip.start_file(entry_name, zip_options())
            .map_err(|e| e.to_string())?;

        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        std::io::copy(&mut file, zip).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Resolve `relative` inside `root`, rejecting absolute paths, drive prefixes
/// and `..` components (zip-slip protection). Returns `None` when nothing is
/// left to resolve.
fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut target = root.to_path_buf();
    let mut has_component = false;
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => {
                target.push(part);
                has_component = true;
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    has_component.then_some(target)
}

// ─── Export ──────────────────────────────────────────────────────────────────

fn emit_profile_export_status(app: &AppHandle, status: &str) {
    let _ = app.emit(PROFILE_EXPORT_STATUS_EVENT, status.to_string());
}

fn escape_sql_string(value: &str) -> String {
    value.replace('\'', "''")
}

/// Snapshot the SQLite database with `VACUUM INTO` so the archive contains a
/// consistent copy while the app keeps its connection open.
fn backup_chat_db(state: &AppState) -> Result<PathBuf, String> {
    let backup_path = state
        .db_path
        .with_file_name(format!("chat-export-{}.db", Uuid::new_v4()));
    let escaped_path = escape_sql_string(&backup_path.to_string_lossy());

    let db = state.db.lock().unwrap();
    db.execute_batch(&format!("VACUUM INTO '{}';", escaped_path))
        .map_err(|e| e.to_string())?;

    Ok(backup_path)
}

fn export_profile(app: &AppHandle, profile_path: &Path) -> Result<(), String> {
    let state = app.state::<AppState>();
    if let Some(parent) = profile_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let file = fs::File::create(profile_path).map_err(|e| e.to_string())?;
    let mut zip = ZipWriter::new(file);

    emit_profile_export_status(app, "Writing config.json");
    let config_json = serde_json::to_string_pretty(&state.config.lock().unwrap().clone())
        .map_err(|e| e.to_string())?;
    write_text_entry(&mut zip, "config.json", &config_json)?;

    emit_profile_export_status(app, "Writing profiles.json");
    let profiles_json = if state.profiles_path.exists() {
        fs::read_to_string(&state.profiles_path).map_err(|e| e.to_string())?
    } else {
        serde_json::to_string_pretty(&serde_json::json!({ "profiles": [] }))
            .map_err(|e| e.to_string())?
    };
    write_text_entry(&mut zip, "profiles.json", &profiles_json)?;

    emit_profile_export_status(app, "Writing mcp_servers.json");
    let mcp_json = serde_json::json!({ "servers": mcp::load_servers(&state.mcp_servers_path) });
    let mcp_json = serde_json::to_string_pretty(&mcp_json).map_err(|e| e.to_string())?;
    write_text_entry(&mut zip, "mcp_servers.json", &mcp_json)?;

    emit_profile_export_status(app, "Writing sub_agents.json");
    let sub_agents_json = if state.agents_config_path.exists() {
        fs::read_to_string(&state.agents_config_path).map_err(|e| e.to_string())?
    } else {
        serde_json::to_string_pretty(&agents::AgentsConfig::default()).map_err(|e| e.to_string())?
    };
    write_text_entry(&mut zip, "sub_agents.json", &sub_agents_json)?;

    emit_profile_export_status(app, "Packing skills directory");
    zip.add_directory("skills/", zip_options())
        .map_err(|e| e.to_string())?;
    add_path_to_zip(&mut zip, &state.skills_dir, &state.skills_dir, "skills")?;

    // Workspace-scoped state: memory notes and todo lists live inside the
    // workspace directory, so they are packed as `workspace/<dir>/...`.
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    for dir_name in WORKSPACE_BACKUP_DIRS {
        let dir_path = workspace_dir.join(dir_name);
        if !dir_path.is_dir() {
            continue;
        }
        emit_profile_export_status(app, &format!("Packing workspace/{dir_name} directory"));
        let zip_prefix = format!("workspace/{dir_name}");
        zip.add_directory(format!("{zip_prefix}/"), zip_options())
            .map_err(|e| e.to_string())?;
        add_path_to_zip(&mut zip, &dir_path, &dir_path, &zip_prefix)?;
    }

    emit_profile_export_status(app, "Exporting chat.db (sending disabled during export)");
    let chat_db_backup = backup_chat_db(&state)?;
    let db_result = (|| -> Result<(), String> {
        zip.start_file("chat.db", zip_options())
            .map_err(|e| e.to_string())?;
        let mut db_file = fs::File::open(&chat_db_backup).map_err(|e| e.to_string())?;
        std::io::copy(&mut db_file, &mut zip).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = fs::remove_file(&chat_db_backup);
    db_result?;

    zip.finish().map_err(|e| e.to_string())?;
    state.logger.lock().unwrap().log(
        "INFO",
        &format!("Profile saved to {}", profile_path.display()),
    );
    Ok(())
}

/// Run the export on a background thread and report progress through the
/// `profile-export-*` events.
pub(crate) fn spawn_profile_export(app: AppHandle, profile_path: PathBuf) {
    std::thread::spawn(move || {
        let _ = app.emit(PROFILE_EXPORT_START_EVENT, ());
        emit_profile_export_status(&app, "Preparing to export profile...");

        match export_profile(&app, &profile_path) {
            Ok(()) => {
                let _ = app.emit(PROFILE_EXPORT_DONE_EVENT, ());
            }
            Err(err) => {
                let _ = app.emit(PROFILE_EXPORT_ERROR_EVENT, err);
            }
        }
    });
}

// ─── Import ──────────────────────────────────────────────────────────────────

/// Restore the workspace-scoped folders (`memory`, `todos`) from the archive.
///
/// Folders present in the archive replace their on-disk counterpart; folders
/// that are absent from the archive are left untouched. Entry paths are
/// sanitized so a crafted archive cannot write outside the workspace.
fn restore_workspace_dirs(
    state: &AppState,
    archive: &mut ZipArchive<fs::File>,
) -> Result<(), String> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();

    for dir_name in WORKSPACE_BACKUP_DIRS {
        let zip_prefix = format!("workspace/{dir_name}/");
        let entry_names: Vec<String> = (0..archive.len())
            .filter_map(|index| {
                archive
                    .by_index(index)
                    .ok()
                    .map(|entry| entry.name().to_string())
            })
            .filter(|name| name.starts_with(&zip_prefix))
            .collect();
        if entry_names.is_empty() {
            continue;
        }

        let target_root = workspace_dir.join(dir_name);
        if target_root.exists() {
            fs::remove_dir_all(&target_root).map_err(|e| e.to_string())?;
        }
        fs::create_dir_all(&target_root).map_err(|e| e.to_string())?;

        for name in entry_names {
            let relative = &name[zip_prefix.len()..];
            let Some(target_path) = safe_join(&target_root, relative) else {
                continue;
            };
            if relative.ends_with('/') {
                fs::create_dir_all(&target_path).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut entry = archive.by_name(&name).map_err(|e| e.to_string())?;
            let mut out = fs::File::create(&target_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }

        state.logger.lock().unwrap().log(
            "INFO",
            &format!("Restored workspace/{dir_name} from profile archive"),
        );
    }

    Ok(())
}

pub(crate) fn import_profile(app: &AppHandle, profile_path: &Path) -> Result<(), String> {
    let state = app.state::<AppState>();
    let file = fs::File::open(profile_path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;

    let config: AppConfig = {
        let mut config_file = archive
            .by_name("config.json")
            .map_err(|_| "missing config.json in profile archive".to_string())?;
        let mut config_json = String::new();
        config_file
            .read_to_string(&mut config_json)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&config_json).map_err(|e| e.to_string())?
    };

    apply_config(app, &state, config)?;

    if let Ok(mut agents_file) = archive.by_name("sub_agents.json") {
        let mut sub_agents_json = String::new();
        agents_file
            .read_to_string(&mut sub_agents_json)
            .map_err(|e| e.to_string())?;
        fs::write(&state.agents_config_path, sub_agents_json).map_err(|e| e.to_string())?;
    }

    if let Ok(mut profiles_file) = archive.by_name("profiles.json") {
        let mut profiles_json = String::new();
        profiles_file
            .read_to_string(&mut profiles_json)
            .map_err(|e| e.to_string())?;
        // Only restore a real profiles file: archives written before this
        // module existed stored a copy of config.json under this name.
        let is_profiles_file = serde_json::from_str::<serde_json::Value>(&profiles_json)
            .ok()
            .and_then(|value| value.get("profiles").map(|profiles| profiles.is_array()))
            .unwrap_or(false);
        if is_profiles_file {
            fs::write(&state.profiles_path, profiles_json).map_err(|e| e.to_string())?;
        }
    }

    restore_workspace_dirs(&state, &mut archive)?;

    let _ = app.emit(PROFILE_RESTORED_EVENT, ());
    state.logger.lock().unwrap().log(
        "INFO",
        &format!("Profile restored from {}", profile_path.display()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_keeps_nested_paths_inside_root() {
        let root = Path::new("/workspace/memory");
        assert_eq!(
            safe_join(root, "repo/note.md"),
            Some(root.join("repo").join("note.md"))
        );
        assert_eq!(
            safe_join(root, "./repo/./note.md"),
            Some(root.join("repo").join("note.md"))
        );
    }

    #[test]
    fn safe_join_rejects_traversal_and_absolute_paths() {
        let root = Path::new("/workspace/todos");
        assert_eq!(safe_join(root, "../outside.md"), None);
        assert_eq!(safe_join(root, "a/../../outside.md"), None);
        assert_eq!(safe_join(root, "/etc/passwd"), None);
        assert_eq!(safe_join(root, ""), None);
        assert_eq!(safe_join(root, "./"), None);
    }
}
