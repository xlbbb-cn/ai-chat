//! Manager-side host methods: the bus attach endpoint, per-plugin settings,
//! and the state the plugin window renders.
//!
//! This is the backend half of the plugin panel. The window itself is a
//! separate webview (`plugin-manager`, capability `plugin-manager.json`) opened
//! from Settings → Plugins; it only ever talks to the commands in
//! `ipc/plugins.rs`, which forward to the methods below.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;

use super::config::{self, ConfigForm, FieldError};
use super::link::{self, LinkEndpoint};
use super::{instance, load_plugins, Plugin, PluginHost, PluginIssue, PluginView, LOG_WARN};

/// One plugin as the manager lists it.
#[derive(Debug, Clone, Serialize)]
pub struct PluginEntry {
    #[serde(flatten)]
    pub view: PluginView,
    /// `spawn` / `service` / `both`.
    pub transport: String,
    /// `schema` (generic form), `page` (plugin-supplied HTML) or `none`.
    pub config_mode: String,
    pub config_page: Option<String>,
    /// Instances that are ready to take a request right now.
    pub running: usize,
}

/// One plugin's settings page, as the window draws it.
#[derive(Debug, Clone, Serialize)]
pub struct PluginConfigView {
    pub plugin_id: String,
    pub mode: String,
    pub page: Option<String>,
    pub form: Option<ConfigForm>,
    /// Per-field validation failures from the last save attempt.
    pub errors: Vec<FieldError>,
    /// The schema file exists but could not be read or parsed.
    pub schema_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginManagerState {
    pub plugins: Vec<PluginEntry>,
    pub instances: Vec<super::InstanceView>,
    /// What an external service dials; `None` only if the listener failed.
    pub endpoint: Option<LinkEndpoint>,
    pub issues: Vec<PluginIssue>,
    pub revision: u64,
}

impl PluginHost {
    /// Bind the attach endpoint once; later calls return the same address.
    pub async fn ensure_link(&self, workspace: &Path) -> Result<LinkEndpoint, String> {
        let mut slot = self.link.lock().await;
        if let Some(endpoint) = slot.as_ref() {
            return Ok(endpoint.clone());
        }
        let plugins_dir = super::plugins_dir(workspace);
        let token = link::load_or_create_token(&plugins_dir)?;
        let (endpoint, rx) = link::start(self.sink.clone(), token)?;
        *self.attach_rx.lock().await = Some(rx);
        *slot = Some(endpoint.clone());
        self.bump_revision();
        Ok(endpoint)
    }

    /// The endpoint once bound (no listener is started here).
    pub async fn endpoint(&self) -> Option<LinkEndpoint> {
        self.link.lock().await.clone()
    }

    /// Adopt service connections that completed their hello.
    pub async fn drain(&self, workspace: &Path) {
        let mut incoming = Vec::new();
        {
            let mut slot = self.attach_rx.lock().await;
            let Some(rx) = slot.as_mut() else {
                return;
            };
            while let Ok(attachment) = rx.try_recv() {
                incoming.push(attachment);
            }
        }
        if incoming.is_empty() {
            return;
        }
        let registry = load_plugins(workspace);
        for attachment in incoming {
            let wanted = attachment.plugin_id.clone();
            let plugin = registry
                .plugins
                .iter()
                .find(|candidate| candidate.manifest.id == wanted)
                .cloned();
            let Some(plugin) = plugin else {
                link::reject(attachment.writer, "unknown plugin").await;
                continue;
            };
            if !plugin.manifest.enabled {
                self.sink
                    .log(&wanted, LOG_WARN, "refused service: plugin disabled");
                link::reject(attachment.writer, "plugin disabled").await;
                continue;
            }
            if !plugin.manifest.accepts_service() {
                self.sink.log(
                    &wanted,
                    LOG_WARN,
                    "refused service: manifest transport is spawn-only",
                );
                link::reject(attachment.writer, "plugin is spawn-only").await;
                continue;
            }
            // Anything else the service announced (version, host, build) is
            // echoed back so the manager can show it without a second call.
            let announced = attachment.hello["version"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let mut pool = self.instances.lock().await;
            let list = pool.entry(wanted.clone()).or_default();
            if list.len() as u32 >= super::MAX_INSTANCES_PER_PLUGIN {
                drop(pool);
                link::reject(attachment.writer, "instance limit reached").await;
                continue;
            }
            let index = list.iter().map(|i| i.index + 1).max().unwrap_or(0);
            let remote = attachment.remote.clone();
            let instance = instance::attach(
                &self.sink,
                &plugin,
                index,
                remote,
                attachment.reader,
                attachment.writer,
            );
            list.push(instance.clone());
            drop(pool);
            // The service learns its identity and its settings in one frame.
            let config = self.config_values(&plugin);
            let _ = instance
                .send_line(&json!({
                    "type": "welcome",
                    "plugin": wanted,
                    "instance": instance.label,
                    "version": announced,
                    "config": config,
                }))
                .await;
            self.bump_revision();
        }
    }

    /// Ready instances of one plugin.
    pub async fn live_count(&self, plugin_id: &str) -> Result<usize, String> {
        let pool = self.instances.lock().await;
        Ok(pool
            .get(plugin_id)
            .map(|list| list.iter().filter(|i| i.is_ready()).count())
            .unwrap_or(0))
    }

    /// Settings as the plugin sees them: defaults overlaid with stored values,
    /// restricted to keys the schema still declares.
    pub fn config_values(&self, plugin: &Plugin) -> Value {
        let stored = config::load_values(&plugin.dir);
        match config::load_schema(&plugin.dir, plugin.manifest.schema_file()) {
            Ok(Some(schema)) => Value::Object(config::effective(&schema, &stored)),
            _ => Value::Object(stored),
        }
    }

    /// Settings page for the manager window.
    pub fn config_view(&self, plugin: &Plugin) -> PluginConfigView {
        let page = page_of(plugin);
        let stored = config::load_values(&plugin.dir);
        let mode = |has_schema: bool| {
            if page.is_some() {
                "page".to_string()
            } else if has_schema {
                "schema".to_string()
            } else {
                "none".to_string()
            }
        };
        match config::load_schema(&plugin.dir, plugin.manifest.schema_file()) {
            Ok(Some(schema)) => PluginConfigView {
                plugin_id: plugin.manifest.id.clone(),
                mode: mode(true),
                page,
                form: Some(config::form(&schema, &stored)),
                errors: Vec::new(),
                schema_error: None,
            },
            Ok(None) => PluginConfigView {
                plugin_id: plugin.manifest.id.clone(),
                mode: mode(false),
                page,
                form: None,
                errors: Vec::new(),
                schema_error: None,
            },
            Err(err) => PluginConfigView {
                plugin_id: plugin.manifest.id.clone(),
                mode: mode(false),
                page,
                form: None,
                errors: Vec::new(),
                schema_error: Some(err),
            },
        }
    }

    /// Validate, persist and hot-push settings; per-field errors come back
    /// instead of a write when anything is wrong.
    pub async fn save_config(
        &self,
        plugin: &Plugin,
        input: &Value,
    ) -> Result<PluginConfigView, Vec<FieldError>> {
        let stored = config::load_values(&plugin.dir);
        let values = match config::load_schema(&plugin.dir, plugin.manifest.schema_file()) {
            Ok(Some(schema)) => config::validate(&schema, &stored, input)?,
            // No schema: a page-mode plugin owns its own shape, so the object
            // is stored as given.
            _ => match input.as_object() {
                Some(map) => map.clone(),
                None => {
                    return Err(vec![FieldError {
                        key: String::new(),
                        message: "expected an object of field values".into(),
                    }])
                }
            },
        };
        config::write_values(&plugin.dir, &values)
            .map_err(|err| vec![FieldError { key: String::new(), message: err }])?;
        self.push_config(plugin).await;
        self.bump_revision();
        Ok(self.config_view(plugin))
    }

    /// Send the current settings to every running instance of `plugin`.
    pub async fn push_config(&self, plugin: &Plugin) {
        let config = self.config_values(plugin);
        let pool = self.instances.lock().await;
        let Some(list) = pool.get(&plugin.manifest.id) else {
            return;
        };
        for instance in list.iter().filter(|i| i.is_ready()) {
            let _ = instance
                .send_line(&json!({ "type": "config", "config": config }))
                .await;
        }
    }

    /// Everything the plugin window needs in one round trip.
    pub async fn manager_state(&self, workspace: &Path) -> PluginManagerState {
        let endpoint = self.ensure_link(workspace).await.ok();
        self.drain(workspace).await;
        let registry = load_plugins(workspace);
        let pool = self.instances.lock().await;
        let mut plugins = Vec::with_capacity(registry.plugins.len());
        let mut instances = Vec::new();
        for plugin in &registry.plugins {
            let list = pool.get(&plugin.manifest.id);
            let running = list
                .map(|list| list.iter().filter(|i| i.is_ready()).count())
                .unwrap_or(0);
            if let Some(list) = list {
                instances.extend(list.iter().map(|i| i.view()));
            }
            let page = page_of(plugin);
            let config_mode = if page.is_some() {
                "page"
            } else if config::schema_path(&plugin.dir, plugin.manifest.schema_file()).is_file() {
                "schema"
            } else {
                "none"
            };
            plugins.push(PluginEntry {
                view: super::plugin_view(plugin),
                transport: match plugin.manifest.transport {
                    super::PluginTransport::Spawn => "spawn",
                    super::PluginTransport::Service => "service",
                    super::PluginTransport::Both => "both",
                }
                .to_string(),
                config_mode: config_mode.to_string(),
                config_page: page,
                running,
            });
        }
        PluginManagerState {
            plugins,
            instances,
            endpoint,
            issues: registry.issues,
            revision: self.revision(),
        }
    }
}

fn page_of(plugin: &Plugin) -> Option<String> {
    if plugin.manifest.config_page.is_empty() {
        None
    } else {
        Some(plugin.manifest.config_page.clone())
    }
}
