//! `skill_read`: read/list/search files inside an active skill directory.

use super::*;

pub(super) async fn handle(ctx: &ToolContext<'_>, args: &Value) -> String {
    let app = ctx.app;
    let mission_id = ctx.mission_id;
    let session_id = ctx.session_id;
    let call_id = ctx.call_id;
    let active_skill_dirs = ctx.active_skill_dirs;
    let action = args["action"].as_str().unwrap_or("");
    let skill_name = args["skill_name"].as_str().unwrap_or("");
    let rel_path = args["path"].as_str().unwrap_or(".");

    if skill_name.is_empty() {
        return "Error: skill_name is required.".to_string();
    }

    // Find the skill directory
    let skill_dir = match active_skill_dirs
        .iter()
        .find(|(name, _)| name == skill_name)
    {
        Some((_, dir)) => dir.clone(),
        None => {
            return format!(
                "Error: skill '{}' is not active or not found. Active skills: {:?}",
                skill_name,
                active_skill_dirs.iter().map(|(n, _)| n).collect::<Vec<_>>()
            );
        }
    };

    // Resolve the target path within the skill directory
    let target_path = if rel_path == "." || rel_path.is_empty() {
        skill_dir.clone()
    } else {
        let resolved = skill_dir.join(rel_path);
        // Security check: ensure the resolved path is still within the skill directory
        match (resolved.canonicalize(), skill_dir.canonicalize()) {
            (Ok(resolved_canonical), Ok(skill_dir_canonical)) => {
                if !resolved_canonical.starts_with(&skill_dir_canonical) {
                    return format!(
                        "Error: path '{}' escapes skill directory '{}'",
                        rel_path, skill_name
                    );
                }
                resolved_canonical
            }
            _ => {
                // Path doesn't exist yet or can't be canonicalized, use as-is
                if !resolved.starts_with(&skill_dir) {
                    return format!(
                        "Error: path '{}' escapes skill directory '{}'",
                        rel_path, skill_name
                    );
                }
                resolved
            }
        }
    };

    match action {
        "read" => {
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!("*Reading skill file {}/{}*\n\n", skill_name, rel_path),
            );

            if !target_path.exists() {
                return format!(
                    "Error: file '{}' does not exist in skill '{}'",
                    rel_path, skill_name
                );
            }

            if target_path.is_dir() {
                return format!(
                    "Error: '{}' is a directory, use action 'list' instead",
                    rel_path
                );
            }

            let start_line = args["start_line"].as_i64();
            let end_line = args["end_line"].as_i64();

            if start_line.is_none() && end_line.is_none() {
                return fs::read_to_string(&target_path)
                    .map(truncate_tool_output)
                    .unwrap_or_else(|e| format!("Error reading file: {}", e));
            }

            let start = start_line.unwrap_or(1);
            let end = end_line.unwrap_or(i64::MAX);
            if start <= 0 || end < start {
                return "Error: invalid line range. start_line must be >= 1 and end_line must be >= start_line.".to_string();
            }

            match fs::File::open(&target_path) {
                Ok(file) => {
                    let reader = BufReader::new(file);
                    let mut result = String::new();
                    for (index, line) in reader.lines().enumerate() {
                        let line_num = (index + 1) as i64;
                        if line_num < start {
                            continue;
                        }
                        if line_num > end {
                            break;
                        }
                        match line {
                            Ok(text) => {
                                if !result.is_empty() {
                                    result.push('\n');
                                }
                                result.push_str(&text);
                            }
                            Err(e) => {
                                return format!("Error reading file: {}", e);
                            }
                        }
                    }
                    if result.is_empty() {
                        "(no matching lines)".to_string()
                    } else {
                        result
                    }
                }
                Err(e) => format!("Error opening file: {}", e),
            }
        }
        "list" => {
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!(
                    "*Listing skill directory {}/{}*\n\n",
                    skill_name, rel_path
                ),
            );

            if !target_path.exists() {
                return format!(
                    "Error: directory '{}' does not exist in skill '{}'",
                    rel_path, skill_name
                );
            }

            if !target_path.is_dir() {
                // It's a file, return file info
                match fs::metadata(&target_path) {
                    Ok(metadata) => {
                        let name = target_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(rel_path)
                            .to_string();
                        return with_root_header(format!("{} ({} bytes)", name, metadata.len()));
                    }
                    Err(e) => return format!("Error reading file metadata: {}", e),
                }
            }

            match fs::read_dir(&target_path) {
                Ok(entries) => {
                    let mut res = Vec::new();
                    for entry in entries.flatten() {
                        if let Ok(name) = entry.file_name().into_string() {
                            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                            if is_dir {
                                res.push(format!("{}/", name));
                            } else {
                                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                res.push(format!("{} ({} bytes)", name, size));
                            }
                        }
                    }
                    let body = if res.is_empty() {
                        "(empty directory)".to_string()
                    } else {
                        res.join("\n")
                    };
                    with_root_header(body)
                }
                Err(e) => format!("Error listing directory: {}", e),
            }
        }
        "search" => {
            let query = args["query"].as_str().unwrap_or("").to_string();
            let options = SearchOptions {
                recursive: args["recursive"].as_bool().unwrap_or(true),
                case_sensitive: args["case_sensitive"].as_bool().unwrap_or(false),
                smart_case: args["smart_case"].as_bool().unwrap_or(false),
                use_regex: args["use_regex"].as_bool().unwrap_or(false),
                include_hidden: args["include_hidden"].as_bool().unwrap_or(true),
                respect_gitignore: args["respect_gitignore"].as_bool().unwrap_or(true),
                glob: args["glob"].as_str().map(str::to_string),
                max_matches: args["max_results"]
                    .as_u64()
                    .map(|value| value.clamp(1, 20_000) as usize)
                    .unwrap_or(SEARCH_MAX_MATCHES),
                abort: None,
            };

            if query.is_empty() {
                return "Error: query is required for search.".to_string();
            }

            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!(
                    "*Searching skill {} for {}*\n\n",
                    skill_name,
                    serde_json::to_string(&query).unwrap_or_else(|_| query.to_string())
                ),
            );

            if !target_path.exists() {
                return format!(
                    "Error: path '{}' does not exist in skill '{}'",
                    rel_path, skill_name
                );
            }

            let search_root = skill_dir.clone();
            let outcome = tauri::async_runtime::spawn_blocking(move || {
                run_integrated_search(&query, &target_path, &search_root, &options)
            })
            .await
            .unwrap_or_else(|e| Err(format!("Search task failed: {}", e)));

            match outcome {
                Ok(output) => with_root_header(output),
                Err(e) => format!("Error: {}", e),
            }
        }
        _ => format!(
            "Unknown action '{}' for skill_read tool. Supported actions: read, list, search",
            action
        ),
    }
}
