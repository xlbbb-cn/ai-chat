// Memory tool: session (in-memory), user/repo (file-backed) and the read-only
// `all` fan-out across every scope.

use crate::db;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

use super::search::{run_integrated_search, SearchOptions};
use super::truncate_tool_output;

/// Handle the `memory` tool call.
///
/// `scope: "all"` re-runs this same tool once per concrete scope and merges
/// the results, so `all` behaves exactly like three separate calls. Mutating
/// actions deliberately require one concrete scope.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_memory_tool(
    app: &AppHandle,
    args: &Value,
    session_id: Option<&str>,
    workspace_dir: &Path,
    config: &crate::AppConfig,
    allowed_commands: &[String],
    protected_skill_roots: &[PathBuf],
    mission_id: Option<&str>,
    active_skill_dirs: &[(String, PathBuf)],
) -> String {
    let action = args["action"].as_str().unwrap_or("");
    let scope = args["scope"].as_str().unwrap_or("session");
    let key = args["key"].as_str().unwrap_or("").to_string();
    let content = args["content"].as_str().unwrap_or("").to_string();
    let query = args["query"].as_str().unwrap_or("").to_string();
    let tags: Vec<String> = args["tags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // ── All scopes (read-only fan-out) ──────────────────────────────
    // `scope: "all"` re-runs this same tool once per concrete scope and
    // merges the results, so `all` behaves exactly like three separate
    // calls. Mutating actions deliberately require one concrete scope.
    if scope == "all" {
        if !matches!(action, "get" | "list" | "search") {
            if matches!(action, "add" | "delete") {
                return format!(
                    "Error: memory scope 'all' is read-only (get, list, search). Call the memory tool again with scope 'session', 'user', or 'repo' to {}.",
                    if action == "add" {
                        "add a memory"
                    } else {
                        "delete a memory"
                    }
                );
            }
            return format!(
                "Unknown memory action '{}'. Supported: add, get, list, search, delete.",
                action
            );
        }
        if action == "search" && query.is_empty() {
            return "Error: 'query' is required for memory search.".to_string();
        }
        if action == "get" && key.is_empty() {
            return "Error: 'key' is required for memory get.".to_string();
        }

        let _ = app.emit(
            "tool-call",
            format!("🧠 *Memory {} across all scopes*\n", action),
        );

        let mut sections: Vec<String> = Vec::new();
        for sub_scope in ["session", "user", "repo"] {
            let mut sub_args = args.clone();
            sub_args["scope"] = json!(sub_scope);
            let sub_args_str = sub_args.to_string();
            let result = Box::pin(super::execute_tool(
                app,
                "memory",
                &sub_args_str,
                workspace_dir.to_path_buf(),
                config,
                allowed_commands,
                protected_skill_roots,
                mission_id,
                session_id,
                active_skill_dirs,
            ))
            .await;
            sections.push(format!("### {}\n{}", sub_scope, result));
        }

        let merged = format!(
            "Memory {} across all scopes:\n\n{}",
            action,
            sections.join("\n\n")
        );
        return truncate_tool_output(merged);
    }

    // ── Session-scoped (in-memory) ──────────────────────────────────
    if scope == "session" {
        let session_id = session_id.unwrap_or("default");
        let state = app.state::<crate::AppState>();
        let mut memories = state.session_memories.lock().unwrap();
        let session_mem = memories.entry(session_id.to_string()).or_default();

        match action {
            "add" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory add.".to_string();
                }
                let entry = serde_json::json!({
                    "key": key,
                    "content": content,
                    "tags": tags,
                    "scope": "session",
                });
                session_mem.insert(key.clone(), entry.clone());
                let _ = app.emit(
                    "tool-call",
                    format!("🧠 *Session memory stored: {}*\n", key),
                );
                serde_json::to_string_pretty(&entry).unwrap_or_else(|_| entry.to_string())
            }
            "get" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory get.".to_string();
                }
                match session_mem.get(&key) {
                    Some(entry) => {
                        let _ = app.emit(
                            "tool-call",
                            format!("🧠 *Session memory retrieved: {}*\n", key),
                        );
                        serde_json::to_string_pretty(entry).unwrap_or_else(|_| entry.to_string())
                    }
                    None => format!("No session memory found for key '{}'.", key),
                }
            }
            "list" => {
                if session_mem.is_empty() {
                    return "(no session memories)".to_string();
                }
                let keys: Vec<&String> = session_mem.keys().collect();
                let _ = app.emit(
                    "tool-call",
                    format!("🧠 *Listing {} session memories*\n", keys.len()),
                );
                serde_json::to_string_pretty(&json!({
                    "scope": "session",
                    "keys": keys,
                    "count": keys.len(),
                }))
                .unwrap_or_else(|_| "[]".to_string())
            }
            "search" => {
                if query.is_empty() {
                    return "Error: 'query' is required for memory search.".to_string();
                }

                // ── In-memory search ──
                let query_lower = query.to_lowercase();
                let in_memory_matches: Vec<&serde_json::Value> = session_mem
                    .values()
                    .filter(|entry| {
                        let key_match = entry["key"]
                            .as_str()
                            .map(|k| k.to_lowercase().contains(&query_lower))
                            .unwrap_or(false);
                        let content_match = entry["content"]
                            .as_str()
                            .map(|c| c.to_lowercase().contains(&query_lower))
                            .unwrap_or(false);
                        key_match || content_match
                    })
                    .collect();

                // ── DB history search across past sessions ──
                let db_guard = state.db.lock().unwrap();
                let history_matches =
                    db::search_history_messages(&db_guard, session_id, &query, 20);
                let summary_matches =
                    db::search_session_summaries(&db_guard, session_id, &query, 5);
                drop(db_guard);

                let _ = app.emit(
                    "tool-call",
                    format!("🧠 *Searching session memories for '{}'*\n", query),
                );

                let has_in_memory = !in_memory_matches.is_empty();
                let has_history = history_matches
                    .as_ref()
                    .map(|h| !h.is_empty())
                    .unwrap_or(false);
                let has_summaries = summary_matches
                    .as_ref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);

                if !has_in_memory && !has_history && !has_summaries {
                    format!(
                        "No session memories or past conversations match '{}'.",
                        query
                    )
                } else {
                    let mut result = json!({
                        "scope": "session",
                        "query": query,
                        "in_memory_matches": in_memory_matches,
                        "in_memory_count": in_memory_matches.len(),
                    });
                    if let Ok(hist) = history_matches {
                        result["session_history_matches"] =
                            serde_json::to_value(&hist).unwrap_or_default();
                        result["session_history_count"] = json!(hist.len());
                    }
                    if let Ok(sum) = summary_matches {
                        result["session_summaries"] =
                            serde_json::to_value(&sum).unwrap_or_default();
                        result["session_summaries_count"] = json!(sum.len());
                    }
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
                }
            }
            "delete" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory delete.".to_string();
                }
                match session_mem.remove(&key) {
                    Some(_) => {
                        let _ = app.emit(
                            "tool-call",
                            format!("🧠 *Session memory deleted: {}*\n", key),
                        );
                        format!("Session memory '{}' deleted.", key)
                    }
                    None => format!("No session memory found for key '{}'.", key),
                }
            }
            _ => format!(
                "Unknown memory action '{}'. Supported: add, get, list, search, delete.",
                action
            ),
        }
    // ── User / Repo scoped (file-backed) ────────────────────────────
    } else if scope == "user" || scope == "repo" {
        let memory_root = workspace_dir.join("memory").join(scope);
        let _ = fs::create_dir_all(&memory_root);

        match action {
            "add" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory add.".to_string();
                }
                // Sanitize key for use as filename
                let safe_key = key
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                let file_path = memory_root.join(format!("{}.md", safe_key));

                let tags_line = if tags.is_empty() {
                    String::new()
                } else {
                    format!("\ntags: {}\n", tags.join(", "))
                };

                let file_content = format!("# {key}\n\n{content}{tags_line}\n");

                match fs::write(&file_path, &file_content) {
                    Ok(_) => {
                        let _ = app.emit(
                            "tool-call",
                            format!("🧠 *{} memory stored: {}*\n", scope, key),
                        );
                        let entry = json!({
                            "key": key,
                            "content": content,
                            "tags": tags,
                            "scope": scope,
                            "file": file_path.to_string_lossy(),
                        });
                        serde_json::to_string_pretty(&entry).unwrap_or_else(|_| entry.to_string())
                    }
                    Err(e) => format!("Error writing memory file: {}", e),
                }
            }
            "get" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory get.".to_string();
                }
                let safe_key = key
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                let file_path = memory_root.join(format!("{}.md", safe_key));

                match fs::read_to_string(&file_path) {
                    Ok(content) => {
                        let _ = app.emit(
                            "tool-call",
                            format!("🧠 *{} memory retrieved: {}*\n", scope, key),
                        );
                        content
                    }
                    Err(_) => format!("No {} memory found for key '{}'.", scope, key),
                }
            }
            "list" => match fs::read_dir(&memory_root) {
                Ok(entries) => {
                    let mut items: Vec<serde_json::Value> = Vec::new();
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".md") {
                            let display_key = name.trim_end_matches(".md").to_string();
                            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                            items.push(json!({
                                "key": display_key,
                                "file": name,
                                "size_bytes": size,
                            }));
                        }
                    }
                    let _ = app.emit(
                        "tool-call",
                        format!("🧠 *Listing {} {} memories*\n", items.len(), scope),
                    );
                    if items.is_empty() {
                        format!("(no {} memories)", scope)
                    } else {
                        serde_json::to_string_pretty(&json!({
                            "scope": scope,
                            "entries": items,
                            "count": items.len(),
                        }))
                        .unwrap_or_else(|_| "[]".to_string())
                    }
                }
                Err(e) => format!("Error listing memory directory: {}", e),
            },
            "search" => {
                if query.is_empty() {
                    return "Error: 'query' is required for memory search.".to_string();
                }
                let _ = app.emit(
                    "tool-call",
                    format!("🧠 *Searching {} memories for '{}'*\n", scope, query),
                );
                match run_integrated_search(
                    &query,
                    &memory_root,
                    workspace_dir,
                    &SearchOptions {
                        include_hidden: false,
                        glob: Some("**/*.md".to_string()),
                        ..SearchOptions::default()
                    },
                ) {
                    Ok(output) => {
                        if output.is_empty() || output == "(no matches)" {
                            format!("No {} memories match '{}'.", scope, query)
                        } else {
                            output
                        }
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "delete" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory delete.".to_string();
                }
                let safe_key = key
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                let file_path = memory_root.join(format!("{}.md", safe_key));

                match fs::remove_file(&file_path) {
                    Ok(_) => {
                        let _ = app.emit(
                            "tool-call",
                            format!("🧠 *{} memory deleted: {}*\n", scope, key),
                        );
                        format!("{} memory '{}' deleted.", scope, key)
                    }
                    Err(e) => format!("Error deleting memory '{}': {}", key, e),
                }
            }
            _ => format!(
                "Unknown memory action '{}'. Supported: add, get, list, search, delete.",
                action
            ),
        }
    } else {
        format!(
            "Unknown memory scope '{}'. Supported scopes: session, user, repo, all.",
            scope
        )
    }
}
