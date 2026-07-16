use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
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

// ─── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_mcp_servers(state: State<'_, AppState>) -> Vec<McpServer> {
    load_servers(&state.mcp_servers_path)
}

#[tauri::command]
pub fn save_mcp_server(state: State<'_, AppState>, server: McpServer) -> Result<(), String> {
    let mut servers = load_servers(&state.mcp_servers_path);
    if let Some(existing) = servers.iter_mut().find(|s| s.id == server.id) {
        *existing = server;
    } else {
        servers.push(server);
    }
    save_servers(&state.mcp_servers_path, &servers)
}

#[tauri::command]
pub fn delete_mcp_server(state: State<'_, AppState>, id: String) -> Result<(), String> {
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
/// Stderr is sent to `/dev/null` to avoid backpressure on a noisy server —
/// the JSON-RPC conversation happens over stdin/stdout only.
fn build_stdio_cmd(server: &McpServer) -> tokio::process::Command {
    use std::process::Stdio;

    let normalized = server.command.trim().to_lowercase();
    let is_known_stdio_runtime = matches!(
        normalized.as_str(),
        "uvx" | "ux" | "python" | "python3" | "py" | "node"
    );

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
            .stderr(Stdio::null());
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
                .stderr(Stdio::null());
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
        .stderr(Stdio::null());
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
    let mut cmd = build_stdio_cmd(server);
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

    let mut cmd = build_stdio_cmd(server);
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
