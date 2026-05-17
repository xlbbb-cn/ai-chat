use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

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
                "description": "Execute a bash, python, or powershell command/script on the user's machine. If called by a skill, runs in the directory containing the skill's SKILL.md; otherwise runs in the app managed workspace directory. Use for calculations, file operations, data processing, system queries, or any task that benefits from running locally.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["bash", "cmd", "powershell", "python"],
                            "description": "The runtime to use. ON WINDOWS, ALWAYS prefer 'powershell' or 'cmd'"
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

    if skill_dir.is_some() || selected_tools.iter().any(|t| t == "file_actions") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "file_actions",
                "description": "Perform file operations (read, write, list) inside the current workspace root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["read", "write", "list", "edit", "patch"],
                            "description": "The file action to perform: read file content, write/overwrite a file, list directory entries, edit by replacing a string, or apply a unified diff patch."
                        },
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file or directory. Use '.' for root when listing."
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write (only for 'write')."
                        },
                        "old_string": {
                            "type": "string",
                            "description": "Exact string to find and replace (only for 'edit')."
                        },
                        "new_string": {
                            "type": "string",
                            "description": "Replacement string (only for 'edit')."
                        },
                        "patch": {
                            "type": "string",
                            "description": "Unified diff patch to apply to the file (only for 'patch'). Must be a valid unified diff (--- / +++ header optional)."
                        }
                    },
                    "required": ["action", "path"]
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "knowledge_graph") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "knowledge_graph",
                "description": "Connect to a knowledge graph and perform queries.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "The cypher query or search query for the knowledge graph." }
                    },
                    "required": ["query"]
                }
            }
        }));
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

/// Resolve workspace root:
/// - if launched from a skill, use the skill directory
/// - otherwise use managed workspace_dir
fn resolve_workspace_root(workspace_dir: &Path, skill_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = skill_dir {
        dir
    } else {
        workspace_dir.to_path_buf()
    }
}

pub async fn execute_tool(
    app: &AppHandle,
    name: &str,
    args_str: &str,
    skill_dir: Option<PathBuf>,
    workspace_dir: PathBuf,
    config: &crate::AppConfig,
) -> String {
    let args: Value = serde_json::from_str(args_str).unwrap_or_default();
    match name {
        "web_search" => {
            let query = args["query"].as_str().unwrap_or("").to_string();
            let engine = config.search_engine.clone();
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
            let cwd = resolve_workspace_root(&workspace_dir, skill_dir.clone());
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
        "file_actions" => {
            let action = args["action"].as_str().unwrap_or("");
            let path_str = args["path"].as_str().unwrap_or("");
            let root_dir = resolve_workspace_root(&workspace_dir, skill_dir.clone());
            match action {
                "read" => {
                    let _ = app.emit("chat-token", format!("📄 *Reading {}*\n\n", path_str));
                    match resolve_safe_path(&root_dir, path_str) {
                        Ok(p) => fs::read_to_string(&p).unwrap_or_else(|e| format!("Error reading file: {}", e)),
                        Err(e) => format!("Error: {}", e)
                    }
                }
                "write" => {
                    let content_str = args["content"].as_str().unwrap_or("");
                    let _ = app.emit("chat-token", format!("💾 *Writing {}*\n\n", path_str));
                    match resolve_safe_path(&root_dir, path_str) {
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
                }
                "list" => {
                    let _ = app.emit("chat-token", format!("📂 *Listing {}*\n\n", path_str));
                    match resolve_safe_path(&root_dir, path_str) {
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
                }
                "edit" => {
                    let old_string = args["old_string"].as_str().unwrap_or("");
                    let new_string = args["new_string"].as_str().unwrap_or("");
                    let _ = app.emit("chat-token", format!("✏️ *Editing {}*\n\n", path_str));
                    match resolve_safe_path(&root_dir, path_str) {
                        Ok(p) => match fs::read_to_string(&p) {
                            Ok(original) => {
                                if !original.contains(old_string) {
                                    format!("Error: old_string not found in {}", path_str)
                                } else {
                                    let updated = original.replacen(old_string, new_string, 1);
                                    match fs::write(&p, &updated) {
                                        Ok(_) => format!("Successfully edited {}", path_str),
                                        Err(e) => format!("Error writing file: {}", e),
                                    }
                                }
                            }
                            Err(e) => format!("Error reading file: {}", e),
                        },
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "patch" => {
                    let patch_str = args["patch"].as_str().unwrap_or("");
                    let _ = app.emit("chat-token", format!("🩹 *Patching {}*\n\n", path_str));
                    match resolve_safe_path(&root_dir, path_str) {
                        Ok(p) => match fs::read_to_string(&p) {
                            Ok(original) => match diffy::Patch::from_str(patch_str) {
                                Ok(patch) => match diffy::apply(&original, &patch) {
                                    Ok(patched) => match fs::write(&p, &patched) {
                                        Ok(_) => format!("Successfully patched {}", path_str),
                                        Err(e) => format!("Error writing file: {}", e),
                                    },
                                    Err(e) => format!("Error applying patch: {}", e),
                                },
                                Err(e) => format!("Error parsing patch: {}", e),
                            },
                            Err(e) => format!("Error reading file: {}", e),
                        },
                        Err(e) => format!("Error: {}", e),
                    }
                }
                _ => format!("Unknown action '{}' for file_actions tool.", action),
            }
        }
        "knowledge_graph" => {
            let query = args["query"].as_str().unwrap_or("").to_string();
            let engine = config.kg_engine.as_deref().unwrap_or("neo4j").to_string();
            let _ = app.emit(
                "chat-token",
                format!("🧠 *Querying Knowledge Graph ({}) with: {}...*\n\n", engine, query),
            );
            
            if engine == "neo4j" {
                use crate::neo4j_db::{KnowledgeGraph, Neo4jRepo};
                let uri = config.neo4j_uri.as_deref().unwrap_or("bolt://localhost:7687");
                let user = config.neo4j_user.as_deref().unwrap_or("neo4j");
                let pass = config.neo4j_password.as_deref().unwrap_or("");
                match Neo4jRepo::new(uri, user, pass).await {
                    Ok(repo) => {
                        match repo.execute_query(&query).await {
                            Ok(res) => format!("Knowledge graph neo4j query executed: {}\nResult: {}", query, res),
                            Err(e) => format!("Error executing neo4j query: {}", e)
                        }
                    }
                    Err(e) => format!("Failed to connect to neo4j: {}", e)
                }
            } else {
                format!("Knowledge graph {} query executed: {}. (Not fully implemented yet)", engine, query)
            }
        }
        _ => format!("Unknown tool: {}", name),
    }
}

pub async fn run_command(cmd_type: String, code: String, cwd: Option<PathBuf>) -> Result<String, String> {
    let mut cmd = match cmd_type.as_str() {
        "python" | "python3" => {
            let mut c = tokio::process::Command::new(if cfg!(windows) { "python" } else { "python3" });
            c.arg("-c").arg(&code);
            c
        }
        "cmd" => {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", &code]);
            c
        }
        "bash" | "sh" => {
            let mut c = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "bash" });
            c.arg(if cfg!(windows) { "/C" } else { "-c" }).arg(&code);
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

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
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