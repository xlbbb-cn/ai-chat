//! The JSON schema catalogue the model is offered for every tool.

use super::*;

/// Build the tool list advertised to the model.
///
/// `self_evolution` adds the dedicated `evolution` memory scope to the memory
/// tool — it only exists while self-evolution mode is on.
pub fn get_all_tools(
    selected_tools: &[String],
    self_evolution: bool,
    background_tools: bool,
) -> Vec<Value> {
    let mut tools = vec![];

    let want_run_cmd = selected_tools.iter().any(|t| t == "run_cmd");
    let want_run_shell = selected_tools.iter().any(|t| t == "run_shell");

    if want_run_cmd {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "run_cmd",
                "description": "Execute a program or a workspace script with patent-compliant risk assessment. L5/L6 (safe/read-only) run directly; L0-L4 need the user's confirmation with the risk score; syntax errors are rejected with a detailed report. Returns the result or the rejection reason.\n\n`action: \"run\"` (default) runs `command`. The first word is a script name in the library `<workspace>/.data/scripts` (extension optional, e.g. `build-frontend`, `deploy.py`) or a bare program name resolved through PATH (`python`, `git`, `node`); the library is prepended to the child's PATH, so a script wins over a same-named program. Absolute and relative paths are refused — use `run_shell` when a path is really needed.\n\n`action: \"list\"` lists the library with its `@description` / `@usage` header, interpreter and use count, and executes nothing. Scripts are plain files in that folder: write them with `file_actions` and give them `@description` / `@usage` header comments.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["run", "list"],
                            "description": "'run' (default) executes `command`; 'list' returns the workspace script library without running anything."
                        },
                        "command": {
                            "type": "string",
                            "description": "Required for 'run'. Full command line (e.g. 'build-frontend --release', 'python -V', 'curl -s https://example.com'): the first word is a library script name or a bare PATH program, the rest are arguments."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 3600,
                            "description": "Optional timeout in seconds. Defaults to 30."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    if want_run_shell {
        // The OS label and the fixed script language live in the tool schema:
        // the model only needs to know which language to write, never which
        // interpreter to run (that is compiled in with the desktop platform).
        let machine = machine_label();
        let language = RUN_SHELL_LANGUAGE;
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "run_shell",
                "description": format!(
                    "Execute or manage a shell script under the same risk assessment and confirmation rules as `run_cmd`.\n\n\
                     This machine runs {machine}. The shell is fixed for this build: write {language} scripts (there is no shell-type argument to choose).\n\n\
                     The script starts in the working directory (the workspace root for the main chat, the task directory inside a sub-agent mission) but may `cd` and run anything — pipes, loops, redirections, absolute paths. Use it when a PATH-resolved program is not enough.\n\n\
                     `run_type: \"direct\"` (default) runs to completion and returns the output. `run_type: \"serve\"` starts a long-lived child (HTTP service, watcher…) and returns a `serve_id`; then `action: \"list\"` shows the processes and `action: \"stop\"` kills one id (or `serve_id: \"all\"`). Serve processes die with this chat turn and at app exit."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "run_type": {
                            "type": "string",
                            "enum": ["direct", "serve"],
                            "description": "'direct' (default) waits for the script and returns its output; 'serve' starts it as a long-lived background process and returns a serve id immediately."
                        },
                        "action": {
                            "type": "string",
                            "enum": ["start", "stop", "list"],
                            "description": "Only with run_type 'serve': 'start' (default), 'stop' (kills the process named by serve_id), 'list' (known serve processes with recent output)."
                        },
                        "serve_id": {
                            "type": "string",
                            "description": "Serve id returned by a `run_type: \"serve\"` start (required for 'stop'). Use 'all' to stop every serve process."
                        },
                        "code": {
                            "type": "string",
                            "description": "The shell script body. Required for run_type 'direct' and serve 'start'; not needed for 'stop'/'list'."
                        },
                        "sudo": {
                            "type": "boolean",
                            "description": "If true (bash only), run the whole script through sudo. Asks for confirmation and sudo credentials."
                        },
                        "elevated": {
                            "type": "boolean",
                            "description": "If true (PowerShell only), request administrator elevation (UAC). Asks for confirmation."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 3600,
                            "description": "Optional timeout for run_type 'direct', in seconds (default 30). Ignored for serve processes, which run until stopped, the turn ends or the app exits."
                        }
                    }
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "file_actions") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "file_actions",
                "description": "File operations strictly inside the workspace. Use `./...` paths (e.g. `./src/App.tsx`) or `.` for the root; absolute paths inside the workspace work, anything resolving outside is rejected — use `run_shell` only when external access is genuinely required. `.data` is part of the workspace and writable: skills under `./.data/skills/...`, the `run_cmd` script library under `./.data/scripts/...`, memory/todos/status — everything except `.data/chat.db`. Temporary and scratch output belongs in `./.data/tmp/` (the shared workspace tmp area: sub-agent deliverables, the PDF cache and composer pastes live there; every plain file sitting directly in it expires after a day, so long-lived output belongs in a subdirectory). Protected self-evolution targets get an automatic `.bak.<n>` backup before mutation.",
                "description": "File operations strictly inside the workspace. Use `./...` paths (e.g. `./src/App.tsx`) or `.` for the root; absolute paths inside the workspace work, anything resolving outside is rejected — use `run_shell` only when external access is genuinely required. `.data` is part of the workspace and writable: skills under `./.data/skills/...`, the `run_cmd` script library under `./.data/scripts/...`, memory/todos/status — everything except `.data/chat.db`. Temporary and scratch output belongs in `./.data/tmp/` (the shared workspace tmp area: sub-agent deliverables, the PDF cache and composer pastes live there; every plain file sitting directly in it expires after a day, so long-lived output belongs in a subdirectory). An `@path` mention in a user message was already inlined at send time — the `<details><summary>Attached File: … — already provided below` block holds those bytes — so read the file only for the parts that are missing (other lines, other files), never to re-check what the user already sent. Protected self-evolution targets get an automatic `.bak.<n>` backup before mutation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["read", "write", "list", "search", "mkdir", "patch", "diff", "rename", "move", "delete"],
                            "description": "read file content, write/overwrite a file, list a directory, search file contents, create directories recursively, apply a unified diff patch (hunks are located by content — `@@` numbers are hints), diff two files, rename, move, delete."
                        },
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative path, e.g. `./src/App.tsx`, or `.`/`./` for the root when listing or searching. Paths outside the workspace are refused."
                        },
                        "new_path": {
                            "type": "string",
                            "description": "Destination path for 'rename'; for 'diff' the second file to compare `path` against."
                        },
                        "start_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "First line to read (1-based, inclusive; 'read' only)."
                        },
                        "end_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Last line to read (1-based, inclusive; 'read' only)."
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write ('write' only)."
                        },
                        "query": {
                            "type": "string",
                            "description": "Search text or regex pattern ('search' only). No external rg executable is needed."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Recurse into subdirectories when `path` is a directory ('search', default true)."
                        },
                        "case_sensitive": {
                            "type": "boolean",
                            "description": "Case-sensitive content search ('search', default false)."
                        },
                        "smart_case": {
                            "type": "boolean",
                            "description": "Uppercase letters in `query` switch to case-sensitive when case_sensitive is false ('search', default false)."
                        },
                        "use_regex": {
                            "type": "boolean",
                            "description": "Treat `query` as a regular expression instead of plain text ('search', default false)."
                        },
                        "glob": {
                            "type": "string",
                            "description": "Path glob filter for search targets, e.g. '**/*.rs' ('search' only)."
                        },
                        "include_hidden": {
                            "type": "boolean",
                            "description": "Include hidden files and directories in directory searches ('search', default true)."
                        },
                        "respect_gitignore": {
                            "type": "boolean",
                            "description": "Respect .gitignore/.ignore/git-exclude rules ('search', default false). Set true on large trees — skipping node_modules/, target/ etc. is the biggest speedup."
                        },
                        "max_results": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 20000,
                            "description": "Max matching lines to return ('search', default 1000). The parallel search stops early once the budget is spent, so a broad query degrades to a fast prefix."
                        },
                        "patch": {
                            "type": "string",
                            "description": "Unified diff to apply ('patch' only). Hunks are located by CONTENT, not line numbers, so stale numbers are tolerated — but context and changed lines must be real copies of the file.\n\nFormat (both header parts optional):\n@@ -<old_line>,<count> +<new_line>,<count> @@\n <unchanged context line>\n-<removed line>\n+<added line>\n <unchanged context line>\n\nTo apply on the first try: (1) read the file first and copy every context/removed/added line character-for-character — keep the leading space of context lines and the file's exact indentation, never retype or re-wrap; (2) give 3 unchanged context lines above and below each change (more when the text repeats); (3) copy line numbers from the read output when possible — they are hints, content decides; (4) one hunk per separated change, in file order, never merge distant edits. `---`/`+++`/`diff --git` headers are optional and ignored, LF and CRLF both work, `\\ No newline at end of file` is understood. Matching tolerates leading/trailing whitespace only; on failure the error shows the closest actual content — re-read, fix the context and retry, or use action 'write' for large rewrites."
                        },
                        "context_lines": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 20,
                            "description": "Unchanged context lines shown around each change in a 'diff' result (default 3)."
                        },
                        "ignore_whitespace": {
                            "type": "boolean",
                            "description": "Ignore leading/trailing and repeated whitespace inside lines when diffing ('diff', default false)."
                        }
                    },
                    "required": ["action", "path"]
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "memory") {
        let evolution_scope = if self_evolution {
            "\n- `evolution`: file-backed self-evolution area — verified facts that must survive across sessions (default interpreter and runtime environment, correct tool usage, frequent file locations). Search it when a problem needs earlier experience."
        } else {
            ""
        };
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "memory",
                "description": format!("Manage persistent memory in these scopes:\n- `session`: in-memory, cleared at app restart — task context and in-progress notes.\n- `user`: file-backed, cross-session — preferences, patterns, general insights.\n- `repo`: file-backed, repository-scoped — conventions, build commands, project facts.{evolution_scope}\n\n`scope: \"all\"` runs get/list/search over every scope at once (read-only); add/delete need one concrete scope. `user`/`repo` are markdown files in the workspace `.data/memory` (app-managed — never edit them with file tools). Nothing is preloaded into the prompt: you look memory up yourself. Evolution entries are structured (key, title, kind, tags, importance, evidence) and ranked by experience, so a search returns the most useful entries first — keep each to one dense fact. `consolidate` folds a large evolution cluster into a skill archive: `phase: \"plan\"` first, then `phase: \"apply\"` with the full `skill_md` and the same `keys`."),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": if self_evolution {
                                json!(["add", "get", "list", "search", "delete", "consolidate"])
                            } else {
                                json!(["add", "get", "list", "search", "delete"])
                            },
                            "description": "'add' stores or updates an entry, 'get' fetches by key, 'list' lists entries (ranked), 'search' finds by content, 'delete' removes one, 'consolidate' folds a large evolution cluster into a skill archive."
                        },
                        "scope": {
                            "type": "string",
                            "enum": if self_evolution {
                                json!(["session", "user", "repo", "evolution", "all"])
                            } else {
                                json!(["session", "user", "repo", "all"])
                            },
                            "description": if self_evolution {
                                "Memory scope: 'session' is in-memory only (cleared on restart); 'user', 'repo' and 'evolution' are file-backed and persist — 'evolution' holds verified reusable facts; 'all' runs get/list/search over every scope (read-only). Defaults to 'session'."
                            } else {
                                "Memory scope: 'session' is in-memory only (cleared on restart); 'user' and 'repo' are file-backed and persist; 'all' runs get/list/search over every scope (read-only). Defaults to 'session'."
                            }
                        },
                        "key": {
                            "type": "string",
                            "description": "Unique key (required for add/get/delete); a short descriptive slug like 'api-keys'. Re-adding the same key updates the entry and raises its evidence count."
                        },
                        "content": {
                            "type": "string",
                            "description": "The fact (required for 'add'). Markdown is allowed; for evolution keep it to one dense sentence — never a whole conversation or answer."
                        },
                        "title": {
                            "type": "string",
                            "description": "Evolution: one-line title for compact rendering (max ~100 chars)."
                        },
                        "kind": {
                            "type": "string",
                            "enum": ["fact", "recipe", "location", "pitfall"],
                            "description": "Evolution: category of the entry; a cluster is grouped by kind when archived into a skill."
                        },
                        "importance": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 5,
                            "description": "Evolution: 1-5; 5 (or pinned) always survives the prompt budget — reserve it for cross-project environment facts."
                        },
                        "pinned": {
                            "type": "boolean",
                            "description": "Evolution: always inject this entry, regardless of ranking."
                        },
                        "merge": {
                            "type": "boolean",
                            "description": "Evolution: merge into a similar existing entry instead of creating a second file (default true)."
                        },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional tags ('add' only)."
                        },
                        "query": {
                            "type": "string",
                            "description": "Search text (required for 'search'); matches keys and content."
                        },
                        "phase": {
                            "type": "string",
                            "enum": ["plan", "apply", "restore", "list"],
                            "description": "'plan' lists the large clusters (start here); 'apply' writes the skill and archives its keys; 'restore' moves archived entries back; 'list' shows archived skills."
                        },
                        "keys": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "apply: the evolution keys being archived — verbatim from the plan; each must also appear in the skill's `source-memory` front matter."
                        },
                        "skill_md": {
                            "type": "string",
                            "description": "apply: the complete skill document (YAML frontmatter with name, description, source-memory, then the grouped runbook body)."
                        },
                        "skill": {
                            "type": "string",
                            "description": "restore: the skill whose archived entries move back."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    if selected_tools
        .iter()
        .any(|t| t == "todo_list" || t == "todo")
    {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "todos",
                "description": "Manage this session's todo list (the `file_actions` counterpart for plans). Call 'list' first — the list is long-running session state shared with every parallel agent and the UI mirrors every change live. 'add' appends an item, 'update_status' moves one through pending → in_progress → completed/cancelled, 'clear_completed' removes finished items, 'archive' starts a fresh list.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "add", "update_status", "clear_completed", "archive"],
                            "description": "list / add / update one item's status / clear finished items / archive the active list."
                        },
                        "title": {
                            "type": "string",
                            "description": "Short, action-oriented title (required for 'add')."
                        },
                        "description": {
                            "type": "string",
                            "description": "Optional longer description with acceptance criteria ('add' only)."
                        },
                        "todo_id": {
                            "type": "string",
                            "description": "Id to update ('update_status' only) — copy it from a 'list' result, never invent one."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "cancelled"],
                            "description": "New status ('update_status' only)."
                        },
                        "new_title": {
                            "type": "string",
                            "description": "Optional title for the list that follows the archive ('archive' only). Defaults to 'Working plan'."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "timer") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "timer_set",
                "description": "Schedule a one-shot delay timer that resumes this conversation later (see the TIMER policy). Start the slow work first, then arm the timer, tell the user and END YOUR TURN: the app injects `message` into this chat as a new user turn, with the full history.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "delay_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 2592000,
                            "description": "How long to wait before resuming, in seconds (1800 = 30 minutes). Maximum 2592000 (30 days)."
                        },
                        "message": {
                            "type": "string",
                            "description": "Self-contained prompt for the future turn: what to do, what to check, where the output lives — it is the only thing that turn knows besides the history."
                        },
                        "label": {
                            "type": "string",
                            "description": "Short label shown in the timer bar, e.g. 'collect logs'."
                        }
                    },
                    "required": ["delay_seconds", "message"]
                }
            }
        }));
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "timer_list",
                "description": "List pending timers (id, label, fire time, remaining seconds, message) — check before arming a duplicate, or to find a timer id to cancel.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        }));
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "timer_cancel",
                "description": "Cancel a pending timer so it never resumes the conversation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "timer_id": {
                            "type": "string",
                            "description": "Id from timer_set or timer_list."
                        }
                    },
                    "required": ["timer_id"]
                }
            }
        }));
    }

    // ── Background execution ────────────────────────────────────────────────
    // Only the long-running, read-mostly tools accept the flag: a background
    // mutation could race an unrelated edit the model starts while it waits.
    // The control tool is advertised whenever the feature is on — MCP tools and
    // sub-agents can be backgrounded even when none of the local tools is
    // selected.
    if background_tools {
        for tool in tools.iter_mut() {
            let name = tool["function"]["name"].as_str().unwrap_or("");
            if matches!(name, "run_cmd" | "run_shell" | "file_actions") {
                tool["function"]["parameters"]["properties"]["run_in_background"] = json!({
                    "type": "boolean",
                    "description": "Run as a background task: return a task id immediately instead of blocking the turn. The result arrives automatically before your next request and the turn stays open until it does. Use it for slow work (large searches, builds, long commands) you can do something else meanwhile — not for a quick command, and never start one and then finish without `background_task` action `wait`."
                });
            }
        }
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "background_task",
                "description": "Manage background tasks started with `run_in_background`:\n- `list`: running and finished tasks of this session (id, tool, status, elapsed).\n- `wait`: block until the given tasks (or every task) finish, up to `timeout_seconds`; returns the results that became available.\n- `get`: full result of one finished task.\n- `cancel`: stop a task (or all) — the process tree is killed and the result discarded.\n\nA finished task's result is also delivered automatically before your next request, so `wait` is only needed when you cannot proceed without it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "wait", "get", "cancel"],
                            "description": "The management action to perform."
                        },
                        "task_id": {
                            "type": "string",
                            "description": "Single task id (`get` / `cancel`). Use `\"all\"` with `cancel` to stop every task of this session."
                        },
                        "task_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Task ids to wait for (`wait`); omit to wait for every task of this session."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 300,
                            "description": "Maximum wait in seconds (default 60)."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    // Self-evolution mode owns a second capability besides skills and memory:
    // the assistant may build and run its own MCP servers (`mcp_self`). The
    // tool is not part of `selected_tools` — the mode is the switch, exactly
    // like `skill_read` / `use_skill` are injected from the session state. It
    // is filtered back out for sub-agents and the main-agent helper, which must
    // not race on the registry (see `agents::SUB_AGENT_EXCLUDED_TOOLS`).
    if self_evolution {
        tools.push(self::mcp_self::schema());
    }

    tools
}

/// Build the `skill_read` tool definition.
/// This tool is auto-injected when one or more skills are active in the session.
pub fn get_skill_read_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "skill_read",
            "description": "Read, list or search files inside an active skill's directory — supplementary material (templates, configs, data, examples) that accompanies its SKILL.md. Paths are relative to the skill root.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "list", "search"],
                        "description": "read file content, list directory entries, or search file contents."
                    },
                    "skill_name": {
                        "type": "string",
                        "description": "Name of the active skill whose directory to access."
                    },
                    "path": {
                        "type": "string",
                        "description": "Path relative to the skill root; '.' (the default) is the root itself."
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "First line to read (1-based, inclusive)."
                    },
                    "end_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Last line to read (1-based, inclusive)."
                    },
                    "query": {
                        "type": "string",
                        "description": "Search text or regex pattern ('search')."
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Recurse into subdirectories ('search', default true)."
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Case-sensitive search ('search', default false)."
                    },
                    "use_regex": {
                        "type": "boolean",
                        "description": "Treat `query` as a regular expression ('search', default false)."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Path glob filter, e.g. '**/*.md' ('search')."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20000,
                        "description": "Max matching lines to return ('search', default 1000)."
                    }
                },
                "required": ["action", "skill_name"]
            }
        }
    })
}

/// Build the `use_skill` tool definition. Advertised to the main chat (when the
/// session has enabled skills) and to the main-agent helper, so both load skill
/// bodies with the same protocol.
pub fn get_use_skill_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "use_skill",
            "description": "Load detailed instructions for a specific skill. Call this only when the current request clearly requires that skill's domain knowledge or files. Do not load skills speculatively.",
            "parameters": {
                "type": "object",
                "properties": {
                    "skill_name": { "type": "string", "description": "The name of the skill to load details for" }
                },
                "required": ["skill_name"]
            }
        }
    })
}

/// Build the `ask_main_agent` tool definition. Only `llm` sub-agents receive
/// this tool (and only while `config.sub_agent_main_help` is on): it hands a
/// self-contained material request to a helper run that has the main model,
/// conversation history, skills, MCP servers and the full main tool set.
pub fn get_main_agent_help_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "ask_main_agent",
            "description": "Ask the main agent to collect material you cannot obtain yourself (conversation context, skills, enabled MCP servers, browser/network access, or extra tools). Use this immediately when the task needs live web/browser/API data or a tool that is not in your tool list — do not burn rounds scanning local snapshots for a missing data channel. The main agent sees the real conversation history and returns concrete material; you continue your task with the result.",
            "parameters": {
                "type": "object",
                "properties": {
                    "request": {
                        "type": "string",
                        "description": "Exactly what you need the main agent to find or clarify. Be specific (paths, names, questions)."
                    },
                    "context": {
                        "type": "string",
                        "description": "Optional: why you need it and what you already tried, so the main agent does not repeat your work."
                    }
                },
                "required": ["request"]
            }
        }
    })
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

