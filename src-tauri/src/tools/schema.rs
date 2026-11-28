//! The JSON schema catalogue the model is offered for every tool.

use super::*;

// ─── Background control tool ────────────────────────────────────────────────

/// True when at least one tool in `tools` carries the `run_in_background` flag.
///
/// The flag is added on three paths — the local backgroundable tools below,
/// MCP tools (`mcp::adapt_tool_for_llm`) and sub-agent delegation
/// (`build_turn_tools`) — so the `background_task` control tool is gated on the
/// assembled payload, never on the settings switch alone.
pub(crate) fn advertises_background(tools: &[Value]) -> bool {
    tools.iter().any(|tool| {
        tool["function"]["parameters"]["properties"]
            .get("run_in_background")
            .is_some()
    })
}

/// Drop `background_task` from a payload in which no tool accepts the flag.
///
/// With background execution switched on but only non-backgroundable tools
/// selected (calculator, memory, todos …) the control tool manages nothing —
/// its description would be prompt noise naming a capability that is not
/// offered.
pub(crate) fn prune_unreachable_background_control(tools: &mut Vec<Value>) {
    if advertises_background(tools) {
        return;
    }
    tools.retain(|tool| tool["function"]["name"] != "background_task");
}

/// Build the tool list advertised to the model.
///
/// `self_evolution` adds the dedicated `evolution` memory scope to the memory
/// tool — it only exists while self-evolution mode is on.
///
/// `memory_hint` must reflect whether the request pipeline actually prepends
/// the keyword-ranked memory hint to the newest user message (main chat:
/// `config.memory_hint`; every other caller: `false`). The memory description
/// states the behaviour, so it has to agree with the gating.
pub fn get_all_tools(
    selected_tools: &[String],
    self_evolution: bool,
    background_tools: bool,
    memory_hint: bool,
) -> Vec<Value> {
    let mut tools = vec![];

    let want_run_cmd = selected_tools.iter().any(|t| t == "run_cmd");
    let want_run_shell = selected_tools.iter().any(|t| t == "run_shell");

    // The plugin control tool. The per-plugin functions
    // (`plugin_<id>_<tool>`) are appended by `build_turn_tools`, because they
    // depend on the manifests of the active workspace rather than on the
    // selection alone.
    if selected_tools.iter().any(|t| t == "plugin") {
        tools.push(crate::plugins::tool::tool_definition());
    }

    if want_run_cmd {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "run_cmd",
                "description": "Run a program or workspace script directly, without a shell. The process starts in the workspace root.\n\n`action: \"run\"` (default) takes `command`: its first word is a script name in the library `.data/scripts` (extension optional, e.g. `build-frontend`) or a bare program name resolved through PATH (`python`, `git`, `node`). Absolute and relative paths are refused, and so are shell interpreters (`bash`, `sh`, `cmd`, `powershell`, …) — their arguments would bypass the script audit. Risk tiers: L0-L3 need the user's confirmation with the risk score, L4-L6 run directly; syntax errors are rejected.\n\n`action: \"list\"` shows the library scripts with their `@description` / `@usage` header, interpreter and use count.",
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
                            "description": "Required for 'run'. Full command line (e.g. 'build-frontend --release', 'python -V', 'curl -s https://example.com'): the first word is a library script name or a bare PATH program (never a path and never a shell interpreter such as bash/sh/cmd/powershell), the rest are arguments."
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
                    "Execute or manage a shell script under the same risk and confirmation rules as `run_cmd`.\n\n\
                     This machine runs {machine}. The shell is fixed for this build: write {language} scripts (no shell-type argument). The script starts in the working directory but may `cd` and run anything — pipes, loops, absolute paths — on the app-data environment, so `python` and `uv` answer without a full path.\n\n\
                     It runs headless: interactive elevation (a `sudo` prompt or a UAC dialog) is not available — tell the user to run that step themselves.\n\n\
                     `run_type: \"direct\"` (default) returns the output; `serve` starts a long-lived child and returns a `serve_id` (`action: \"list\"` / `\"stop\"`), which dies with this turn and at app exit.\n"
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
                "description": "File operations strictly inside the workspace: `read` / `write` / `list` / `search` / `mkdir` / `patch` / `diff` / `rename` / `move` / `delete`. Use `./...` paths, or `.` for the root; anything resolving outside is refused. `.data` is writable (skills, scripts, memory, todos) — everything except `.data/chat.db`. Keep scratch in `.data/tmp/`; files the user keeps go in `./<project>/...`.",
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
                            "description": "Respect .gitignore/.ignore/git-exclude rules ('search', default true). Build and dependency directories (node_modules/, target/, dist/, …) are pruned regardless, unless `path` points inside one, so this rarely needs changing; set false to also search files the repository ignores. `.data` stays searchable either way."
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
        // The hint clause must match the actual gating: when the request
        // pipeline does not prepend the hint, the description must not claim
        // that it does (the model would otherwise wait for a hint that never
        // arrives).
        let hint_clause = if memory_hint {
            " A short hint (under 200 characters, ranked by confidence) listing the titles of the \
             memories whose keywords match the newest user message is prepended to that message — \
             it is a pointer, not the record: `search`/`get` the scope to read the whole entry."
        } else {
            ""
        };
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "memory",
                "description": format!("Manage persistent memory:\n- `session`: in-memory, cleared at app restart — task context and in-progress notes.\n- `user`: file-backed, cross-session — preferences, patterns, insights.\n- `repo`: file-backed, repository-scoped — conventions, build commands, project facts.{evolution_scope}\n\n`scope: \"all\"` reads every scope at once; add/delete need one concrete scope. `user`/`repo` are app-managed files in `.data/memory` — never edit them with file tools.{hint_clause} Keep each entry to one dense fact. `consolidate` archives an evolution cluster into a skill: `plan` first, then `apply` with the full `skill_md` and the same `keys`.\n\nTRUST CALIBRATION (evolution): a retrieved fact is a claim, not the truth. When the real result disagrees, `add` the same key with `conflict: true` (plus the corrected `content`) — the entry is rewritten and its confidence drops to 1; a plain re-add raises it by one. Re-verify anything at confidence 2 or below before relying on it."),
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
                            "description": "Unique key (required for add/get/delete); a short descriptive slug like 'api-keys'. Re-adding the same key updates the entry and raises its evidence and confidence (one confirmation each)."
                        },
                        "content": {
                            "type": "string",
                            "description": "The fact (required for 'add'). Markdown is allowed; for evolution keep it to one dense sentence — never a whole conversation or answer. On a conflict, pass the corrected fact here and the entry is rewritten in the same call."
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
                        "confidence": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 5,
                            "description": "Evolution: explicit trust 1-5. Unlike `importance` it OVERWRITES the stored value, so it is the way to lower trust; omit it on a plain re-add to let the confirmation add one point."
                        },
                        "conflict": {
                            "type": "boolean",
                            "description": "Evolution: the real result contradicted this stored fact. Forces `confidence` to 1, counts a contradiction, keeps `note` as the reason and rewrites the body when `content` is given. Only valid for an existing key you retrieved."
                        },
                        "note": {
                            "type": "string",
                            "description": "Evolution: one-line reason for a conflict (max ~200 chars), e.g. 'python3 works here, the stored fact is wrong'."
                        },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional tags ('add' only)."
                        },
                        "query": {
                            "type": "string",
                            "description": "Search text (required for 'search'); a token-level match over keys, titles, tags and content — a key/title hit outranks a tag hit, which outranks a body hit."
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
                "description": "Manage this session's todo list. Call 'list' first. 'add' appends an item, 'update_status' moves one (pending -> in_progress -> completed/cancelled), 'clear_completed' drops finished items, 'archive' starts a fresh list. 'ask' shows a question form in the UI and blocks until the user answers — use it for decisions that need their choice, not for what the conversation already answers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "add", "update_status", "clear_completed", "archive", "ask"],
                            "description": "list / add / update one item's status / clear finished items / archive the active list / ask the user a short question form."
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
                        },
                        "topic": {
                            "type": "string",
                            "description": "Optional header for the question form ('ask' only), e.g. 'Plan review'."
                        },
                        "questions": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {
                                        "type": "string",
                                        "description": "Optional stable id echoed back with the answer; auto-generated when missing or duplicated."
                                    },
                                    "title": {
                                        "type": "string",
                                        "description": "The question itself, shown as the card title."
                                    },
                                    "description": {
                                        "type": "string",
                                        "description": "Optional context shown under the title."
                                    },
                                    "options": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Selectable answers, each a short label; omit for a free-text question."
                                    },
                                    "allow_custom": {
                                        "type": "boolean",
                                        "description": "Also offer a manual input beside the options (default false when options exist, always true when they don't)."
                                    }
                                },
                                "required": ["title"]
                            },
                            "description": "Questions to ask ('ask' only), 1–8 per call. Each item: { title (required), description?, options?, allow_custom? }."
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
                "description": "One-shot delay timer that resumes this conversation later. Start the slow work first, then arm it, tell the user and END YOUR TURN: the message arrives as a new user turn with the full history.",
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
                "description": "List pending timers (id, label, fire time, remaining seconds, message).",
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

    if selected_tools.iter().any(|t| t == "calculator") {
        // The calculator pulls `run_cmd` in as a dependency (AppConfig::
        // enforce_tool_dependencies), so the fallback sentence below can name
        // it unconditionally. `run_shell` is mentioned only when selected, so
        // the model is never pointed at a tool it does not have.
        let shell_fallback = if selected_tools.iter().any(|t| t == "run_shell") {
            " For what the grammar cannot express, run a short python / node snippet with `run_cmd` or `run_shell` instead."
        } else {
            " For what the grammar cannot express, run a short python / node snippet with `run_cmd` instead."
        };
        let description = concat!(
            "Evaluate a mathematical expression and return the numeric result.\n\n",
            "Use this tool for any computation whose result you report: arithmetic, powers, roots, unit conversions, percentages, statistics, date arithmetic, any exact figure. A single obvious mental step may be stated directly. NEVER guess a number — when in doubt, call the tool.\n\n",
            "Operators (lowest to highest): `|` (OR), `^^` / `xor` (XOR), `&` / `and` (AND), `<<` / `>>` (shifts), `+` `-`, `*` `/` `%`, unary `+` `-` `~`, `^` / `**` (exponentiation, right-associative; `^-2` and `^0.5` work), postfix `!` (factorial, max 20!). Parentheses nest; implicit multiplication works (`2(3+4)`, `2pi`).\n\n",
            "Numbers: decimal and scientific, `0x` / `0b` / `0o`, `_` separators. Constants: `pi`, `e`, `tau`. Functions: `sqrt abs exp ln log log10 log2 sin cos tan asin acos atan atan2 sinh cosh tanh floor ceil round trunc sign min max pow hypot fact factorial not and or xor shl shr`.\n\n",
            "`^` is exponentiation — XOR is `^^` / `xor(...)`. Bitwise operators, shifts and factorial require integers and return exact 64-bit values; the rest is f64. Invalid input returns an `Error:` message with the position."
        );
        let description = format!("{description}{shell_fallback}");
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "calculator",
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "The expression to evaluate, e.g. `(2+3)^2/4`, `0xFF & 0b1111`, `sqrt(2)*10`, `5!`."
                        },
                        "precision": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 15,
                            "description": "Optional number of decimal places to round the displayed result to (0-15). Omit for full precision."
                        }
                    },
                    "required": ["expression"]
                }
            }
        }));
    }

    // ── Background execution ────────────────────────────────────────────────
    // Only the long-running, read-mostly tools accept the flag: a background
    // mutation could race an unrelated edit the model starts while it waits.
    // The control tool is pushed whenever the feature is on — MCP tools and
    // sub-agents can be backgrounded even when none of the local tools is
    // selected, and those two flags are added after this function returns.
    // `prune_unreachable_background_control` then drops it again if no tool in
    // the finished payload carries the flag.
    if background_tools {
        for tool in tools.iter_mut() {
            let name = tool["function"]["name"].as_str().unwrap_or("");
            if matches!(name, "run_cmd" | "run_shell" | "file_actions") {
                tool["function"]["parameters"]["properties"]["run_in_background"] = json!({
                    "type": "boolean",
                    "description": "Run as a background task: return a task id immediately instead of blocking the turn. The result is delivered automatically before your next request (the turn is held open until it arrives), so there is nothing to clean up. Use it for slow work (large searches, builds, long commands) you can do something else meanwhile — not for a quick command. Use `background_task` action `wait` only when you cannot continue without the result right away."
                });
            }
        }
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "background_task",
                "description": "Manage background tasks started with `run_in_background`: `list` (running and finished, with elapsed), `wait` (block up to `timeout_seconds`), `get` (one finished result), `cancel` (kill the tree). A finished result is delivered automatically before your next request, so `wait` is only needed when you cannot proceed.",
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

    // Global experience packages are only reachable while self-evolution mode
    // is on, with exactly this one tool. They are deliberately *not* skills:
    // `use_skill`, the workspace skills root, the script library and the MCP
    // roster never see them.
    if self_evolution {
        tools.push(self::experience::tool_definition());
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
            "description": "Read, list or search files inside an active skill's directory — templates, configs, data, examples that accompany its SKILL.md. Paths are relative to the skill root.",
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

pub fn get_agent_task_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "add_task",
                "description": "Add a task to the current autonomous mission — mission state, not the user-visible todo list.",
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

