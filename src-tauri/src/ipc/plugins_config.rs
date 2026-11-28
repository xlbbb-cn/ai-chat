//! IPC behind the plugin manager window (Settings → Run → Plugin manager opens
//! it as its own webview). Everything here forwards to `plugins::manager`, which owns
//! the bus endpoint, the instance pool and the settings files.

use serde_json::Value;
use tauri::{Emitter, Manager, State};

use crate::plugins::config::FieldError;
use crate::plugins::manager::{PluginConfigView, PluginManagerState};
use crate::plugins::{self, config};
use crate::AppState;

/// Plugins, their instances, the bus endpoint and settings modes in one trip.
#[tauri::command]
pub(crate) async fn get_plugin_manager_state(
    state: State<'_, AppState>,
) -> Result<PluginManagerState, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    Ok(state.plugins.manager_state(&workspace).await)
}

/// The settings page of one plugin: a generic form, or the URL of its own page.
#[tauri::command]
pub(crate) fn get_plugin_config(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<PluginConfigView, String> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let registry = plugins::load_plugins(&workspace);
    let plugin = registry
        .plugins
        .iter()
        .find(|plugin| plugin.manifest.id == plugin_id)
        .ok_or_else(|| format!("unknown plugin: {plugin_id}"))?;
    Ok(state.plugins.config_view(plugin))
}

/// Validate, persist and hot-push one plugin's settings.
///
/// Errors are per field so the window can mark inputs instead of showing a
/// banner: `Vec<FieldError>` with an empty `key` for a whole-page failure.
#[tauri::command]
pub(crate) async fn save_plugin_config(
    state: State<'_, AppState>,
    plugin_id: String,
    values: Value,
) -> Result<PluginConfigView, Vec<FieldError>> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let registry = plugins::load_plugins(&workspace);
    let plugin = registry
        .plugins
        .iter()
        .find(|plugin| plugin.manifest.id == plugin_id)
        .cloned()
        .ok_or_else(|| {
            vec![FieldError {
                key: String::new(),
                message: format!("unknown plugin: {plugin_id}"),
            }]
        })?;
    state.plugins.save_config(&plugin, &values).await
}

/// Drop the stored values so the schema defaults apply again.
#[tauri::command]
pub(crate) async fn reset_plugin_config(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<PluginConfigView, Vec<FieldError>> {
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let registry = plugins::load_plugins(&workspace);
    let plugin = registry
        .plugins
        .iter()
        .find(|plugin| plugin.manifest.id == plugin_id)
        .cloned()
        .ok_or_else(|| {
            vec![FieldError {
                key: String::new(),
                message: format!("unknown plugin: {plugin_id}"),
            }]
        })?;
    let path = config::values_path(&plugin.dir);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|err| {
            vec![FieldError {
                key: String::new(),
                message: format!("delete {}: {err}", path.display()),
            }]
        })?;
    }
    state.plugins.push_config(&plugin).await;
    Ok(state.plugins.config_view(&plugin))
}

/// Label of the plugin manager webview (`plugin-manager.html`). Its own
/// capability file (`capabilities/plugin-manager.json`) grants `core:default`
/// plus the native open/save dialogs the package pickers use — no fs, no
/// shell — so the window can only listen, ask the user for a file and call
/// app commands.
pub const MANAGER_WINDOW: &str = "plugin-manager";

#[tauri::command]
pub(crate) async fn open_plugin_manager(
    app: tauri::AppHandle,
    plugin_id: Option<String>,
) -> Result<(), String> {
    // WebView2 window creation blocks, and a sync command runs on the main
    // thread: `build()` then posts the navigation to the event loop it is
    // blocking and never returns — leaving a white webview that never loads
    // anything. Same rule (and same fix) as `notify::ensure_notify_window`.
    let builder = app.clone();
    let (window, created) = tauri::async_runtime::spawn_blocking(move || manager_window(&builder))
        .await
        .map_err(|err| format!("plugin window task failed: {err}"))??;
    if !created {
        window.show().map_err(|err| err.to_string())?;
        window.set_focus().map_err(|err| err.to_string())?;
    }
    if let Some(id) = plugin_id {
        // Preselect the plugin the user came from; the panel may ignore it.
        let _ = window.emit("plugin-manager-select", id);
    }
    Ok(())
}

/// Resolve the manager window, building it on first use.
///
/// Returns `(window, created)`. A new window is shown right away: hiding it
/// until `page-load` looks tidier when everything works, but a navigation that
/// never reports back then leaves a window nobody can see and nothing to
/// diagnose. The page paints its own themed shell on the first frame instead.
fn manager_window(app: &tauri::AppHandle) -> Result<(tauri::WebviewWindow, bool), String> {
    if let Some(window) = app.get_webview_window(MANAGER_WINDOW) {
        return Ok((window, false));
    }

    trace("building window");
    let window = tauri::WebviewWindowBuilder::new(
        app,
        MANAGER_WINDOW,
        tauri::WebviewUrl::App("plugin-manager.html".into()),
    )
        .title("Plugins")
        .inner_size(1100.0, 760.0)
        .min_inner_size(840.0, 540.0)
        .on_page_load(move |_window, payload| {
            trace(&format!(
                "page-load {:?} {}",
                payload.event(),
                payload.url()
            ));
        })
        .build()
        .map_err(|err| {
            trace(&format!("build failed: {err}"));
            err.to_string()
        })?;

    trace(&format!(
        "created url={:?}",
        window.url().map(|url| url.to_string())
    ));
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    probe_after_load(&window);

    Ok((window, true))
}

/// Ask the page what it actually managed to render, once, a few seconds after
/// the window exists.
///
/// A webview has no console anyone can read back, and "blank window" has a
/// dozen causes; the answer lands in the same log as the Rust-side trace so the
/// page and the window can be compared in one place.
fn probe_after_load(window: &tauri::WebviewWindow) {
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(3000));
        let script = r#"
            (() => {
              const root = document.getElementById("plugin-manager-root");
              const report = {
                href: location.href,
                ready: document.readyState,
                rootChildren: root ? root.childElementCount : -1,
                rootText: root ? (root.innerText || "").slice(0, 160) : "",
                htmlLength: document.documentElement.outerHTML.length,
                bodyBg: getComputedStyle(document.body).backgroundColor,
                theme: document.documentElement.dataset.theme || ""
              };
              try {
                window.__TAURI_INTERNALS__.invoke("plugin_window_report", {
                  message: JSON.stringify(report)
                });
              } catch (_) {}
            })();
        "#;
        let _ = window.eval(script);
    });
}

/// Beacon used by the manager page itself (`src/plugin-manager.tsx`).
#[tauri::command]
pub(crate) fn plugin_window_report(message: String) {
    trace(&format!("js {message}"));
}

/// Append a line to `%TEMP%/rabbit-plugin-window.log` and to stderr.
///
/// No state, no locks, no panic path: this also runs on the main thread while a
/// window is being created, and must never be able to stall that.
fn trace(message: &str) {
    use std::io::Write;
    println!("[plugin-window] {message}");
    let line = format!("{} {message}\n", crate::util::now_ms_u64());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("rabbit-plugin-window.log"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}
