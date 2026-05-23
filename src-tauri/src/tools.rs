use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

// ─── Dangerous command detection ─────────────────────────────────────────────

/// Patterns that indicate a script may have destructive/system-altering effects.
static DANGEROUS_SCRIPT_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "del /f",
    "del /s",
    "rd /s",
    "rmdir /s",
    "format ",
    "diskpart",
    "fdisk",
    "reg add",
    "reg delete",
    "reg import",
    "netsh ",
    "net user",
    "net localgroup",
    "sc delete",
    "sc stop",
    "sc config",
    "schtasks /create",
    "schtasks /delete",
    "bcdedit",
    "bootcfg",
    "takeown",
    "icacls",
    "apt install",
    "apt remove",
    "apt purge",
    "yum install",
    "yum remove",
    "dnf remove",
    "winget install",
    "winget uninstall",
    "choco install",
    "choco uninstall",
    "pip install",
    "pip uninstall",
    "pip3 install",
    "npm install -g",
    "npm uninstall -g",
    "set-executionpolicy",
    "invoke-expression",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
];

/// Executables considered dangerous when used in direct mode.
static DANGEROUS_EXECUTABLES: &[&str] = &[
    "rm", "del", "format", "fdisk", "diskpart", "netsh", "sc", "reg", "regedit", "bcdedit",
    "schtasks", "net", "takeown", "icacls", "dd", "mkfs", "shutdown", "reboot", "halt", "poweroff",
    "attrib",
];

/// Returns a human-readable reason if the command is considered dangerous, or None.
fn is_dangerous(cmd_type: &str, code: &str) -> Option<String> {
    let lower = code.to_lowercase();
    if cmd_type == "direct" {
        let parts = split_command_line(code);
        if let Some(first) = parts.first() {
            let exe_name = Path::new(first.as_str())
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(first.as_str())
                .to_lowercase();
            if DANGEROUS_EXECUTABLES.contains(&exe_name.as_str()) {
                return Some(format!(
                    "executable '{}' is potentially destructive",
                    exe_name
                ));
            }
        }
    }
    for pattern in DANGEROUS_SCRIPT_PATTERNS {
        if lower.contains(pattern) {
            return Some(format!("dangerous pattern detected: '{}'", pattern));
        }
    }
    None
}

/// Simple command-line splitter that handles single and double quotes.
fn split_command_line(code: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in code.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

fn is_sudo_executable(token: &str) -> bool {
    Path::new(token)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(token)
        .eq_ignore_ascii_case("sudo")
}

fn code_requests_sudo(code: &str) -> bool {
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed == "sudo" || trimmed.starts_with("sudo ") || trimmed.starts_with("sudo\t") {
            return true;
        }
    }
    false
}

fn format_process_output(output: std::process::Output) -> String {
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
        result.push_str(&format!(
            "\nExit code: {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    if result.is_empty() {
        result = "(no output)".to_string();
    }
    result
}

async fn run_command_with_stdin(
    mut cmd: tokio::process::Command,
    stdin_data: String,
) -> Result<String, String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_data.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }

    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    Ok(format_process_output(output))
}

async fn request_tool_confirmation(
    app: &AppHandle,
    reason: String,
    cmd_type: String,
    code: String,
    confirm_kind: &'static str,
    requires_auth: &'static str,
) -> crate::ToolConfirmation {
    let _ = app.emit(
        "confirm-required",
        serde_json::json!({
            "reason": reason,
            "cmd_type": cmd_type,
            "code": code,
            "confirm_kind": confirm_kind,
            "requires_auth": requires_auth,
        }),
    );

    let (tx, rx) = tokio::sync::oneshot::channel::<crate::ToolConfirmation>();
    {
        let state = app.state::<crate::AppState>();
        *state.confirm_sender.lock().unwrap() = Some(tx);
    }

    tokio::time::timeout(std::time::Duration::from_secs(120), rx)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(crate::ToolConfirmation {
            confirmed: false,
            username: None,
            password: None,
        })
}

pub fn get_all_tools(selected_tools: &[String]) -> Vec<Value> {
    let mut tools = vec![];

    let want_run_cmd = selected_tools.iter().any(|t| t == "run_cmd");
    let want_run_shell = selected_tools.iter().any(|t| t == "run_shell");

    if want_run_cmd {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "run_cmd",
                "description": "Run an executable program directly (without a shell) on the user's machine. The first token is the executable; the rest are arguments. Preferred for simple commands like curl, wget, git, etc. If called by a skill, runs in the skill's directory; otherwise in the managed workspace directory. Dangerous or privileged operations (sudo) will require explicit user confirmation before execution.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The full command line (e.g. 'curl -s https://example.com'). The first word is the executable; the rest are arguments. Handles basic single- and double-quote grouping."
                        }
                    },
                    "required": ["command"]
                }
            }
        }));
    }

    if want_run_shell {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "run_shell",
                "description": "Execute a script in a shell on the user's machine. 
                                Supports pipes, loops, variables, and other shell features. 
                                If called by a skill, runs in the skill's directory; 
                                otherwise in the managed workspace directory. 
                                Dangerous or privileged operations (sudo / admin elevation) will require explicit user confirmation before execution.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["powershell",  "bash"],
                            "description": "Shell to use. On Windows prefer 'powershell'; on Linux/macOS use 'bash'."
                        },
                        "code": {
                            "type": "string",
                            "description": "The shell script body to execute."
                        },
                        "sudo": {
                            "type": "boolean",
                            "description": "If true (bash only), run the entire script via sudo. Requires explicit user confirmation and sudo credentials."
                        },
                        "elevated": {
                            "type": "boolean",
                            "description": "If true (PowerShell only), request administrator elevation (UAC). Requires explicit user confirmation."
                        }
                    },
                    "required": ["type", "code"]
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "file_actions") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "file_actions",
                "description": "Perform file operations (read, write, list, mkdir, rename/move, patch, delete) inside the current workspace root. Existing paths can also resolve relative to active skill roots, self-evolution skill roots, and protected self-evolution files when enabled. When protected skill/sub-agent files are modified through this tool, a sibling `.bak.<number>` backup is created automatically first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["read", "write", "list", "mkdir", "patch", "rename", "move", "delete"],
                            "description": "The file action to perform: read file content, write/overwrite a file, list directory entries, recursively create directories, apply a unified diff patch, rename a file/directory, move a file/directory, or delete a file/directory."
                        },
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file or directory. Use '.' for root when listing."
                        },
                        "new_path": {
                            "type": "string",
                            "description": "Destination relative path for rename operations. Required when action is 'rename'."
                        },
                        "start_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Optional 1-based starting line number to read from (inclusive). Only used for 'read'."
                        },
                        "end_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Optional 1-based ending line number to read to (inclusive). Only used for 'read'."
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write (only for 'write')."
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

fn resolve_safe_path(root_dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
    // If an absolute path is given and it starts with root_dir, strip the prefix
    // so the AI can pass either relative or absolute paths within the workspace.
    let stripped;
    let rel_path_str = if Path::new(rel_path).is_absolute() {
        let canonical_root = root_dir
            .canonicalize()
            .unwrap_or_else(|_| root_dir.to_path_buf());
        let abs = Path::new(rel_path);
        let abs_canonical = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
        if let Ok(suffix) = abs_canonical.strip_prefix(&canonical_root) {
            stripped = suffix.to_string_lossy().into_owned();
            &stripped as &str
        } else {
            return Err(format!(
                "Absolute path '{}' is outside the workspace root '{}'",
                rel_path,
                root_dir.display()
            ));
        }
    } else {
        rel_path
    };

    let rel_path = Path::new(rel_path_str);

    let mut resolved = root_dir.to_path_buf();
    for comp in rel_path.components() {
        match comp {
            std::path::Component::ParentDir => {
                resolved.pop();
                if !resolved.starts_with(root_dir) {
                    return Err("Path escapes workspace directory".to_string());
                }
            }
            std::path::Component::Normal(c) => {
                resolved.push(c);
            }
            _ => {}
        }
    }

    // Final guard: canonicalize the resolved path and verify it is still inside root_dir
    match resolved.canonicalize() {
        Ok(canonical) => {
            let canonical_root = root_dir
                .canonicalize()
                .unwrap_or_else(|_| root_dir.to_path_buf());
            if !canonical.starts_with(&canonical_root) {
                return Err("Path escapes workspace directory".to_string());
            }
            Ok(canonical)
        }
        // File does not exist yet (e.g. write to a new file) — fall back to the unresolved path
        // after confirming it still starts with root_dir lexically
        Err(_) => {
            if !resolved.starts_with(root_dir) {
                return Err("Path escapes workspace directory".to_string());
            }
            Ok(resolved)
        }
    }
}

fn build_candidate_roots(primary_root: &Path, extra_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = vec![primary_root.to_path_buf()];
    for root in extra_roots {
        if !roots.iter().any(|r| r == root) {
            roots.push(root.clone());
        }
    }
    roots
}

fn is_backup_variant(base_file: &Path, candidate: &Path) -> bool {
    let Some(base_name) = base_file.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(candidate_name) = candidate.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    candidate_name
        .strip_prefix(&format!("{base_name}.bak."))
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
        && candidate.parent() == base_file.parent()
}

fn resolve_safe_explicit_file(
    input_path: &str,
    allowed_files: &[PathBuf],
) -> Option<(PathBuf, PathBuf)> {
    let input = Path::new(input_path);
    let is_bare_name = input
        .parent()
        .map(|parent| parent.as_os_str().is_empty())
        .unwrap_or(true);
    let input_abs = input
        .is_absolute()
        .then(|| input.canonicalize().unwrap_or_else(|_| input.to_path_buf()));

    for file in allowed_files {
        let file_abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        let parent = file_abs
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let matched = if let Some(input_abs) = &input_abs {
            input_abs == &file_abs || is_backup_variant(&file_abs, input_abs)
        } else if !is_bare_name {
            false
        } else {
            input
                .file_name()
                .map(|name| parent.join(name))
                .is_some_and(|candidate| {
                    candidate == file_abs || is_backup_variant(&file_abs, &candidate)
                })
        };

        if matched {
            let resolved = if let Some(input_abs) = &input_abs {
                input_abs.clone()
            } else {
                parent.join(input.file_name()?)
            };
            return Some((resolved, parent));
        }
    }

    None
}

fn resolve_safe_path_with_roots(
    primary_root: &Path,
    extra_roots: &[PathBuf],
    input_path: &str,
    require_exists: bool,
) -> Result<(PathBuf, PathBuf), String> {
    let roots = build_candidate_roots(primary_root, extra_roots);

    // 1. Try to find the exact existing file in ANY root
    for root in &roots {
        if let Ok(resolved) = resolve_safe_path(root, input_path) {
            if resolved.exists() {
                return Ok((resolved, root.clone()));
            }
        }
    }

    if require_exists {
        return Err(format!(
            "Path '{}' was not found under workspace root '{}' or any active skill root",
            input_path,
            primary_root.display()
        ));
    }

    // 2. File doesn't exist, and require_exists is false.
    let is_absolute = Path::new(input_path).is_absolute();
    if is_absolute {
        for root in &roots {
            if let Ok(resolved) = resolve_safe_path(root, input_path) {
                return Ok((resolved, root.clone()));
            }
        }
        return Err(format!(
            "Absolute path '{}' does not belong to workspace or any active skill root",
            input_path
        ));
    }

    // 3. For relative paths, see if the target's parent directory already exists in a skill root.
    // This allows creating new files in existing skill subdirectories.
    if let Some(parent) = Path::new(input_path).parent() {
        if !parent.as_os_str().is_empty() {
            // Check skill roots (skip the primary root at index 0 initially)
            for root in roots.iter().skip(1) {
                if let Ok(resolved) = resolve_safe_path(root, input_path) {
                    if let Some(resolved_parent) = resolved.parent() {
                        if resolved_parent.exists() {
                            return Ok((resolved, root.clone()));
                        }
                    }
                }
            }
        }
    }

    // 4. Default to primary workspace root for new relative paths
    resolve_safe_path(primary_root, input_path).map(|p| (p, primary_root.to_path_buf()))
}

fn is_path_protected(
    path: &Path,
    protected_roots: &[PathBuf],
    protected_files: &[PathBuf],
) -> bool {
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    protected_files.iter().any(|file| {
        let normalized_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        normalized == normalized_file || is_backup_variant(&normalized_file, &normalized)
    }) || protected_roots.iter().any(|root| {
        let normalized_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        normalized.starts_with(&normalized_root)
    })
}

fn next_backup_path(path: &Path) -> Result<PathBuf, String> {
    let Some(parent) = path.parent() else {
        return Err(format!(
            "Cannot create backup for '{}': missing parent directory",
            path.display()
        ));
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(format!(
            "Cannot create backup for '{}': invalid file name",
            path.display()
        ));
    };

    for index in 1.. {
        let candidate = parent.join(format!("{file_name}.bak.{index}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(format!("Cannot create backup for '{}'", path.display()))
}

fn backup_protected_file(
    path: &Path,
    protected_roots: &[PathBuf],
    protected_files: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    if !path.exists()
        || !path.is_file()
        || !is_path_protected(path, protected_roots, protected_files)
    {
        return Ok(None);
    }

    let backup_path = next_backup_path(path)?;
    fs::copy(path, &backup_path).map_err(|e| {
        format!(
            "Failed to back up '{}' to '{}': {}",
            path.display(),
            backup_path.display(),
            e
        )
    })?;
    Ok(Some(backup_path))
}

pub async fn execute_tool(
    app: &AppHandle,
    name: &str,
    args_str: &str,
    workspace_dir: PathBuf,
    config: &crate::AppConfig,
    // Allowlist of executable names from the active skill (empty = unrestricted).
    allowed_commands: &[String],
    // Root directories of active skills used as the default command cwd.
    command_skill_roots: &[PathBuf],
    // Extra roots allowed for file access, such as skill directories available in self-evolution mode.
    accessible_skill_roots: &[PathBuf],
    // Skill roots that require automatic `.bak.N` backups before mutation.
    protected_skill_roots: &[PathBuf],
    // Exact files that require automatic `.bak.N` backups before mutation.
    protected_exact_files: &[PathBuf],
) -> String {
    let args: Value = serde_json::from_str(args_str).unwrap_or_default();
    match name {
        "run_cmd" => {
            let command = args["command"].as_str().unwrap_or("").to_string();
            let command_cwd = command_skill_roots
                .first()
                .cloned()
                .unwrap_or_else(|| workspace_dir.clone());

            // ── Allowed-commands enforcement (skill context) ──────────────────
            if !allowed_commands.is_empty() {
                let parts = split_command_line(&command);
                let program = parts.first().map(|s| s.as_str()).unwrap_or("");
                let exe_name = Path::new(program)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(program)
                    .to_lowercase();
                let permitted = allowed_commands
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&exe_name));
                if !permitted {
                    return format!(
                        "⛔ Command '{}' is not in this skill's allowed-commands list ({}).",
                        exe_name,
                        allowed_commands.join(", ")
                    );
                }
            }

            let parts = split_command_line(&command);
            let sudo_requested = parts.first().is_some_and(|p| is_sudo_executable(p));
            let mut reasons: Vec<String> = vec![];
            if sudo_requested {
                reasons.push("privileged execution requested via sudo".to_string());
            }
            if let Some(reason) = is_dangerous("direct", &command) {
                reasons.push(reason);
            }

            // ── Confirmation / privilege prompts ─────────────────────────────
            let mut sudo_username: Option<String> = None;
            let mut sudo_password: Option<String> = None;
            if !reasons.is_empty() {
                let requires_auth = if sudo_requested { "sudo" } else { "none" };
                let kind = if sudo_requested { "sudo" } else { "dangerous" };
                let confirm = request_tool_confirmation(
                    app,
                    reasons.join("; "),
                    "direct".to_string(),
                    command.clone(),
                    kind,
                    requires_auth,
                )
                .await;

                if !confirm.confirmed {
                    return "⛔ Command execution denied by user.".to_string();
                }
                sudo_username = confirm.username;
                sudo_password = confirm.password;
            }

            if sudo_requested {
                let password = sudo_password.unwrap_or_default();
                if password.is_empty() {
                    return "⛔ sudo password is required.".to_string();
                }

                let mut sudo_args: Vec<String> = parts.into_iter().skip(1).collect();
                let username = sudo_username
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                if let Some(ref u) = username {
                    let has_user_flag = sudo_args.iter().any(|t| t == "-u");
                    if !has_user_flag {
                        sudo_args.splice(0..0, vec!["-u".to_string(), u.clone()]);
                    }
                }

                let _ = app.emit(
                    "chat-token",
                    format!("⚙️ *Running (sudo):*\n```\n{}\n```\n\n", command),
                );

                let mut cmd = tokio::process::Command::new("sudo");
                cmd.arg("-S").arg("-p").arg("");
                cmd.args(sudo_args);
                cmd.current_dir(command_cwd);

                return tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    run_command_with_stdin(cmd, format!("{}\n", password)),
                )
                .await
                .unwrap_or_else(|_| Ok("Command timed out after 30 seconds.".to_string()))
                .unwrap_or_else(|e| format!("Error: {}", e));
            }

            let _ = app.emit(
                "chat-token",
                format!("⚙️ *Running:*\n```\n{}\n```\n\n", command),
            );
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                run_command("direct".to_string(), command, Some(command_cwd)),
            )
            .await
            .unwrap_or_else(|_| Ok("Command timed out after 30 seconds.".to_string()))
            .unwrap_or_else(|e| format!("Error: {}", e))
        }
        "run_shell" => {
            let shell_type = args["type"].as_str().unwrap_or("powershell").to_string();
            let code = args["code"].as_str().unwrap_or("").to_string();
            let sudo_flag = args["sudo"].as_bool().unwrap_or(false);
            let elevated_flag = args["elevated"].as_bool().unwrap_or(false);
            let command_cwd = command_skill_roots
                .first()
                .cloned()
                .unwrap_or_else(|| workspace_dir.clone());

            let sudo_requested = shell_type == "bash" && (sudo_flag || code_requests_sudo(&code));
            let elevated_requested = shell_type == "powershell" && elevated_flag;

            let mut reasons: Vec<String> = vec![];
            if sudo_requested {
                reasons.push("privileged execution requested via sudo".to_string());
            }
            if elevated_requested {
                reasons.push("administrator elevation requested (UAC)".to_string());
            }
            if let Some(reason) = is_dangerous(&shell_type, &code) {
                reasons.push(reason);
            }

            let mut sudo_username: Option<String> = None;
            let mut sudo_password: Option<String> = None;
            if !reasons.is_empty() {
                let (kind, requires_auth) = if sudo_requested {
                    ("sudo", "sudo")
                } else if elevated_requested {
                    ("elevation", "elevation")
                } else {
                    ("dangerous", "none")
                };

                let confirm = request_tool_confirmation(
                    app,
                    reasons.join("; "),
                    shell_type.clone(),
                    code.clone(),
                    kind,
                    requires_auth,
                )
                .await;

                if !confirm.confirmed {
                    return "⛔ Command execution denied by user.".to_string();
                }
                sudo_username = confirm.username;
                sudo_password = confirm.password;
            }

            if sudo_requested {
                let password = sudo_password.unwrap_or_default();
                if password.is_empty() {
                    return "⛔ sudo password is required.".to_string();
                }
                let username = sudo_username
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                let _ = app.emit(
                    "chat-token",
                    format!(
                        "⚙️ *Running {} (sudo):*\n```{}\n{}\n```\n\n",
                        shell_type, shell_type, code
                    ),
                );

                let mut cmd = tokio::process::Command::new("sudo");
                cmd.arg("-S").arg("-p").arg("");
                if let Some(u) = username {
                    cmd.arg("-u").arg(u);
                }
                cmd.arg("bash").arg("-lc").arg(code);
                cmd.current_dir(command_cwd);

                return tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    run_command_with_stdin(cmd, format!("{}\n", password)),
                )
                .await
                .unwrap_or_else(|_| Ok("Command timed out after 30 seconds.".to_string()))
                .unwrap_or_else(|e| format!("Error: {}", e));
            }

            if elevated_requested {
                let _ = app.emit(
                    "chat-token",
                    format!(
                        "⚙️ *Running {} (elevated):*\n```{}\n{}\n```\n\n",
                        shell_type, shell_type, code
                    ),
                );

                return tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    run_powershell_elevated(code, Some(command_cwd)),
                )
                .await
                .unwrap_or_else(|_| Ok("Command timed out after 30 seconds.".to_string()))
                .unwrap_or_else(|e| format!("Error: {}", e));
            }

            let _ = app.emit(
                "chat-token",
                format!(
                    "⚙️ *Running {}:*\n```{}\n{}\n```\n\n",
                    shell_type, shell_type, code
                ),
            );
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                run_command(shell_type, code, Some(command_cwd)),
            )
            .await
            .unwrap_or_else(|_| Ok("Command timed out after 30 seconds.".to_string()))
            .unwrap_or_else(|e| format!("Error: {}", e))
        }
        "file_actions" => {
            let action = args["action"].as_str().unwrap_or("");
            let path_str = args["path"].as_str().unwrap_or("");
            let root_dir = workspace_dir.clone();
            let resolve_file_path = |input: &str, require_exists: bool| {
                resolve_safe_explicit_file(input, protected_exact_files)
                    .or_else(|| {
                        resolve_safe_path_with_roots(
                            &root_dir,
                            accessible_skill_roots,
                            input,
                            require_exists,
                        )
                        .ok()
                    })
                    .ok_or_else(|| {
                        format!(
                            "Path '{}' was not found under workspace root '{}' or any allowed self-evolution target",
                            input,
                            root_dir.display()
                        )
                    })
            };
            match action {
                "read" => {
                    let _ = app.emit("chat-token", format!("📄 *Reading {}*\n\n", path_str));
                    let start_line = args["start_line"].as_i64();
                    let end_line = args["end_line"].as_i64();
                    match resolve_file_path(path_str, true) {
                        Ok((p, _)) => {
                            match fs::metadata(&p) {
                                Ok(metadata) if metadata.is_dir() => {
                                    return format!("Error: {} is a directory", path_str);
                                }
                                Err(e) => {
                                    return format!("Error reading file metadata: {}", e);
                                }
                                _ => {}
                            }

                            if start_line.is_none() && end_line.is_none() {
                                return fs::read_to_string(&p)
                                    .unwrap_or_else(|e| format!("Error reading file: {}", e));
                            }

                            let start = start_line.unwrap_or(1);
                            let end = end_line.unwrap_or(i64::MAX);
                            if start <= 0 || end < start {
                                return "Error: invalid line range. start_line must be >= 1 and end_line must be >= start_line.".to_string();
                            }

                            match fs::File::open(&p) {
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
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "write" => {
                    let content_str = args["content"].as_str().unwrap_or("");
                    let _ = app.emit("chat-token", format!("💾 *Writing {}*\n\n", path_str));
                    match resolve_file_path(path_str, false) {
                        Ok((p, _)) => {
                            let backup = match backup_protected_file(
                                &p,
                                protected_skill_roots,
                                protected_exact_files,
                            ) {
                                Ok(backup) => backup,
                                Err(e) => return format!("Error: {}", e),
                            };
                            if let Some(parent) = p.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            match fs::write(&p, content_str) {
                                Ok(_) => {
                                    if let Some(backup) = backup {
                                        format!(
                                            "Successfully backed up to {} and wrote to {}",
                                            backup.display(),
                                            path_str
                                        )
                                    } else {
                                        format!("Successfully wrote to {}", path_str)
                                    }
                                }
                                Err(e) => format!("Error writing file: {}", e),
                            }
                        }
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "list" => {
                    let _ = app.emit("chat-token", format!("📂 *Listing {}*\n\n", path_str));
                    match resolve_file_path(path_str, true) {
                        Ok((p, _)) => {
                            match fs::metadata(&p) {
                                Ok(metadata) => {
                                    if metadata.is_file() {
                                        let name = p
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or(path_str)
                                            .to_string();
                                        return format!("{} ({} bytes)", name, metadata.len());
                                    }
                                }
                                Err(_) => {}
                            }

                            match fs::read_dir(&p) {
                                Ok(entries) => {
                                    let mut res = Vec::new();
                                    for entry in entries.flatten() {
                                        if let Ok(name) = entry.file_name().into_string() {
                                            let is_dir = entry
                                                .file_type()
                                                .map(|t| t.is_dir())
                                                .unwrap_or(false);
                                            if is_dir {
                                                res.push(format!("{}/", name));
                                            } else {
                                                let size =
                                                    entry.metadata().map(|m| m.len()).unwrap_or(0);
                                                res.push(format!("{} ({} bytes)", name, size));
                                            }
                                        }
                                    }
                                    if res.is_empty() {
                                        "(empty directory)".to_string()
                                    } else {
                                        res.join("\n")
                                    }
                                }
                                Err(e) => format!("Error listing directory: {}", e),
                            }
                        }
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "rename" | "move" => {
                    let new_path = args["new_path"].as_str().unwrap_or("");
                    let _ = app.emit(
                        "chat-token",
                        format!("🔁 *Moving {} -> {}*\n\n", path_str, new_path),
                    );
                    if new_path.is_empty() {
                        return "Error: new_path is required for move/rename.".to_string();
                    }
                    match resolve_file_path(path_str, true) {
                        Ok((src, src_root)) => match resolve_safe_path_with_roots(
                            &src_root,
                            accessible_skill_roots,
                            new_path,
                            false,
                        ) {
                            Ok((dst, _)) => {
                                let backup = match backup_protected_file(
                                    &src,
                                    protected_skill_roots,
                                    protected_exact_files,
                                ) {
                                    Ok(backup) => backup,
                                    Err(e) => return format!("Error: {}", e),
                                };
                                if let Some(parent) = dst.parent() {
                                    if let Err(e) = fs::create_dir_all(parent) {
                                        return format!(
                                            "Error creating destination directory: {}",
                                            e
                                        );
                                    }
                                }
                                match fs::rename(&src, &dst) {
                                    Ok(_) => {
                                        if let Some(backup) = backup {
                                            format!(
                                                "Successfully backed up to {} and moved {} to {}",
                                                backup.display(),
                                                path_str,
                                                new_path
                                            )
                                        } else {
                                            format!(
                                                "Successfully moved {} to {}",
                                                path_str, new_path
                                            )
                                        }
                                    }
                                    Err(e) => format!("Error moving file: {}", e),
                                }
                            }
                            Err(e) => format!("Error: {}", e),
                        },
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "patch" => {
                    let patch_str = args["patch"].as_str().unwrap_or("");
                    let _ = app.emit("chat-token", format!("🩹 *Patching {}*\n\n", path_str));
                    match resolve_file_path(path_str, true) {
                        Ok((p, _)) => {
                            let backup = match backup_protected_file(
                                &p,
                                protected_skill_roots,
                                protected_exact_files,
                            ) {
                                Ok(backup) => backup,
                                Err(e) => return format!("Error: {}", e),
                            };
                            match fs::read_to_string(&p) {
                                Ok(original) => match diffy::Patch::from_str(patch_str) {
                                    Ok(patch) => match diffy::apply(&original, &patch) {
                                        Ok(patched) => match fs::write(&p, &patched) {
                                            Ok(_) => {
                                                if let Some(backup) = backup {
                                                    format!(
                                                        "Successfully backed up to {} and patched {}",
                                                        backup.display(),
                                                        path_str
                                                    )
                                                } else {
                                                    format!("Successfully patched {}", path_str)
                                                }
                                            }
                                            Err(e) => format!("Error writing file: {}", e),
                                        },
                                        Err(e) => format!("Error applying patch: {}", e),
                                    },
                                    Err(e) => format!("Error parsing patch: {}", e),
                                },
                                Err(e) => format!("Error reading file: {}", e),
                            }
                        }
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "mkdir" => {
                    let _ = app.emit(
                        "chat-token",
                        format!("📁 *Creating directory {}*\n\n", path_str),
                    );
                    match resolve_safe_path_with_roots(
                        &root_dir,
                        accessible_skill_roots,
                        path_str,
                        false,
                    ) {
                        Ok((p, _)) => {
                            if p.exists() && p.is_file() {
                                return format!("Error: {} is an existing file", path_str);
                            }
                            match fs::create_dir_all(&p) {
                                Ok(_) => format!("Successfully created directory {}", path_str),
                                Err(e) => format!("Error creating directory: {}", e),
                            }
                        }
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "delete" => {
                    let _ = app.emit("chat-token", format!("🗑️ *Deleting {}*\n\n", path_str));
                    match resolve_file_path(path_str, true) {
                        Ok((p, _)) => {
                            let backup = match backup_protected_file(
                                &p,
                                protected_skill_roots,
                                protected_exact_files,
                            ) {
                                Ok(backup) => backup,
                                Err(e) => return format!("Error: {}", e),
                            };
                            if p.is_dir() {
                                match fs::remove_dir_all(&p) {
                                    Ok(_) => format!("Successfully deleted directory {}", path_str),
                                    Err(e) => format!("Error deleting directory: {}", e),
                                }
                            } else {
                                match fs::remove_file(&p) {
                                    Ok(_) => {
                                        if let Some(backup) = backup {
                                            format!(
                                                "Successfully backed up to {} and deleted file {}",
                                                backup.display(),
                                                path_str
                                            )
                                        } else {
                                            format!("Successfully deleted file {}", path_str)
                                        }
                                    }
                                    Err(e) => format!("Error deleting file: {}", e),
                                }
                            }
                        }
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
                format!(
                    "🧠 *Querying Knowledge Graph ({}) with: {}...*\n\n",
                    engine, query
                ),
            );

            if engine == "neo4j" {
                use crate::neo4j_db::{KnowledgeGraph, Neo4jRepo};
                let uri = config
                    .neo4j_uri
                    .as_deref()
                    .unwrap_or("bolt://localhost:7687");
                let user = config.neo4j_user.as_deref().unwrap_or("neo4j");
                let pass = config.neo4j_password.as_deref().unwrap_or("");
                match Neo4jRepo::new(uri, user, pass).await {
                    Ok(repo) => match repo.execute_query(&query).await {
                        Ok(res) => format!(
                            "Knowledge graph neo4j query executed: {}\nResult: {}",
                            query, res
                        ),
                        Err(e) => format!("Error executing neo4j query: {}", e),
                    },
                    Err(e) => format!("Failed to connect to neo4j: {}", e),
                }
            } else {
                format!(
                    "Knowledge graph {} query executed: {}. (Not fully implemented yet)",
                    engine, query
                )
            }
        }
        _ => format!("Unknown tool: {}", name),
    }
}

pub async fn run_command(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
) -> Result<String, String> {
    let mut cmd = match cmd_type.as_str() {
        "direct" => {
            // Run the executable directly — no shell wrapper needed.
            let parts = split_command_line(&code);
            if parts.is_empty() {
                return Err("Empty command".to_string());
            }
            let mut c = tokio::process::Command::new(&parts[0]);
            if parts.len() > 1 {
                c.args(&parts[1..]);
            }
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
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output().await.map_err(|e| e.to_string())?;
    Ok(format_process_output(output))
}

async fn run_powershell_elevated(_code: String, cwd: Option<PathBuf>) -> Result<String, String> {
    #[cfg(not(windows))]
    {
        let _ = cwd;
        return Err("Elevated PowerShell execution is only supported on Windows.".to_string());
    }

    #[cfg(windows)]
    {
        use std::time::{SystemTime, UNIX_EPOCH};

        fn ps_quote(s: &str) -> String {
            format!("'{}'", s.replace('\'', "''"))
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();

        let mut temp_dir = std::env::temp_dir();
        temp_dir.push("ai-chat");
        fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

        let script_path = temp_dir.join(format!("elevated-{}.ps1", now));
        let out_path = temp_dir.join(format!("elevated-{}.out.txt", now));
        let err_path = temp_dir.join(format!("elevated-{}.err.txt", now));

        fs::write(&script_path, _code).map_err(|e| e.to_string())?;

        let script_path_s = script_path.to_string_lossy().to_string();
        let out_path_s = out_path.to_string_lossy().to_string();
        let err_path_s = err_path.to_string_lossy().to_string();

        let launcher = format!(
            "$ErrorActionPreference='Stop';\n\
            $script={script};\n\
            $out={out};\n\
            $err={err};\n\
            $args=@('-NoProfile','-ExecutionPolicy','Bypass','-File',$script);\n\
            $p=Start-Process -FilePath 'powershell' -ArgumentList $args -Verb RunAs -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err;\n\
            exit $p.ExitCode\n",
            script = ps_quote(&script_path_s),
            out = ps_quote(&out_path_s),
            err = ps_quote(&err_path_s)
        );

        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &launcher]);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let output = cmd.output().await.map_err(|e| e.to_string())?;
        let mut res = format_process_output(output);

        let out_txt = fs::read_to_string(&out_path).unwrap_or_default();
        let err_txt = fs::read_to_string(&err_path).unwrap_or_default();

        let have_redirected_output = !out_txt.trim().is_empty() || !err_txt.trim().is_empty();
        if have_redirected_output && res == "(no output)" {
            res.clear();
        }

        if !out_txt.trim().is_empty() {
            if !res.is_empty() {
                res.push('\n');
            }
            res.push_str(&out_txt);
        }
        if !err_txt.trim().is_empty() {
            if !res.is_empty() {
                res.push('\n');
            }
            res.push_str("STDERR (elevated):\n");
            res.push_str(&err_txt);
        }

        let _ = fs::remove_file(&script_path);
        let _ = fs::remove_file(&out_path);
        let _ = fs::remove_file(&err_path);

        Ok(res)
    }
}
