Rabbit Chat Studio records model requests and tool calls locally, so you can answer "why did the model say that" and "what did the tool actually do". There are three surfaces: the Interaction Monitor, the Agent Mission Monitor and diagnostic logs.

## Interaction Monitor

Open **Settings → Debug → Interaction Monitor**.

> The entry appears when a chat session exists; it queries the current `session_id`.

### UI

The panel follows the light/dark theme and the active palette like every other surface. The left side is the interaction list; the right side shows details, and the bottom third is a **session report** over the current session.

List items show:

- interaction type badge;
- actor (model name, tool name, MCP server);
- action name;
- timestamp;
- error message (if any);
- output preview.

Details show:

| Field | Description |
| --- | --- |
| Type | `llm_api` / `llm_error` / `tool_output` / `mcp_call` / `mcp_response`, etc. |
| Actor | Model or tool |
| Action | e.g. `chat_completion`, `memory`, `read_file` |
| Timestamp | Local time |
| Duration | Milliseconds |
| Error | Failure reason |
| Input | Request body / tool arguments, JSON-formatted |
| Output | Response / tool result |
| Metadata | Extra information |

The **session report** aggregates the current session in SQL (`get_session_stats` over `api_requests` + `interaction_log`): total tokens, cache hit/miss with the weighted hit rate, the share of the context window in use, request count, thinking time per round, tool traffic and the busiest tools. The context window is looked up from the newest request's model and shows a dash when unknown; a round without thinking stores `NULL` for its reasoning time, so averages are not diluted by fake 0 ms rounds.

Top controls:

- **Auto-refresh**: every 2 seconds;
- **Clear logs**: delete **all** rows from `api_requests` and `interaction_log` (not filtered by session); chat history is untouched;
- **Compact database**: apply the retention window and `VACUUM` to reclaim disk space.

### What is logged

| Type | Trigger |
| --- | --- |
| `llm_api` | Each successful Chat Completions request; full request body, response content, duration |
| `llm_error` | Failed request; error and duration (request body is not duplicated) |
| `tool_output` | Built-in tool result |
| `mcp_call` | MCP tool invocation |
| `mcp_response` | MCP tool result |

`api_requests` also stores a metrics row: model, finish_reason, prompt/completion tokens, duration, reasoning time (`reasoning_ms`, the first delta after streamed reasoning) and time to first token (`first_token_ms`), plus error. The Interaction Monitor mainly displays `interaction_log`; a log row can read "· thinking 3.6s" from those columns.

### Retention

**Settings → Debug → Log retention days**:

- default 90 days;
- `0` = keep forever;
- on startup, rows older than the window are deleted from `api_requests` / `interaction_log`;
- **Compact database** applies the policy again and runs `VACUUM`;
- **chat history `history` is never pruned automatically**.

### Privacy

`llm_api` input includes the full request body: system prompt, conversation, attachment text and tool results may all be there. Request bodies are the main driver of `chat.db` growth and may contain secrets or business data. Compact/clear logs before sharing `chat.db` or an export.

## Agent Mission Monitor

Open **Settings → Debug → Agent Missions Monitor**.

It shows the current session's mission records:

| Field | Description |
| --- | --- |
| Mission | Root task description |
| Agent | Agent that executed it |
| Status | `running` / `completed` / `stopped`, etc. |
| Created/updated | Database timestamps |
| Context | Root task context |
| Active tasks | Each task's id, name, description, status |
| Deliverables | Files the finished mission left in the exchange area |
| Episodic summary | Summary saved during execution |
| Final report | Report written by `mark_mission_accomplished` |

Refreshes every 2 seconds, can be refreshed manually, and has a **Deliverables folder** button that opens `<workspace>/.data/deliver/<session-id>` — the sub-agent deliverable exchange area — for manual cleanup.

Tables:

- `agent_missions`
- `agent_tasks`

See [Sub-Agents](Sub-Agents).

## MCP diagnostic logs

**MCP** panel → **Logs** on a server:

- in-memory ring buffer; cleared on restart;
- records save, start, `initialize`, `tools/list`, `tools/call` and errors;
- refreshes every 1.5 seconds;
- can be refreshed or cleared;
- testing opens the log automatically.

This is the first place to look when MCP fails: `npx` download, PATH, auth and protocol handshake errors all appear here.

## `app-<date>.log`

Location (`app-<YYYY-MM-DD>.log`, one file per local day):

| Platform | Path |
| --- | --- |
| Windows | `%APPDATA%\com.leonard.aichat\app-<date>.log` |
| macOS | `~/Library/Application Support/com.leonard.aichat/app-<date>.log` |
| Linux | `~/.local/share/com.leonard.aichat/app-<date>.log` |

Notes:

- Under the Microsoft Store (MSIX) build, Windows redirects this path to `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\Roaming\com.leonard.aichat\app-<date>.log`.
- Only **debug builds** (`cargo` / `tauri dev`) write it; release builds disable it by default.
- **Settings → Debug → Log output** selects file vs terminal (`println`); debug builds only.
- It contains startup, config apply, workspace switch, MCP start, auto-accept, context compression, goal-loop continuations and request success/failure.
- Every line carries a process tag (`[primary-<pid>]` / `[shadow-<pid>]`), so several app windows (multi-instance) writing the same file stay attributable.
- The date is resolved per log line, so a window running across midnight starts the next day's file automatically.
- Old files are swept at startup by the same **log retention days** setting that prunes the database log rows (`0` keeps them forever). The legacy `app.log` is no longer written and is left in place.

## Debugging workflows

### The model's answer is wrong

1. Interaction Monitor → latest `llm_api`;
2. inspect the system message and history in **Input**: system prompt, skills, todos;
3. check finish_reason, tokens and duration;
4. for `llm_error`, read the error and HTTP status.

### A tool did not run

1. Is there a `tool_output` entry?
2. None: the model did not call a tool. Check the tool toggle and whether the model supports tools.
3. Present but wrong: inspect the tool input/output, paths, arguments and permissions.
4. Stuck: is a confirmation dialog waiting? L0–L3 commands time out after 120 seconds.

### MCP tool unavailable

1. MCP panel → Logs;
2. check whether `initialize` and `tools/list` succeeded;
3. check command/PATH/URL/token;
4. run **Test connection**.

### Sub-agent failed

1. Agent Mission Monitor: mission status and task errors;
2. Interaction Monitor: the sub-agent's tool calls;
3. CLI: process output and timeout; cloud: HTTP error and polling status.

## FAQ

| Symptom | Explanation |
| --- | --- |
| Monitor is empty | No interactions in this session, or logs were cleared |
| Disk usage did not shrink after clearing | Deleting rows does not shrink the file; run **Compact database** |
| Compaction is slow | `VACUUM` on a large database takes time; do not click repeatedly |
| Release build has no `app-<date>.log` | By design; use the Interaction Monitor and MCP logs |
| Logs contain secrets | Request bodies include prompts and tool results; clean before exporting/sharing |
| Mission monitor has no missions | No sub-agent ran in this session, or orchestration fell back |
