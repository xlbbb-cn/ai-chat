use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::State;

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
struct McpServersFile {
    servers: Vec<McpServer>,
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn load_servers(path: &PathBuf) -> Vec<McpServer> {
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

async fn test_stdio_server(server: &McpServer) -> Result<String, String> {
    use std::process::Stdio;
    use tokio::process::Command;
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

    let mut cmd = Command::new(&server.command);
    cmd.args(&server.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    for (k, v) in &server.env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| format!("Failed to start process: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("No stdin")?;
    let mut stdout = child.stdout.take().ok_or("No stdout")?;

    stdin
        .write_all(request_line.as_bytes())
        .await
        .map_err(|e| format!("Write error: {e}"))?;
    drop(stdin);

    let read_result = timeout(Duration::from_secs(5), async {
        use tokio::io::AsyncBufReadExt;
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
                Ok(format!("Connected successfully (received {} bytes)", line.len()))
            } else {
                Err(format!("Unexpected response: {}", line.chars().take(200).collect::<String>()))
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

    let resp = req.send().await.map_err(|e| format!("Request failed: {e}"))?;
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
