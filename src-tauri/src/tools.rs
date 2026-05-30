use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Copy)]
enum OutputDecodeHint {
    Default,
    Direct,
    CmdShell,
    PowerShell,
}

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

enum SearchMatcher {
    Literal {
        needle: String,
        needle_lower: String,
        case_sensitive: bool,
    },
    Regex(regex::Regex),
}

impl SearchMatcher {
    fn build(query: &str, case_sensitive: bool, use_regex: bool) -> Result<Self, String> {
        if use_regex {
            let regex = regex::RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .map_err(|e| format!("Invalid search regex: {}", e))?;
            Ok(Self::Regex(regex))
        } else {
            Ok(Self::Literal {
                needle: query.to_string(),
                needle_lower: query.to_lowercase(),
                case_sensitive,
            })
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            SearchMatcher::Literal {
                needle,
                needle_lower,
                case_sensitive,
            } => {
                if *case_sensitive {
                    line.contains(needle)
                } else {
                    line.to_lowercase().contains(needle_lower)
                }
            }
            SearchMatcher::Regex(regex) => regex.is_match(line),
        }
    }
}

struct SearchOptions<'a> {
    recursive: bool,
    case_sensitive: bool,
    use_regex: bool,
    smart_case: bool,
    include_hidden: bool,
    respect_gitignore: bool,
    glob: Option<&'a str>,
}

fn should_use_case_sensitive_search(
    query: &str,
    case_sensitive: bool,
    smart_case: bool,
) -> bool {
    case_sensitive || (smart_case && query.chars().any(|ch| ch.is_uppercase()))
}

fn normalize_search_match_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn build_search_glob_matcher(glob: Option<&str>) -> Result<Option<globset::GlobSet>, String> {
    let Some(glob) = glob.map(str::trim).filter(|glob| !glob.is_empty()) else {
        return Ok(None);
    };

    let compiled_glob = globset::GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .map_err(|e| format!("Invalid search glob: {}", e))?;

    let mut builder = globset::GlobSetBuilder::new();
    builder.add(compiled_glob);
    builder
        .build()
        .map(Some)
        .map_err(|e| format!("Invalid search glob set: {}", e))
}

fn build_search_display_path(target: &Path, root: &Path) -> String {
    target
        .strip_prefix(root)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(target)
        .to_string_lossy()
        .to_string()
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(1024).any(|&byte| byte == 0)
}

fn search_file_contents(
    target: &Path,
    target_root: &Path,
    matcher: &SearchMatcher,
    glob_matcher: Option<&globset::GlobSet>,
    results: &mut Vec<String>,
) -> Result<(), String> {
    if let Some(glob_matcher) = glob_matcher {
        let match_path = normalize_search_match_path(target, target_root);
        if !glob_matcher.is_match(&match_path) {
            return Ok(());
        }
    }

    let bytes = fs::read(target)
        .map_err(|e| format!("Failed to read '{}': {}", target.display(), e))?;

    if is_probably_binary(&bytes) {
        return Ok(());
    }

    let text = String::from_utf8_lossy(&bytes);
    let display_path = build_search_display_path(target, target_root);
    for (index, line) in text.lines().enumerate() {
        if matcher.is_match(line) {
            results.push(format!("{}:{}:{}", display_path, index + 1, line));
        }
    }

    Ok(())
}

fn run_integrated_search(
    query: &str,
    target: &Path,
    target_root: &Path,
    options: SearchOptions<'_>,
) -> Result<String, String> {
    let case_sensitive = should_use_case_sensitive_search(
        query,
        options.case_sensitive,
        options.smart_case,
    );
    let matcher = SearchMatcher::build(query, case_sensitive, options.use_regex)?;
    let glob_matcher = build_search_glob_matcher(options.glob)?;
    let mut results = Vec::new();
    let mut errors = Vec::new();

    if target.is_file() {
        search_file_contents(
            target,
            target_root,
            &matcher,
            glob_matcher.as_ref(),
            &mut results,
        )?;
    } else if target.is_dir() {
        let mut walker = ignore::WalkBuilder::new(target);
        walker.standard_filters(false);
        walker.hidden(!options.include_hidden);
        walker.git_ignore(options.respect_gitignore);
        walker.git_exclude(options.respect_gitignore);
        walker.parents(options.respect_gitignore);
        walker.ignore(options.respect_gitignore);
        walker.follow_links(false);

        if !options.recursive {
            walker.max_depth(Some(1));
        }

        for entry in walker.build() {
            match entry {
                Ok(entry) => {
                    if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
                        continue;
                    }

                    if let Err(err) = search_file_contents(
                        entry.path(),
                        target_root,
                        &matcher,
                        glob_matcher.as_ref(),
                        &mut results,
                    ) {
                        errors.push(err);
                    }
                }
                Err(err) => errors.push(err.to_string()),
            }
        }
    } else {
        return Err(format!("Search target '{}' does not exist", target.display()));
    }

    if results.is_empty() && errors.is_empty() {
        return Ok("(no matches)".to_string());
    }

    let mut output = results.join("\n");
    if !errors.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("STDERR:\n");
        output.push_str(&errors.join("\n"));
    }

    Ok(output)
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

fn decode_process_bytes(bytes: &[u8], hint: OutputDecodeHint) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Some(decoded) = decode_with_bom(bytes) {
        return decoded;
    }

    if let Ok(decoded) = std::str::from_utf8(bytes) {
        return decoded.to_string();
    }

    #[cfg(windows)]
    if let Some(decoded) = decode_windows_process_bytes(bytes, hint) {
        return decoded;
    }

    String::from_utf8_lossy(bytes).to_string()
}

fn decode_with_bom(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(&bytes[3..]).ok().map(|text| text.to_string());
    }

    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Some(String::from_utf16_lossy(&utf16_units_le(&bytes[2..])));
    }

    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Some(String::from_utf16_lossy(&utf16_units_be(&bytes[2..])));
    }

    None
}

fn utf16_units_le(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn utf16_units_be(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect()
}

#[cfg(windows)]
fn decode_windows_process_bytes(bytes: &[u8], hint: OutputDecodeHint) -> Option<String> {
    use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};

    let mut code_pages = vec![65001];
    match hint {
        OutputDecodeHint::CmdShell => {
            code_pages.push(unsafe { GetOEMCP() });
            code_pages.push(unsafe { GetACP() });
        }
        _ => {
            code_pages.push(unsafe { GetACP() });
            code_pages.push(unsafe { GetOEMCP() });
        }
    }

    decode_windows_process_bytes_with_code_pages(bytes, &code_pages)
}

#[cfg(windows)]
fn decode_windows_process_bytes_with_code_pages(
    bytes: &[u8],
    code_pages: &[u32],
) -> Option<String> {
    let mut attempted = Vec::new();
    for &code_page in code_pages {
        if code_page == 0 || attempted.contains(&code_page) {
            continue;
        }
        attempted.push(code_page);
        if let Some(decoded) = decode_windows_code_page(bytes, code_page) {
            return Some(decoded);
        }
    }
    None
}

#[cfg(windows)]
fn decode_windows_code_page(bytes: &[u8], code_page: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, MB_ERR_INVALID_CHARS};

    let flags = if code_page == 65001 {
        MB_ERR_INVALID_CHARS
    } else {
        0
    };

    let wide_len = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return None;
    }

    let mut wide = vec![0u16; wide_len as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return None;
    }

    Some(String::from_utf16_lossy(&wide[..written as usize]))
}

#[cfg(windows)]
fn wrap_cmd_script_for_utf8(code: &str) -> String {
    format!("chcp 65001>nul & {code}")
}

#[cfg(not(windows))]
fn wrap_cmd_script_for_utf8(code: &str) -> String {
    code.to_string()
}

fn wrap_powershell_script_for_utf8(code: &str) -> String {
    #[cfg(windows)]
    {
        return format!(
            "$utf8NoBom = New-Object System.Text.UTF8Encoding($false); \
[Console]::InputEncoding = $utf8NoBom; \
[Console]::OutputEncoding = $utf8NoBom; \
$OutputEncoding = $utf8NoBom; \
chcp 65001 > $null; \
{code}"
        );
    }

    #[cfg(not(windows))]
    {
        code.to_string()
    }
}

fn format_process_output(output: std::process::Output, hint: OutputDecodeHint) -> String {
    let stdout = decode_process_bytes(&output.stdout, hint);
    let stderr = decode_process_bytes(&output.stderr, hint);

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
    Ok(format_process_output(output, OutputDecodeHint::Default))
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
                "description": "Run an executable program directly (without a shell) on the user's machine. The first token is the executable; the rest are arguments. Preferred for simple commands like curl, wget, git, etc. CRITICAL: The tool always runs with the workspace directory as its current working directory. Dangerous or privileged operations (sudo) will require explicit user confirmation before execution.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The full command line (e.g. 'curl -s https://example.com'). The first word is the executable; the rest are arguments. Handles basic single- and double-quote grouping."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 3600,
                            "description": "Optional timeout for the command, in seconds. Defaults to 30 if omitted. Use a larger value for long-running compile or build steps."
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
                "description": "Execute a script in a shell on the user's machine. \n                                Supports pipes, loops, variables, and other shell features. \n                                CRITICAL: The tool always switches to the workspace directory before running the script, and directory changes must stay within the workspace root. \n                                Dangerous or privileged operations (sudo / admin elevation) will require explicit user confirmation.",
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
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 3600,
                            "description": "Optional timeout for the command, in seconds. Defaults to 30 if omitted. Use a larger value for long-running compile or build steps."
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
                "description": "Perform file operations. Path rules: workspace is the default root — always use `./...` form (e.g. `./src/App.tsx`), never `workspace/...`. Writes are restricted to the workspace root. Skill files are read-only references: use `./skills/<name>/...` for workspace skills or `app_data/skills/<name>/...` for app-managed skills — never `./app_data/...`. workspace/skills becomes writable when self-evolution mode is enabled. Changes to protected files create automated backups.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["read", "write", "list", "search", "mkdir", "patch", "rename", "move", "delete"],
                            "description": "The file action to perform: read file content, write/overwrite a file, list directory entries, search file contents with the built-in search engine, recursively create directories, apply a unified diff patch, rename a file/directory, move a file/directory, or delete a file/directory."
                        },
                        "path": {
                            "type": "string",
                            "description": "Path to the file or directory. Workspace paths should use the `./...` form, for example `./src/App.tsx`, and should not use `workspace/...`. Only use explicit skill prefixes such as './skills/<skill_name>/...' or 'app_data/skills/<skill_name>/...' when you intentionally want to access skill-owned data. Use '.' or './' for the workspace root when listing or searching."
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
                        "query": {
                            "type": "string",
                            "description": "Content search text or regex pattern (only for 'search'). No external rg executable is required."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Whether to search subdirectories recursively when path is a directory (only for 'search'). Defaults to true."
                        },
                        "case_sensitive": {
                            "type": "boolean",
                            "description": "Whether the content search is case-sensitive (only for 'search'). Defaults to false."
                        },
                        "smart_case": {
                            "type": "boolean",
                            "description": "Whether uppercase letters in query should automatically switch search to case-sensitive when case_sensitive is false (only for 'search'). Defaults to false."
                        },
                        "use_regex": {
                            "type": "boolean",
                            "description": "Whether query should be treated as a regular expression instead of plain text (only for 'search'). Defaults to false."
                        },
                        "glob": {
                            "type": "string",
                            "description": "Optional file path glob filter for search targets, such as '**/*.rs' or 'src/**/*.ts' (only for 'search')."
                        },
                        "include_hidden": {
                            "type": "boolean",
                            "description": "Whether hidden files and directories should be included during directory searches (only for 'search'). Defaults to true."
                        },
                        "respect_gitignore": {
                            "type": "boolean",
                            "description": "Whether .gitignore, .ignore, and git exclude rules should be respected during directory searches (only for 'search'). Defaults to false."
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

pub fn get_agent_task_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "add_task",
                "description": "Add a new task to the current autonomous mission. Use this instead of relying on chat history to remember future work.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Short task title"
                        },
                        "description": {
                            "type": "string",
                            "description": "Detailed task description"
                        }
                    },
                    "required": ["description"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "update_task_status",
                "description": "Update the status of an existing mission task.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "string",
                            "description": "Mission task identifier"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"],
                            "description": "New task status"
                        }
                    },
                    "required": ["task_id", "status"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_active_tasks",
                "description": "Return all active mission tasks that are not completed.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mark_mission_accomplished",
                "description": "Mark the current mission as accomplished so the autonomous loop can terminate cleanly.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "final_report": {
                            "type": "string",
                            "description": "Optional final report to persist as the mission outcome"
                        }
                    }
                }
            }
        }),
    ]
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

fn build_root_relative_candidates(root: &Path, input_path: &str) -> Vec<PathBuf> {
    let input = Path::new(input_path);
    let mut candidates = vec![input.to_path_buf()];

    if input.is_absolute() {
        return candidates;
    }

    let Some(root_name) = root.file_name() else {
        return candidates;
    };

    let mut components = input.components();
    let Some(std::path::Component::Normal(first)) = components.next() else {
        return candidates;
    };

    if first == root_name {
        let mut stripped = PathBuf::new();
        for component in components {
            match component {
                std::path::Component::Normal(segment) => stripped.push(segment),
                std::path::Component::ParentDir => stripped.push(".."),
                std::path::Component::CurDir => {}
                _ => {}
            }
        }

        if !stripped.as_os_str().is_empty() && !candidates.iter().any(|p| p == &stripped) {
            candidates.push(stripped);
        }
    }

    let input_components: Vec<_> = input
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(segment) => Some(segment.to_os_string()),
            _ => None,
        })
        .collect();

    let root_parent_name = root
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_os_string());

    if input_components.len() >= 3
        && input_components[0] == "app_data"
        && input_components[1] == "skills"
        && input_components[2] == root_name
        && root_parent_name.as_deref() == Some(std::ffi::OsStr::new("skills"))
    {
        let mut virtual_stripped = PathBuf::new();
        for component in input.components().skip(3) {
            match component {
                std::path::Component::Normal(segment) => virtual_stripped.push(segment),
                std::path::Component::ParentDir => virtual_stripped.push(".."),
                std::path::Component::CurDir => {}
                _ => {}
            }
        }

        let candidate = if virtual_stripped.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            virtual_stripped
        };

        if !candidates.iter().any(|p| p == &candidate) {
            candidates.push(candidate);
        }
    }

    candidates
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
        for candidate in build_root_relative_candidates(root, input_path) {
            if let Ok(resolved) = resolve_safe_path(root, &candidate.to_string_lossy()) {
                if resolved.exists() {
                    return Ok((resolved, root.clone()));
                }
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
            for candidate in build_root_relative_candidates(root, input_path) {
                if let Ok(resolved) = resolve_safe_path(root, &candidate.to_string_lossy()) {
                    return Ok((resolved, root.clone()));
                }
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
                for candidate in build_root_relative_candidates(root, input_path) {
                    if let Ok(resolved) = resolve_safe_path(root, &candidate.to_string_lossy()) {
                        if let Some(resolved_parent) = resolved.parent() {
                            if resolved_parent.exists() {
                                return Ok((resolved, root.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    // 4. Default to primary workspace root for new relative paths
    resolve_safe_path(primary_root, input_path).map(|p| (p, primary_root.to_path_buf()))
}

fn strip_explicit_app_data_skills_prefix(input_path: &str) -> Option<PathBuf> {
    let input = Path::new(input_path);
    if input.is_absolute() {
        return None;
    }

    let components: Vec<_> = input
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect();
    if components.len() < 2 {
        return None;
    }

    match (&components[0], &components[1]) {
        (
            std::path::Component::Normal(first),
            std::path::Component::Normal(second),
        ) if *first == std::ffi::OsStr::new("app_data")
            && *second == std::ffi::OsStr::new("skills") =>
        {
            let mut suffix = PathBuf::new();
            for component in components.into_iter().skip(2) {
                match component {
                    std::path::Component::Normal(segment) => suffix.push(segment),
                    std::path::Component::ParentDir => suffix.push(".."),
                    std::path::Component::CurDir => {}
                    _ => {}
                }
            }

            if suffix.as_os_str().is_empty() {
                Some(PathBuf::from("."))
            } else {
                Some(suffix)
            }
        }
        _ => None,
    }
}

fn resolve_app_data_skills_read_path(
    app_data_skills_dir: &Path,
    input_path: &str,
    require_exists: bool,
) -> Result<(PathBuf, PathBuf), String> {
    let Some(relative_suffix) = strip_explicit_app_data_skills_prefix(input_path) else {
        return Err("Path is not under app_data/skills".to_string());
    };

    let resolved = resolve_safe_path(app_data_skills_dir, &relative_suffix.to_string_lossy())?;
    if require_exists && !resolved.exists() {
        return Err(format!(
            "Path '{}' was not found under app_data skills root '{}'",
            input_path,
            app_data_skills_dir.display()
        ));
    }

    Ok((resolved, app_data_skills_dir.to_path_buf()))
}

fn resolve_workspace_read_path(
    workspace_root: &Path,
    input_path: &str,
    require_exists: bool,
) -> Result<(PathBuf, PathBuf), String> {
    let resolved = resolve_safe_path(workspace_root, input_path)?;
    if require_exists && !resolved.exists() {
        return Err(format!(
            "Path '{}' was not found under workspace root '{}'",
            input_path,
            workspace_root.display()
        ));
    }

    Ok((resolved, workspace_root.to_path_buf()))
}

fn resolve_read_path(
    workspace_root: &Path,
    app_data_skills_dir: &Path,
    readable_skill_roots: &[PathBuf],
    input_path: &str,
    require_exists: bool,
) -> Result<(PathBuf, PathBuf), String> {
    let input = Path::new(input_path);
    if strip_explicit_app_data_skills_prefix(input_path).is_some() {
        return resolve_app_data_skills_read_path(app_data_skills_dir, input_path, require_exists);
    }

    if input.is_absolute() {
        let mut readable_roots = readable_skill_roots.to_vec();
        if !readable_roots.iter().any(|root| root == app_data_skills_dir) {
            readable_roots.push(app_data_skills_dir.to_path_buf());
        }
        return resolve_safe_path_with_roots(
            workspace_root,
            &readable_roots,
            input_path,
            require_exists,
        );
    }

    resolve_workspace_read_path(workspace_root, input_path, require_exists)
}

fn logical_root_label(workspace_root: &Path, resolved_root: &Path) -> String {
    let normalized_workspace = normalize_path_for_comparison(workspace_root);
    let normalized_root = normalize_path_for_comparison(resolved_root);

    if let Ok(relative) = normalized_root.strip_prefix(&normalized_workspace) {
        if relative.as_os_str().is_empty() {
            return "./".to_string();
        }

        return format!(
            "./{}",
            relative.to_string_lossy().replace('\\', "/")
        );
    }

    if normalized_root
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new("skills"))
    {
        return "app_data/skills".to_string();
    }

    if normalized_root
        .parent()
        .and_then(|parent| parent.file_name())
        .is_some_and(|name| name == std::ffi::OsStr::new("skills"))
    {
        return format!(
            "app_data/skills/{}",
            normalized_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
        );
    }

    normalized_root.to_string_lossy().replace('\\', "/")
}

fn with_root_header(workspace_root: &Path, resolved_root: &Path, body: String) -> String {
    let header = format!("ROOT: {}", logical_root_label(workspace_root, resolved_root));
    if body.is_empty() {
        header
    } else {
        format!("{header}\n{body}")
    }
}

fn normalize_path_for_comparison(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    normalize_path_for_comparison(path).starts_with(normalize_path_for_comparison(root))
}

fn path_is_within_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path_is_within(path, root))
}

fn workspace_skills_root(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("skills")
}

fn ensure_mutation_target_allowed(
    path: &Path,
    workspace_dir: &Path,
    writable_skill_roots: &[PathBuf],
) -> Result<(), String> {
    let workspace_skills = workspace_skills_root(workspace_dir);
    if path_is_within(path, &workspace_skills) && !path_is_within_any_root(path, writable_skill_roots)
    {
        return Err(
            "Skill directories are read-only. Only workspace/skills is writable in self-evolution mode."
                .to_string(),
        );
    }

    Ok(())
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
        {
            return &value[1..value.len() - 1];
        }
    }

    value
}

fn contains_dynamic_shell_path_syntax(value: &str) -> bool {
    value.contains('$') || value.contains('%') || value.contains('`')
}

fn validate_directory_change_target(workspace_dir: &Path, raw_target: &str) -> Result<(), String> {
    let trimmed = strip_matching_quotes(raw_target.trim()).trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if contains_dynamic_shell_path_syntax(trimmed) {
        return Err(
            "Shell directory changes must use literal paths that stay within the workspace root."
                .to_string(),
        );
    }

    resolve_safe_path(workspace_dir, trimmed)
        .map(|_| ())
        .map_err(|_| {
            format!(
                "Shell directory changes must stay within the workspace root '{}'.",
                workspace_dir.display()
            )
        })
}

fn extract_directory_change_target(shell_type: &str, statement: &str) -> Option<String> {
    let lower = statement.to_ascii_lowercase();
    let command = if shell_type == "powershell" {
        ["set-location", "push-location", "pushd", "cd", "sl"]
            .into_iter()
            .find(|candidate| {
                lower == *candidate
                    || lower.starts_with(&format!("{candidate} "))
                    || lower.starts_with(&format!("{candidate}\t"))
            })?
    } else {
        ["cd", "pushd"]
            .into_iter()
            .find(|candidate| {
                lower == *candidate
                    || lower.starts_with(&format!("{candidate} "))
                    || lower.starts_with(&format!("{candidate}\t"))
            })?
    };

    let rest = statement[command.len()..].trim();
    if rest.is_empty() {
        return None;
    }

    let mut parts = split_command_line(rest);
    if shell_type == "powershell" {
        parts.retain(|part| {
            !part.eq_ignore_ascii_case("-path") && !part.eq_ignore_ascii_case("-literalpath")
        });
    }

    parts.into_iter().next()
}

fn validate_shell_working_directory_changes(
    shell_type: &str,
    code: &str,
    workspace_dir: &Path,
) -> Result<(), String> {
    for statement in code.replace(';', "\n").lines() {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(target) = extract_directory_change_target(shell_type, trimmed) {
            validate_directory_change_target(workspace_dir, &target)?;
        }
    }

    Ok(())
}

fn quote_cmd_path(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\"\""))
}

fn quote_powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn quote_posix_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_workspace_scoped_shell_code(shell_type: &str, workspace_dir: &Path, code: &str) -> String {
    match shell_type {
        "powershell" => format!(
            "Set-Location -LiteralPath {};\n{}",
            quote_powershell_literal(&workspace_dir.display().to_string()),
            code
        ),
        _ => {
            #[cfg(windows)]
            {
                format!("cd /d {} && {}", quote_cmd_path(workspace_dir), code)
            }

            #[cfg(not(windows))]
            {
                format!(
                    "cd {}\n{}",
                    quote_posix_literal(&workspace_dir.display().to_string()),
                    code
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::decode_process_bytes;
    #[cfg(windows)]
    use super::decode_windows_process_bytes_with_code_pages;
    use super::ensure_mutation_target_allowed;
    use super::OutputDecodeHint;
    use super::SearchOptions;
    use super::logical_root_label;
    use super::run_integrated_search;
    use super::resolve_read_path;
    use super::resolve_safe_path_with_roots;
    use super::validate_shell_working_directory_changes;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-chat-tools-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn normalize(path: &PathBuf) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.clone())
    }

    #[test]
    fn resolves_existing_skill_file_with_root_name_prefix() {
        let workspace_root = make_temp_dir("workspace");
        let skill_root = make_temp_dir("skills").join("skills");
        let skill_file = skill_root.join("demo").join("skill.md");
        fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        fs::write(&skill_file, "content").unwrap();

        let resolved = resolve_safe_path_with_roots(
            &workspace_root,
            std::slice::from_ref(&skill_root),
            "skills/demo/skill.md",
            true,
        )
        .unwrap();

        assert_eq!(normalize(&resolved.0), normalize(&skill_file));
        assert_eq!(normalize(&resolved.1), normalize(&skill_root));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(skill_root.parent().unwrap());
    }

    #[test]
    fn resolves_new_skill_file_with_root_name_prefix_to_skill_root() {
        let workspace_root = make_temp_dir("workspace");
        let skill_root = make_temp_dir("skills").join("skills");
        let existing_dir = skill_root.join("demo");
        fs::create_dir_all(&existing_dir).unwrap();

        let resolved = resolve_safe_path_with_roots(
            &workspace_root,
            std::slice::from_ref(&skill_root),
            "skills/demo/improved.md",
            false,
        )
        .unwrap();

        assert_eq!(resolved.0, existing_dir.join("improved.md"));
        assert_eq!(normalize(&resolved.1), normalize(&skill_root));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(skill_root.parent().unwrap());
    }

    #[test]
    fn resolves_existing_app_data_skill_file_with_virtual_prefix() {
        let workspace_root = make_temp_dir("workspace");
        let app_data_root = make_temp_dir("app-data");
        let skill_root = app_data_root.join("skills").join("demo");
        let skill_file = skill_root.join("ref").join("index.md");
        fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        fs::write(&skill_file, "content").unwrap();

        let resolved = resolve_safe_path_with_roots(
            &workspace_root,
            std::slice::from_ref(&skill_root),
            "app_data/skills/demo/ref/index.md",
            true,
        )
        .unwrap();

        assert_eq!(normalize(&resolved.0), normalize(&skill_file));
        assert_eq!(normalize(&resolved.1), normalize(&skill_root));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&app_data_root);
    }

    #[test]
    fn resolves_existing_app_data_skill_root_directory_with_virtual_prefix() {
        let workspace_root = make_temp_dir("workspace");
        let app_data_root = make_temp_dir("app-data");
        let app_data_skills_dir = app_data_root.join("skills");
        let skill_root = app_data_skills_dir.join("zhangxuefeng-perspective");
        fs::create_dir_all(skill_root.join("ref")).unwrap();

        let resolved = resolve_read_path(
            &workspace_root,
            &app_data_skills_dir,
            std::slice::from_ref(&skill_root),
            "app_data/skills/zhangxuefeng-perspective",
            true,
        )
        .unwrap();

        assert_eq!(normalize(&resolved.0), normalize(&skill_root));
        assert_eq!(normalize(&resolved.1), normalize(&app_data_skills_dir));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&app_data_root);
    }

    #[test]
    fn resolves_dot_prefixed_app_data_skill_path() {
        let workspace_root = make_temp_dir("workspace");
        let app_data_root = make_temp_dir("app-data");
        let app_data_skills_dir = app_data_root.join("skills");
        let skill_root = app_data_skills_dir.join("zhangxuefeng-perspective");
        let references = skill_root.join("references");
        fs::create_dir_all(&references).unwrap();

        let resolved = resolve_read_path(
            &workspace_root,
            &app_data_skills_dir,
            std::slice::from_ref(&skill_root),
            "./app_data/skills/zhangxuefeng-perspective/references",
            true,
        )
        .unwrap();

        assert_eq!(normalize(&resolved.0), normalize(&references));
        assert_eq!(normalize(&resolved.1), normalize(&app_data_skills_dir));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&app_data_root);
    }

    #[test]
    fn labels_workspace_root_as_dot_slash() {
        let workspace_root = make_temp_dir("workspace");

        assert_eq!(logical_root_label(&workspace_root, &workspace_root), "./");

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn labels_app_data_skill_root_with_virtual_prefix() {
        let workspace_root = make_temp_dir("workspace");
        let app_data_root = make_temp_dir("app-data");
        let skill_root = app_data_root.join("skills").join("zhangxuefeng-perspective");
        fs::create_dir_all(&skill_root).unwrap();

        assert_eq!(
            logical_root_label(&workspace_root, &skill_root),
            "app_data/skills/zhangxuefeng-perspective"
        );

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&app_data_root);
    }

    #[test]
    fn labels_app_data_skills_directory_root() {
        let workspace_root = make_temp_dir("workspace");
        let app_data_root = make_temp_dir("app-data");
        let app_data_skills_dir = app_data_root.join("skills");
        fs::create_dir_all(&app_data_skills_dir).unwrap();

        assert_eq!(
            logical_root_label(&workspace_root, &app_data_skills_dir),
            "app_data/skills"
        );

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&app_data_root);
    }

    #[test]
    fn resolves_app_data_skills_directory_root() {
        let workspace_root = make_temp_dir("workspace");
        let app_data_root = make_temp_dir("app-data");
        let app_data_skills_dir = app_data_root.join("skills");
        fs::create_dir_all(app_data_skills_dir.join("zhangxuefeng-perspective")).unwrap();

        let resolved = resolve_read_path(
            &workspace_root,
            &app_data_skills_dir,
            &[],
            "app_data/skills",
            true,
        )
        .unwrap();

        assert_eq!(normalize(&resolved.0), normalize(&app_data_skills_dir));
        assert_eq!(normalize(&resolved.1), normalize(&app_data_skills_dir));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&app_data_root);
    }

    #[test]
    fn ambiguous_relative_read_path_does_not_fallback_to_skill_root() {
        let workspace_root = make_temp_dir("workspace");
        let app_data_root = make_temp_dir("app-data");
        let skill_root = app_data_root.join("skills").join("demo");
        let skill_file = skill_root.join("ref").join("index.md");
        fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        fs::write(&skill_file, "content").unwrap();

        let err = resolve_read_path(
            &workspace_root,
            &app_data_root.join("skills"),
            std::slice::from_ref(&skill_root),
            "ref/index.md",
            true,
        )
        .unwrap_err();

        assert!(err.contains("workspace root"));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&app_data_root);
    }

    #[test]
    fn decodes_utf8_bom_output() {
        let decoded = decode_process_bytes(
            &[0xEF, 0xBB, 0xBF, 0x54, 0x65, 0x73, 0x74],
            OutputDecodeHint::Direct,
        );

        assert_eq!(decoded, "Test");
    }

    #[test]
    fn searches_directory_non_recursively() {
        let root = make_temp_dir("search-non-recursive");
        let dir = root.join("src");
        let nested = dir.join("nested");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("top.txt"), "Needle in top level\n").unwrap();
        fs::write(nested.join("deep.txt"), "Needle in nested\n").unwrap();

        let output = run_integrated_search(
            "needle",
            &dir,
            &root,
            SearchOptions {
                recursive: false,
                case_sensitive: false,
                use_regex: false,
                smart_case: false,
                include_hidden: true,
                respect_gitignore: false,
                glob: None,
            },
        )
        .unwrap();

        assert!(output.contains("src\\top.txt:1:Needle in top level"));
        assert!(!output.contains("deep.txt"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn searches_with_regex_recursively() {
        let root = make_temp_dir("search-regex");
        let dir = root.join("src");
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("deep.txt"), "prefix Alpha42 suffix\n").unwrap();

        let output = run_integrated_search(
            "alpha\\d+",
            &dir,
            &root,
            SearchOptions {
                recursive: true,
                case_sensitive: false,
                use_regex: true,
                smart_case: false,
                include_hidden: true,
                respect_gitignore: false,
                glob: None,
            },
        )
        .unwrap();

        assert!(output.contains("src\\nested\\deep.txt:1:prefix Alpha42 suffix"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn smart_case_upgrades_to_case_sensitive_search() {
        let root = make_temp_dir("search-smart-case");
        let file = root.join("sample.txt");
        fs::write(&file, "needle\nNeedle\n").unwrap();

        let output = run_integrated_search(
            "Needle",
            &file,
            &root,
            SearchOptions {
                recursive: true,
                case_sensitive: false,
                use_regex: false,
                smart_case: true,
                include_hidden: true,
                respect_gitignore: false,
                glob: None,
            },
        )
        .unwrap();

        assert!(!output.contains("sample.txt:1:needle"));
        assert!(output.contains("sample.txt:2:Needle"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_glob_and_filters_limit_directory_results() {
        let root = make_temp_dir("search-filters");
        let dir = root.join("src");
        let git_dir = root.join(".git");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(root.join(".gitignore"), "src/ignored.md\n").unwrap();
        fs::write(dir.join("keep.md"), "needle keep\n").unwrap();
        fs::write(dir.join("keep.txt"), "needle text\n").unwrap();
        fs::write(dir.join("ignored.md"), "needle ignored\n").unwrap();
        fs::write(dir.join(".hidden.md"), "needle hidden\n").unwrap();

        let output = run_integrated_search(
            "needle",
            &dir,
            &root,
            SearchOptions {
                recursive: true,
                case_sensitive: false,
                use_regex: false,
                smart_case: false,
                include_hidden: false,
                respect_gitignore: true,
                glob: Some("**/*.md"),
            },
        )
        .unwrap();

        assert!(output.contains("src\\keep.md:1:needle keep"));
        assert!(!output.contains("keep.txt"));
        assert!(!output.contains("ignored.md"));
        assert!(!output.contains(".hidden.md"));

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn decodes_gbk_output_with_explicit_code_page() {
        let decoded = decode_windows_process_bytes_with_code_pages(
            &[0x54, 0x65, 0x73, 0x74],
            &[936],
        )
        .unwrap();

        assert_eq!(decoded, "Test");
    }

    #[test]
    fn workspace_skill_dir_is_read_only_without_self_evolution() {
        let workspace_root = make_temp_dir("readonly-skills");
        let skill_file = workspace_root.join("skills").join("demo").join("skill.md");

        let err = ensure_mutation_target_allowed(&skill_file, &workspace_root, &[]).unwrap_err();

        assert!(err.contains("read-only"));

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn workspace_skill_dir_is_writable_in_self_evolution_mode() {
        let workspace_root = make_temp_dir("writable-skills");
        let writable_root = workspace_root.join("skills");
        let skill_file = writable_root.join("demo").join("skill.md");

        let result = ensure_mutation_target_allowed(
            &skill_file,
            &workspace_root,
            std::slice::from_ref(&writable_root),
        );

        assert!(result.is_ok());

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn shell_directory_changes_cannot_escape_workspace() {
        let workspace_root = make_temp_dir("shell-dir-guard");

        let err = validate_shell_working_directory_changes(
            "powershell",
            "Set-Location ..\\..",
            &workspace_root,
        )
        .unwrap_err();

        assert!(err.contains("workspace root"));

        let _ = fs::remove_dir_all(&workspace_root);
    }
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
    // Active skill roots are readable for skill-owned reference data.
    _command_skill_roots: &[PathBuf],
    // Read-only skill roots that can be addressed by file_actions reads/searches.
    readable_skill_roots: &[PathBuf],
    // Writable skill roots that require automatic `.bak.N` backups before mutation.
    protected_skill_roots: &[PathBuf],
    // Exact files that require automatic `.bak.N` backups before mutation.
    protected_exact_files: &[PathBuf],
    // Current autonomous mission identifier when executing inside a sub-agent.
    mission_id: Option<&str>,
) -> String {
    let args: Value = serde_json::from_str(args_str).unwrap_or_default();
    match name {
        "add_task" => {
            let Some(mission_id) = mission_id else {
                return "Error: add_task is only available inside an autonomous sub-agent mission.".to_string();
            };
            let description = args["description"].as_str().unwrap_or("");
            let name = args["name"].as_str().unwrap_or(description);
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::add_mission_task(&db, mission_id, name, description) {
                Ok(task) => {
                    let payload = json!({
                        "mission_id": mission_id,
                        "task": task,
                    });
                    let _ = app.emit("agent-task-state", payload.clone());
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "update_task_status" => {
            let Some(mission_id) = mission_id else {
                return "Error: update_task_status is only available inside an autonomous sub-agent mission.".to_string();
            };
            let task_id = args["task_id"].as_str().unwrap_or("");
            let status = args["status"].as_str().unwrap_or("");
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::update_mission_task_status(&db, mission_id, task_id, status) {
                Ok(task) => {
                    let payload = json!({
                        "mission_id": mission_id,
                        "task": task,
                    });
                    let _ = app.emit("agent-task-state", payload.clone());
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "get_active_tasks" => {
            let Some(mission_id) = mission_id else {
                return "Error: get_active_tasks is only available inside an autonomous sub-agent mission.".to_string();
            };
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::get_active_mission_tasks(&db, mission_id) {
                Ok(tasks) => serde_json::to_string_pretty(&json!({
                    "mission_id": mission_id,
                    "active_tasks": tasks,
                }))
                .unwrap_or_else(|_| "[]".to_string()),
                Err(err) => format!("Error: {err}"),
            }
        }
        "mark_mission_accomplished" => {
            let Some(mission_id) = mission_id else {
                return "Error: mark_mission_accomplished is only available inside an autonomous sub-agent mission.".to_string();
            };
            let final_report = args["final_report"].as_str();
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::mark_mission_accomplished(&db, mission_id, final_report) {
                Ok(()) => {
                    let payload = json!({
                        "mission_id": mission_id,
                        "status": "completed",
                        "final_report": final_report.unwrap_or(""),
                    });
                    let _ = app.emit("agent-task-state", payload.clone());
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "run_cmd" => {
            let command = args["command"].as_str().unwrap_or("").to_string();
            let command_cwd = workspace_dir.clone();

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

            let timeout_secs = args["timeout_seconds"]
                .as_i64()
                .unwrap_or(30)
                .clamp(1, 3600) as u64;

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
                    "tool-call",
                    format!("⚙️ *Running (sudo):*\n```\n{}\n```\n\n", command),
                );

                let mut cmd = tokio::process::Command::new("sudo");
                cmd.arg("-S").arg("-p").arg("");
                cmd.args(sudo_args);
                cmd.current_dir(command_cwd);

                return tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    run_command_with_stdin(cmd, format!("{}\n", password)),
                )
                .await
                .unwrap_or_else(|_| Ok(format!("Command timed out after {} seconds.", timeout_secs)))
                .unwrap_or_else(|e| format!("Error: {}", e));
            }

            let _ = app.emit(
                "tool-call",
                format!("⚙️ *Running:*\n```\n{}\n```\n\n", command),
            );
            tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                run_command("direct".to_string(), command, Some(command_cwd)),
            )
            .await
            .unwrap_or_else(|_| Ok(format!("Command timed out after {} seconds.", timeout_secs)))
            .unwrap_or_else(|e| format!("Error: {}", e))
        }
        "run_shell" => {
            let shell_type = args["type"].as_str().unwrap_or("powershell").to_string();
            let code = args["code"].as_str().unwrap_or("").to_string();
            let sudo_flag = args["sudo"].as_bool().unwrap_or(false);
            let elevated_flag = args["elevated"].as_bool().unwrap_or(false);
            let command_cwd = workspace_dir.clone();
            let timeout_secs = args["timeout_seconds"]
                .as_i64()
                .unwrap_or(30)
                .clamp(1, 3600) as u64;

            if let Err(err) = validate_shell_working_directory_changes(
                &shell_type,
                &code,
                &workspace_dir,
            ) {
                return format!("⛔ {}", err);
            }

            let scoped_code = build_workspace_scoped_shell_code(&shell_type, &workspace_dir, &code);

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
                    "tool-call",
                    format!(
                        "⚙️ *Running {} (sudo):*\n```{}\n{}\n```\n\n",
                        shell_type, shell_type, scoped_code
                    ),
                );

                let mut cmd = tokio::process::Command::new("sudo");
                cmd.arg("-S").arg("-p").arg("");
                if let Some(u) = username {
                    cmd.arg("-u").arg(u);
                }
                cmd.arg("bash").arg("-lc").arg(scoped_code.clone());
                cmd.current_dir(command_cwd);

                return tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    run_command_with_stdin(cmd, format!("{}\n", password)),
                )
                .await
                .unwrap_or_else(|_| Ok(format!("Command timed out after {} seconds.", timeout_secs)))
                .unwrap_or_else(|e| format!("Error: {}", e));
            }

            if elevated_requested {
                let _ = app.emit(
                    "tool-call",
                    format!(
                        "⚙️ *Running {} (elevated):*\n```{}\n{}\n```\n\n",
                        shell_type, shell_type, scoped_code
                    ),
                );

                return tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    run_powershell_elevated(scoped_code.clone(), Some(command_cwd)),
                )
                .await
                .unwrap_or_else(|_| Ok(format!("Command timed out after {} seconds.", timeout_secs)))
                .unwrap_or_else(|e| format!("Error: {}", e));
            }

            let _ = app.emit(
                "tool-call",
                format!(
                    "⚙️ *Running {}:*\n```{}\n{}\n```\n\n",
                    shell_type, shell_type, scoped_code
                ),
            );
            tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                run_command(shell_type, scoped_code, Some(command_cwd)),
            )
            .await
            .unwrap_or_else(|_| Ok(format!("Command timed out after {} seconds.", timeout_secs)))
            .unwrap_or_else(|e| format!("Error: {}", e))
        }
        "file_actions" => {
            let action = args["action"].as_str().unwrap_or("");
            let path_str = args["path"].as_str().unwrap_or("");
            let root_dir = workspace_dir.clone();
            let app_data_skills_dir = app.state::<crate::AppState>().skills_dir.clone();
            let resolve_read_path = |input: &str, require_exists: bool| {
                resolve_safe_explicit_file(input, protected_exact_files)
                    .or_else(|| {
                        resolve_read_path(
                            &root_dir,
                            &app_data_skills_dir,
                            readable_skill_roots,
                            input,
                            require_exists,
                        )
                        .ok()
                    })
                    .ok_or_else(|| {
                        format!(
                            "Path '{}' was not found under workspace root '{}' or any explicit skill path",
                            input,
                            root_dir.display()
                        )
                    })
            };
            let resolve_write_path = |input: &str, require_exists: bool| {
                resolve_safe_path(&root_dir, input)
                    .map(|path| (path, root_dir.clone()))
                    .map_err(|_| {
                        format!(
                            "Path '{}' must stay under workspace root '{}'",
                            input,
                            root_dir.display()
                        )
                    })
                    .and_then(|(path, resolved_root)| {
                        if require_exists && !path.exists() {
                            return Err(format!(
                                "Path '{}' was not found under workspace root '{}'",
                                input,
                                root_dir.display()
                            ));
                        }
                        ensure_mutation_target_allowed(&path, &root_dir, protected_skill_roots)?;
                        Ok((path, resolved_root))
                    })
            };
            match action {
                "read" => {
                    let _ = app.emit("tool-call", format!("📄 *Reading {}*\n\n", path_str));
                    let start_line = args["start_line"].as_i64();
                    let end_line = args["end_line"].as_i64();
                    match resolve_read_path(path_str, true) {
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
                    let _ = app.emit("tool-call", format!("💾 *Writing {}*\n\n", path_str));
                    match resolve_write_path(path_str, false) {
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
                    let _ = app.emit("tool-call", format!("📂 *Listing {}*\n\n", path_str));
                    match resolve_read_path(path_str, true) {
                        Ok((p, resolved_root)) => {
                            match fs::metadata(&p) {
                                Ok(metadata) => {
                                    if metadata.is_file() {
                                        let name = p
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or(path_str)
                                            .to_string();
                                        return with_root_header(
                                            &root_dir,
                                            &resolved_root,
                                            format!("{} ({} bytes)", name, metadata.len()),
                                        );
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
                                    let body = if res.is_empty() {
                                        "(empty directory)".to_string()
                                    } else {
                                        res.join("\n")
                                    };
                                    with_root_header(&root_dir, &resolved_root, body)
                                }
                                Err(e) => format!("Error listing directory: {}", e),
                            }
                        }
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "search" => {
                    let query = args["query"].as_str().unwrap_or("");
                    let recursive = args["recursive"].as_bool().unwrap_or(true);
                    let case_sensitive = args["case_sensitive"].as_bool().unwrap_or(false);
                    let smart_case = args["smart_case"].as_bool().unwrap_or(false);
                    let use_regex = args["use_regex"].as_bool().unwrap_or(false);
                    let glob = args["glob"].as_str();
                    let include_hidden = args["include_hidden"].as_bool().unwrap_or(true);
                    let respect_gitignore = args["respect_gitignore"].as_bool().unwrap_or(false);

                    if query.is_empty() {
                        return "Error: query is required for search.".to_string();
                    }

                    let _ = app.emit(
                        "tool-call",
                        format!(
                            "🔎 *Searching {} for {}*\n\n",
                            path_str,
                            serde_json::to_string(query).unwrap_or_else(|_| query.to_string())
                        ),
                    );

                    match resolve_read_path(path_str, true) {
                        Ok((p, resolved_root)) => {
                            if !p.exists() {
                                return format!("Error: {} does not exist", path_str);
                            }

                            match run_integrated_search(
                                query,
                                &p,
                                &resolved_root,
                                SearchOptions {
                                    recursive,
                                    case_sensitive,
                                    use_regex,
                                    smart_case,
                                    include_hidden,
                                    respect_gitignore,
                                    glob,
                                },
                            )
                            {
                                Ok(output) => with_root_header(&root_dir, &resolved_root, output),
                                Err(e) => format!("Error: {}", e),
                            }
                        }
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "rename" | "move" => {
                    let new_path = args["new_path"].as_str().unwrap_or("");
                    let _ = app.emit(
                        "tool-call",
                        format!("🔁 *Moving {} -> {}*\n\n", path_str, new_path),
                    );
                    if new_path.is_empty() {
                        return "Error: new_path is required for move/rename.".to_string();
                    }
                    match resolve_write_path(path_str, true) {
                        Ok((src, _)) => match resolve_write_path(new_path, false) {
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
                    let _ = app.emit("tool-call", format!("🩹 *Patching {}*\n\n", path_str));
                    match resolve_write_path(path_str, true) {
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
                        "tool-call",
                        format!("📁 *Creating directory {}*\n\n", path_str),
                    );
                    match resolve_write_path(path_str, false) {
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
                    let _ = app.emit("tool-call", format!("🗑️ *Deleting {}*\n\n", path_str));
                    match resolve_write_path(path_str, true) {
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
                "tool-call",
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
    let decode_hint = match cmd_type.as_str() {
        "direct" => OutputDecodeHint::Direct,
        "bash" | "sh" if cfg!(windows) => OutputDecodeHint::CmdShell,
        "powershell" | "pwsh" => OutputDecodeHint::PowerShell,
        _ => OutputDecodeHint::Default,
    };

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
            #[cfg(windows)]
            let c = {
                let mut cmd = tokio::process::Command::new("cmd.exe");
                let wrapped = wrap_cmd_script_for_utf8(&code);
                cmd.args(["/D", "/S", "/C", &wrapped]);
                cmd
            };

            #[cfg(not(windows))]
            let c = {
                let mut cmd = tokio::process::Command::new("bash");
                cmd.args(["-c", &code]);
                cmd
            };

            c
        }
        "powershell" | "pwsh" => {
            let mut c = tokio::process::Command::new("powershell");
            let wrapped = wrap_powershell_script_for_utf8(&code);
            c.args(["-NoProfile", "-NonInteractive", "-Command", &wrapped]);
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
    Ok(format_process_output(output, decode_hint))
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

        let wrapped_code = wrap_powershell_script_for_utf8(&_code);
        fs::write(&script_path, wrapped_code).map_err(|e| e.to_string())?;

        let script_path_s = script_path.to_string_lossy().to_string();
        let out_path_s = out_path.to_string_lossy().to_string();
        let err_path_s = err_path.to_string_lossy().to_string();
        let cwd_s = cwd.as_ref().map(|dir| dir.to_string_lossy().to_string());

        let launcher = format!(
            "$ErrorActionPreference='Stop';\n\
            $script={script};\n\
            $out={out};\n\
            $err={err};\n\
            $args=@('-NoProfile','-ExecutionPolicy','Bypass','-File',$script);\n\
            $working={working};\n\
            $p=Start-Process -FilePath 'powershell' -ArgumentList $args -WorkingDirectory $working -Verb RunAs -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err;\n\
            exit $p.ExitCode\n",
            script = ps_quote(&script_path_s),
            out = ps_quote(&out_path_s),
            err = ps_quote(&err_path_s),
            working = ps_quote(cwd_s.as_deref().unwrap_or("."))
        );

        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &launcher]);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let output = cmd.output().await.map_err(|e| e.to_string())?;
        let mut res = format_process_output(output, OutputDecodeHint::PowerShell);

        let out_txt = fs::read(&out_path)
            .map(|bytes| decode_process_bytes(&bytes, OutputDecodeHint::PowerShell))
            .unwrap_or_default();
        let err_txt = fs::read(&err_path)
            .map(|bytes| decode_process_bytes(&bytes, OutputDecodeHint::PowerShell))
            .unwrap_or_default();

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
