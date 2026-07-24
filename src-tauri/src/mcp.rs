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
        McpTransport::Sse => {
            let url = if server.auth_token.is_empty() {
                server.url.clone()
            } else {
                // crude redaction: keep scheme://host[:port]/path, drop query
                let base = server.url.split('?').next().unwrap_or(&server.url);
                format!("{base}?<token redacted>")
            };
            format!(
                "transport=sse url={} auth_token={} enabled={}",
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

/// Cancel a running MCP server test.
#[tauri::command]
pub fn cancel_mcp_test(state: State<'_, AppState>, id: String) {
    state.mcp_cancelled_tests.lock().unwrap().insert(id);
}

/// Check whether a test has been cancelled for the given server id.
fn is_test_cancelled(state: &AppState, id: &str) -> bool {
    state.mcp_cancelled_tests.lock().unwrap().contains(id)
}

/// Remove a test from the cancelled set (called when the test finishes).
fn clear_test_cancelled(state: &AppState, id: &str) {
    state.mcp_cancelled_tests.lock().unwrap().remove(id);
}

/// Test connectivity to an MCP server.
/// Returns Ok(message) on success or Err(message) on failure.
#[tauri::command]
pub async fn test_mcp_server(
    state: State<'_, AppState>,
    server: McpServer,
) -> Result<String, String> {
    // Clear any stale cancellation for this server
    clear_test_cancelled(&state, &server.id);

    append_mcp_log(
        &state,
        &server.id,
        McpLogLevel::Info,
        format!("Test started: {}", summarise_server_for_log(&server)),
    );

    // Log the test timeout
    let test_timeout_s = 120u64;
    append_mcp_log(
        &state,
        &server.id,
        McpLogLevel::Info,
        format!(
            "Test timeout set to {}s (first run may install packages)",
            test_timeout_s
        ),
    );

    let result = {
        let test_fut = async {
            match server.transport {
                McpTransport::Stdio => test_stdio_server(&state, &server).await,
                McpTransport::Sse => test_sse_server(&state, &server).await,
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(test_timeout_s), test_fut)
            .await
            .unwrap_or_else(|_| {
                append_mcp_log(
                    &state,
                    &server.id,
                    McpLogLevel::Error,
                    format!(
                        "Test timed out after {}s (package installation may still be in progress)",
                        test_timeout_s
                    ),
                );
                Err(format!("Test timed out after {}s", test_timeout_s))
            })
    };

    // If the test was cancelled, return a cancelled message
    if is_test_cancelled(&state, &server.id) {
        clear_test_cancelled(&state, &server.id);
        append_mcp_log(
            &state,
            &server.id,
            McpLogLevel::Warn,
            "Test cancelled by user".to_string(),
        );
        return Err("Test cancelled by user".to_string());
    }

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
/// On Unix, we wrap through a shell for the same reason.  On macOS we use
/// the user's login shell (`zsh -l`) so that `.zprofile` / `.zshrc` are
/// sourced — this is essential for commands like `npx` installed via nvm
/// or other version managers that modify PATH in shell init files. Other
/// Unix flavours use `sh -c` as before.
///
/// `pipe_stderr` controls whether stderr is captured. Diagnostic paths
/// (test, stdio_init used by the LLM) pipe stderr so it can be drained
/// asynchronously and surfaced in the server's log buffer. Callers that
/// don't read stderr (i.e. would block on a noisy server) should pass
/// `false` and let stderr go to `/dev/null`.
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

            // On macOS, use the user's login shell (defaulting to zsh) so
            // that `.zprofile` / `.zshrc` is sourced, giving access to
            // PATH modifications from nvm, brew, rustup, etc.
            // On other Unix, fall back to plain `sh -c`.
            #[cfg(target_os = "macos")]
            let mut cmd = {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
                let mut c = tokio::process::Command::new(&shell);
                c.args(["-l", "-c", &shell_cmd]);
                c
            };

            #[cfg(not(target_os = "macos"))]
            let mut cmd = {
                let mut c = tokio::process::Command::new("sh");
                c.args(["-c", &shell_cmd]);
                c
            };

            cmd.stdin(Stdio::piped())
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
    let id = server.id.clone();
    let name = server.name.clone();
    let logs = state.mcp_logs.clone();
    let (mut child, mut stdin, mut reader) = stdio_init(server, |stderr| {
        spawn_stderr_drain(logs, id, name, stderr);
    })
    .await?;

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
    let id = server.id.clone();
    let name = server.name.clone();
    let logs = state.mcp_logs.clone();
    let (mut child, mut stdin, mut reader) = stdio_init(server, |stderr| {
        spawn_stderr_drain(logs, id, name, stderr);
    })
    .await?;

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

async fn test_stdio_server(state: &AppState, server: &McpServer) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    use tokio::time::{timeout, Duration};

    let id = server.id.clone();
    let name = server.name.clone();
    let log = |level: McpLogLevel, msg: String| append_mcp_log(state, &id, level, msg);

    if server.command.trim().is_empty() {
        let msg = "Command is empty".to_string();
        log(McpLogLevel::Error, msg.clone());
        return Err(msg);
    }

    // Early cancellation check
    if is_test_cancelled(state, &id) {
        return Err("Test cancelled by user".to_string());
    }

    let started = std::time::Instant::now();
    log(
        McpLogLevel::Info,
        format!(
            "⏳ Phase: spawning — {} {}",
            server.command,
            server.args.join(" ")
        ),
    );
    log(
        McpLogLevel::Info,
        format!(
            "Env vars: {} (keys only, values redacted)",
            server.env.len()
        ),
    );

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

    if is_test_cancelled(state, &id) {
        return Err("Test cancelled by user".to_string());
    }

    let mut cmd = build_stdio_cmd(server, true);
    let mut child = match cmd.spawn() {
        Ok(c) => {
            log(
                McpLogLevel::Info,
                format!("Process spawned (pid={:?})", c.id()),
            );
            c
        }
        Err(e) => {
            let msg = format!("Failed to start process: {e}");
            log(McpLogLevel::Error, msg.clone());
            return Err(msg);
        }
    };

    // Drain stderr into the log buffer so the user can see why a server
    // crashed at startup. The drain task ends when stderr closes (EOF).
    if let Some(stderr) = child.stderr.take() {
        let logs_arc = state.mcp_logs.clone();
        spawn_stderr_drain(logs_arc, id.clone(), name.clone(), stderr);
    }

    let mut stdin = child.stdin.take().ok_or("No stdin")?;
    let mut stdout = child.stdout.take().ok_or("No stdout")?;

    if is_test_cancelled(state, &id) {
        let _ = child.kill().await;
        return Err("Test cancelled by user".to_string());
    }

    log(
        McpLogLevel::Info,
        "⏳ Phase: initializing — sending MCP initialize request".to_string(),
    );

    if let Err(e) = stdin.write_all(request_line.as_bytes()).await {
        let msg = format!("Write error: {e}");
        log(McpLogLevel::Error, msg.clone());
        let _ = child.kill().await;
        return Err(msg);
    }
    drop(stdin);
    log(
        McpLogLevel::Info,
        "Sent initialize request (id=1)".to_string(),
    );

    log(
        McpLogLevel::Info,
        "⏳ Phase: waiting for response — may take a while if installing packages...".to_string(),
    );

    let read_result = timeout(Duration::from_secs(120), async {
        let mut reader = tokio::io::BufReader::new(&mut stdout);
        let mut line = String::new();
        reader.read_line(&mut line).await.map(|_| line)
    })
    .await;

    let _ = child.kill().await;

    // Check cancellation again after the long wait
    if is_test_cancelled(state, &id) {
        return Err("Test cancelled by user".to_string());
    }

    let result = match read_result {
        Ok(Ok(line)) if !line.trim().is_empty() => {
            log(
                McpLogLevel::Info,
                format!("Received {} bytes from server", line.len()),
            );
            if line.contains("\"result\"") || line.contains("jsonrpc") {
                Ok(format!(
                    "✅ Connected successfully (received {} bytes)",
                    line.len()
                ))
            } else {
                let preview: String = line.chars().take(200).collect();
                log(
                    McpLogLevel::Warn,
                    format!("Response did not look like JSON-RPC: {preview}"),
                );
                Err(format!("Unexpected response: {preview}"))
            }
        }
        Ok(Ok(_)) => {
            let msg = "Server closed connection without response".to_string();
            log(McpLogLevel::Error, msg.clone());
            Err(msg)
        }
        Ok(Err(e)) => {
            let msg = format!("Read error: {e}");
            log(McpLogLevel::Error, msg.clone());
            Err(msg)
        }
        Err(_) => {
            let msg = "⏱ Timeout: no response within 120 seconds (package installation may still be in progress)".to_string();
            log(McpLogLevel::Error, msg.clone());
            Err(msg)
        }
    };
    let elapsed_ms = started.elapsed().as_millis();
    log(
        McpLogLevel::Info,
        format!("Test finished in {} ms", elapsed_ms),
    );
    result
}

async fn test_sse_server(state: &AppState, server: &McpServer) -> Result<String, String> {
    let id = server.id.clone();
    let log = |level: McpLogLevel, msg: String| append_mcp_log(state, &id, level, msg);

    if server.url.trim().is_empty() {
        let msg = "URL is empty".to_string();
        log(McpLogLevel::Error, msg.clone());
        return Err(msg);
    }

    let started = std::time::Instant::now();
    let url_display = if server.auth_token.is_empty() {
        server.url.clone()
    } else {
        format!(
            "{}?<token redacted>",
            server.url.split('?').next().unwrap_or(&server.url)
        )
    };
    log(McpLogLevel::Info, format!("GET {url_display}"));

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

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Request failed: {e}");
            log(McpLogLevel::Error, msg.clone());
            return Err(msg);
        }
    };
    let status = resp.status();
    log(McpLogLevel::Info, format!("HTTP {status}"));

    let result = if status.is_success() {
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok(format!("Connected: HTTP {status} ({content_type})"))
    } else {
        Err(format!("Server returned HTTP {status}"))
    };
    let elapsed_ms = started.elapsed().as_millis();
    log(
        McpLogLevel::Info,
        format!("Test finished in {} ms", elapsed_ms),
    );
    result
}
