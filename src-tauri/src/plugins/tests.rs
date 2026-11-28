//! Unit + integration tests for the plugin system.
//!
//! The pure layers (topics, manifest, tool-name mapping, bus correlation) run
//! everywhere. The end-to-end test drives a real child process through the same
//! code path the app uses (`PluginHost::call` → bus → instance → reply); it is
//! skipped when no `node` is on PATH, so a machine without a JS runtime still
//! gets a green suite.

use super::*;
use serde_json::json;
use std::time::Duration;

// ─── Topics ──────────────────────────────────────────────────────────────────

#[test]
fn topic_wildcards_follow_mqtt_rules() {
    assert!(bus::topic_matches("clock/#", "clock/tick"));
    assert!(bus::topic_matches("clock/#", "clock"));
    assert!(bus::topic_matches("#", "anything/at/all"));
    assert!(bus::topic_matches("clock/+", "clock/tick"));
    assert!(!bus::topic_matches("clock/+", "clock/tick/extra"));
    assert!(!bus::topic_matches("clock/+", "clock"));
    assert!(!bus::topic_matches("clock/tick", "clock/tock"));
    assert!(bus::topic_matches("Clock/Tick", "clock/tick"));
}

#[test]
fn normalize_topic_rejects_junk() {
    assert_eq!(
        bus::normalize_topic(" Clock/Tick ", false).unwrap(),
        "clock/tick"
    );
    assert!(bus::normalize_topic("", false).is_err());
    assert!(bus::normalize_topic("clock//tick", false).is_err());
    assert!(bus::normalize_topic("clock/*", false).is_err());
    assert!(bus::normalize_topic("clock/+", false).is_err());
    assert_eq!(bus::normalize_topic("clock/+", true).unwrap(), "clock/+");
    assert!(bus::normalize_topic("bad level/x", false).is_err());
}

// ─── Manifest ────────────────────────────────────────────────────────────────

#[test]
fn manifest_defaults_are_conservative() {
    let manifest: PluginManifest =
        serde_json::from_str(r#"{"name":"Echo","entry":"server.js"}"#).unwrap();
    assert!(manifest.enabled);
    assert!(manifest.auto_start);
    assert_eq!(manifest.instance_count(), 1);
    assert_eq!(manifest.resolved_runtime(), PluginRuntime::Node);
    assert_eq!(
        manifest.call_timeout(),
        Duration::from_millis(DEFAULT_CALL_TIMEOUT_MS)
    );
    assert_eq!(manifest.effective_subscriptions(), vec!["/#".to_string()]);
}

#[test]
fn instance_count_is_clamped() {
    let mut manifest = PluginManifest {
        id: "x".to_string(),
        name: "X".to_string(),
        version: String::new(),
        description: String::new(),
        runtime: PluginRuntime::Node,
        command: String::new(),
        args: vec![],
        entry: "server.js".to_string(),
        env: Default::default(),
        instances: 0,
        enabled: true,
        auto_start: true,
        call_timeout_ms: 0,
        tools: vec![],
        subscriptions: vec![],
        source: String::new(),
        ..Default::default()
    };
    assert_eq!(manifest.instance_count(), 1);
    manifest.instances = 99;
    assert_eq!(manifest.instance_count(), MAX_INSTANCES_PER_PLUGIN);
}

#[test]
fn manifest_needs_an_entry_or_a_command() {
    let manifest: PluginManifest = serde_json::from_str(r#"{"name":"Empty"}"#).unwrap();
    assert!(manifest.validate().is_err());

    let unknown_runtime: PluginManifest =
        serde_json::from_str(r#"{"name":"X","entry":"main.rb"}"#).unwrap();
    assert_eq!(unknown_runtime.resolved_runtime(), PluginRuntime::Auto);
    assert!(unknown_runtime.validate().is_err());
}

// ─── Tool names ──────────────────────────────────────────────────────────────

#[test]
fn adapted_tool_names_are_stable_and_reversible() {
    assert_eq!(adapt_tool_name("clock", "now"), "plugin_clock_now");
    assert_eq!(
        adapt_tool_name("My Plugin", "Get-Time"),
        "plugin_my_plugin_get_time"
    );
    assert_eq!(slugify_segment("  a  b  "), "a_b");

    let registry = PluginRegistry {
        plugins: vec![Plugin {
            workspace: PathBuf::from("."),
            dir: PathBuf::from(".data/plugins/clock"),
            manifest: PluginManifest {
                id: "clock".to_string(),
                name: "Clock".to_string(),
                version: "1.0.0".to_string(),
                description: String::new(),
                runtime: PluginRuntime::Node,
                command: String::new(),
                args: vec![],
                entry: "server.js".to_string(),
                env: Default::default(),
                instances: 1,
                enabled: true,
                auto_start: true,
                call_timeout_ms: 1_000,
                tools: vec![PluginTool {
                    name: "now".to_string(),
                    description: "Current time".to_string(),
                    parameters: json!({"type":"object","properties":{}}),
                    topic: "clock".to_string(),
                }],
                subscriptions: vec!["clock/#".to_string()],
                source: String::new(),
                ..Default::default()
            },
        }],
        issues: vec![],
    };

    let (plugin, tool) = registry.resolve_adapted("plugin_clock_now").unwrap();
    assert_eq!(plugin.id(), "clock");
    assert_eq!(tool.name, "now");
    assert!(registry.resolve_adapted("plugin_clock_later").is_none());
    assert!(registry.resolve_adapted("plugin").is_none());

    let tools = tool::adapted_tools(&registry);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"], "plugin_clock_now");
}

// ─── Bus ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bus_correlates_one_reply_per_request() {
    let bus = PluginBus::new();
    let pending = bus.begin("echo", "echo#0", "echo", "echo").unwrap();
    assert_eq!(bus.pending_len(), 1);
    let record = bus
        .complete(
            &pending.id,
            Reply {
                ok: true,
                result: json!("hi"),
                error: None,
            },
        )
        .expect("reply resolves the request");
    assert_eq!(record.kind, bus::BusKind::Reply);
    assert_eq!(record.tool.as_deref(), Some("echo"));
    assert_eq!(bus.pending_len(), 0);
    // A duplicate reply is not a second completion.
    assert!(bus
        .complete(
            &pending.id,
            Reply {
                ok: true,
                result: json!("again"),
                error: None
            }
        )
        .is_none());
    assert_eq!(pending.rx.await.unwrap().result, json!("hi"));
    assert!(bus
        .records(0)
        .iter()
        .any(|record| record.kind == bus::BusKind::Request));
}

#[tokio::test]
async fn bus_expire_removes_the_slot() {
    let bus = PluginBus::new();
    let pending = bus.begin("echo", "echo#0", "echo", "echo").unwrap();
    let record = bus.expire(&pending.id).expect("timeout is recorded");
    assert_eq!(record.kind, bus::BusKind::Timeout);
    assert_eq!(bus.pending_len(), 0);
    // The sender is dropped, so the waiter sees a closed channel.
    assert!(pending.rx.await.is_err());
}

#[test]
fn bus_records_are_bounded() {
    let bus = PluginBus::new();
    for index in 0..(bus::MAX_BUS_RECORDS + 25) {
        bus.record_rejected("echo", "echo", format!("nope {index}"));
    }
    assert_eq!(bus.records(0).len(), bus::MAX_BUS_RECORDS);
    assert_eq!(bus.records(5).len(), 5);
}

// ─── Sink queues ─────────────────────────────────────────────────────────────

#[test]
fn instance_events_queue_and_drain() {
    let host = PluginHost::new(None);
    host.sink()
        .instance_event("echo", "echo#0", "echo/tick", json!({"n": 1}));
    host.sink()
        .instance_event("echo", "echo#1", "echo/tick", json!({"n": 2}));

    let peeked = host.sink().events("echo", false);
    assert_eq!(peeked.len(), 2);
    assert_eq!(peeked[1].instance, "echo#1");
    assert_eq!(
        host.sink().events("echo", false).len(),
        2,
        "peek keeps the queue"
    );

    let drained = host.sink().events("echo", true);
    assert_eq!(drained.len(), 2);
    assert!(host.sink().events("echo", false).is_empty());

    // The bus audit saw both events.
    assert!(host
        .bus()
        .records(0)
        .iter()
        .any(|record| record.kind == bus::BusKind::Event && record.topic == "echo/tick"));
}

#[test]
fn logs_are_ring_buffered() {
    let host = PluginHost::new(None);
    for index in 0..(MAX_LOG_ENTRIES + 5) {
        host.sink().log("echo", LOG_INFO, format!("line {index}"));
    }
    assert_eq!(host.logs("echo", 0).len(), MAX_LOG_ENTRIES);
    assert_eq!(host.logs("echo", 3).len(), 3);
    host.sink().clear_logs("echo");
    assert!(host.logs("echo", 0).is_empty());
}

#[test]
fn revision_tracks_registry_changes() {
    let host = PluginHost::new(None);
    let before = host.revision();
    assert_eq!(host.bump_revision(), before + 1);
}

// ─── End to end: a real child process ───────────────────────────────────────

const ECHO_SERVER: &str = r#"
const readline = require('node:readline');
const rl = readline.createInterface({ input: process.stdin });
const send = (value) => process.stdout.write(JSON.stringify(value) + '\n');
rl.on('line', (line) => {
  if (!line.trim()) return;
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.type === 'hello') {
    send({ type: 'ready', plugin: msg.plugin, version: '1.0.0' });
    return;
  }
  if (msg.type === 'request') {
    const args = msg.args || {};
    if (msg.tool === 'echo') {
      send({ type: 'reply', id: msg.id, ok: true,
             result: { echo: args.message || '', instance: process.env.PLUGIN_INSTANCE } });
      return;
    }
    if (msg.tool === 'boom') {
      send({ type: 'reply', id: msg.id, ok: false, error: 'boom as requested' });
      return;
    }
    send({ type: 'reply', id: msg.id, ok: false, error: 'unknown tool ' + msg.tool });
    return;
  }
  if (msg.type === 'event') {
    send({ type: 'event', topic: 'echo/seen', payload: msg.payload });
    return;
  }
  if (msg.type === 'shutdown') process.exit(0);
});
"#;

fn write_echo_plugin(dir: &Path) -> Plugin {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("server.js"), ECHO_SERVER).unwrap();
    std::fs::write(dir.join("plugin.json"), "{}").unwrap();
    Plugin {
        workspace: dir.to_path_buf(),
        dir: dir.to_path_buf(),
        manifest: PluginManifest {
            id: "echo".to_string(),
            name: "Echo".to_string(),
            version: "1.0.0".to_string(),
            description: "test plugin".to_string(),
            runtime: PluginRuntime::Node,
            command: String::new(),
            args: vec![],
            entry: "server.js".to_string(),
            env: Default::default(),
            instances: 2,
            enabled: true,
            auto_start: true,
            call_timeout_ms: 5_000,
            tools: vec![PluginTool {
                name: "echo".to_string(),
                description: "Echo a message".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "message": { "type": "string" } }
                }),
                topic: "echo".to_string(),
            }],
            subscriptions: vec!["echo/#".to_string()],
            source: String::new(),
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn a_real_plugin_answers_over_the_bus() {
    if crate::executable::resolve_executable("node").is_err() {
        eprintln!("skipping: node is not on PATH");
        return;
    }
    let root = std::env::temp_dir().join(format!("rabbit-plugin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let plugin = write_echo_plugin(&root);

    let host = PluginHost::new(None);
    let reply = host
        .call(
            &plugin,
            "echo",
            "echo",
            json!({ "message": "hello bus" }),
            Duration::from_secs(10),
        )
        .await
        .expect("the echo tool answers");
    let parsed: Value = serde_json::from_str(&reply).expect("JSON result");
    assert_eq!(parsed["echo"], "hello bus");

    // Two instances, both ready: the request went to one of them.
    let live = host.status().await;
    assert_eq!(live.iter().filter(|view| view.state == "ready").count(), 2);

    // An error reply reaches the model as text, not as a failure of the call.
    let failure = host
        .call(&plugin, "boom", "echo", json!({}), Duration::from_secs(10))
        .await
        .expect("a plugin-level error still resolves the request");
    assert!(failure.contains("boom as requested"), "{failure}");

    // Fan-out: `echo/#` matches a publish to `echo/tick` for both instances.
    let mut registry = PluginRegistry::default();
    registry.plugins.push(plugin.clone());
    let recipients = host
        .publish(&registry, "echo/tick", json!({ "n": 1 }))
        .await
        .expect("both instances subscribe");
    assert_eq!(recipients.len(), 2);

    // … and the instance-pushed event was queued for the model.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let events = host.sink().events("echo", false);
    assert!(!events.is_empty(), "instances pushed echo/seen events");
    assert_eq!(events[0].topic, "echo/seen");

    host.stop("echo").await;
    assert!(host.status().await.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// The creation manual's settings example has to load: a doc that does not
/// parse is exactly how `missing field 'key'` reaches a user.
#[test]
fn the_creation_manual_settings_example_parses() {
    let manual = crate::prompt::plugin_manual();
    let example = manual
        .split("```yaml")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("the manual carries a yaml example");
    let schema: config::ConfigSchema =
        serde_yaml::from_str(example).unwrap_or_else(|err| panic!("manual example: {err}"));
    assert!(!schema.sections.is_empty());
    for section in &schema.sections {
        assert!(!section.key.is_empty(), "a section needs a key");
        assert!(!section.fields.is_empty(), "a section needs fields");
        for field in &section.fields {
            assert!(!field.key.is_empty(), "a field needs a key");
        }
    }
}
