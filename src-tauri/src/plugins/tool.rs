//! The LLM-facing half of the plugin system.
//!
//! Two surfaces, one bus:
//!
//! * the built-in **`plugin`** tool — discovery, generic `call`, fire-and-forget
//!   `publish`, instance lifecycle and diagnostics;
//! * one **`plugin_<slug>_<tool>`** function per manifest-declared tool, so a
//!   plugin's capabilities sit in the prompt like any other tool. Both routes
//!   end in [`crate::plugins::PluginHost::call`], i.e. on the message bus.

//! Neither surface asks for confirmation: a plugin is workspace content the
//! user installed (like a skill's scripts), and the child process never gets a
//! shell — it is spawned from an explicit interpreter + entry path.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::plugins::{self, Plugin, PluginRegistry};
use std::path::Path;

/// Hard cap on how much of a manifest-declared tool list a single prompt gets.
const MAX_ADAPTED_TOOLS: usize = 64;

/// The `plugin` tool definition (`plugin.json` → the model).
pub fn tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "plugin",
            "description": "Plugin system. The chain is: you -> this tool -> the plugin message bus -> N long-lived plugin instances. Plugins live in `<workspace>/.data/plugins/<id>/` (`plugin.json` + an entry script); each plugin runs `instances` child processes that speak line-delimited JSON, and every declared tool is also advertised directly as `plugin_<id>_<tool>`.\n\n\
                `action: \"list\"` — installed plugins, their tools, live instances and the bus/log counters (start here).\n\
                `action: \"call\"` — send one request through the bus to a plugin instance and return its reply: `{plugin, tool, args}`; optional `topic`, `timeout_ms`. The instance is chosen by least in-flight work and the call fails over to a sibling instance once.\n\
                `action: \"publish\"` — fan one event out to every instance subscribed to `topic` (fire-and-forget, no reply).\n\
                `action: \"events\"` — read (and optionally drain) the events an instance pushed without a request.\n\
                `action: \"status\"` — per-instance state, pid, in-flight count, restarts, last error.\n\
                `action: \"logs\"` — a plugin's own log ring (spawn errors, stderr, rejected lines).\n\
                `action: \"start\"` / `\"stop\"` / `\"reload\"` — bring a plugin's instance pool up, take it down, or re-read the manifests from disk after editing `plugin.json`.\n\n\
                A plugin that declares no tools is still usable through `call` with the tool name it accepts. Use `plugin_<id>_<tool>` for the normal path.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "call", "publish", "events", "status", "logs", "start", "stop", "reload"],
                        "description": "'list' (installed plugins + instances), 'call' (bus request → reply), 'publish' (bus fan-out), 'events' (instance-pushed events), 'status' (instance detail), 'logs' (plugin log ring), 'start' / 'stop' / 'reload' (lifecycle)."
                    },
                    "plugin": {
                        "type": "string",
                        "description": "Plugin id (the folder name under `.data/plugins/`). Required for 'call', 'events', 'logs'; optional for 'status' (all plugins when omitted), 'start' and 'stop'."
                    },
                    "tool": {
                        "type": "string",
                        "description": "Required for 'call': the tool the plugin implements (a name from its `tools`, or any name it accepts)."
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments for 'call'; passed through to the plugin as `args`."
                    },
                    "topic": {
                        "type": "string",
                        "description": "Bus topic. 'call' defaults to the tool's declared topic (else the plugin id); 'publish' requires it, e.g. 'clock/tick'."
                    },
                    "payload": {
                        "description": "Payload for 'publish' (any JSON value)."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1000,
                        "maximum": plugins::MAX_CALL_TIMEOUT_MS,
                        "description": "Reply deadline for 'call' (default: the plugin's `call_timeout_ms`)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": plugins::MAX_LOG_ENTRIES,
                        "description": "How many newest log / bus entries 'logs' returns (default 30)."
                    },
                    "drain": {
                        "type": "boolean",
                        "description": "'events' only: remove the returned events from the queue (default false)."
                    }
                },
                "required": ["action"]
            }
        }
    })
}

/// One OpenAI tool definition per manifest-declared plugin tool.
///
/// Sorted by function name: the tool payload sits in the cached request prefix,
/// so its order must not depend on directory enumeration (same rule as
/// `mcp::sort_adapted_tools`).
pub fn adapted_tools(registry: &PluginRegistry) -> Vec<Value> {
    let mut tools: Vec<Value> = Vec::new();
    for plugin in registry.enabled() {
        for tool in &plugin.manifest.tools {
            if tools.len() >= MAX_ADAPTED_TOOLS {
                return tools;
            }
            let function_name = plugins::adapt_tool_name(&plugin.manifest.id, &tool.name);
            let description = if tool.description.trim().is_empty() {
                format!(
                    "Plugin '{}' ({}): tool '{}'.",
                    plugin.manifest.name, plugin.manifest.id, tool.name
                )
            } else {
                format!(
                    "Plugin '{}' ({}): {}",
                    plugin.manifest.name, plugin.manifest.id, tool.description
                )
            };
            let parameters = if tool.parameters.is_object() {
                tool.parameters.clone()
            } else {
                json!({ "type": "object", "properties": {} })
            };
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": function_name,
                    "description": description,
                    "parameters": parameters,
                }
            }));
        }
    }
    tools.sort_by(|a, b| {
        a["function"]["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["function"]["name"].as_str().unwrap_or(""))
    });
    tools
}

/// Entry point for both surfaces. `name` is `plugin` or `plugin_<slug>_<tool>`.
pub async fn handle(
    app: &AppHandle,
    name: &str,
    args: &Value,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    call_id: Option<&str>,
) -> String {
    let state = app.state::<crate::AppState>();
    let workspace = state.workspace_dir.lock().unwrap().clone();
    let registry = plugins::load_plugins(&workspace);

    if name != "plugin" {
        let Some((plugin, tool)) = registry.resolve_adapted(name) else {
            return format!(
                "Error: '{name}' is not an installed plugin tool. Call `plugin` with action \"list\" to see the current plugins."
            );
        };
        let tool_name = tool.name.clone();
        let topic = if tool.topic.trim().is_empty() {
            plugin.manifest.id.clone()
        } else {
            tool.topic.clone()
        };
        let timeout = plugin.manifest.call_timeout();
        return run_call(
            app,
            &state,
            plugin,
            &tool_name,
            &topic,
            args.clone(),
            timeout,
            mission_id,
            session_id,
            call_id,
            name,
        )
        .await;
    }

    let action = args["action"].as_str().unwrap_or("").trim().to_ascii_lowercase();
    match action.as_str() {
        "list" => render_list(&state, &registry).await,
        "call" => {
            let plugin_id = args["plugin"].as_str().unwrap_or("").trim();
            let tool = args["tool"].as_str().unwrap_or("").trim();
            if plugin_id.is_empty() || tool.is_empty() {
                return "Error: 'call' needs both `plugin` and `tool`.".to_string();
            }
            let Some(plugin) = registry.get(plugin_id) else {
                return format!(
                    "Error: plugin '{plugin_id}' is not installed. Call `plugin` with action \"list\"."
                );
            };
            if !plugin.manifest.enabled {
                return format!("Error: plugin '{plugin_id}' is disabled.");
            }
            let declared_topic = plugin
                .manifest
                .tools
                .iter()
                .find(|declared| declared.name == tool)
                .map(|declared| declared.topic.clone())
                .unwrap_or_default();
            let topic = match args["topic"].as_str().map(str::trim) {
                Some(topic) if !topic.is_empty() => topic.to_string(),
                _ if !declared_topic.trim().is_empty() => declared_topic,
                _ => plugin.manifest.id.clone(),
            };
            let timeout = args["timeout_ms"]
                .as_u64()
                .map(|millis| {
                    std::time::Duration::from_millis(
                        millis.clamp(1_000, plugins::MAX_CALL_TIMEOUT_MS),
                    )
                })
                .unwrap_or_else(|| plugin.manifest.call_timeout());
            let call_args = args
                .get("args")
                .cloned()
                .filter(|value| !value.is_null())
                .unwrap_or_else(|| json!({}));
            run_call(
                app,
                &state,
                plugin,
                tool,
                &topic,
                call_args,
                timeout,
                mission_id,
                session_id,
                call_id,
                "plugin",
            )
            .await
        }
        "publish" => {
            let topic = args["topic"].as_str().unwrap_or("").trim();
            if topic.is_empty() {
                return "Error: 'publish' needs a `topic` (e.g. \"clock/tick\").".to_string();
            }
            let payload = args.get("payload").cloned().unwrap_or(Value::Null);
            match state.plugins.publish(&registry, topic, payload).await {
                Ok(recipients) => serde_json::to_string_pretty(&json!({
                    "topic": topic,
                    "delivered_to": recipients,
                    "count": recipients.len(),
                }))
                .unwrap_or_else(|_| format!("Delivered to {} instance(s).", recipients.len())),
                Err(err) => format!("Error: {err}"),
            }
        }
        "events" => {
            let plugin_id = args["plugin"].as_str().unwrap_or("").trim();
            if plugin_id.is_empty() {
                return "Error: 'events' needs a `plugin`.".to_string();
            }
            let drain = args["drain"].as_bool().unwrap_or(false);
            let events = state.plugins.sink().events(plugin_id, drain);
            if events.is_empty() {
                return format!("No queued events for plugin '{plugin_id}'.");
            }
            serde_json::to_string_pretty(&json!({
                "plugin": plugin_id,
                "drained": drain,
                "count": events.len(),
                "events": events,
            }))
            .unwrap_or_else(|_| "[]".to_string())
        }
        "status" => {
            let filter = args["plugin"].as_str().unwrap_or("").trim().to_string();
            let instances = state.plugins.status().await;
            let instances: Vec<_> = instances
                .into_iter()
                .filter(|view| filter.is_empty() || view.plugin_id == filter)
                .collect();
            let payload = json!({
                "instances": instances,
                "bus_pending": state.plugins.bus().pending_len(),
                "revision": state.plugins.revision(),
            });
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        }
        "logs" => {
            let plugin_id = args["plugin"].as_str().unwrap_or("").trim();
            if plugin_id.is_empty() {
                return "Error: 'logs' needs a `plugin`.".to_string();
            }
            let limit = args["limit"].as_u64().unwrap_or(30).max(1) as usize;
            let entries = state.plugins.logs(plugin_id, limit);
            if entries.is_empty() {
                return format!("No log entries for plugin '{plugin_id}'.");
            }
            serde_json::to_string_pretty(&json!({
                "plugin": plugin_id,
                "entries": entries,
            }))
            .unwrap_or_else(|_| "[]".to_string())
        }
        "start" => {
            let plugin_id = args["plugin"].as_str().unwrap_or("").trim();
            let Some(plugin) = registry.get(plugin_id) else {
                return format!("Error: plugin '{plugin_id}' is not installed.");
            };
            match state.plugins.start(plugin).await {
                Ok(count) => {
                    state.plugins.bump_revision();
                    format!("Started plugin '{plugin_id}' with {count} instance(s).")
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "stop" => {
            let plugin_id = args["plugin"].as_str().unwrap_or("").trim();
            if plugin_id.is_empty() {
                return "Error: 'stop' needs a `plugin`.".to_string();
            }
            let stopped = state.plugins.stop(plugin_id).await;
            state.plugins.bump_revision();
            format!("Stopped {stopped} instance(s) of plugin '{plugin_id}'.")
        }
        "reload" => {
            // Manifests are files: re-read them, retire instances of plugins
            // that disappeared or were disabled, and let the next turn (or the
            // next request boundary of this one) pick up the new tool list.
            let ids: Vec<String> = state
                .plugins
                .status()
                .await
                .into_iter()
                .map(|view| view.plugin_id)
                .collect();
            for plugin_id in ids {
                let keep = registry
                    .get(&plugin_id)
                    .map(|plugin| plugin.manifest.enabled)
                    .unwrap_or(false);
                if !keep {
                    state.plugins.stop(&plugin_id).await;
                }
            }
            let revision = state.plugins.bump_revision();
            serde_json::to_string_pretty(&json!({
                "plugins": registry.plugins.len(),
                "enabled": registry.enabled().count(),
                "issues": registry.issues,
                "revision": revision,
            }))
            .unwrap_or_else(|_| "Reloaded.".to_string())
        }
        other => format!(
            "Error: unknown plugin action '{other}'. Use list, call, publish, events, status, logs, start, stop or reload."
        ),
    }
}

async fn render_list(state: &crate::AppState, registry: &PluginRegistry) -> String {
    let instances = state.plugins.status().await;
    let plugins: Vec<Value> = registry
        .plugins
        .iter()
        .map(|plugin| {
            let my_instances: Vec<Value> = instances
                .iter()
                .filter(|view| view.plugin_id == plugin.manifest.id)
                .map(|view| {
                    json!({
                        "label": view.label,
                        "state": view.state,
                        "pid": view.pid,
                        "inflight": view.inflight,
                        "last_error": view.last_error,
                    })
                })
                .collect();
            json!({
                "id": plugin.manifest.id,
                "name": plugin.manifest.name,
                "version": plugin.manifest.version,
                "description": plugin.manifest.description,
                "enabled": plugin.manifest.enabled,
                "runtime": plugin.manifest.resolved_runtime().name(),
                "instances": plugin.manifest.instance_count(),
                "tools": plugin.adapted_tool_names(),
                "topics": plugin.manifest.effective_subscriptions(),
                "dir": plugin.dir.to_string_lossy(),
                "running": my_instances,
            })
        })
        .collect();
    let payload = json!({
        "plugins": plugins,
        "count": plugins.len(),
        "issues": registry.issues,
        "bus_pending": state.plugins.bus().pending_len(),
        "policy": "Instances are started on demand: a plugin with no running instance is spawned by the first `call` / `start`.",
    });
    if plugins.is_empty() {
        let workspace = state.workspace_dir.lock().unwrap().clone();
        return saw_no_plugins(&workspace);
    }
    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
}

fn saw_no_plugins(workspace: &Path) -> String {
    let dir = plugins::plugins_dir(workspace);
    format!(
        "No plugins installed.\n\nCreate `{}<id>/plugin.json` plus an entry script, then call \
         `plugin` with action \"reload\". Minimal manifest:\n\n```json\n{{\n  \"name\": \"Echo\",\n  \
         \"entry\": \"server.js\",\n  \"runtime\": \"node\",\n  \"instances\": 1,\n  \
         \"tools\": [{{ \"name\": \"echo\", \"description\": \"Echo the message back\", \
         \"parameters\": {{\"type\":\"object\",\"properties\":{{\"message\":{{\"type\":\"string\"}}}}}} }}]\n}}\n```\n\n\
         The entry answers the `hello` line with `{{\"type\":\"ready\"}}` and every `request` with \
         `{{\"type\":\"reply\",\"id\":\"<same id>\",\"ok\":true,\"result\":…}}`.",
        dir.display()
    )
}

/// One bus round trip, plus the transcript line that shows which plugin
/// answered (the same `emit_tool_progress` the built-in tools use).
#[allow(clippy::too_many_arguments)]
async fn run_call(
    app: &AppHandle,
    state: &crate::AppState,
    plugin: &Plugin,
    tool: &str,
    topic: &str,
    args: Value,
    timeout: std::time::Duration,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    call_id: Option<&str>,
    surface: &str,
) -> String {
    let _ = surface;
    let args_block = serde_json::to_string_pretty(&args).unwrap_or_else(|_| args.to_string());
    let _ = crate::tools::emit_tool_progress(
        app,
        mission_id,
        session_id,
        call_id,
        format!(
            "*Plugin [{}]: {tool}* · topic `{topic}`\n\n```json\n{}\n```\n",
            plugin.manifest.id,
            truncate_block(&args_block, 800)
        ),
    );
    match state.plugins.call(plugin, tool, topic, args, timeout).await {
        Ok(text) => plugins::cap_result(text),
        Err(err) => format!("Error: {err}"),
    }
}

fn truncate_block(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}…")
}
