use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::Notify;

use crate::AppState;

// ─── MCP Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Sse,
}

impl Default for McpTransport {
    fn default() -> Self {
        McpTransport::Stdio
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub transport: McpTransport,
    /// For stdio: the executable command
    #[serde(default)]
    pub command: String,
    /// For stdio: command-line arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// For stdio: environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// For SSE: server URL
    #[serde(default)]
    pub url: String,
    /// For SSE: optional auth token (Bearer)
    #[serde(default)]
    pub auth_token: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct McpServersFile {
    pub servers: Vec<McpServer>,
}

// ─── Monitor status / log event payloads ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpStatusKind {
    Starting,
    Running,
    Stopped,
    Error,
    /// SSE transport — no process to monitor, just an endpoint.
    Ready,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpStatus {
    pub id: String,
    pub name: String,
    pub status: McpStatusKind,
    pub pid: Option<u32>,
    /// Milliseconds since UNIX epoch.
    pub started_at_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpLogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpLogEvent {
    pub id: String,
    pub name: String,
    pub stream: McpLogStream,
    pub line: String,
    /// Milliseconds since UNIX epoch.
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpStatusEvent {
    pub id: String,
    pub name: String,
    pub status: McpStatusKind,
    pub pid: Option<u32>,
    pub message: Option<String>,
}

pub const MCP_STATUS_EVENT: &str = "mcp-status";
pub const MCP_LOG_EVENT: &str = "mcp-log";

// ─── Internal: runtime registry ───────────────────────────────────────────────

/// Per-server runtime metadata kept in `AppState::mcp_runtimes`.
pub struct McpRuntime {
    pub id: String,
    pub name: String,
    pub status: McpStatusKind,
    pub pid: Option<u32>,
    pub started_at_ms: Option<u64>,
    pub last_error: Option<String>,
    /// Notify handle the wait task listens on. `None` for SSE (no process).
    pub stop: Option<Arc<Notify>>,
}

pub type McpRuntimeMap = HashMap<String, McpRuntime>;

// ─── Internal helpers ─────────────────────────────────────────────────────────

pub fn load_servers(path: &PathBuf) -> Vec<McpServer> {
    if !path.exists() {
        return vec![];
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<McpServersFile>(&s).ok())
        .map(|f| f.servers)
        .unwrap_or_default()
}

fn save_servers(path: &PathBuf, servers: &[McpServer]) -> Result<(), String> {
    let file = McpServersFile {
        servers: servers.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn emit_status(
    app: &AppHandle,
    id: &str,
    name: &str,
    status: McpStatusKind,
    pid: Option<u32>,
    message: Option<String>,
) {
    let _ = app.emit(
        MCP_STATUS_EVENT,
        McpStatusEvent {
            id: id.to_string(),
            name: name.to_string(),
            status,
            pid,
            message,
        },
    );
}

fn emit_log(app: &AppHandle, id: &str, name: &str, stream: McpLogStream, line: String) {
    let _ = app.emit(
        MCP_LOG_EVENT,
        McpLogEvent {
            id: id.to_string(),
            name: name.to_string(),
            stream,
            line,
            ts: now_ms(),
        },
    );
}

// ─── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_mcp_servers(state: State<'_, AppState>) -> Vec<McpServer> {
    load_servers(&state.mcp_servers_path)
}

#[tauri::command]
pub fn save_mcp_server(
    app: AppHandle,
    state: State<'_, AppState>,
    server: McpServer,
) -> Result<(), String> {
    let was_enabled = load_servers(&state.mcp_servers_path)
        .into_iter()
        .find(|s| s.id == server.id)
        .map(|s| s.enabled)
        .unwrap_or(false);

    let mut servers = load_servers(&state.mcp_servers_path);
    if let Some(existing) = servers.iter_mut().find(|s| s.id == server.id) {
        *existing = server.clone();
    } else {
        servers.push(server.clone());
    }
    let save_result = save_servers(&state.mcp_servers_path, &servers);
    if save_result.is_err() {
        return save_result;
    }

    // Reconcile monitor session: start if newly enabled, stop if newly disabled,
    // restart if transport/command/args/env/url/auth_token changed while enabled.
    let runtimes = state.mcp_runtimes.clone();
    if server.enabled {
        if !was_enabled {
            spawn_monitor_session(app, server, runtimes);
        } else {
            // Could compare fields and restart; for simplicity, always restart on save.
            stop_persistent_session(&server.id, &state.mcp_runtimes);
            spawn_monitor_session(app, server, runtimes);
        }
    } else if was_enabled {
        stop_persistent_session(&server.id, &state.mcp_runtimes);
    }

    Ok(())
}

#[tauri::command]
pub fn delete_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    stop_persistent_session(&id, &state.mcp_runtimes);
    let mut servers = load_servers(&state.mcp_servers_path);
    servers.retain(|s| s.id != id);
    save_servers(&state.mcp_servers_path, &servers)
}

/// Test connectivity to an MCP server.
/// Returns Ok(message) on success or Err(message) on failure.
#[tauri::command]
pub async fn test_mcp_server(server: McpServer) -> Result<String, String> {
    match server.transport {
        McpTransport::Stdio => test_stdio_server(&server).await,
        McpTransport::Sse => test_sse_server(&server).await,
    }
}

#[tauri::command]
pub fn start_mcp_server(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let server = load_servers(&state.mcp_servers_path)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Server {id} not found"))?;
    if !server.enabled {
        return Err("Server is disabled".to_string());
    }
    spawn_monitor_session(app, server, state.mcp_runtimes.clone());
    Ok(())
}

#[tauri::command]
pub fn stop_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    stop_persistent_session(&id, &state.mcp_runtimes);
    Ok(())
}

#[tauri::command]
pub fn restart_mcp_server(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let server = load_servers(&state.mcp_servers_path)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Server {id} not found"))?;
    if !server.enabled {
        return Err("Server is disabled".to_string());
    }
    stop_persistent_session(&id, &state.mcp_runtimes);
    spawn_monitor_session(app, server, state.mcp_runtimes.clone());
    Ok(())
}

#[tauri::command]
pub fn list_mcp_status(state: State<'_, AppState>) -> Vec<McpStatus> {
    let map = state.mcp_runtimes.lock().unwrap();
    map.values()
        .map(|rt| McpStatus {
            id: rt.id.clone(),
            name: rt.name.clone(),
            status: rt.status.clone(),
            pid: rt.pid,
            started_at_ms: rt.started_at_ms,
            last_error: rt.last_error.clone(),
        })
        .collect()
}

// ─── Persistent session lifecycle ─────────────────────────────────────────────

/// Fire-and-forget: spawn a long-lived monitor process for the given server
/// and stream its stdout/stderr to the frontend via Tauri events.
pub fn spawn_monitor_session(
    app: AppHandle,
    server: McpServer,
    runtimes: Arc<Mutex<McpRuntimeMap>>,
) {
    tauri::async_runtime::spawn(async move {
        let _ = start_monitor_session_inner(app, server, runtimes).await;
    });
}

async fn start_monitor_session_inner(
    app: AppHandle,
    server: McpServer,
    runtimes: Arc<Mutex<McpRuntimeMap>>,
) -> Result<(), String> {
    if !matches!(server.transport, McpTransport::Stdio) {
        // SSE has no child process to monitor; record as Ready so the UI can show it.
        let started = now_ms();
        {
            let mut map = runtimes.lock().unwrap();
            map.insert(
                server.id.clone(),
                McpRuntime {
                    id: server.id.clone(),
                    name: server.name.clone(),
                    status: McpStatusKind::Ready,
                    pid: None,
                    started_at_ms: Some(started),
                    last_error: None,
                    stop: None,
                },
            );
        }
        emit_status(
            &app,
            &server.id,
            &server.name,
            McpStatusKind::Ready,
            None,
            None,
        );
        return Ok(());
    }

    // Stdio path
    emit_status(
        &app,
        &server.id,
        &server.name,
        McpStatusKind::Starting,
        None,
        None,
    );

    let mut cmd = build_stdio_cmd(&server, true);
    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let err = format!("Failed to start process: {e}");
            {
                let mut map = runtimes.lock().unwrap();
                map.insert(
                    server.id.clone(),
                    McpRuntime {
                        id: server.id.clone(),
                        name: server.name.clone(),
                        status: McpStatusKind::Error,
                        pid: None,
                        started_at_ms: None,
                        last_error: Some(err.clone()),
                        stop: None,
                    },
                );
            }
            emit_status(
                &app,
                &server.id,
                &server.name,
                McpStatusKind::Error,
                None,
                Some(err.clone()),
            );
            return Err(err);
        }
    };
    let pid = child.id();

    let stdout = child.stdout.take().ok_or_else(|| "no stdout".to_string())?;
    let stderr = child.stderr.take().ok_or_else(|| "no stderr".to_string())?;
    // Leave stdin attached to the Child. The monitor process never sends
    // commands, so the server typically blocks on stdin read and stays alive
    // until we kill it. Closing stdin would deliver EOF and likely cause the
    // server to exit before we have a chance to observe its output.

    let stop = Arc::new(Notify::new());
    {
        let mut map = runtimes.lock().unwrap();
        map.insert(
            server.id.clone(),
            McpRuntime {
                id: server.id.clone(),
                name: server.name.clone(),
                status: McpStatusKind::Starting,
                pid,
                started_at_ms: Some(now_ms()),
                last_error: None,
                stop: Some(stop.clone()),
            },
        );
    }

    // Stdout reader task
    {
        let app = app.clone();
        let id = server.id.clone();
        let name = server.name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim_end();
                        if !trimmed.is_empty() {
                            emit_log(&app, &id, &name, McpLogStream::Stdout, trimmed.to_string());
                        }
                    }
                }
            }
        });
    }

    // Stderr reader task
    {
        let app = app.clone();
        let id = server.id.clone();
        let name = server.name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim_end();
                        if !trimmed.is_empty() {
                            emit_log(&app, &id, &name, McpLogStream::Stderr, trimmed.to_string());
                        }
                    }
                }
            }
        });
    }

    // Promote to running now that readers are attached.
    {
        let mut map = runtimes.lock().unwrap();
        if let Some(rt) = map.get_mut(&server.id) {
            rt.status = McpStatusKind::Running;
        }
    }
    emit_status(
        &app,
        &server.id,
        &server.name,
        McpStatusKind::Running,
        pid,
        None,
    );

    // Wait task: handle either user-initiated stop or natural exit.
    {
        let app = app.clone();
        let id = server.id.clone();
        let name = server.name.clone();
        let runtimes = runtimes.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let (final_status, message) = tokio::select! {
                biased;
                _ = stop.notified() => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    (McpStatusKind::Stopped, Some("stopped by user".to_string()))
                }
                res = child.wait() => match res {
                    Ok(s) => (McpStatusKind::Stopped, Some(format!("exited: {s}"))),
                    Err(e) => (McpStatusKind::Error, Some(format!("wait error: {e}"))),
                },
            };
            {
                let mut map = runtimes.lock().unwrap();
                if let Some(rt) = map.get_mut(&id) {
                    // Only mutate if the entry's stop handle still points at
                    // THIS session. A restart replaces the entry, so the
                    // previous wait task must not clobber the new one.
                    let same_session = rt
                        .stop
                        .as_ref()
                        .map(|s| Arc::ptr_eq(s, &stop))
                        .unwrap_or(false);
                    if same_session {
                        rt.status = final_status.clone();
                        rt.pid = None;
                        rt.last_error = message.clone();
                    }
                }
            }
            emit_status(&app, &id, &name, final_status, None, message);
        });
    }

    Ok(())
}

/// Signal the wait task of a running monitor session to stop the process.
/// SSE sessions have no stop handle and are a no-op.
pub fn stop_persistent_session(id: &str, runtimes: &Arc<Mutex<McpRuntimeMap>>) {
    let stop = {
        let map = runtimes.lock().unwrap();
        map.get(id).and_then(|rt| rt.stop.clone())
    };
    if let Some(stop) = stop {
        stop.notify_one();
    }
}

// ─── LLM integration helpers ──────────────────────────────────────────────────

/// Recursively walk through a JSON value and resolve any relative file-path
/// strings to absolute paths by joining them with `workspace`.
///
/// A string is treated as a path that should be resolved when:
/// - it is non-empty
/// - it does not contain `://` (not a URL)
/// - it is a relative `std::path::Path` (not absolute)
/// - it starts with `.` OR contains a `/` or `\` separator
///
/// Additionally, relative file URLs that begin with `file://./` or `file://.\`
/// are resolved relative to `workspace` and rewritten as absolute `file://` URLs.
pub fn resolve_paths_in_args(value: &mut serde_json::Value, workspace: &std::path::Path) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(resolved) = try_resolve_relative_path(s, workspace) {
                *s = resolved;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                resolve_paths_in_args(item, workspace);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                resolve_paths_in_args(v, workspace);
            }
        }
        _ => {}
    }
}

fn try_resolve_relative_path(s: &str, workspace: &std::path::Path) -> Option<String> {
    if s.is_empty() {
        return None;
    }

    if let Some(file_url_path) = s
        .strip_prefix("file://./")
        .or_else(|| s.strip_prefix("file://.\\"))
    {
        let p = std::path::Path::new(file_url_path);
        let absolute = workspace.join(p);
        let normalized = absolute.to_string_lossy().replace('\\', "/");
        return Some(format!("file:///{normalized}"));
    }

    if s.contains("://") {
        return None;
    }

    let p = std::path::Path::new(s);
    if p.is_absolute() {
        return None;
    }
    // Only process strings that look like paths
    let looks_like_path = s.starts_with('.') || s.contains('/') || s.contains('\\');
    if !looks_like_path {
        return None;
    }
    let absolute = workspace.join(p);
    Some(absolute.to_string_lossy().into_owned())
}

/// Sanitize a string so it can be used as part of an OpenAI function name.
/// Allowed chars: letters, digits, underscores.
pub fn sanitize_fn_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Build a `tokio::process::Command` for a stdio MCP server.
///
/// On Windows, Tauri apps may not inherit the full system PATH, making
/// commands like `npx` or `uvx` fail with "program not found".
/// The fix is to route through `cmd.exe /C` so the shell resolves PATH.
/// On Unix, we use `sh -c` for the same reason.
///
/// `pipe_stderr` controls whether stderr is piped. The persistent monitor
/// process pipes stderr so it can stream to the UI; one-shot test/warmup
/// calls keep it null to avoid backpressure on a noisy server.
fn build_stdio_cmd(server: &McpServer, pipe_stderr: bool) -> tokio::process::Command {
    use std::process::Stdio;

    let normalized = server.command.trim().to_lowercase();
    let is_known_stdio_runtime = matches!(
        normalized.as_str(),
        "uvx" | "ux" | "python" | "python3" | "py" | "node"
    );

    let stderr_cfg = if pipe_stderr {
        Stdio::piped()
    } else {
        Stdio::null()
    };

    #[cfg(windows)]
    {
        // Check if the command looks like an absolute path (contains \ or /)
        let is_absolute = server.command.contains('\\')
            || server.command.contains('/')
            || std::path::Path::new(&server.command).is_absolute();

        if !is_absolute || is_known_stdio_runtime {
            // Wrap in PowerShell so that PATH is inherited from the Windows shell
            // and the console uses UTF-8 for stdin/stdout.
            let mut args_str = shell_escape_powershell(&server.command);
            for arg in &server.args {
                args_str.push(' ');
                args_str.push_str(&shell_escape_powershell(arg));
            }
            let wrapped = format!(
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; [Console]::InputEncoding = [System.Text.Encoding]::UTF8; & {args_str}"
            );
            let mut cmd = tokio::process::Command::new("powershell.exe");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &wrapped,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr_cfg);
            for (k, v) in &server.env {
                cmd.env(k, v);
            }
            // Prevent a console window from flashing on Windows
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
            return cmd;
        }
    }

    #[cfg(unix)]
    {
        let is_absolute = server.command.starts_with('/');
        if !is_absolute || is_known_stdio_runtime {
            let mut shell_cmd = server.command.clone();
            for arg in &server.args {
                shell_cmd.push(' ');
                shell_cmd.push_str(&shell_quote_unix(arg));
            }
            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", &shell_cmd])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(stderr_cfg);
            for (k, v) in &server.env {
                cmd.env(k, v);
            }
            return cmd;
        }
    }

    // Absolute path — spawn directly
    let mut cmd = tokio::process::Command::new(&server.command);
    cmd.args(&server.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr_cfg);
    for (k, v) in &server.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(windows)]
fn shell_escape_powershell(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let needs_quotes = s.contains(' ')
        || s.contains('"')
        || s.contains('\t')
        || s.contains('&')
        || s.contains('|')
        || s.contains(';')
        || s.contains('>')
        || s.contains('<')
        || s.contains('$')
        || s.contains('`')
        || s.contains('(')
        || s.contains(')');
    if needs_quotes {
        let escaped = s.replace('\'', "''");
        format!("'{}'", escaped)
    } else {
        s.to_string()
    }
}

#[cfg(unix)]
fn shell_quote_unix(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── stdio JSON-RPC helpers ──────────────────────────────────────────────────

async fn stdio_write_json(
    stdin: &mut tokio::process::ChildStdin,
    value: &serde_json::Value,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let line = format!("{}\n", value);
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("stdin write: {e}"))
}

/// Read lines from stdout until we get a JSON object whose `id` matches `expected_id`.
/// Notifications (messages without an `id` field, or with `method`) are skipped.
async fn stdio_read_response(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    expected_id: u64,
) -> Result<serde_json::Value, String> {
    use tokio::time::{timeout, Duration};
    timeout(Duration::from_secs(10), async {
        let mut line = String::new();
        loop {
            line.clear();
            reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("stdout read: {e}"))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                continue;
            };
            // Skip notifications (they have "method" but no matching id)
            if parsed.get("method").is_some() {
                continue;
            }
            if parsed["id"].as_u64() == Some(expected_id) {
                return Ok(parsed);
            }
        }
    })
    .await
    .map_err(|_| "MCP stdio timeout".to_string())?
}

/// Spawn an MCP stdio process and run `initialize` + `notifications/initialized`.
/// Returns (child, stdin, stdout_reader) ready for further RPC calls.
async fn stdio_init(
    server: &McpServer,
) -> Result<
    (
        tokio::process::Child,
        tokio::process::ChildStdin,
        tokio::io::BufReader<tokio::process::ChildStdout>,
    ),
    String,
> {
    let mut cmd = build_stdio_cmd(server, false);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start process: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut reader = tokio::io::BufReader::new(stdout);

    // initialize
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "ai-chat", "version": "0.1.0" }
        },
        "id": 1
    });
    stdio_write_json(&mut stdin, &init_req).await?;
    stdio_read_response(&mut reader, 1).await?;

    // send initialized notification (no response expected)
    let notif = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    stdio_write_json(&mut stdin, &notif).await?;

    Ok((child, stdin, reader))
}

// ── Public functions used by llm_complete ───────────────────────────────────

/// Get the list of tools from an MCP server, in OpenAI /chat/completions tool format.
pub async fn get_server_tools(server: &McpServer) -> Result<Vec<serde_json::Value>, String> {
    match server.transport {
        McpTransport::Stdio => get_tools_stdio(server).await,
        McpTransport::Sse => get_tools_sse(server).await,
    }
}

async fn get_tools_stdio(server: &McpServer) -> Result<Vec<serde_json::Value>, String> {
    let (mut child, mut stdin, mut reader) = stdio_init(server).await?;

    let list_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "params": {},
        "id": 2
    });
    stdio_write_json(&mut stdin, &list_req).await?;
    let resp = stdio_read_response(&mut reader, 2).await;
    let _ = child.kill().await;

    let resp = resp?;
    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tools/list error: {err}"));
    }
    let tools = resp["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(convert_mcp_tools(tools))
}

async fn get_tools_sse(server: &McpServer) -> Result<Vec<serde_json::Value>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "params": {},
        "id": 1
    });

    let mut builder = client.post(&server.url).json(&req_body);
    if !server.auth_token.is_empty() {
        builder = builder.header("Authorization", format!("Bearer {}", server.auth_token));
    }

    let resp = builder
        .send()
        .await
        .map_err(|e| format!("HTTP error: {e}"))?;
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Response parse: {e}"))?;

    if let Some(err) = body.get("error") {
        return Err(format!("MCP tools/list error: {err}"));
    }
    let tools = body["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(convert_mcp_tools(tools))
}

/// Convert MCP tool definitions to OpenAI function-calling tool format.
fn convert_mcp_tools(mcp_tools: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    mcp_tools
        .into_iter()
        .filter_map(|t| {
            let name = t["name"].as_str()?;
            let description = t["description"].as_str().unwrap_or("").to_string();
            let input_schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
            Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": description,
                    "parameters": input_schema
                }
            }))
        })
        .collect()
}

/// Call a tool on an MCP server and return its text result.
pub async fn invoke_mcp_tool(
    server: &McpServer,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    match server.transport {
        McpTransport::Stdio => invoke_tool_stdio(server, tool_name, arguments).await,
        McpTransport::Sse => invoke_tool_sse(server, tool_name, arguments).await,
    }
}

async fn invoke_tool_stdio(
    server: &McpServer,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    let (mut child, mut stdin, mut reader) = stdio_init(server).await?;

    let call_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
        },
        "id": 2
    });
    stdio_write_json(&mut stdin, &call_req).await?;
    let resp = stdio_read_response(&mut reader, 2).await;
    let _ = child.kill().await;

    let resp = resp?;
    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tool error: {err}"));
    }
    Ok(extract_tool_result(&resp["result"]))
}

async fn invoke_tool_sse(
    server: &McpServer,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": { "name": tool_name, "arguments": arguments },
        "id": 1
    });

    let mut builder = client.post(&server.url).json(&req_body);
    if !server.auth_token.is_empty() {
        builder = builder.header("Authorization", format!("Bearer {}", server.auth_token));
    }

    let resp = builder
        .send()
        .await
        .map_err(|e| format!("HTTP error: {e}"))?;
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Response parse: {e}"))?;

    if let Some(err) = body.get("error") {
        return Err(format!("MCP tool error: {err}"));
    }
    Ok(extract_tool_result(&body["result"]))
}

/// Extract a string result from an MCP tools/call response result.
fn extract_tool_result(result: &serde_json::Value) -> String {
    // MCP result content is usually an array of content blocks
    if let Some(content) = result["content"].as_array() {
        let parts: Vec<String> = content
            .iter()
            .filter_map(|block| {
                if block["type"].as_str() == Some("text") {
                    block["text"].as_str().map(|s| s.to_string())
                } else {
                    Some(serde_json::to_string(block).unwrap_or_default())
                }
            })
            .collect();
        return parts.join("\n");
    }
    // Fallback: serialize the whole result
    serde_json::to_string_pretty(result).unwrap_or_default()
}

async fn test_stdio_server(server: &McpServer) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    use tokio::time::{timeout, Duration};

    if server.command.trim().is_empty() {
        return Err("Command is empty".to_string());
    }

    // MCP initialize request (protocol version 2025-03-26)
    let init_request = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "ai-chat",
                "version": "0.1.0"
            }
        },
        "id": 1
    });
    let request_line = format!("{}\n", init_request);

    let mut cmd = build_stdio_cmd(server, false);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start process: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("No stdin")?;
    let mut stdout = child.stdout.take().ok_or("No stdout")?;

    stdin
        .write_all(request_line.as_bytes())
        .await
        .map_err(|e| format!("Write error: {e}"))?;
    drop(stdin);

    let read_result = timeout(Duration::from_secs(5), async {
        let mut reader = tokio::io::BufReader::new(&mut stdout);
        let mut line = String::new();
        reader.read_line(&mut line).await.map(|_| line)
    })
    .await;

    let _ = child.kill().await;

    match read_result {
        Ok(Ok(line)) if !line.trim().is_empty() => {
            // Validate it looks like a JSON-RPC response
            if line.contains("\"result\"") || line.contains("jsonrpc") {
                Ok(format!(
                    "Connected successfully (received {} bytes)",
                    line.len()
                ))
            } else {
                Err(format!(
                    "Unexpected response: {}",
                    line.chars().take(200).collect::<String>()
                ))
            }
        }
        Ok(Ok(_)) => Err("Server closed connection without response".to_string()),
        Ok(Err(e)) => Err(format!("Read error: {e}")),
        Err(_) => Err("Timeout: no response within 5 seconds".to_string()),
    }
}

async fn test_sse_server(server: &McpServer) -> Result<String, String> {
    if server.url.trim().is_empty() {
        return Err("URL is empty".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.get(&server.url);
    if !server.auth_token.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", server.auth_token));
    }
    // For SSE endpoints, request the event stream content type
    req = req.header("Accept", "text/event-stream,application/json");

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    let status = resp.status();

    if status.is_success() {
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok(format!("Connected: HTTP {status} ({content_type})"))
    } else {
        Err(format!("Server returned HTTP {status}"))
    }
}

/// Start a non-blocking monitor session for an enabled MCP server.
/// Used at app startup and on server enable.
/// (Retained for compatibility with `lib.rs` startup; equivalent to
/// `spawn_monitor_session`.)
pub fn spawn_warmup(server: McpServer, app: AppHandle, runtimes: Arc<Mutex<McpRuntimeMap>>) {
    spawn_monitor_session(app, server, runtimes);
}
