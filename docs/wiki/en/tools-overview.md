Tools are the "what can the model do to the local world" switches. All built-in tools are off by default; they only appear in the model's tool list after you enable them under **Tools** in the top toolbar.

## Tool list

| Panel name | Config id | Tool(s) seen by the model | Purpose | Tutorial |
| --- | --- | --- | --- | --- |
| File Actions | `file_actions` | `file_actions` | Read/write/search/patch/rename/delete files inside the workspace (including `.data`) | [Files and Commands](Files-and-Commands) |
| Run Command | `run_cmd` | `run_cmd` | Run one program or a workspace script directly (no shell); maintains `.data/scripts` | [Files and Commands](Files-and-Commands) |
| Run Script | `run_shell` | `run_shell` | Run PowerShell/Bash scripts, including background serve | [Files and Commands](Files-and-Commands) |
| Calculator | `calculator` | `calculator` | Evaluate expressions: arithmetic, powers, parentheses, factorial, bitwise, functions and constants | — |
| Memory | `memory` | `memory` | Four scopes: session / user / repo / evolution | [Memory, Todos and Timers](Memory-Todos-and-Timers) |
| Todos | `todo_list` (alias `todo`) | `todos` | Session-level plan and progress | [Memory, Todos and Timers](Memory-Todos-and-Timers) |
| Timer | `timer` | `timer_set` / `timer_list` / `timer_cancel` | Resume the chat later | [Memory, Todos and Timers](Memory-Todos-and-Timers) |

The state is stored in `config.selected_tools`; changes apply to the next request without a restart.

## Tool scopes

The three execution tools have deliberately different boundaries:

| Tool | Where it may reach | How it is audited |
| --- | --- | --- |
| `file_actions` | Inside the workspace root only, **including** the hidden `.data` folder. The one exception is the workspace database (`.data/chat.db`), which cannot be modified. | Path sandbox: absolute paths outside the workspace, `../` escapes and symlink escapes are rejected |
| `run_cmd` | The first word resolves against the workspace script library `.data/scripts` (bare name) first, then against `PATH` (`python`, `python3`, `git`, …). Absolute and relative paths are refused. | Risk assessment L0–L6, confirmation for L0–L3, content-based approval for risky scripts, skill `allowed-commands` union |
| `run_shell` | Anywhere: the script starts in the workspace but may `cd` out and run anything. Its shell is fixed at build time and its schema carries the host OS. | Risk assessment L0–L6, confirmation for L0–L3, syntax gate |

See [Files and Commands](Files-and-Commands) and [Security and Boundaries](Security-and-Boundaries).

## Background tool tasks

With **Settings → Run → Background execution** on (`config.background_tools`), slow tools accept `run_in_background: true`:

| Tool | Background form | What happens |
| --- | --- | --- |
| `run_cmd` | any call | The program runs detached through the timed runner (600 s default) |
| `run_shell` | `run_type: "direct"` only | `serve` already has its own lifecycle |
| `file_actions` | `action: "search"` only | The parallel walker polls the task's cancel flag |
| MCP tools | any call | A stdio server gets its own short-lived process, so the shared pool is never blocked |
| `call_subagent` | any call | The sub-agent runs while the main agent keeps working |

How it behaves:

- The call returns immediately with a placeholder tool result carrying a `bg-…` id, so the model can keep working instead of blocking on a slow build, search or MCP round trip.
- The real output is delivered before the model's next request as a `user` message (`[BACKGROUND TASK RESULT …]`) and stored on the assistant row, so it survives a reload and is replayed like a mid-turn update.
- The turn is **held open** if the model tries to finish while a task is unsettled (max 3 rounds × 120 s, then the remainder is cancelled).
- The model manages tasks with `background_task`: `list` (running and finished, with elapsed time), `wait` (block until they finish, up to `timeout_seconds`), `get` (full result) and `cancel` (`task_id` or `"all"`).
- The transcript groups running/waiting tasks and serve processes in a collapsible **Background** card at the end of the reply, with cancel/stop buttons; results go to the model, not the transcript.
- Tasks are **turn-scoped**: Stop or the end of the turn sets every cancel flag and each runner kills its process tree; app exit is covered by the process guard.

Background tasks keep every normal check: risk assessment and confirmation still happen before the task starts, and a cancelled task stops waiting for a confirmation dialog. Sub-agents never receive the flag.

## Parallel tool calls

One assistant round can carry several tool calls. **Tools sidebar → Parallel tool calls** (`config.parallel_tools`, at the bottom of the panel, on by default) decides whether they run at the same time:

- **Concurrent**: `file_actions` `read` / `list` / `search` / `diff`, `run_cmd`, `run_shell`, `calculator`, `skill_read`, `call_subagent` and every MCP tool. A batch costs the slowest call instead of the sum.
- **Alone, in order** (and they split the round into segments): `use_skill` (it changes the allowed commands and skill directories the rest of the round sees), `memory` / `todos` / `timer_*` (read-modify-write on a session file), mutating `file_actions` actions (write / patch / rename / move / mkdir / delete), `background_task`, `mcp_self`, `skill_merge`, `experience`, the mission-board tools, and any tool the app does not know.
- **Calls per round** (`config.max_parallel_tools`, default 4, range 1–8): how many independent calls may run at once. `1` behaves like the switch being off — strictly one call at a time.
- **Sub-agents** follow the same rules, with `mbus` and every MCP call as extra barriers (an MCP call owns that task's `AgentMcpSession`).
- **Order never changes**: tool results, failure counting and the transcript rows always follow the model's order, whatever order the calls finished in. Stop still exits after the current batch returns.
- One behaviour change: when a call trips the failure limit, the calls beside it are already in flight and are not cancelled — the verdict applies to the next round.

## Self-evolution mode forces two tools on

**Settings → Workspace & Profiles → Enable Self-evolution Mode** does not only unlock the skill folder — it also **forces `file_actions` and `memory` on**:

- the two switches in the Tools panel are shown locked (checked, greyed out, marked *Required*);
- the backend enforces the same invariant (`AppConfig::effective_tools()`), so the Tools panel, a profile, a backup restore or a hand-edited `config.json` cannot switch them off;
- `use_skill` is advertised even when no skill is enabled yet, so a skill the assistant just wrote can be loaded in the same turn;
- the model also receives `mcp_self` (see [MCP Servers](MCP-Servers)) and the dedicated `evolution` memory scope.

Turning the mode off leaves the selection alone — the tools simply become optional again.

## Auto-injected tools

These are not controlled by the Tools panel; they depend on context:

| Tool | When it appears |
| --- | --- |
| `use_skill` | The session has enabled skills — or self-evolution mode is on |
| `skill_read` | An enabled/loaded skill directory is accessible |
| `call_subagent` | **Agents → Sub-agent system** is on and at least one sub-agent is enabled (automatic planning is a separate switch) |
| MCP tools | At least one MCP server is enabled under **MCP**; names come from the server |
| `mcp_self` | Self-evolution mode is on — build/register/start the assistant's own MCP servers |
| `todos` | The Todos tool is enabled |
| `timer_*` | The Timer tool is enabled |
| `background_task` | **Settings → Run → Background execution** is on (manages `run_in_background` tasks) |

Sub-agents get the tools their `allowed_tools` list grants, minus `timer` (a chat-session tool that would resume a conversation later). Every `llm` sub-agent starts with the shared `memory` tool (the panel offers it and you can uncheck it per agent): `session` scope is keyed by the main chat's session id and `user` / `repo` are workspace files, so a task can search what the main agent already learned. In self-evolution mode a sub-agent additionally receives `file_actions` and `memory` forced on — it may evolve skills and must be able to remember what it learned. `mcp_self` is never handed to a sub-agent.

## Turn budget and failing tools

**Settings → Advanced** caps a turn (both `0` = unlimited):

| Field | Default | Meaning |
| --- | --- | --- |
| `main_agent_max_rounds` | 40 | Requests per turn |
| `main_agent_max_tool_calls` | 120 | Tool calls per turn |

When a limit is spent the turn does not die mid-flight: a context-budget marker is appended and **one wrap-up request** goes out with the same tool list but tool use forbidden, so the model answers from what it already collected. A tool call in that final round is answered with a notice instead of executing.

| Field | Default | Meaning |
| --- | --- | --- |
| `tool_failure_limit` | 3 | Consecutive failures before a tool is disabled for the rest of the session |

The counter is per session and per tool; one success resets it. A disabled tool stays advertised until the next turn (the prompt prefix, and its cache, stay stable) but is refused with a notice; it reappears in the payload on the next turn or after the limit is changed. The guard is shared between the main agent and its sub-agents, so a tool the chat gave up on does not keep failing in parallel.

## Risk levels L0–L6

`run_cmd` and `run_shell` run a static risk assessment before execution, producing a 0–100 score mapped to seven levels:

| Level | Score | Meaning | Confirmation |
| --- | --- | --- | --- |
| L0 | ≥ 85 | System risk; may damage OS files/boot/services | Yes (`dangerous`) |
| L1 | 70–84 | Modify system config or system software | Yes (`system_config`) |
| L2 | 55–69 | Install/uninstall/modify user applications | Yes (`user_software`) |
| L3 | 40–54 | Modify or delete user data | Yes (`user_data`) |
| L4 | 25–39 | Sensitive read (configs, keys, logs) | No; runs and is logged |
| L5 | 10–24 | General query / read-only status | No |
| L6 | 0–9 | Known safe / read-only | No |

The score is a static heuristic, not a sandbox. The command still runs with your user permissions.

### Confirmation flow

L0–L3 trigger a frontend confirmation:

- The dialog shows the risk level, score, matched rules and penalty items.
- Choose **Allow** or **Deny**.
- Waiting more than **120 seconds** is treated as a denial.
- A denial returns `⛔ Execution rejected by user`; the model sees the reason.
- Some commands show an inline allow/deny row in the tool-call card instead of a dialog.

### Auto-accept

**Tools → Auto-accept mode** can skip confirmations:

| Option | Meaning |
| --- | --- |
| Dangerous commands (L0) | Skip L0 confirmation |
| System config changes (L1) | Skip L1 confirmation |
| User software changes (L2) | Skip L2 confirmation |
| User data changes (L3) | Skip L3 confirmation |
| Auto-accept everything | Store the `"*"` wildcard; skips every current and future confirmation kind |

Stored in `config.auto_accept_confirm_kinds`. `"*"` also covers `external_agent`.

`cli` / `cloud` sub-agent approval is **not** in this panel; it is controlled per agent by **Require approval** in the Agents panel, once per app session.

> Auto-accept only skips the dialog. It does not skip risk assessment, path checks, skill command allowlists or logging.

## Tool output and UI

- Tool calls are grouped into a "Completed N tool calls" card; expand it to see each step's input/output/duration.
- Very large tool results keep the **first 6,000 characters + last 2,000 characters**, with a truncation marker in the middle. The model can re-read a smaller range.
- `todos` changes are mirrored live into a todo card in the assistant message.
- `run_shell` serve processes appear in the transcript's **Background** card with pid, uptime and a stop button. They live only for the chat turn that started them: at turn end the app stops every tree, verifies the exit and logs the result.
- Background tool tasks (`run_in_background`) and serve processes are grouped in the **Background** card at the end of the reply, each with a cancel/stop button; their result reaches the model as a `[BACKGROUND TASK RESULT …]` user message and is not printed into the transcript.
- Every tool call is written to the interaction log; see [Monitoring and Logs](Monitoring-and-Logs).

## Recommended enable order

Start small:

1. **File Actions**: begin read-only and confirm the model understands workspace paths.
2. **Memory + Todos**: let it remember conclusions and maintain a plan.
3. **Run Command**: start with read-only commands such as `git status` or `ls`.
4. **Run Script**: enable when you need pipes, loops or builds.
5. **MCP / Sub-Agents**: as needed.

Do not enable high-privilege tools in a workspace containing production secrets, production databases or unrecoverable data.

## Related pages

- [Files and Commands](Files-and-Commands)
- [Memory, Todos and Timers](Memory-Todos-and-Timers)
- [Security and Boundaries](Security-and-Boundaries)
