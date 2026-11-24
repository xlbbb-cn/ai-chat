//! One long-lived plugin instance: spawn, line protocol, supervision.
//!
//! A plugin runs as a child process speaking **line-delimited JSON** over
//! stdin/stdout (the same shape MCP stdio servers use, minus JSON-RPC framing —
//! a plugin is an app extension, not an MCP server, and the bus already carries
//! the correlation ids).
//!
//! Host → instance
//! ```json
//! {"type":"hello","plugin":"clock","instance":0,"instances":2,
//!  "workspace":"E:/proj","dir":"E:/proj/.data/plugins/clock","topics":["clock/#"]}
//! {"type":"request","id":"p12","topic":"clock","tool":"now","args":{},"timeout_ms":30000}
//! {"type":"event","topic":"clock/tick","payload":{...},"sender":"host"}
//! {"type":"ping","id":"p13"}
//! {"type":"shutdown"}
//! ```
//!
//! Instance → host
//! ```json
//! {"type":"ready","plugin":"clock","version":"1.0.0","tools":["now"]}
//! {"type":"reply","id":"p12","ok":true,"result":"2026-01-01T00:00:00Z"}
//! {"type":"reply","id":"p12","ok":false,"error":"bad args"}
//! {"type":"event","topic":"clock/tick","payload":{...}}
//! {"type":"log","level":"info","message":"tick loop started"}
//! {"type":"pong","id":"p13"}
//! ```
//!
//! Anything else on stdout is logged and ignored; a line that is not JSON is
//! logged as a warning (a plugin that prints debug output must not kill the
//! instance). stderr is drained into the plugin's log ring.

use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::process::{Child, ChildStdin};
use tokio::task::JoinHandle;

use super::bus::Reply;
use super::{Plugin, Sink, LOG_INFO, LOG_WARN};

/// How one instance is reached: a spawned child's stdin, or the socket of a
/// service that dialled the plugin bus (`plugins/link.rs`).
pub enum Outbound {
    Stdio(ChildStdin),
    Link(OwnedWriteHalf),
}

impl Outbound {
    async fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Outbound::Stdio(stdin) => stdin.write_all(bytes).await,
            Outbound::Link(writer) => writer.write_all(bytes).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Outbound::Stdio(stdin) => stdin.flush().await,
            Outbound::Link(writer) => writer.flush().await,
        }
    }
}

/// Who owns the process behind an instance.
///
/// `Managed` instances are children we started; `External` ones are services
/// that attached to the bus and keep running (and restarting) on their own —
/// shutting one down must never kill a process we did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceOwner {
    Managed,
    External,
}

/// How long a freshly spawned instance has to answer `hello` with `ready`.
pub const READY_TIMEOUT_MS: u64 = 15_000;

/// Runtime state of one instance, as reported by the reader task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceState {
    /// Spawned, waiting for the `ready` line.
    Starting,
    /// Handshaken and able to take requests.
    Ready,
    /// Exited or refused to start; the string is the reason shown in the UI.
    Failed(String),
    /// Stopped on purpose (plugin disabled, workspace switch, app exit).
    Stopped,
}

impl InstanceState {
    pub fn name(&self) -> &'static str {
        match self {
            InstanceState::Starting => "starting",
            InstanceState::Ready => "ready",
            InstanceState::Failed(_) => "failed",
            InstanceState::Stopped => "stopped",
        }
    }
}

/// One running child process plus its bookkeeping.
pub struct Instance {
    pub plugin_id: String,
    pub index: u32,
    /// Stable log/label id (`clock#0`).
    pub label: String,
    /// Bumped on every respawn, so a stale event from an older process can be
    /// told apart in the log / audit ring.
    pub generation: u32,
    pid: AtomicU32,
    inflight: AtomicU32,
    restarts: AtomicU32,
    started_at_ms: AtomicU64,
    state: Mutex<InstanceState>,
    last_error: Mutex<Option<String>>,
    out: tokio::sync::Mutex<Option<Outbound>>,
    owner: InstanceOwner,
    child: Mutex<Option<Child>>,
    tracked: Mutex<Option<crate::process_guard::TrackedProcess>>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Instance {
    pub fn state(&self) -> InstanceState {
        self.state.lock().unwrap().clone()
    }

    pub fn set_state(&self, next: InstanceState) {
        if let InstanceState::Failed(reason) = &next {
            *self.last_error.lock().unwrap() = Some(reason.clone());
        }
        *self.state.lock().unwrap() = next;
    }

    pub fn pid(&self) -> Option<u32> {
        let pid = self.pid.load(Ordering::SeqCst);
        (pid != 0).then_some(pid)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    pub fn restarts(&self) -> u32 {
        self.restarts.load(Ordering::SeqCst)
    }

    pub fn started_at_ms(&self) -> u64 {
        self.started_at_ms.load(Ordering::SeqCst)
    }

    pub fn inflight(&self) -> u32 {
        self.inflight.load(Ordering::SeqCst)
    }

    /// A request slot for this instance: incremented for the duration of one
    /// bus round trip so the scheduler prefers an idle instance.
    pub fn mark_inflight(&self) -> InflightGuard<'_> {
        self.inflight.fetch_add(1, Ordering::SeqCst);
        InflightGuard { instance: self }
    }

    pub fn is_ready(&self) -> bool {
        self.state() == InstanceState::Ready
    }

    /// Ask the OS whether the child is still there. A process that exited
    /// without a line is a failure the reader task may not have observed yet.
    pub fn is_alive(&self) -> bool {
        if self.owner == InstanceOwner::External {
            // No child to poll: closing the socket flips the state instead.
            return !matches!(self.state(), InstanceState::Failed(_) | InstanceState::Stopped);
        }
        if self.is_ready() {
            return true;
        }
        let mut child = self.child.lock().unwrap();
        match child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) => true,
                Err(_) => false,
            },
            None => false,
        }
    }

    pub async fn send_line(&self, value: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(value).map_err(|err| err.to_string())?;
        line.push('\n');
        let mut out = self.out.lock().await;
        let Some(out) = out.as_mut() else {
            return Err(format!("{} has no channel (instance gone)", self.label));
        };
        out.write_all(line.as_bytes())
            .await
            .map_err(|err| format!("write to {} failed: {err}", self.label))?;
        out.flush()
            .await
            .map_err(|err| format!("write to {} failed: {err}", self.label))
    }

    /// Wait until the instance reports `ready` (or fails).
    pub async fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.state() {
                InstanceState::Ready => return Ok(()),
                InstanceState::Failed(reason) => return Err(reason),
                InstanceState::Stopped => {
                    return Err(format!("{} was stopped before it became ready", self.label))
                }
                InstanceState::Starting => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "{} did not report ready within {} ms (the plugin must answer the hello \
                     line with {{\"type\":\"ready\"}})",
                    self.label,
                    timeout.as_millis()
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Kill the child (whole tree) and stop its reader task.
    pub async fn shutdown(&self, graceful: bool) {
        if graceful && self.is_ready() {
            let _ = self.send_line(&json!({ "type": "shutdown" })).await;
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        self.set_state(InstanceState::Stopped);
        *self.out.lock().await = None;
        if self.owner == InstanceOwner::External {
            // An attached service owns its process: dropping the socket is the
            // whole shutdown.
            if let Some(handle) = self.reader.lock().unwrap().take() {
                handle.abort();
            }
            return;
        }
        if let Some(pid) = self.pid() {
            crate::process_guard::kill_process_tree(Some(pid));
        }
        {
            let mut child = self.child.lock().unwrap();
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
                let _ = child.try_wait();
            }
            *child = None;
        }
        if let Some(handle) = self.reader.lock().unwrap().take() {
            handle.abort();
        }
        *self.tracked.lock().unwrap() = None;
    }

    pub fn view(&self) -> super::InstanceView {
        super::InstanceView {
            plugin_id: self.plugin_id.clone(),
            label: self.label.clone(),
            index: self.index,
            generation: self.generation,
            state: self.state().name().to_string(),
            pid: self.pid(),
            inflight: self.inflight(),
            restarts: self.restarts(),
            started_at_ms: self.started_at_ms(),
            last_error: self.last_error(),
        }
    }
}

/// Decrements the in-flight counter when the bus round trip ends.
pub struct InflightGuard<'a> {
    instance: &'a Instance,
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.instance.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // `shutdown` already cleared everything; this is the unwind safety net
        // (pool cleared on workspace switch, panic in a caller).
        if let Some(pid) = self.pid() {
            crate::process_guard::kill_process_tree(Some(pid));
        }
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
        }
        if let Ok(mut reader) = self.reader.lock() {
            if let Some(handle) = reader.take() {
                handle.abort();
            }
        }
    }
}

/// Adopt a service connection as an instance of `plugin`.
///
/// The connection already said `hello` with the bus token; from here it is
/// one instance among `N`, dispatched exactly like a spawned child.
pub fn attach(
    sink: &Sink,
    plugin: &Plugin,
    index: u32,
    remote: String,
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: OwnedWriteHalf,
) -> Arc<Instance> {
    let id = plugin.manifest.id.clone();
    let instance = Arc::new(Instance {
        plugin_id: id.clone(),
        index,
        label: format!("{id}#link{index}"),
        generation: 1,
        pid: AtomicU32::new(0),
        inflight: AtomicU32::new(0),
        restarts: AtomicU32::new(0),
        started_at_ms: AtomicU64::new(crate::util::now_ms_u64()),
        state: Mutex::new(InstanceState::Starting),
        last_error: Mutex::new(None),
        out: tokio::sync::Mutex::new(Some(Outbound::Link(writer))),
        owner: InstanceOwner::External,
        child: Mutex::new(None),
        tracked: Mutex::new(None),
        reader: Mutex::new(None),
    });
    let reader_sink = sink.clone();
    let reader_instance: Weak<Instance> = Arc::downgrade(&instance);
    let label = instance.label.clone();
    let handle = tokio::spawn(async move {
        let mut lines = reader.lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let Some(instance) = reader_instance.upgrade() else {
                        return;
                    };
                    handle_line(&reader_sink, &instance, &line).await;
                }
                Ok(None) => break,
                Err(err) => {
                    reader_sink.log(&label, LOG_WARN, format!("link read failed: {err}"));
                    break;
                }
            }
        }
        reader_sink.log(&label, LOG_INFO, format!("service {remote} disconnected"));
        if let Some(instance) = reader_instance.upgrade() {
            instance.set_state(InstanceState::Failed("service disconnected".into()));
            reader_sink.emit_state(&instance, "failed");
        }
    });
    *instance.reader.lock().unwrap() = Some(handle);
    instance.set_state(InstanceState::Ready);
    sink.emit_state(&instance, "ready");
    instance
}

/// Spawn one instance of `plugin` and wait for its `ready` line.
pub async fn spawn(sink: &Sink, plugin: &Plugin, index: u32, generation: u32) -> Result<Arc<Instance>, String> {
    let manifest = &plugin.manifest;
    let mut command = build_command(plugin, index)?;
    command
        .current_dir(&plugin.dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|err| {
        format!(
            "failed to start plugin '{}' ({}): {err}",
            manifest.id,
            command_label(plugin)
        )
    })?;

    let pid = child.id();
    let tracked = crate::process_guard::track_child(&child, format!("plugin:{}#{index}", manifest.id));
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let instance = Arc::new(Instance {
        plugin_id: manifest.id.clone(),
        index,
        label: format!("{}#{index}", manifest.id),
        generation,
        pid: AtomicU32::new(pid.unwrap_or(0)),
        inflight: AtomicU32::new(0),
        // The generation is bumped for every respawn of the pool, so an
        // instance that came back after a crash reads as "restarted once".
        restarts: AtomicU32::new(generation.saturating_sub(1)),
        started_at_ms: AtomicU64::new(crate::util::now_ms_u64()),
        state: Mutex::new(InstanceState::Starting),
        last_error: Mutex::new(None),
        out: tokio::sync::Mutex::new(stdin.map(Outbound::Stdio)),
        owner: InstanceOwner::Managed,
        child: Mutex::new(Some(child)),
        tracked: Mutex::new(Some(tracked)),
        reader: Mutex::new(None),
    });

    if let Some(stderr) = stderr {
        spawn_stderr_drain(stderr, sink.clone(), manifest.id.clone(), instance.label.clone());
    }

    if let Some(stdout) = stdout {
        let reader_sink = sink.clone();
        let reader_instance: Weak<Instance> = Arc::downgrade(&instance);
        let handle = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let Some(instance) = reader_instance.upgrade() else {
                            return;
                        };
                        handle_line(&reader_sink, &instance, &line).await;
                    }
                    Ok(None) => break,
                    Err(err) => {
                        reader_sink.log(
                            &instance_label(&reader_instance),
                            LOG_WARN,
                            format!("stdout read failed: {err}"),
                        );
                        break;
                    }
                }
            }
            if let Some(instance) = reader_instance.upgrade() {
                if instance.state() != InstanceState::Stopped {
                    instance.set_state(InstanceState::Failed(
                        "the plugin process ended (stdout closed)".to_string(),
                    ));
                }
                reader_sink.emit_state(&instance, instance.state().name());
            }
        });
        *instance.reader.lock().unwrap() = Some(handle);
    }

    let hello = json!({
        "type": "hello",
        "plugin": manifest.id,
        "name": manifest.name,
        "instance": index,
        "instances": manifest.instances,
        "workspace": plugin.workspace.to_string_lossy(),
        "dir": plugin.dir.to_string_lossy(),
        "topics": manifest.subscriptions,
        "protocol": 1,
    });
    instance.send_line(&hello).await?;

    match instance.wait_ready(Duration::from_millis(READY_TIMEOUT_MS)).await {
        Ok(()) => {
            sink.log(
                &manifest.id,
                LOG_INFO,
                format!(
                    "instance {} started (pid {:?}, generation {generation})",
                    instance.label,
                    instance.pid()
                ),
            );
            sink.emit_state(&instance, "ready");
            Ok(instance)
        }
        Err(err) => {
            instance.shutdown(false).await;
            sink.log(&manifest.id, LOG_WARN, format!("instance {} failed: {err}", instance.label));
            Err(err)
        }
    }
}

fn instance_label(instance: &Weak<Instance>) -> String {
    instance
        .upgrade()
        .map(|instance| instance.label.clone())
        .unwrap_or_else(|| "plugin".to_string())
}

/// Classify one stdout line and route it.
async fn handle_line(sink: &Sink, instance: &Arc<Instance>, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(_) => {
            sink.log(
                &instance.plugin_id,
                LOG_WARN,
                format!(
                    "{} wrote a non-JSON line (ignored): {}",
                    instance.label,
                    truncate(trimmed, 400)
                ),
            );
            return;
        }
    };
    match value["type"].as_str().unwrap_or("") {
        "ready" => {
            instance.set_state(InstanceState::Ready);
            sink.log(
                &instance.plugin_id,
                LOG_INFO,
                format!(
                    "{} ready{}",
                    instance.label,
                    value["version"]
                        .as_str()
                        .map(|version| format!(" (plugin version {version})"))
                        .unwrap_or_default()
                ),
            );
            sink.emit_state(instance, "ready");
        }
        "reply" => {
            let id = value["id"].as_str().unwrap_or("").to_string();
            let reply = Reply {
                ok: value["ok"].as_bool().unwrap_or(true),
                result: value.get("result").cloned().unwrap_or(Value::Null),
                error: value["error"].as_str().map(str::to_string),
            };
            if sink.resolve(&id, reply).is_none() {
                sink.log(
                    &instance.plugin_id,
                    LOG_WARN,
                    format!(
                        "{} replied to request '{id}', which is unknown or already expired",
                        instance.label
                    ),
                );
            }
        }
        "event" => {
            let topic = value["topic"].as_str().unwrap_or("").trim().to_string();
            if topic.is_empty() {
                sink.log(
                    &instance.plugin_id,
                    LOG_WARN,
                    format!("{} sent an event without a topic (ignored)", instance.label),
                );
                return;
            }
            sink.instance_event(
                &instance.plugin_id,
                &instance.label,
                &topic,
                value.get("payload").cloned().unwrap_or(Value::Null),
            );
        }
        "log" => {
            let level = match value["level"].as_str().unwrap_or("info") {
                "error" => "error",
                "warn" => LOG_WARN,
                _ => LOG_INFO,
            };
            sink.log(
                &instance.plugin_id,
                level,
                format!(
                    "{}: {}",
                    instance.label,
                    truncate(value["message"].as_str().unwrap_or(""), 2_000)
                ),
            );
        }
        "pong" => {}
        other => sink.log(
            &instance.plugin_id,
            LOG_WARN,
            format!("{} sent an unknown message type '{other}' (ignored)", instance.label),
        ),
    }
}

fn spawn_stderr_drain(
    stderr: tokio::process::ChildStderr,
    sink: Sink,
    plugin_id: String,
    label: String,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            sink.log(&plugin_id, "stderr", format!("{label}: {}", truncate(&line, 2_000)));
        }
    });
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}…")
}

/// What the OS-level launch line looks like, for an error message.
fn command_label(plugin: &Plugin) -> String {
    let manifest = &plugin.manifest;
    if !manifest.command.trim().is_empty() {
        return format!("{} {}", manifest.command.trim(), manifest.args.join(" "));
    }
    format!("{} {}", manifest.resolved_runtime().name(), manifest.entry)
}

/// Build the child command from the manifest.
///
/// `entry` is resolved against the plugin's own directory and the child starts
/// there, so a plugin is self-contained: it may keep data files next to its
/// entry script. The workspace root reaches it through `PLUGIN_WORKSPACE` and
/// the `hello` line, never through the working directory.
fn build_command(plugin: &Plugin, index: u32) -> Result<tokio::process::Command, String> {
    let manifest = &plugin.manifest;
    let (program, args) = if !manifest.command.trim().is_empty() {
        // Escape hatch: a manifest that names its own executable (a compiled
        // binary, `npx …`, a vendored interpreter).
        let program = crate::executable::resolve_executable(&manifest.command)?;
        let mut args = manifest.args.clone();
        if !manifest.entry.trim().is_empty() {
            let entry = resolve_entry(plugin)?;
            if !args.iter().any(|arg| Path::new(arg) == entry) {
                args.push(entry.to_string_lossy().to_string());
            }
        }
        (program, args)
    } else {
        let entry = resolve_entry(plugin)?;
        let runtime = manifest.resolved_runtime();
        if runtime == super::PluginRuntime::Exec {
            // The entry is the program: a compiled binary or a script with its
            // own shebang. A+x is the manifest's job, not ours.
            (entry, manifest.args.clone())
        } else {
            let (program, mut args) = runtime.interpreter()?;
            args.extend(manifest.args.clone());
            args.push(entry.to_string_lossy().to_string());
            (std::path::PathBuf::from(program), args)
        }
    };

    let mut command = spawn_command(&program, &args);

    command.env("PLUGIN_ID", &manifest.id);
    command.env("PLUGIN_INSTANCE", index.to_string());
    command.env("PLUGIN_INSTANCES", manifest.instances.to_string());
    command.env("PLUGIN_DIR", plugin.dir.to_string_lossy().to_string());
    command.env("PLUGIN_WORKSPACE", plugin.workspace.to_string_lossy().to_string());
    command.env("PYTHONIOENCODING", "utf-8");
    command.env("PYTHONUNBUFFERED", "1");
    command.env("NODE_NO_WARNINGS", "1");
    for (key, value) in &manifest.env {
        command.env(key, value);
    }

    #[cfg(unix)]
    {
        // Own process group, so app exit / stop can kill the whole tree.
        command.process_group(0);
    }

    Ok(command)
}

fn resolve_entry(plugin: &Plugin) -> Result<std::path::PathBuf, String> {
    let manifest = &plugin.manifest;
    let entry = manifest.entry.trim();
    if entry.is_empty() {
        return Err(format!(
            "plugin '{}' declares neither an entry script nor a command",
            manifest.id
        ));
    }
    let path = plugin.dir.join(entry);
    if !path.is_file() {
        return Err(format!(
            "plugin '{}': entry '{}' does not exist",
            manifest.id,
            path.display()
        ));
    }
    Ok(path)
}

/// `tokio::process::Command` for a resolved program.
///
/// Windows needs two adjustments that `Command::new` alone does not give:
/// no console window (a GUI app spawning `node` would otherwise flash one) and
/// a `cmd.exe` wrapper for `.cmd` / `.bat` shims, which `CreateProcess` cannot
/// start directly.
fn spawn_command(program: &Path, args: &[String]) -> tokio::process::Command {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let is_batch = program
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
            .unwrap_or(false);
        let mut command = if is_batch {
            let mut command = tokio::process::Command::new("cmd.exe");
            command.arg("/C").arg(program);
            command
        } else {
            tokio::process::Command::new(program)
        };
        command.args(args);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = tokio::process::Command::new(program);
        command.args(args);
        command
    }
}

