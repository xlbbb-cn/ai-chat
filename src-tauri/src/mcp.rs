use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use tokio::io::AsyncBufReadExt;


use crate::AppState;

// ─── MCP Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Sse,
    /// Plain HTTP JSON-RPC: POST the request, read the JSON response.
    Http,
    /// MCP "Streamable HTTP" (2025-03-26): POST with
    /// `Accept: application/json, text/event-stream`; the server replies
    /// with a JSON body or an SSE stream; session via `Mcp-Session-Id`.
    #[serde(rename = "stream-http", alias = "streamable-http")]
    StreamHttp,
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

// ─── Diagnostic log ──────────────────────────────────────────────────────────

/// Per-server diagnostic log entry. Kept in memory (ring-buffered) so the
/// UI can show "what happened" without writing to disk. Useful for telling
/// "the MCP process wouldn't start" from "the tool returned an error" — the
/// former produces spawn / stderr / exit-status entries, the latter produces
/// a single tools/call error.
#[derive(Debug, Clone, Serialize)]
pub struct McpLogEntry {
    /// Milliseconds since UNIX epoch.
    pub ts: u64,
    pub level: McpLogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpLogLevel {
    Info,
    Warn,
    Error,
}

/// Max entries kept per server in the in-memory ring buffer.
pub const MAX_MCP_LOG_ENTRIES: usize = 200;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Append an entry to the per-server ring buffer. No-op if `state` is
/// somehow not initialised (defensive — should never happen in practice).
pub fn append_mcp_log(state: &AppState, id: &str, level: McpLogLevel, message: String) {
    let mut map = state.mcp_logs.lock().unwrap();
    let buf = map
        .entry(id.to_string())
        .or_insert_with(|| VecDeque::with_capacity(MAX_MCP_LOG_ENTRIES));
    if buf.len() >= MAX_MCP_LOG_ENTRIES {
        buf.pop_front();
    }
    buf.push_back(McpLogEntry {
        ts: now_ms(),
        level,
        message,
    });
}

// ─── Stdio process pool ───────────────────────────────────────────────────────

/// A live stdio MCP child process with its stdin/stdout handles.
/// Stored in `AppState.mcp_children` and shared across LLM turns.
pub struct StdioProcessHandle {
    pub child: tokio::process::Child,
    pub stdin: tokio::process::ChildStdin,
    pub stdout_reader: tokio::io::BufReader<tokio::process::ChildStdout>,
    pub next_id: u64,
}

/// Start a stdio MCP process: validate → spawn → initialize handshake → store in pool.
pub async fn start_stdio_process(
    state: &AppState,
    server: &McpServer,
) -> Result<(), String> {
    let id = server.id.clone();
    let name = server.name.clone();
    let logs = state.mcp_logs.clone();

    // Pre-flight validation: check command exists BEFORE spawning
    if !command_exists(&server.command) {
        let msg = format!(
            "Command not found in PATH: {}. Make sure it is installed.",
            server.command
        );
        append_mcp_log(&state, &id, McpLogLevel::Error, msg.clone());
        return Err(msg);
    }

    append_mcp_log(
        &state,
        &id,
        McpLogLevel::Info,
        format!(
            "Starting stdio process: {} {}",
            server.command,
            server.args.join(" ")
        ),
    );

    let (mut child, stdin_handle, stdout_reader) = stdio_init(server, |stderr| {
        spawn_stderr_drain(logs, id.clone(), name.clone(), stderr);
    })
    .await?;

    // Move the owned handles into the handle struct.
    child.stdin = None;
    child.stdout = None;

    let handle = StdioProcessHandle {
        child,
        stdin: stdin_handle,
        stdout_reader,
        next_id: 2, // id 1 was used by initialize
    };

    state
        .mcp_children
        .lock()
        .await
        .insert(server.id.clone(), handle);

    append_mcp_log(
        &state,
        &id,
        McpLogLevel::Info,
        "Stdio process started and initialized".to_string(),
    );

    Ok(())
}

/// Kill a running stdio process and remove it from the pool.
pub async fn stop_stdio_process(state: &AppState, server_id: &str) {
    let mut pool = state.mcp_children.lock().await;
    if let Some(mut handle) = pool.remove(server_id) {
        let _ = handle.child.start_kill();
        drop(handle);
        append_mcp_log(
            state,
            server_id,
            McpLogLevel::Info,
            "Stdio process stopped".to_string(),
        );
    }
}

/// Check whether a stdio process is still alive (hasn't exited).
fn is_process_alive(handle: &mut StdioProcessHandle) -> bool {
    handle
        .child
        .try_wait()
        .map(|status| status.is_none())
        .unwrap_or(true)
}

/// Ensure a stdio process is running for the given server. Returns true if
/// a process was (re)started. The caller should then call `with_stdio_handle`
/// to use the process.
pub async fn ensure_stdio_process(state: &AppState, server: &McpServer) -> bool {
    // Check if already alive
    {
        let mut pool = state.mcp_children.lock().await;
        if let Some(handle) = pool.get_mut(&server.id) {
            if is_process_alive(handle) {
                return false; // Already running
            }
            // Dead process — remove and restart
            append_mcp_log(
                state,
                &server.id,
                McpLogLevel::Warn,
                "Previous process had exited; restarting".to_string(),
            );
        }
    }

    // Start fresh
    match start_stdio_process(state, server).await {
        Ok(()) => true,
        Err(e) => {
            append_mcp_log(
                state,
                &server.id,
                McpLogLevel::Error,
                format!("Failed to start process: {e}"),
            );
            false
        }
    }
}

/// Send a JSON-RPC request on the persistent stdio process and wait for
/// the matching response. Locks the pool for the duration, serializing
/// requests through the process.
pub async fn stdio_request_persistent(
    state: &AppState,
    server_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut pool = state.mcp_children.lock().await;
    let handle = pool
        .get_mut(server_id)
        .ok_or_else(|| "Process not running".to_string())?;
    stdio_request(handle, method, params).await
}

/// Send a JSON-RPC request on an existing stdio process and wait for the
/// matching response. Requires exclusive access to the handle.
async fn stdio_request(
    handle: &mut StdioProcessHandle,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let id = handle.next_id;
    handle.next_id += 1;

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": id,
    });
    stdio_write_json(&mut handle.stdin, &req).await?;
    stdio_read_response(&mut handle.stdout_reader, id).await
}

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

/// Canonical lowercase name for a transport (used in logs and errors).
fn transport_name(transport: &McpTransport) -> &'static str {
    match transport {
        McpTransport::Stdio => "stdio",
        McpTransport::Sse => "sse",
        McpTransport::Http => "http",
        McpTransport::StreamHttp => "stream-http",
    }
}

/// Sanitize env keys/values for logging — never log env VALUES, just the
/// key names and a count. Auth tokens are also redacted from URLs.
fn summarise_server_for_log(server: &McpServer) -> String {
    match server.transport {
        McpTransport::Stdio => format!(
            "transport=stdio command={:?} args={} env_keys={} enabled={}",
            server.command,
            server.args.len(),
            server.env.len(),
            server.enabled
        ),
        McpTransport::Sse | McpTransport::Http | McpTransport::StreamHttp => {
            let url = if server.auth_token.is_empty() {
                server.url.clone()
            } else {
                // crude redaction: keep scheme://host[:port]/path, drop query
                let base = server.url.split('?').next().unwrap_or(&server.url);
                format!("{base}?<token redacted>")
            };
            format!(
                "transport={} url={} auth_token={} enabled={}",
                transport_name(&server.transport),
                url,
                if server.auth_token.is_empty() {
                    "no"
                } else {
                    "yes"
                },
                server.enabled
            )
        }
    }
}

// ─── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_mcp_servers(state: State<'_, AppState>) -> Vec<McpServer> {
    load_servers(&state.mcp_servers_path)
}

#[tauri::command]
pub fn save_mcp_server(state: State<'_, AppState>, server: McpServer) -> Result<(), String> {
    let mut servers = load_servers(&state.mcp_servers_path);
    let action = if servers.iter().any(|s| s.id == server.id) {
        "updated"
    } else {
        "added"
    };
    if let Some(existing) = servers.iter_mut().find(|s| s.id == server.id) {
        *existing = server.clone();
    } else {
        servers.push(server.clone());
    }
    let save_result = save_servers(&state.mcp_servers_path, &servers);
    if save_result.is_ok() {
        append_mcp_log(
            &state,
            &server.id,
            McpLogLevel::Info,
            format!("Server {}: {}", action, summarise_server_for_log(&server)),
        );
    }
    save_result
}

#[tauri::command]
pub fn delete_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut servers = load_servers(&state.mcp_servers_path);
    servers.retain(|s| s.id != id);
    let save_result = save_servers(&state.mcp_servers_path, &servers);
    if save_result.is_ok() {
        append_mcp_log(&state, &id, McpLogLevel::Info, "Server deleted".to_string());
        // Drop the log buffer for the removed server.
        state.mcp_logs.lock().unwrap().remove(&id);
    }
    save_result
}

/// Start a stdio MCP process (called when user enables a server).
#[tauri::command]
pub async fn mcp_start_stdio(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<(), String> {
    let server = load_servers(&state.mcp_servers_path)
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or("Server not found")?;
    start_stdio_process(&state, &server).await
}

/// Stop a stdio MCP process (called when user disables a server).
#[tauri::command]
pub async fn mcp_stop_stdio(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<(), String> {
    stop_stdio_process(&state, &server_id).await;
    Ok(())
}

/// Stop all running stdio MCP processes (called on app shutdown).
pub async fn stop_all_stdio_processes(state: &AppState) {
    let ids: Vec<String> = state.mcp_children.lock().await.keys().cloned().collect();
    for id in ids {
        stop_stdio_process(state, &id).await;
    }
}

/// Return the most recent diagnostic log entries for a server.
#[tauri::command]
pub fn get_mcp_logs(state: State<'_, AppState>, id: String) -> Vec<McpLogEntry> {
    state
        .mcp_logs
        .lock()
        .unwrap()
        .get(&id)
        .map(|buf| buf.iter().cloned().collect())
        .unwrap_or_default()
}

/// Clear the diagnostic log for a server.
#[tauri::command]
pub fn clear_mcp_logs(state: State<'_, AppState>, id: String) {
    state.mcp_logs.lock().unwrap().remove(&id);
}



/// Test connectivity to an MCP server.
/// Returns Ok(message) on success or Err(message) on failure.
///
/// NOTE: This is a pure config validation — it does NOT spawn any process.
/// For stdio it only checks basic config sanity (command non-empty).
/// Command existence is validated in `start_stdio_process` before spawning.
/// For SSE/HTTP it pings the URL. No MCP handshake is performed.
#[tauri::command]
pub async fn test_mcp_server(
    state: State<'_, AppState>,
    server: McpServer,
) -> Result<String, String> {
    // Guard: only enabled servers can be tested
    if !server.enabled {
        return Err("请先启用该服务器".to_string());
    }

    append_mcp_log(
        &state,
        &server.id,
        McpLogLevel::Info,
        format!("Test started: {}", summarise_server_for_log(&server)),
    );

    let result = match server.transport {
        McpTransport::Stdio => test_stdio_config(&server),
        McpTransport::Sse | McpTransport::Http | McpTransport::StreamHttp => {
            test_http_mcp_server(&server).await
        }
    };

    match &result {
        Ok(msg) => append_mcp_log(
            &state,
            &server.id,
            McpLogLevel::Info,
            format!("Test passed: {msg}"),
        ),
        Err(err) => append_mcp_log(
            &state,
            &server.id,
            McpLogLevel::Error,
            format!("Test failed: {err}"),
        ),
    }
    result
}

/// Pure config validation for stdio servers — no process spawn.
/// Note: command existence is already validated by `start_stdio_process`
/// before spawning, so this only checks basic config sanity.
fn test_stdio_config(server: &McpServer) -> Result<String, String> {
    if server.command.trim().is_empty() {
        return Err("Command is empty".to_string());
    }

    Ok("Configuration valid".to_string())
}

/// Check if a command exists in PATH (cross-platform, no spawn).
fn command_exists(cmd: &str) -> bool {
    use std::path::Path;

    // If it's an absolute path, check directly
    let path = Path::new(cmd);
    if path.is_absolute() {
        return path.exists();
    }

    // Search PATH entries
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let candidate = Path::new(dir).join(cmd);
            if candidate.exists() {
                return true;
            }
            // On Windows also check with .exe extension
            if cfg!(windows) {
                let with_exe = Path::new(dir).join(format!("{cmd}.exe"));
                if with_exe.exists() {
                    return true;
                }
                let with_cmd = Path::new(dir).join(format!("{cmd}.cmd"));
                if with_cmd.exists() {
                    return true;
                }
                let with_bat = Path::new(dir).join(format!("{cmd}.bat"));
                if with_bat.exists() {
                    return true;
                }
            }
        }
    }
    false
}

/// Test HTTP-based MCP servers by sending a real `initialize` request.
/// This validates the full MCP handshake without spawning any process.
async fn test_http_mcp_server(server: &McpServer) -> Result<String, String> {
    use tokio::time::{timeout, Duration};

    let url = server.url.trim();
    if url.is_empty() {
        return Err("URL is empty".to_string());
    }

    match server.transport {
        McpTransport::StreamHttp => {
            // Streamable HTTP: POST initialize with session handling
            let mut session = StreamHttpSession::new(server)?;
            let name = timeout(Duration::from_secs(30), session.initialize())
                .await
                .map_err(|_| "Initialize timed out after 30s".to_string())??;
            Ok(format!("Connected successfully (server: {name})"))
        }
        McpTransport::Http => {
            // Plain HTTP JSON-RPC: POST initialize
            let resp = http_post_jsonrpc(server, &init_request_json()).await?;
            if let Some(err) = resp.get("error") {
                return Err(format!("MCP initialize error: {err}"));
            }
            let name = server_name_from_init(&resp);
            Ok(format!("Connected successfully (server: {name})"))
        }
        McpTransport::Sse => {
            // SSE: open stream, get endpoint, POST initialize
            let mut session = SseSession::connect(server).await?;
            let init_result = session.post_json(&init_request_json()).await.and_then(|_| {
                // Read response with timeout
                let read_fut = session.read_response(1);
                tokio::runtime::Handle::current().block_on(timeout(Duration::from_secs(30), read_fut))
                    .map_err(|_| "Initialize timed out after 30s".to_string())
                    .and_then(|r| r)
            })?;
            if let Some(err) = init_result.get("error") {
                return Err(format!("MCP initialize error: {err}"));
            }
            let name = server_name_from_init(&init_result);
            Ok(format!("Connected successfully (server: {name})"))
        }
        _ => Err("unsupported transport".to_string()),
    }
}

// ─── LLM integration helpers ──────────────────────────────────────────────────

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
/// On Unix, we wrap through a shell for the same reason.  We detect the
/// user's login shell (`$SHELL`) and run with `-l -c` so that shell init
/// files (`.profile`, `.bashrc`, `.zshrc`, etc.) are sourced, picking up
/// PATH modifications from nvm, Homebrew, rustup, pyenv, etc.  GUI apps
/// (or any non-terminal-launched process) don't inherit the full PATH of
/// an interactive shell, so wrapping through a login shell is the only
/// portable way to make commands like `npx` work.
///
/// `pipe_stderr` controls whether stderr is captured. Diagnostic paths
/// (test, stdio_init used by the LLM) pipe stderr so it can be drained
/// asynchronously and surfaced in the server's log buffer. Callers that
/// don't read stderr (i.e. would block on a noisy server) should pass
/// `false` and let stderr go to `/dev/null`.
fn build_stdio_cmd(server: &McpServer, pipe_stderr: bool) -> tokio::process::Command {

    use std::process::Stdio;

    



    let stderr_cfg = if pipe_stderr {
        Stdio::piped()
    } else {
        Stdio::null()
    };

    #[cfg(windows)]
    {
        use std::path::Path;
        // 提取文件名，例如 "python.exe" -> "python"
        let cmd_path = Path::new(server.command.trim());
        let file_stem = cmd_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(server.command.trim())
            .to_lowercase();
        let is_absolute =
            cmd_path.is_absolute() || server.command.contains('\\') || server.command.contains('/');

        // 判断是否是脚本命令（通常需要 Shell 才能运行）
        let is_batch_script = cmd_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("cmd") || s.eq_ignore_ascii_case("bat"))
            .unwrap_or(false);

        // 这些通常是 .cmd 封装（在 Windows 上），需要 Shell 帮忙解析 PATH 或执行
        let is_shell_dependent_tool = matches!(
            file_stem.as_str(),
            "npx" | "npm" | "pnpm" | "yarn" | "uvx" | "ux" | "conda"
        );

        let needs_shell_wrap = (!is_absolute && is_shell_dependent_tool) || is_batch_script;

        if needs_shell_wrap {
            // ==========================================
            // 进入 PowerShell 包装逻辑 (保留你原本的代码)
            // ==========================================
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
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
            return cmd;
        }
    }

    #[cfg(unix)]
    {
        let is_absolute = server.command.starts_with('/');
        if !is_absolute {
            let mut shell_cmd = server.command.clone();
            for arg in &server.args {
                shell_cmd.push(' ');
                shell_cmd.push_str(&shell_quote_unix(arg));
            }

            // Use the user's login shell (`$SHELL`) with `-l -c` so that
            // shell init files (`.profile`, `.bashrc`, `.zshrc`) are
            // sourced.  GUI apps don't inherit the full PATH from the
            // interactive shell, so wrapping through a login shell is the
            // only portable way to make commands like `npx` work.
            // `.zshrc` / `.bashrc` are normally NOT sourced by login
            // shells (only interactive ones), so we also source them
            // explicitly when the shell is zsh or bash.
            let shell = std::env::var("SHELL").unwrap_or_else(|_| {
                #[cfg(target_os = "macos")]
                {
                    "/bin/zsh".to_string()
                }
                #[cfg(not(target_os = "macos"))]
                {
                    "/bin/sh".to_string()
                }
            });
            let shell_name = std::path::Path::new(&shell)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("sh");
            let source_rc = match shell_name {
                "zsh" => "[ -r ~/.zshrc ] && source ~/.zshrc; ",
                "bash" => "[ -r ~/.bashrc ] && source ~/.bashrc; ",
                _ => "",
            };
            let full_cmd = format!("{}{}", source_rc, shell_cmd);
            let mut cmd = tokio::process::Command::new(&shell);
            cmd.args(["-l", "-c", &full_cmd])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(stderr_cfg);
            for (k, v) in &server.env {
                cmd.env(k, v);
            }
            return cmd;
        }
    }

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

/// Spawn a background task that drains `stderr` from a child process and
/// appends each line to the server's diagnostic log. Returns immediately.
/// Exits naturally when the child closes stderr (typically on process exit).
fn spawn_stderr_drain<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    state: std::sync::Arc<std::sync::Mutex<HashMap<String, VecDeque<McpLogEntry>>>>,
    id: String,
    name: String,
    stderr: R,
) {
    use tokio::io::BufReader;
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let mut map = state.lock().unwrap();
                    let buf = map
                        .entry(id.clone())
                        .or_insert_with(|| VecDeque::with_capacity(MAX_MCP_LOG_ENTRIES));
                    if buf.len() >= MAX_MCP_LOG_ENTRIES {
                        buf.pop_front();
                    }
                    buf.push_back(McpLogEntry {
                        ts: now_ms(),
                        level: McpLogLevel::Warn,
                        message: format!("[{name} stderr] {trimmed}"),
                    });
                }
            }
        }
    });
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
///
/// `on_stderr` is invoked once the child is spawned so the caller can move
/// `stderr` into a background drainer (logging each line to the per-server
/// diagnostic buffer). Pass `|_| {}` if you don't want stderr captured.
async fn stdio_init(
    server: &McpServer,
    on_stderr: impl FnOnce(tokio::process::ChildStderr) + Send,
) -> Result<
    (
        tokio::process::Child,
        tokio::process::ChildStdin,
        tokio::io::BufReader<tokio::process::ChildStdout>,
    ),
    String,
> {
    let mut cmd = build_stdio_cmd(server, true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start process: {e}"))?;
    if let Some(stderr) = child.stderr.take() {
        on_stderr(stderr);
    }
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

// ── SSE transport (HTTP + Server-Sent Events) ───────────────────────────────
//
// MCP's SSE transport is NOT a plain "POST JSON-RPC to the URL" protocol.
// Per the spec (and the Python SDK reference implementation):
//   1. The client opens a long-lived GET to the SSE endpoint with
//      `Accept: text/event-stream`.
//   2. The server immediately pushes an `endpoint` event whose data is the
//      URL the client must POST JSON-RPC messages to — usually a relative
//      path carrying a session id, e.g. `/messages/?session_id=abc123`.
//   3. The client POSTs each JSON-RPC request to that endpoint.
//   4. Responses (and server notifications) come back as `message` events
//      on the SSE stream opened in step 1.

/// Timeouts for the SSE handshake / request-response round trips (seconds).
const SSE_CONNECT_TIMEOUT_SECS: u64 = 30;
const SSE_RESPONSE_TIMEOUT_SECS: u64 = 60;

struct SseFrame {
    event: String,
    data: String,
}

/// An established SSE session with an MCP server.
struct SseSession {
    /// Absolute URL to POST JSON-RPC messages to (from the `endpoint` event).
    endpoint: String,
    auth_token: String,
    client: reqwest::Client,
    /// The long-lived GET response; its body is the event stream.
    response: reqwest::Response,
    /// Byte buffer for incremental SSE frame parsing.
    buf: Vec<u8>,
}

impl SseSession {
    /// Open the SSE stream and wait for the server's `endpoint` event.
    async fn connect(server: &McpServer) -> Result<Self, String> {
        use tokio::time::{timeout, Duration};

        let url = server.url.trim();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // No overall request timeout: the stream must stay open for the
            // whole session. Per-read timeouts are applied by the callers.
            .build()
            .map_err(|e| format!("HTTP client: {e}"))?;

        let mut req = client
            .get(url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache");
        if !server.auth_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", server.auth_token));
        }
        let response = req
            .send()
            .await
            .map_err(|e| format!("SSE connect failed: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("SSE connect failed: HTTP {status}"));
        }

        let mut session = Self {
            endpoint: String::new(),
            auth_token: server.auth_token.clone(),
            client,
            response,
            buf: Vec::new(),
        };
        let raw_endpoint = timeout(
            Duration::from_secs(SSE_CONNECT_TIMEOUT_SECS),
            session.wait_for_endpoint(),
        )
        .await
        .map_err(|_| "Timed out waiting for the SSE endpoint event".to_string())??;
        session.endpoint = resolve_endpoint(url, &raw_endpoint);
        Ok(session)
    }

    /// Read frames until the `endpoint` event arrives; return its data.
    async fn wait_for_endpoint(&mut self) -> Result<String, String> {
        loop {
            let Some(frame) = self.next_frame().await? else {
                return Err("SSE stream closed before the endpoint event".to_string());
            };
            if frame.event == "endpoint" {
                return Ok(frame.data);
            }
            // Other events before `endpoint` are unexpected — keep reading.
        }
    }

    /// Read one complete SSE frame (`event:` + `data:` lines) from the
    /// stream. Returns `None` on EOF. Comment/keep-alive frames are skipped.
    async fn next_frame(&mut self) -> Result<Option<SseFrame>, String> {
        loop {
            if let Some((frame_end, sep_len)) = find_sse_frame(&self.buf) {
                let raw: Vec<u8> = self.buf.drain(..frame_end + sep_len).collect();
                if let Some(frame) = parse_sse_frame(&raw[..frame_end]) {
                    return Ok(Some(frame));
                }
                continue; // empty frame (keep-alive comment) — read the next one
            }
            let Some(chunk) = self
                .response
                .chunk()
                .await
                .map_err(|e| format!("SSE stream read: {e}"))?
            else {
                return Ok(None); // EOF
            };
            self.buf.extend_from_slice(&chunk);
        }
    }

    /// POST a JSON-RPC message to the session endpoint. The HTTP response is
    /// just an acknowledgement (often 202 with an empty body) — the actual
    /// JSON-RPC response arrives on the SSE stream.
    async fn post_json(&self, body: &serde_json::Value) -> Result<(), String> {
        use tokio::time::{timeout, Duration};

        let mut req = self.client.post(&self.endpoint).json(body);
        if !self.auth_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.auth_token));
        }
        let resp = timeout(Duration::from_secs(30), req.send())
            .await
            .map_err(|_| format!("POST {} timed out", self.endpoint))?
            .map_err(|e| format!("POST {}: {e}", self.endpoint))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let preview: String = text.chars().take(200).collect();
            return Err(format!(
                "POST {} returned HTTP {status}: {preview}",
                self.endpoint
            ));
        }
        Ok(())
    }

    /// Read `message` events from the stream until the JSON-RPC response
    /// with `expected_id` arrives. Server notifications are skipped.
    async fn read_response(&mut self, expected_id: u64) -> Result<serde_json::Value, String> {
        use tokio::time::{timeout, Duration};

        timeout(
            Duration::from_secs(SSE_RESPONSE_TIMEOUT_SECS),
            read_jsonrpc_from_sse_stream(&mut self.response, &mut self.buf, expected_id),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out after {SSE_RESPONSE_TIMEOUT_SECS}s waiting for the JSON-RPC response"
            )
        })?
    }
}

/// Find the end of the first complete SSE frame in `buf`.
/// Returns `(frame_end, separator_len)`: the frame body is `buf[..frame_end]`
/// and the blank-line separator is `buf[frame_end..frame_end + separator_len]`.
fn find_sse_frame(buf: &[u8]) -> Option<(usize, usize)> {
    let n = buf.len();
    for i in 0..n {
        match buf[i] {
            b'\n' if i + 1 < n && buf[i + 1] == b'\n' => return Some((i, 2)),
            b'\r'
                if i + 3 < n
                    && buf[i + 1] == b'\n'
                    && buf[i + 2] == b'\r'
                    && buf[i + 3] == b'\n' =>
            {
                return Some((i, 4))
            }
            b'\r' if i + 1 < n && buf[i + 1] == b'\r' => return Some((i, 2)),
            _ => {}
        }
    }
    None
}

/// Parse an SSE frame body into its `event` name and joined `data` payload.
fn parse_sse_frame(raw: &[u8]) -> Option<SseFrame> {
    let text = String::from_utf8_lossy(raw);
    let mut event = String::new();
    let mut data_lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue; // blank line or comment/keep-alive
        }
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            data_lines.push(v.strip_prefix(' ').unwrap_or(v).to_string());
        }
        // Other fields (id:, retry:) are irrelevant here.
    }
    if event.is_empty() && data_lines.is_empty() {
        return None;
    }
    Some(SseFrame {
        event,
        data: data_lines.join("\n"),
    })
}

/// Resolve the `endpoint` event data against the base SSE URL. Servers
/// usually send a relative path like `/messages/?session_id=abc`.
fn resolve_endpoint(base_url: &str, endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_string();
    }
    let Some(idx) = base_url.find("://") else {
        return endpoint.to_string();
    };
    let after = &base_url[idx + 3..];
    let host_end = after.find('/').unwrap_or(after.len());
    let origin = &base_url[..idx + 3 + host_end];
    if endpoint.starts_with('/') {
        format!("{origin}{endpoint}")
    } else {
        format!("{origin}/{endpoint}")
    }
}

// ── HTTP transports (plain JSON-RPC + Streamable HTTP) ──────────────────────

/// Overall timeout for one HTTP JSON-RPC round trip (seconds).
const HTTP_RESPONSE_TIMEOUT_SECS: u64 = 60;

/// Canonical `initialize` request used by every transport handshake.
fn init_request_json() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "ai-chat", "version": "0.1.0" }
        },
        "id": 1
    })
}

/// Extract the server name from an `initialize` response.
fn server_name_from_init(resp: &serde_json::Value) -> String {
    resp["result"]["serverInfo"]["name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string()
}

/// Read JSON-RPC messages from an SSE byte stream until the one with
/// `expected_id` arrives. Notifications and non-JSON frames are skipped.
/// Shared by the SSE session and the streamable-HTTP transport.
async fn read_jsonrpc_from_sse_stream(
    response: &mut reqwest::Response,
    buf: &mut Vec<u8>,
    expected_id: u64,
) -> Result<serde_json::Value, String> {
    loop {
        if let Some((frame_end, sep_len)) = find_sse_frame(buf) {
            let raw: Vec<u8> = buf.drain(..frame_end + sep_len).collect();
            if let Some(frame) = parse_sse_frame(&raw[..frame_end]) {
                if !frame.event.is_empty() && frame.event != "message" {
                    continue; // ignore other event types
                }
                if frame.data.trim().is_empty() {
                    continue;
                }
                let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&frame.data) else {
                    continue; // not JSON — skip
                };
                // Skip notifications (they have "method" but no id).
                if parsed.get("method").is_some() && parsed.get("id").is_none() {
                    continue;
                }
                if parsed["id"].as_u64() == Some(expected_id) {
                    return Ok(parsed);
                }
            }
            continue;
        }
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("SSE stream read: {e}"))?
        else {
            return Err("SSE stream closed before the response arrived".to_string());
        };
        buf.extend_from_slice(&chunk);
    }
}

/// Extract a JSON-RPC response from an HTTP response whose body is either
/// plain JSON (`application/json`) or an SSE stream (`text/event-stream`).
/// Used by the streamable-HTTP transport.
async fn read_jsonrpc_response_from_http(
    resp: reqwest::Response,
    expected_id: u64,
) -> Result<serde_json::Value, String> {
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if content_type.contains("text/event-stream") {
        let mut buf: Vec<u8> = Vec::new();
        let mut resp = resp;
        read_jsonrpc_from_sse_stream(&mut resp, &mut buf, expected_id).await
    } else {
        resp.json()
            .await
            .map_err(|e| format!("Response parse: {e}"))
    }
}

/// Plain HTTP JSON-RPC transport: POST the JSON-RPC message straight to the
/// URL and parse the JSON response. No handshake, no session state.
async fn http_post_jsonrpc(
    server: &McpServer,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use tokio::time::{timeout, Duration};

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;
    let mut req = client
        .post(server.url.trim())
        .header("Accept", "application/json")
        .json(body);
    if !server.auth_token.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", server.auth_token));
    }
    let resp = timeout(Duration::from_secs(HTTP_RESPONSE_TIMEOUT_SECS), req.send())
        .await
        .map_err(|_| "HTTP request timed out".to_string())?
        .map_err(|e| format!("HTTP error: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let preview: String = text.chars().take(200).collect();
        return Err(format!("Server returned HTTP {status}: {preview}"));
    }
    resp.json()
        .await
        .map_err(|e| format!("Response parse: {e}"))
}

/// MCP "Streamable HTTP" transport (protocol version 2025-03-26).
///
/// Every JSON-RPC message is POSTed to the single endpoint with
/// `Accept: application/json, text/event-stream`. The server answers either
/// with a JSON body or by streaming the response as SSE. The `initialize`
/// response carries a `Mcp-Session-Id` header that must be echoed back on
/// all subsequent requests.
struct StreamHttpSession {
    url: String,
    auth_token: String,
    session_id: Option<String>,
    client: reqwest::Client,
}

impl StreamHttpSession {
    fn new(server: &McpServer) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("HTTP client: {e}"))?;
        Ok(Self {
            url: server.url.trim().to_string(),
            auth_token: server.auth_token.clone(),
            session_id: None,
            client,
        })
    }

    fn post_builder(&self, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Protocol-Version", "2025-03-26")
            .json(body);
        if !self.auth_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.auth_token));
        }
        if let Some(sid) = &self.session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        req
    }

    /// POST a JSON-RPC request and return its response (JSON body or SSE
    /// stream). Captures the `Mcp-Session-Id` header if the server sends one.
    async fn request(
        &mut self,
        body: &serde_json::Value,
        expected_id: u64,
    ) -> Result<serde_json::Value, String> {
        use tokio::time::{timeout, Duration};

        let resp = timeout(
            Duration::from_secs(HTTP_RESPONSE_TIMEOUT_SECS),
            self.post_builder(body).send(),
        )
        .await
        .map_err(|_| format!("POST {} timed out", self.url))?
        .map_err(|e| format!("POST {}: {e}", self.url))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let preview: String = text.chars().take(200).collect();
            return Err(format!(
                "POST {} returned HTTP {status}: {preview}",
                self.url
            ));
        }
        if self.session_id.is_none() {
            if let Some(sid) = resp
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                self.session_id = Some(sid.to_string());
            }
        }
        timeout(
            Duration::from_secs(HTTP_RESPONSE_TIMEOUT_SECS),
            read_jsonrpc_response_from_http(resp, expected_id),
        )
        .await
        .map_err(|_| "Timed out waiting for the JSON-RPC response".to_string())?
    }

    /// POST a notification (no response body expected; 202 Accepted).
    async fn notify(&self, body: &serde_json::Value) -> Result<(), String> {
        use tokio::time::{timeout, Duration};

        let resp = timeout(
            Duration::from_secs(HTTP_RESPONSE_TIMEOUT_SECS),
            self.post_builder(body).send(),
        )
        .await
        .map_err(|_| format!("POST {} timed out", self.url))?
        .map_err(|e| format!("POST {}: {e}", self.url))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("POST {} returned HTTP {status}", self.url));
        }
        Ok(())
    }

    /// Run `initialize` + `notifications/initialized`; returns the server name.
    async fn initialize(&mut self) -> Result<String, String> {
        let resp = self.request(&init_request_json(), 1).await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP initialize error: {err}"));
        }
        let name = server_name_from_init(&resp);
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        self.notify(&notif).await?;
        Ok(name)
    }
}

// ── Public functions used by llm_complete ───────────────────────────────────

/// Get the list of tools from an MCP server, in OpenAI /chat/completions tool format.
pub async fn get_server_tools(
    state: &AppState,
    server: &McpServer,
) -> Result<Vec<serde_json::Value>, String> {
    append_mcp_log(
        state,
        &server.id,
        McpLogLevel::Info,
        "LLM requested tools/list".to_string(),
    );
    let result = match server.transport {
        McpTransport::Stdio => get_tools_stdio(state, server).await,
        McpTransport::Sse => get_tools_sse(server).await,
        McpTransport::Http => get_tools_http(server).await,
        McpTransport::StreamHttp => get_tools_stream_http(server).await,
    };
    if let Err(ref err) = result {
        append_mcp_log(
            state,
            &server.id,
            McpLogLevel::Error,
            format!("tools/list failed: {err}"),
        );
    } else {
        let count = result.as_ref().map(|t| t.len()).unwrap_or(0);
        append_mcp_log(
            state,
            &server.id,
            McpLogLevel::Info,
            format!("tools/list returned {count} tools"),
        );
    }
    result
}

async fn get_tools_stdio(
    state: &AppState,
    server: &McpServer,
) -> Result<Vec<serde_json::Value>, String> {
    // Ensure the persistent process is running
    ensure_stdio_process(state, server).await;

    let resp = stdio_request_persistent(state, &server.id, "tools/list", serde_json::json!({})).await?;

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
    // Open the SSE stream, get the message endpoint from the `endpoint`
    // event, POST tools/list there and read the response from the stream.
    let mut session = SseSession::connect(server).await?;

    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "params": {},
        "id": 1
    });
    session.post_json(&req_body).await?;
    let resp = session.read_response(1).await?;

    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tools/list error: {err}"));
    }
    let tools = resp["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(convert_mcp_tools(tools))
}

async fn get_tools_http(server: &McpServer) -> Result<Vec<serde_json::Value>, String> {
    // Plain HTTP JSON-RPC: POST tools/list directly, read the JSON response.
    let resp = http_post_jsonrpc(
        server,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "params": {},
            "id": 1
        }),
    )
    .await?;
    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tools/list error: {err}"));
    }
    let tools = resp["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(convert_mcp_tools(tools))
}

async fn get_tools_stream_http(server: &McpServer) -> Result<Vec<serde_json::Value>, String> {
    // Streamable HTTP: initialize handshake first, then tools/list with the
    // session id. The response may be a JSON body or an SSE stream.
    let mut session = StreamHttpSession::new(server)?;
    session.initialize().await?;

    let resp = session
        .request(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "tools/list",
                "params": {},
                "id": 2
            }),
            2,
        )
        .await?;
    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tools/list error: {err}"));
    }
    let tools = resp["result"]["tools"]
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
    state: &AppState,
    server: &McpServer,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    append_mcp_log(
        state,
        &server.id,
        McpLogLevel::Info,
        format!("LLM tools/call: {tool_name}"),
    );
    let result = match server.transport {
        McpTransport::Stdio => invoke_tool_stdio(state, server, tool_name, arguments).await,
        McpTransport::Sse => invoke_tool_sse(server, tool_name, arguments).await,
        McpTransport::Http => invoke_tool_http(server, tool_name, arguments).await,
        McpTransport::StreamHttp => invoke_tool_stream_http(server, tool_name, arguments).await,
    };
    if let Err(ref err) = result {
        append_mcp_log(
            state,
            &server.id,
            McpLogLevel::Error,
            format!("tools/call failed: {err}"),
        );
    }
    result
}

async fn invoke_tool_stdio(
    state: &AppState,
    server: &McpServer,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    // Ensure the persistent process is running
    ensure_stdio_process(state, server).await;

    let resp = stdio_request_persistent(
        state,
        &server.id,
        "tools/call",
        serde_json::json!({
            "name": tool_name,
            "arguments": arguments
        }),
    )
    .await?;

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
    // Open the SSE stream, get the message endpoint from the `endpoint`
    // event, POST tools/call there and read the response from the stream.
    let mut session = SseSession::connect(server).await?;

    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": { "name": tool_name, "arguments": arguments },
        "id": 1
    });
    session.post_json(&req_body).await?;
    let resp = session.read_response(1).await?;

    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tool error: {err}"));
    }
    Ok(extract_tool_result(&resp["result"]))
}

async fn invoke_tool_http(
    server: &McpServer,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    // Plain HTTP JSON-RPC: POST tools/call directly, read the JSON response.
    let resp = http_post_jsonrpc(
        server,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": tool_name, "arguments": arguments },
            "id": 1
        }),
    )
    .await?;
    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tool error: {err}"));
    }
    Ok(extract_tool_result(&resp["result"]))
}

async fn invoke_tool_stream_http(
    server: &McpServer,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    // Streamable HTTP: initialize handshake first, then tools/call with the
    // session id. The response may be a JSON body or an SSE stream.
    let mut session = StreamHttpSession::new(server)?;
    session.initialize().await?;

    let resp = session
        .request(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "tools/call",
                "params": { "name": tool_name, "arguments": arguments },
                "id": 2
            }),
            2,
        )
        .await?;
    if let Some(err) = resp.get("error") {
        return Err(format!("MCP tool error: {err}"));
    }
    Ok(extract_tool_result(&resp["result"]))
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


