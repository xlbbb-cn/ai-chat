//! Commands for the plugin system — thin forwarders.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.
//! Everything here is the *panel* side (discovery, lifecycle, diagnostics); the
//! model's own route is the `plugin` tool (`plugins::tool`), not IPC.

use crate::plugins;
use crate::AppState;
use std::path::Path;
use tauri::State;

#[tauri::command]
pub(crate) fn list_plugins(state: State<'_, AppState>) -> Vec<plugins::PluginView> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    plugins::load_plugins(&workspace)
        .plugins
        .iter()
        .map(plugins::plugin_view)
        .collect()
}

#[tauri::command]
pub(crate) async fn get_plugin_diagnostics(
    state: State<'_, AppState>,
) -> Result<plugins::PluginDiagnostics, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let registry = plugins::load_plugins(&workspace);
    Ok(plugins::PluginDiagnostics {
        plugins: registry.plugins.iter().map(plugins::plugin_view).collect(),
        instances: state.plugins.status().await,
        issues: registry.issues,
        dir: plugins::plugins_dir(&workspace).to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub(crate) async fn start_plugin(state: State<'_, AppState>, id: String) -> Result<usize, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let registry = plugins::load_plugins(&workspace);
    let plugin = registry
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("plugin '{id}' is not installed"))?;
    let count = state.plugins.start(&plugin).await?;
    state.plugins.bump_revision();
    Ok(count)
}

#[tauri::command]
pub(crate) async fn stop_plugin(state: State<'_, AppState>, id: String) -> Result<usize, String> {
    let stopped = state.plugins.stop(&id).await;
    state.plugins.bump_revision();
    Ok(stopped)
}

/// Re-read every `plugin.json` and retire the instances of plugins that are
/// gone or disabled. Returns the (possibly broken) manifest count + issues.
#[tauri::command]
pub(crate) async fn reload_plugins(
    state: State<'_, AppState>,
) -> Result<plugins::PluginDiagnostics, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let registry = plugins::load_plugins(&workspace);
    for view in state.plugins.status().await {
        let keep = registry
            .get(&view.plugin_id)
            .map(|plugin| plugin.manifest.enabled)
            .unwrap_or(false);
        if !keep {
            state.plugins.stop(&view.plugin_id).await;
        }
    }
    state.plugins.bump_revision();
    Ok(plugins::PluginDiagnostics {
        plugins: registry.plugins.iter().map(plugins::plugin_view).collect(),
        instances: state.plugins.status().await,
        issues: registry.issues,
        dir: plugins::plugins_dir(&workspace).to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub(crate) fn save_plugin_manifest(
    state: State<'_, AppState>,
    manifest: plugins::PluginManifest,
) -> Result<plugins::PluginView, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let plugin = plugins::save_plugin(&workspace, &manifest)?;
    state.plugins.bump_revision();
    Ok(plugins::plugin_view(&plugin))
}

#[tauri::command]
pub(crate) fn delete_plugin(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let removed = plugins::delete_plugin(&workspace, &id)?;
    if removed {
        state.plugins.bump_revision();
    }
    Ok(removed)
}

#[tauri::command]
pub(crate) fn get_plugin_logs(
    state: State<'_, AppState>,
    plugin_id: String,
    limit: Option<usize>,
) -> Vec<plugins::PluginLogEntry> {
    state.plugins.logs(&plugin_id, limit.unwrap_or(0))
}

#[tauri::command]
pub(crate) fn clear_plugin_logs(state: State<'_, AppState>, plugin_id: String) {
    state.plugins.sink().clear_logs(&plugin_id);
}

#[tauri::command]
pub(crate) fn get_plugin_bus_records(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Vec<plugins::BusRecord> {
    state.plugins.bus_records(limit.unwrap_or(0))
}

/// Queued instance→host events (`drain: true` empties the queue).
#[tauri::command]
pub(crate) fn get_plugin_events(
    state: State<'_, AppState>,
    plugin_id: String,
    drain: Option<bool>,
) -> Vec<plugins::PluginEvent> {
    state
        .plugins
        .sink()
        .events(&plugin_id, drain.unwrap_or(false))
}

/// Pack one installed plugin into the `.zip` path the caller picked.
///
/// Secrets the plugin's settings schema declares are stripped before packing
/// (`plugins::package`), so an archive can be shared without carrying a token.
#[tauri::command]
pub(crate) fn export_plugin_package(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> Result<plugins::package::PluginExportReport, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    plugins::package::export_plugin(&workspace, &id, Path::new(&path))
}

/// Install a package the caller picked.
///
/// The plugin the archive replaces is retired *before* its folder is touched:
/// an instance keeps the old directory open, and Windows refuses the rename
/// while it does. Stopping it afterwards covers a package whose id only the
/// import could resolve.
#[tauri::command]
pub(crate) async fn import_plugin_package(
    state: State<'_, AppState>,
    path: String,
) -> Result<plugins::package::PluginImportReport, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let source = Path::new(&path);
    let replaced = plugins::package::package_id(source);
    let was_running = match replaced.as_deref() {
        Some(id) => state.plugins.stop(id).await > 0,
        None => false,
    };
    let report = match plugins::package::import_plugin(&workspace, source) {
        Ok(report) => report,
        Err(err) => {
            // An import that failed must not also leave offline the plugin it
            // could not replace.
            if was_running {
                let registry = plugins::load_plugins(&workspace);
                if let Some(plugin) = replaced.as_deref().and_then(|id| registry.get(id)) {
                    let _ = state.plugins.start(plugin).await;
                    state.plugins.bump_revision();
                }
            }
            return Err(err);
        }
    };
    state.plugins.stop(&report.plugin.id).await;
    state.plugins.bump_revision();
    Ok(report)
}
