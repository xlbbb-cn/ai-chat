use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

pub fn get_all_tools(selected_tools: &[String], allow_commands: bool, skill_dir: Option<&Path>) -> Vec<Value> {
    let mut tools = vec![];

    if selected_tools.iter().any(|t| t == "web_search") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web for current information using the configured search engine. Use when the user asks about recent events, current data, or anything requiring up-to-date information.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "The search query" }
                    },
                    "required": ["query"]
                }
            }
        }));
    }

    if allow_commands {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "execute_command",
                "description": "Execute a bash, python, or powershell command/script on the user's machine. Runs in the directory containing the skill's SKILL.md by default. Use for calculations, file operations, data processing, system queries, or any task that benefits from running locally.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["bash", "python", "powershell"],
                            "description": "The runtime to use"
                        },
                        "code": {
                            "type": "string",
                            "description": "The command or code to execute"
                        }
                    },
                    "required": ["type", "code"]
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "fetch_web") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "fetch_web",
                "description": "Fetch the content of a given URL. This resolves JS rendering and bypasses anti-bot/scraping strategies to read the true webpage content in markdown.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The URL to fetch, e.g. https://example.com" }
                    },
                    "required": ["url"]
                }
            }
        }));
    }

    if skill_dir.is_some() || selected_tools.iter().any(|t| t == "read_file" || t == "write_file" || t == "list_dir") {
        if selected_tools.iter().any(|t| t == "read_file") || skill_dir.is_some() {
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read the contents of a file inside the skill directory (or the active process directory if no skill).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file" }
                        },
                        "required": ["path"]
                    }
                }
            }));
        }
        if selected_tools.iter().any(|t| t == "write_file") || skill_dir.is_some() {
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Write contents to a file inside the skill directory.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file" },
                            "content": { "type": "string", "description": "Content to write" }
                        },
                        "required": ["path", "content"]
                    }
                }
            }));
        }
        if selected_tools.iter().any(|t| t == "list_dir") || skill_dir.is_some() {
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": "list_dir",
                    "description": "List contents of a directory inside the skill directory.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the directory (use '.' for root)" }
                        },
                        "required": ["path"]
                    }
                }
            }));
        }
    }

    tools
}

fn resolve_safe_path(skill_dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel_path);
    let mut resolved = skill_dir.to_path_buf();
    
    for comp in rel_path.components() {
        match comp {
            std::path::Component::ParentDir => {
                if !resolved.pop() || !resolved.starts_with(skill_dir) {
                    return Err("Path escapes skill directory".to_string());
                }
            }
            std::path::Component::Normal(c) => {
                resolved.push(c);
            }
            _ => {}
        }
    }
    
    if !resolved.starts_with(skill_dir) {
        return Err("Path escapes skill directory".to_string());
    }
    Ok(resolved)
}

/// Resolve command working directory:
/// - if launched from a skill, use the skill directory
/// - otherwise prefer app data dir, fallback to current workspace dir
fn resolve_cwd(app: &AppHandle, skill_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = skill_dir {
        dir
    } else {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

pub async fn execute_tool(
    app: &AppHandle,
    name: &str,
    args_str: &str,
    skill_dir: Option<PathBuf>,
    configured_search_engine: &str,
) -> String {
    let args: Value = serde_json::from_str(args_str).unwrap_or_default();
    match name {
        "web_search" => {
            let query = args["query"].as_str().unwrap_or("").to_string();
            let engine = configured_search_engine.to_string();
            let _ = app.emit(
                "chat-token",
                format!("🔍 *Searching with {}: {}...*\n\n", engine, query),
            );
            crate::search::search_by(engine, query).await
                .unwrap_or_else(|e| format!("Search failed: {}", e))
        }
        "execute_command" => {
            let cmd_type = args["type"].as_str().unwrap_or("bash").to_string();
            let code = args["code"].as_str().unwrap_or("").to_string();
            let _ = app.emit("chat-token", format!("⚙️ *Running {}:*\n```{}\n{}\n```\n\n", cmd_type, cmd_type, code));
            let cwd = resolve_cwd(app, skill_dir.clone());
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                run_command(cmd_type, code, Some(cwd)),
            )
            .await
            .unwrap_or_else(|_| Ok("Command timed out after 30 seconds.".to_string()))
            .unwrap_or_else(|e| format!("Error: {}", e))
        }
        "fetch_web" => {
            let url = args["url"].as_str().unwrap_or("").to_string();
            let _ = app.emit("chat-token", format!("🌐 *Fetching {}*\n\n", url));
            fetch_web_content(&url).await.unwrap_or_else(|e| format!("Failed to fetch web content: {}", e))
        }
        "read_file" => {
            if let Some(dir) = skill_dir {
                let path_str = args["path"].as_str().unwrap_or("");
                let _ = app.emit("chat-token", format!("📄 *Reading {}*\n\n", path_str));
                match resolve_safe_path(&dir, path_str) {
                    Ok(p) => fs::read_to_string(&p).unwrap_or_else(|e| format!("Error reading file: {}", e)),
                    Err(e) => format!("Error: {}", e)
                }
            } else {
                "Error: No skill directory context".to_string()
            }
        }
        "write_file" => {
            if let Some(dir) = skill_dir {
                let path_str = args["path"].as_str().unwrap_or("");
                let content_str = args["content"].as_str().unwrap_or("");
                let _ = app.emit("chat-token", format!("💾 *Writing {}*\n\n", path_str));
                match resolve_safe_path(&dir, path_str) {
                    Ok(p) => {
                        if let Some(parent) = p.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        match fs::write(&p, content_str) {
                            Ok(_) => format!("Successfully wrote to {}", path_str),
                            Err(e) => format!("Error writing file: {}", e)
                        }
                    }
                    Err(e) => format!("Error: {}", e)
                }
            } else {
                "Error: No skill directory context".to_string()
            }
        }
        "list_dir" => {
            if let Some(dir) = skill_dir {
                let path_str = args["path"].as_str().unwrap_or("");
                let _ = app.emit("chat-token", format!("📂 *Listing {}*\n\n", path_str));
                match resolve_safe_path(&dir, path_str) {
                    Ok(p) => {
                        match fs::read_dir(&p) {
                            Ok(entries) => {
                                let mut res = Vec::new();
                                for entry in entries.flatten() {
                                    if let Ok(name) = entry.file_name().into_string() {
                                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                                        res.push(format!("{}{}", name, if is_dir { "/" } else { "" }));
                                    }
                                }
                                if res.is_empty() {
                                    "(empty directory)".to_string()
                                } else {
                                    res.join("\n")
                                }
                            }
                            Err(e) => format!("Error listing directory: {}", e)
                        }
                    }
                    Err(e) => format!("Error: {}", e)
                }
            } else {
                "Error: No skill directory context".to_string()
            }
        }
        _ => format!("Unknown tool: {}", name),
    }
}

pub async fn run_command(cmd_type: String, code: String, cwd: Option<PathBuf>) -> Result<String, String> {
    let mut cmd = match cmd_type.as_str() {
        "python" | "python3" => {
            let mut c = tokio::process::Command::new("python3");
            c.arg("-c").arg(&code);
            c
        }
        "bash" | "sh" => {
            let mut c = tokio::process::Command::new("bash");
            c.arg("-c").arg(&code);
            c
        }
        "powershell" | "pwsh" => {
            let mut c = tokio::process::Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-Command", &code]);
            c
        }
        _ => return Err(format!("Unsupported command type: {}", cmd_type)),
    };

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let output = cmd.output().await.map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("STDERR:\n");
        result.push_str(&stderr);
    }
    if !output.status.success() {
        result.push_str(&format!("\nExit code: {}", output.status.code().unwrap_or(-1)));
    }
    if result.is_empty() {
        result = "(no output)".to_string();
    }
    Ok(result)
}

pub async fn fetch_web_content(url: &str) -> Result<String, String> {
    // Attempt local direct extraction first (falling back to Jina if needed or just attempting both)
    // Wait, let's implement local content extraction and if blocked or invalid, maybe fallback.
    // Or we can just use scraper to parse locally.
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    let jina_url = format!("https://r.jina.ai/{}", url);
    let res = client.get(&jina_url)
        .header("X-Return-Format", "markdown")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if res.status().is_success() {
        res.text().await.map_err(|e| e.to_string())
    } else {
        Err(format!("Error: received status code {}", res.status()))
    }
}