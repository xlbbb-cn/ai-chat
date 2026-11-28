//! Plugin system: manifests, instances and the plugin message bus.
//!
//! ```text
//! LLM  ->  tool ("plugin" / "plugin_<slug>_<tool>")
//!       ->  Plugin message bus        (ids, topics, audit, correlation)
//!       ->  plugin instance x N       (long-lived child processes, line JSON)
//! ```
//!
//! A plugin is a folder under `<workspace>/.data/plugins/<slug>/` with a
//! `plugin.json` manifest and an entry script (or its own `command`). Plugins
//! are **workspace-scoped**, like skills, scripts and the self-built MCP
//! roster: switching the workspace retires every instance, and the next call
//! re-spawns them rooted in the new tree.
//!
//! A plugin declares `instances: N`. The host keeps N child processes alive,
//! spreads requests over them (least in-flight, round-robin on ties) and
//! fails over to a sibling instance when one cannot take the message — that is
//! what "x N" buys: parallel capacity plus resilience without any thread pool
//! inside the plugin.
//!
//! The bus (`bus.rs`) is the only path between the tool layer and an instance.
//! Nothing writes to a child's stdin without a bus id, which is what makes
//! requests cancellable, observable (`plugin-bus` event) and testable.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub mod bus;
pub mod config;
pub mod instance;
pub mod link;
pub mod manager;
pub mod package;
#[cfg(test)]
mod tests;
pub mod tool;

pub use bus::{BusRecord, PluginBus, Reply};
pub use instance::{Instance, InstanceState};

// ─── Limits ──────────────────────────────────────────────────────────────────

/// Folder under `<workspace>/.data/` holding the plugins.
pub const PLUGINS_DIR_NAME: &str = "plugins";
/// Hard ceiling on installed plugins (a prompt/tool-list guard, not a disk one).
pub const MAX_PLUGINS: usize = 32;
/// Hard ceiling on `instances` per plugin.
pub const MAX_INSTANCES_PER_PLUGIN: u32 = 8;
/// Default `call_timeout_ms`.
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;
/// Ceiling for a single call, in ms.
pub const MAX_CALL_TIMEOUT_MS: u64 = 600_000;
/// Cap on a plugin's own log ring.
pub const MAX_LOG_ENTRIES: usize = 200;
/// Cap on the per-plugin queue of instance events the model can poll.
pub const MAX_INSTANCE_EVENTS: usize = 64;
/// Cap on a single result handed back to the model.
pub const MAX_TOOL_RESULT_CHARS: usize = 24_000;

pub const LOG_INFO: &str = "info";
pub const LOG_WARN: &str = "warn";
pub const LOG_ERROR: &str = "error";

// ─── Manifest ────────────────────────────────────────────────────────────────

/// How the entry script is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginRuntime {
    /// Derived from the entry's extension.
    #[default]
    Auto,
    Python,
    Node,
    /// `pwsh` on Windows, `bash` elsewhere.
    Shell,
    /// The entry is itself executable (compiled binary, file with a shebang).
    Exec,
}

impl PluginRuntime {
    pub fn name(&self) -> &'static str {
        match self {
            PluginRuntime::Auto => "auto",
            PluginRuntime::Python => "python",
            PluginRuntime::Node => "node",
            PluginRuntime::Shell => "shell",
            PluginRuntime::Exec => "exec",
        }
    }

    /// The interpreter plus the flags that come before the entry path.
    pub fn interpreter(&self) -> Result<(String, Vec<String>), String> {
        let flags = |values: &[&str]| values.iter().map(|value| value.to_string()).collect();
        match self {
            PluginRuntime::Python => {
                resolve_first(&["python", "python3", "py"])
                    .map(|program| (program, flags(&["-u"])))
                    .ok_or_else(|| {
                        "Python was not found on PATH (install it or set the plugin's `command`)"
                            .to_string()
                    })
            }
            PluginRuntime::Node => resolve_first(&["node", "nodejs"])
                .map(|program| (program, flags(&[])))
                .ok_or_else(|| {
                    "Node.js was not found on PATH (install it or set the plugin's `command`)"
                        .to_string()
                }),
            PluginRuntime::Shell => {
                #[cfg(windows)]
                {
                    resolve_first(&["pwsh", "powershell"])
                        .map(|program| {
                            (
                                program,
                                flags(&["-NoProfile", "-NonInteractive", "-File"]),
                            )
                        })
                        .ok_or_else(|| "PowerShell (pwsh) was not found on PATH".to_string())
                }
                #[cfg(not(windows))]
                {
                    resolve_first(&["bash", "sh"])
                        .map(|program| (program, flags(&[])))
                        .ok_or_else(|| "bash was not found on PATH".to_string())
                }
            }
            PluginRuntime::Exec | PluginRuntime::Auto => Err(
                "this runtime is started from the entry path itself, not an interpreter".to_string(),
            ),
        }
    }
}

fn resolve_first(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|candidate| {
        crate::executable::resolve_executable(candidate)
            .ok()
            .map(|path| path.to_string_lossy().to_string())
    })
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            description: String::new(),
            runtime: PluginRuntime::Auto,
            command: String::new(),
            args: Vec::new(),
            entry: String::new(),
            env: HashMap::new(),
            instances: default_instances(),
            enabled: true,
            auto_start: true,
            call_timeout_ms: 0,
            tools: Vec::new(),
            subscriptions: Vec::new(),
            source: String::new(),
            transport: PluginTransport::Spawn,
            config_schema: String::new(),
            config_page: String::new(),
        }
    }
}

/// One tool a plugin exposes to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTool {
    /// Tool name inside the plugin (`now`). Sanitised into the LLM-facing
    /// function name together with the plugin id.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON schema of the arguments. Empty → an object with no properties.
    #[serde(default)]
    pub parameters: Value,
    /// Bus topic the request goes to. Empty → the plugin id.
    #[serde(default)]
    pub topic: String,
}

/// How a plugin provides its instances.
///
/// A plugin is a *service on the bus*, which a Rust, Node or Python process
/// can serve two ways: the app spawns it as a child and speaks stdio
/// (`spawn`), or it runs under its own supervisor and dials the bus endpoint
/// (`service`). `both` accepts either for the same plugin id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PluginTransport {
    #[default]
    Spawn,
    Service,
    Both,
}

/// `plugin.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Always the folder name; a mismatch is rejected at load time.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub runtime: PluginRuntime,
    /// Executable override; when set, `entry` is appended as the last argument.
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Entry script, relative to the plugin folder.
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_instances")]
    pub instances: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Start the instances when the app opens the workspace (the first call
    /// starts them anyway; this is only about the warm pool).
    #[serde(default = "default_true")]
    pub auto_start: bool,
    /// Per-call reply deadline. `0` → [`DEFAULT_CALL_TIMEOUT_MS`].
    #[serde(default)]
    pub call_timeout_ms: u64,
    #[serde(default)]
    pub tools: Vec<PluginTool>,
    /// Bus topics this plugin listens to (`+/#` wildcards allowed). Empty →
    /// `<id>/#`.
    #[serde(default)]
    pub subscriptions: Vec<String>,
    /// `""` / `user`, or `self-evolution` for one the assistant wrote.
    #[serde(default)]
    pub source: String,
    /// Spawned child, attached service, or either.
    #[serde(default)]
    pub transport: PluginTransport,
    /// Declarative settings page; defaults to `config.schema.yaml` when that
    /// file exists.
    #[serde(default)]
    pub config_schema: String,
    /// Bespoke settings page (plugin-relative HTML), for a plugin that needs
    /// more than a form. The schema still owns values and validation.
    #[serde(default)]
    pub config_page: String,
}

fn default_instances() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

impl PluginManifest {
    /// `instances` clamped to a sane range (at least one process).
    pub fn instance_count(&self) -> u32 {
        self.instances.clamp(1, MAX_INSTANCES_PER_PLUGIN)
    }

    /// Whether the app may start this plugin itself.
    pub fn spawns_process(&self) -> bool {
        matches!(self.transport, PluginTransport::Spawn | PluginTransport::Both)
    }

    /// Whether this plugin may attach to the bus endpoint.
    pub fn accepts_service(&self) -> bool {
        matches!(self.transport, PluginTransport::Service | PluginTransport::Both)
    }

    /// Schema file name inside the plugin folder.
    pub fn schema_file(&self) -> &str {
        if self.config_schema.is_empty() {
            config::SCHEMA_FILE
        } else {
            &self.config_schema
        }
    }

    pub fn call_timeout(&self) -> std::time::Duration {
        let millis = if self.call_timeout_ms == 0 {
            DEFAULT_CALL_TIMEOUT_MS
        } else {
            self.call_timeout_ms.min(MAX_CALL_TIMEOUT_MS)
        };
        std::time::Duration::from_millis(millis)
    }

    /// The runtime to launch with, resolving `auto` from the entry extension.
    pub fn resolved_runtime(&self) -> PluginRuntime {
        if self.runtime != PluginRuntime::Auto {
            return self.runtime;
        }
        runtime_from_entry(&self.entry)
    }

    /// Effective subscriptions (default: everything under the plugin id).
    pub fn effective_subscriptions(&self) -> Vec<String> {
        if self.subscriptions.is_empty() {
            return vec![format!("{}/#", self.id)];
        }
        self.subscriptions.clone()
    }

    /// Sanity checks run at load time; the first failure is reported to the UI.
    fn validate(&self) -> Result<(), String> {
        if self.entry.trim().is_empty() && self.command.trim().is_empty() {
            return Err("the manifest has neither `entry` nor `command`".to_string());
        }
        if !self.command.trim().is_empty() {
            return Ok(());
        }
        match self.resolved_runtime() {
            PluginRuntime::Exec => Ok(()),
            PluginRuntime::Auto => Err(
                "`runtime` is `auto` but the entry extension is not recognised; set `runtime`"
                    .to_string(),
            ),
            _ => Ok(()),
        }
    }
}

fn runtime_from_entry(entry: &str) -> PluginRuntime {
    let extension = Path::new(entry)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "py" => PluginRuntime::Python,
        "js" | "mjs" | "cjs" => PluginRuntime::Node,
        "ps1" | "sh" | "bash" => PluginRuntime::Shell,
        _ => PluginRuntime::Auto,
    }
}

/// A manifest plus where it lives.
#[derive(Debug, Clone)]
pub struct Plugin {
    /// Workspace root the plugin belongs to (children start in `dir`; the
    /// workspace reaches them through `PLUGIN_WORKSPACE` + the hello line).
    pub workspace: PathBuf,
    pub dir: PathBuf,
    pub manifest: PluginManifest,
}

impl Plugin {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// LLM-facing tool names of this plugin (`plugin_<slug>_<tool>`).
    pub fn adapted_tool_names(&self) -> Vec<String> {
        self.manifest
            .tools
            .iter()
            .map(|tool| adapt_tool_name(&self.manifest.id, &tool.name))
            .collect()
    }
}

/// Sanitise one name segment for an OpenAI function name.
pub fn slugify_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// `plugin_<slug>_<tool>` — the function name the model sees.
pub fn adapt_tool_name(plugin_id: &str, tool: &str) -> String {
    format!(
        "plugin_{}_{}",
        slugify_segment(plugin_id),
        slugify_segment(tool)
    )
}

// ─── Views (IPC / tool output) ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub runtime: String,
    pub entry: String,
    pub command: String,
    pub enabled: bool,
    pub auto_start: bool,
    pub instances: u32,
    pub tools: Vec<String>,
    pub subscriptions: Vec<String>,
    pub source: String,
    pub dir: String,
}

impl PluginView {
    fn of(plugin: &Plugin) -> Self {
        Self {
            id: plugin.manifest.id.clone(),
            name: if plugin.manifest.name.trim().is_empty() {
                plugin.manifest.id.clone()
            } else {
                plugin.manifest.name.clone()
            },
            version: plugin.manifest.version.clone(),
            description: plugin.manifest.description.clone(),
            runtime: plugin.manifest.resolved_runtime().name().to_string(),
            entry: plugin.manifest.entry.clone(),
            command: plugin.manifest.command.clone(),
            enabled: plugin.manifest.enabled,
            auto_start: plugin.manifest.auto_start,
            instances: plugin.manifest.instance_count(),
            tools: plugin.manifest.tools.iter().map(|tool| tool.name.clone()).collect(),
            subscriptions: plugin.manifest.effective_subscriptions(),
            source: plugin.manifest.source.clone(),
            dir: plugin.dir.to_string_lossy().to_string(),
        }
    }
}

/// Public view constructor for the IPC layer.
pub fn plugin_view(plugin: &Plugin) -> PluginView {
    PluginView::of(plugin)
}

#[derive(Debug, Clone, Serialize)]
pub struct InstanceView {
    pub plugin_id: String,
    pub label: String,
    pub index: u32,
    pub generation: u32,
    pub state: String,
    pub pid: Option<u32>,
    pub inflight: u32,
    pub restarts: u32,
    pub started_at_ms: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginIssue {
    pub dir: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginDiagnostics {
    pub plugins: Vec<PluginView>,
    pub instances: Vec<InstanceView>,
    pub issues: Vec<PluginIssue>,
    pub dir: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginLogEntry {
    pub ts_ms: u64,
    pub level: String,
    pub message: String,
}

/// An event an instance pushed without a request (the asynchronous half of the
/// bus). Queued per plugin so the model can poll it with `plugin` action
/// `events`, and emitted live as `plugin-bus` for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct PluginEvent {
    pub plugin_id: String,
    pub instance: String,
    pub topic: String,
    pub payload: Value,
    pub ts_ms: u64,
}

// ─── Discovery ───────────────────────────────────────────────────────────────

pub fn plugins_dir(workspace: &Path) -> PathBuf {
    crate::workspace::data_root(workspace).join(PLUGINS_DIR_NAME)
}

/// Everything under `<workspace>/.data/plugins` that looks like a plugin.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    pub plugins: Vec<Plugin>,
    pub issues: Vec<PluginIssue>,
}

impl PluginRegistry {
    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|plugin| plugin.manifest.id == id)
    }

    pub fn enabled(&self) -> impl Iterator<Item = &Plugin> {
        self.plugins.iter().filter(|plugin| plugin.manifest.enabled)
    }

    /// Resolve an LLM-facing function name back to `(plugin, tool)`.
    pub fn resolve_adapted(&self, function_name: &str) -> Option<(&Plugin, &PluginTool)> {
        for plugin in self.enabled() {
            for tool in &plugin.manifest.tools {
                if adapt_tool_name(&plugin.manifest.id, &tool.name) == function_name {
                    return Some((plugin, tool));
                }
            }
        }
        None
    }
}

pub fn load_plugins(workspace: &Path) -> PluginRegistry {
    let mut registry = PluginRegistry::default();
    let root = plugins_dir(workspace);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(_) => return registry,
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let manifest_path = dir.join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        let folder = dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_string();
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(err) => {
                registry.issues.push(PluginIssue {
                    dir: dir.to_string_lossy().to_string(),
                    error: format!("plugin.json could not be read: {err}"),
                });
                continue;
            }
        };
        let mut manifest: PluginManifest = match serde_json::from_str(&text) {
            Ok(manifest) => manifest,
            Err(err) => {
                registry.issues.push(PluginIssue {
                    dir: dir.to_string_lossy().to_string(),
                    error: format!("plugin.json is not valid: {err}"),
                });
                continue;
            }
        };
        let slug = slugify_segment(&folder);
        if slug.is_empty() {
            registry.issues.push(PluginIssue {
                dir: dir.to_string_lossy().to_string(),
                error: "the folder name is not a usable plugin id".to_string(),
            });
            continue;
        }
        if !manifest.id.trim().is_empty() && slugify_segment(&manifest.id) != slug {
            registry.issues.push(PluginIssue {
                dir: dir.to_string_lossy().to_string(),
                error: format!(
                    "manifest id '{}' does not match the folder '{slug}'",
                    manifest.id
                ),
            });
            continue;
        }
        manifest.id = slug;
        if manifest.name.trim().is_empty() {
            manifest.name = manifest.id.clone();
        }
        manifest.instances = manifest.instance_count();
        manifest.subscriptions = manifest
            .effective_subscriptions()
            .into_iter()
            .filter_map(|topic| bus::normalize_topic(&topic, true).ok())
            .collect();
        if manifest.subscriptions.is_empty() {
            manifest.subscriptions = vec![format!("{}/#", manifest.id)];
        }
        if let Err(err) = manifest.validate() {
            registry.issues.push(PluginIssue {
                dir: dir.to_string_lossy().to_string(),
                error: err,
            });
            continue;
        }
        if registry.plugins.len() >= MAX_PLUGINS {
            registry.issues.push(PluginIssue {
                dir: dir.to_string_lossy().to_string(),
                error: format!("only the first {MAX_PLUGINS} plugins are loaded"),
            });
            break;
        }
        registry.plugins.push(Plugin {
            workspace: workspace.to_path_buf(),
            dir,
            manifest,
        });
    }
    registry
}

/// Persist one manifest (the panel writes what it edits).
pub fn save_plugin(workspace: &Path, manifest: &PluginManifest) -> Result<Plugin, String> {
    let slug = slugify_segment(&manifest.id);
    if slug.is_empty() {
        return Err("The plugin id is empty or has no letters/digits.".to_string());
    }
    let mut stored = manifest.clone();
    stored.id = slug;
    if let Err(err) = stored.validate() {
        return Err(err);
    }
    let dir = plugins_dir(workspace).join(&stored.id);
    std::fs::create_dir_all(&dir).map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    let path = dir.join("plugin.json");
    let text = serde_json::to_string_pretty(&stored).map_err(|err| err.to_string())?;
    std::fs::write(&path, format!("{text}\n"))
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    Ok(Plugin {
        workspace: workspace.to_path_buf(),
        dir,
        manifest: stored,
    })
}

/// Remove a plugin folder (manifest and its files) from the workspace.
pub fn delete_plugin(workspace: &Path, id: &str) -> Result<bool, String> {
    let slug = slugify_segment(id);
    if slug.is_empty() || slug != id {
        return Err(format!("'{id}' is not a valid plugin id"));
    }
    let dir = plugins_dir(workspace).join(&slug);
    if !dir.is_dir() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir).map_err(|err| format!("could not delete {}: {err}", dir.display()))?;
    Ok(true)
}

// ─── Sink: the shared emitter the reader tasks use ───────────────────────────

/// Everything a background reader task needs (cheap to clone, no `AppState`
/// dependency — which is also what lets the tests drive it without Tauri).
#[derive(Clone)]
pub struct Sink {
    app: Option<AppHandle>,
    pub bus: Arc<PluginBus>,
    logs: Arc<Mutex<HashMap<String, VecDeque<PluginLogEntry>>>>,
    events: Arc<Mutex<HashMap<String, VecDeque<PluginEvent>>>>,
}

impl Sink {
    fn new(app: Option<AppHandle>) -> Self {
        Self {
            app,
            bus: Arc::new(PluginBus::new()),
            logs: Arc::new(Mutex::new(HashMap::new())),
            events: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn log(&self, plugin_id: &str, level: &str, message: impl Into<String>) {
        let entry = PluginLogEntry {
            ts_ms: crate::util::now_ms_u64(),
            level: level.to_string(),
            message: message.into(),
        };
        {
            let mut logs = self.logs.lock().unwrap();
            let ring = logs.entry(plugin_id.to_string()).or_default();
            if ring.len() >= MAX_LOG_ENTRIES {
                ring.pop_front();
            }
            ring.push_back(entry.clone());
        }
        if let Some(app) = &self.app {
            let _ = app.emit(
                crate::events::PLUGIN_LOG,
                json!({
                    "plugin_id": plugin_id,
                    "ts_ms": entry.ts_ms,
                    "level": entry.level,
                    "message": entry.message,
                }),
            );
        }
    }

    pub fn logs(&self, plugin_id: &str, limit: usize) -> Vec<PluginLogEntry> {
        let logs = self.logs.lock().unwrap();
        let Some(ring) = logs.get(plugin_id) else {
            return vec![];
        };
        let all: Vec<PluginLogEntry> = ring.iter().cloned().collect();
        if limit == 0 || limit >= all.len() {
            return all;
        }
        all[all.len() - limit..].to_vec()
    }

    pub fn clear_logs(&self, plugin_id: &str) {
        self.logs.lock().unwrap().remove(plugin_id);
    }

    /// Resolve a pending bus request; emits the `reply` record.
    pub fn resolve(&self, id: &str, reply: Reply) -> Option<BusRecord> {
        let record = self.bus.complete(id, reply)?;
        self.emit_bus(&record);
        Some(record)
    }

    pub fn emit_bus(&self, record: &BusRecord) {
        if let Some(app) = &self.app {
            let _ = app.emit(crate::events::PLUGIN_BUS, record);
        }
    }

    pub fn emit_state(&self, instance: &Instance, state: &str) {
        if let Some(app) = &self.app {
            let _ = app.emit(
                crate::events::PLUGIN_STATE,
                json!({
                    "plugin_id": instance.plugin_id,
                    "instance": instance.label,
                    "state": state,
                    "pid": instance.pid(),
                }),
            );
        }
    }

    /// An unsolicited event from an instance: queued for the model, recorded on
    /// the bus, emitted for the UI.
    pub fn instance_event(&self, plugin_id: &str, instance: &str, topic: &str, payload: Value) {
        let event = PluginEvent {
            plugin_id: plugin_id.to_string(),
            instance: instance.to_string(),
            topic: topic.to_string(),
            payload,
            ts_ms: crate::util::now_ms_u64(),
        };
        {
            let mut events = self.events.lock().unwrap();
            let ring = events.entry(plugin_id.to_string()).or_default();
            if ring.len() >= MAX_INSTANCE_EVENTS {
                ring.pop_front();
            }
            ring.push_back(event.clone());
        }
        let record = self.bus.record_event(
            plugin_id,
            topic,
            &[instance.to_string()],
            format!("instance event from {instance}"),
        );
        self.emit_bus(&record);
    }

    pub fn events(&self, plugin_id: &str, drain: bool) -> Vec<PluginEvent> {
        let mut events = self.events.lock().unwrap();
        let Some(ring) = events.get_mut(plugin_id) else {
            return vec![];
        };
        let taken: Vec<PluginEvent> = ring.iter().cloned().collect();
        if drain {
            ring.clear();
        }
        taken
    }
}

// ─── Host: instance pool + policy ────────────────────────────────────────────

/// The app-wide plugin host: owns the bus, the per-plugin instance pools and
/// the restart / failover policy.
pub struct PluginHost {
    pub(crate) sink: Sink,
    pub(crate) instances: tokio::sync::Mutex<HashMap<String, Vec<Arc<Instance>>>>,
    /// The bus attach endpoint, bound on first use (`plugins/link.rs`).
    pub(crate) link: tokio::sync::Mutex<Option<link::LinkEndpoint>>,
    /// Connections that passed their hello and wait to be adopted.
    pub(crate) attach_rx:
        tokio::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<link::Attachment>>>,
    /// Serialises spawns, so two parallel tool calls cannot start the same
    /// plugin twice.
    start_lock: tokio::sync::Mutex<()>,
    revision: AtomicU64,
    cursor: AtomicU64,
}

impl PluginHost {
    pub fn new(app: Option<AppHandle>) -> Self {
        Self {
            sink: Sink::new(app),
            instances: tokio::sync::Mutex::new(HashMap::new()),
            link: tokio::sync::Mutex::new(None),
            attach_rx: tokio::sync::Mutex::new(None),
            start_lock: tokio::sync::Mutex::new(()),
            revision: AtomicU64::new(1),
            cursor: AtomicU64::new(0),
        }
    }

    pub fn sink(&self) -> &Sink {
        &self.sink
    }

    pub fn bus(&self) -> &PluginBus {
        &self.sink.bus
    }

    /// Monotonic revision of the *effective* tool payload (plugins added,
    /// removed, enabled, disabled). A running turn rebuilds its tool list when
    /// it changes, exactly like the MCP revision.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }

    pub fn bump_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn logs(&self, plugin_id: &str, limit: usize) -> Vec<PluginLogEntry> {
        self.sink.logs(plugin_id, limit)
    }

    pub fn bus_records(&self, limit: usize) -> Vec<BusRecord> {
        self.sink.bus.records(limit)
    }

    pub async fn status(&self) -> Vec<InstanceView> {
        let map = self.instances.lock().await;
        let mut views: Vec<InstanceView> = map
            .values()
            .flat_map(|list| list.iter().map(|instance| instance.view()))
            .collect();
        views.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id).then(a.index.cmp(&b.index)));
        views
    }

    /// Live instances of one plugin, dead ones dropped and shut down.
    async fn live_instances(&self, plugin_id: &str) -> Vec<Arc<Instance>> {
        let mut map = self.instances.lock().await;
        let Some(list) = map.get_mut(plugin_id) else {
            return vec![];
        };
        let mut live: Vec<Arc<Instance>> = Vec::with_capacity(list.len());
        let mut dead: Vec<Arc<Instance>> = Vec::new();
        for instance in list.drain(..) {
            if instance.is_alive() {
                live.push(instance);
            } else {
                dead.push(instance);
            }
        }
        *list = live.clone();
        drop(map);
        for instance in dead {
            self.sink.log(
                plugin_id,
                LOG_WARN,
                format!("instance {} is gone; it will be respawned on demand", instance.label),
            );
            self.sink.emit_state(&instance, "failed");
            instance.shutdown(false).await;
        }
        live
    }

    /// The ready subset of [`PluginHost::live_instances`].
    async fn ready_instances(&self, plugin_id: &str) -> Vec<Arc<Instance>> {
        self.live_instances(plugin_id)
            .await
            .into_iter()
            .filter(|instance| instance.is_ready())
            .collect()
    }

    /// Make sure `instances` child processes are running for `plugin`.
    async fn ensure_started(&self, plugin: &Plugin) -> Result<usize, String> {
        if !plugin.manifest.enabled {
            return Err(format!("plugin '{}' is disabled", plugin.manifest.id));
        }
        if !plugin.manifest.spawns_process() {
            // A service plugin is started by its own supervisor: the app only
            // provides the endpoint, adopts what attached, and reports it.
            let _ = self.ensure_link(&plugin.workspace).await;
            self.drain(&plugin.workspace).await;
            return self.live_count(&plugin.manifest.id).await;
        }
        let desired = plugin.manifest.instance_count();
        let _serialized = self.start_lock.lock().await;

        let mut live = self.live_instances(&plugin.manifest.id).await;
        let ready = live.iter().filter(|instance| instance.is_ready()).count();
        if ready as u32 >= desired {
            return Ok(ready);
        }

        let generation = live.first().map(|instance| instance.generation).unwrap_or(0) + 1;
        let mut first_error: Option<String> = None;
        while (live.len() as u32) < desired {
            let index = live.len() as u32;
            match instance::spawn(&self.sink, plugin, index, generation).await {
                Ok(instance) => live.push(instance),
                Err(err) => {
                    self.sink.log(
                        &plugin.manifest.id,
                        LOG_ERROR,
                        format!("instance {index} could not start: {err}"),
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                    break;
                }
            }
        }
        if live.is_empty() {
            return Err(first_error.unwrap_or_else(|| "no instance could be started".to_string()));
        }
        let count = live.iter().filter(|instance| instance.is_ready()).count();
        self.instances
            .lock()
            .await
            .insert(plugin.manifest.id.clone(), live);
        if count == 0 {
            return Err(first_error.unwrap_or_else(|| {
                "the plugin process started but never reported ready".to_string()
            }));
        }
        Ok(count)
    }

    /// Explicit start (panel / `plugin` action `start`): restart the pool.
    pub async fn start(&self, plugin: &Plugin) -> Result<usize, String> {
        self.stop(&plugin.manifest.id).await;
        self.ensure_started(plugin).await
    }

    pub async fn stop(&self, plugin_id: &str) -> usize {
        let removed = {
            let mut map = self.instances.lock().await;
            map.remove(plugin_id).unwrap_or_default()
        };
        let count = removed.len();
        for instance in removed {
            instance.shutdown(true).await;
        }
        if count > 0 {
            self.sink
                .log(plugin_id, LOG_INFO, format!("{count} instance(s) stopped"));
        }
        count
    }

    /// Retire every instance (workspace switch, app exit, plugin system off).
    pub async fn stop_all(&self) {
        let all = {
            let mut map = self.instances.lock().await;
            std::mem::take(&mut *map)
        };
        for (_, list) in all {
            for instance in list {
                instance.shutdown(true).await;
            }
        }
    }

    /// Round-robin over the least busy ready instances.
    fn pick(&self, candidates: &[Arc<Instance>]) -> Option<Arc<Instance>> {
        if candidates.is_empty() {
            return None;
        }
        let cursor = self.cursor.fetch_add(1, Ordering::SeqCst) as usize;
        let start = cursor % candidates.len();
        let mut best = start;
        let mut best_load = candidates[start].inflight();
        for step in 1..candidates.len() {
            let index = (start + step) % candidates.len();
            let load = candidates[index].inflight();
            if load < best_load {
                best = index;
                best_load = load;
            }
        }
        Some(candidates[best].clone())
    }

    /// Send one tool request through the bus to a live instance and wait for
    /// the reply. Fails over once to a sibling instance when the first one
    /// cannot take the message.
    pub async fn call(
        &self,
        plugin: &Plugin,
        tool: &str,
        topic: &str,
        args: Value,
        timeout: std::time::Duration,
    ) -> Result<String, String> {
        // A service that attached since the last touch becomes an instance of
        // this plugin before we go looking for one.
        self.drain(&plugin.workspace).await;
        self.ensure_started(plugin).await?;
        let candidates = self.ready_instances(&plugin.id()).await;
        if candidates.is_empty() {
            return Err(format!(
                "plugin '{}' has no ready instance (see `plugin` action \"logs\")",
                plugin.id()
            ));
        }
        let topic = bus::normalize_topic(topic, false).unwrap_or_else(|_| plugin.id().to_string());
        let attempts = candidates.len().min(2);
        let mut errors: Vec<String> = Vec::new();

        for _ in 0..attempts {
            let Some(instance) = self.pick(&candidates) else {
                break;
            };
            let Some(pending) = self
                .sink
                .bus
                .begin(plugin.id(), &instance.label, &topic, tool)
            else {
                return Err(
                    "the plugin message bus is saturated (too many requests in flight)".to_string(),
                );
            };
            let _inflight = instance.mark_inflight();
            let request = json!({
                "type": "request",
                "id": pending.id,
                "topic": pending.topic,
                "tool": tool,
                "args": args,
                "timeout_ms": timeout.as_millis() as u64,
            });
            if let Err(err) = instance.send_line(&request).await {
                self.sink.bus.expire(&pending.id);
                instance.set_state(InstanceState::Failed(err.clone()));
                self.sink.emit_state(&instance, "failed");
                self.sink.log(&plugin.id(), LOG_WARN, err.clone());
                errors.push(err);
                continue;
            }
            match tokio::time::timeout(timeout, pending.rx).await {
                Ok(Ok(reply)) => return Ok(reply.text()),
                Ok(Err(_)) => {
                    errors.push(format!("{} closed the reply channel", instance.label));
                }
                Err(_) => {
                    if let Some(record) = self.sink.bus.expire(&pending.id) {
                        self.sink.emit_bus(&record);
                    }
                    errors.push(format!(
                        "{} did not reply within {} ms",
                        instance.label,
                        timeout.as_millis()
                    ));
                }
            }
        }

        Err(format!(
            "plugin '{}' tool '{}' failed: {}",
            plugin.id(),
            tool,
            errors.join("; ")
        ))
    }

    /// Fire-and-forget publish: fan one event out to every instance whose
    /// plugin subscribes to `topic`.
    pub async fn publish(
        &self,
        registry: &PluginRegistry,
        topic: &str,
        payload: Value,
    ) -> Result<Vec<String>, String> {
        let topic = bus::normalize_topic(topic, false)?;
        let mut recipients: Vec<String> = Vec::new();
        let all = {
            let map = self.instances.lock().await;
            map.iter()
                .flat_map(|(plugin_id, list)| {
                    list.iter()
                        .map(move |instance| (plugin_id.clone(), instance.clone()))
                })
                .collect::<Vec<_>>()
        };
        for (plugin_id, instance) in all {
            let Some(plugin) = registry.get(&plugin_id) else {
                continue;
            };
            if !instance.is_ready() {
                continue;
            }
            let subscribed = plugin
                .manifest
                .effective_subscriptions()
                .iter()
                .any(|filter| bus::topic_matches(filter, &topic));
            if !subscribed {
                continue;
            }
            let event = json!({
                "type": "event",
                "topic": topic,
                "payload": payload,
                "sender": "host",
                "ts_ms": crate::util::now_ms_u64(),
            });
            match instance.send_line(&event).await {
                Ok(()) => recipients.push(instance.label.clone()),
                Err(err) => self.sink.log(&plugin_id, LOG_WARN, err),
            }
        }
        let detail = if recipients.is_empty() {
            format!("no instance subscribed to '{topic}'")
        } else {
            format!("delivered to {} instance(s)", recipients.len())
        };
        let record = self
            .sink
            .bus
            .record_event("bus", &topic, &recipients, detail.clone());
        self.sink.emit_bus(&record);
        if recipients.is_empty() {
            return Err(format!("no plugin instance subscribed to '{topic}' ({detail})"));
        }
        Ok(recipients)
    }
}

// ─── Convenience used by the tool + IPC layers ───────────────────────────────

/// `plugin` tool result cap (the dispatcher truncates generic tool output, but
/// a plugin answer is usually a document and deserves its own limit first).
pub fn cap_result(text: String) -> String {
    if text.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return text;
    }
    let head: String = text.chars().take(MAX_TOOL_RESULT_CHARS).collect();
    format!("{head}\n\n… (plugin result truncated)")
}
