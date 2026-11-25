Sub-agents split work out: the main agent understands the request, plans and writes the final answer; sub-agents run tools, read code, drive a CLI or call a remote endpoint.

Rabbit Chat Studio supports three runtimes:

| Type | Where it runs | Good for |
| --- | --- | --- |
| `llm` | In-app, with its own tool loop | Reasoning, research, summarizing, controlled tool use |
| `cli` | A headless coding CLI installed on this machine | Large code changes, refactors, running tests |
| `cloud` | A remote HTTP endpoint | Existing remote agent services, isolated execution |

## Definitions on disk

An `llm` agent's identity and prompt live in a Markdown file, not in `sub_agents.json`:

```text
<app data>/agents/
  <category>/<slug>.md   # imported/library definitions (read-only in the panel)
  custom/<slug>.md       # agents created from the panel
  index.yaml             # discovery manifest, replaced on Refresh
```

- The file is YAML frontmatter (`name`, `description`, `emoji`, `color`) plus the system prompt as the body — the same shape as a skill.
- `sub_agents.json` keeps only the runtime knobs (`enabled`, `model`, token limits, `allowed_tools`, `allowed_skills`, `max_iterations`, `enable_mcp`) and a `file` reference. `cli` / `cloud` agents keep their full definition there.
- **Discovery is explicit**: opening the panel reads `agents/index.yaml`; only **Refresh** rescans the tree and replaces the manifest. A file added after the last refresh stays invisible until the next one.
- A discovered file with no config entry is listed as a disabled *virtual* agent (`configured: false`) and is not written to `sub_agents.json` until you enable or edit it.
- Only `agents/custom/**` can be deleted from the panel; library files are read-only there — disable the agent instead.
- A panel save rewrites the file only when the parsed definition actually changed, and keeps a `.bak.<n>` copy first.
- Legacy inline `llm` prompts (old `sub_agents.json` or an old profile) are migrated into Markdown files automatically.

## Panel and orchestration

Click **Agents** in the top toolbar. The orchestration settings (automatic planning, agent selection, execution mode, concurrency, messages per pair) are folded behind the panel's section toggle, so the agent list owns the space by default: only the **Sub-agent system** master switch stays visible when collapsed.

### Sub-agent system (`use_agents`)

The master switch: it decides whether the system exists at all.

- **Off**: the main agent answers directly; `call_subagent` is neither advertised nor callable.
- **On**: `call_subagent` is advertised, so the main agent can delegate by hand at any point; whether it plans *automatically* is the next switch.

### Automatic planning (`auto_plan`, on by default)

- **Off**: no planning pass; the main agent answers directly. `call_subagent` stays available for the model to delegate on its own initiative.
- **On**: as long as at least one sub-agent is enabled, **every user message** first goes through orchestration:
  1. the planner uses the main model to split the request into tasks;
  2. each task is assigned to a sub-agent;
  3. tasks run in parallel or sequentially;
  4. the mission report is injected into the main agent's context;
  5. the main agent writes the final answer.

  If orchestration fails (no enabled agents, planning failure), it logs a warning and falls back to a direct answer.

  During execution, the chat stream shows plan start, task start/stream/done/error, and aggregation events.

  **Delegation gate.** The selection call only proposes a roster; the planner request itself answers "delegate?" once more. An explicit *no* skips the mission — the selection is reported as declined and the main agent answers alone. The roster chip is emitted only after the planner confirms, so a declined plan never leaves a stale roster behind. A single-task plan is valid.

  **Live task rows.** Every task row owns its status: while running it shows a live thinking counter and a tool-activity line, a *stopped* task renders neutral instead of completed, and a provider error surfaces on the task row only — never as a chat error that would close your turn.

> With automatic planning on, every message pays planning overhead. Turn it off for simple Q&A (manual delegation still works), or keep only one lightweight agent.

### Agent selection (`auto_configure`)

When on, one model call picks the sub-agents that take part in this turn before the planner runs. Useful when task types vary widely; it costs one extra LLM call. When off, every enabled sub-agent takes part and the planner decides. The switch only applies while **automatic planning** is on.

### Execution mode

| Mode | Behavior |
| --- | --- |
| Parallel | Tasks run concurrently, limited by **Max concurrency** |
| Sequential | Tasks run one by one in plan order |

The planner can also mark a particular plan as sequential; the plan wins.

**Max concurrency** is 1–10, default 3.

### Agent list

Each agent shows:

- enable toggle;
- name and type badge (LLM / CLI / Cloud);
- status (running / done / error);
- target (model, command or endpoint);
- latest summary / error / token usage;
- edit and delete buttons.

The list can be **filtered by enablement** — All / Enabled / Disabled — combined with the search box.

The number next to **Agents** in the toolbar is the number of enabled agents.

### Mission bus and planner takeover

Every planner-dispatched mission creates an in-process pub/sub bus (`mbus`): the planner and the plan's tasks are peers, `broadcast` reaches everyone, `+`/`#` topic subscriptions are supported, and the bus keeps a live status registry for the mission. Messages reach a sub-agent at its next request boundary, never mid-stream.

- **Per-pair cap** — panel setting **Messages per agent pair** (default 4, `0` = off). Both directions count together; a topic publish counts each recipient separately; the planner is exempt. When two agents hit the cap, that last message is still delivered, the planner is notified once, and later messages between the pair are rejected with a "planner has been notified" note.
- **Planner takeover** — the planner can steer a running task with two bus actions:
  - `direct` (target + objective + optional done-condition) stores an authoritative objective on that task; it is re-rendered into the task's prompt every round, so it survives context compression;
  - `end_task` (target + optional note) asks the task to wrap up; at its next reasoning boundary the task reports its current result as a success instead of spending another request. `cli` / `cloud` tasks cannot poll the bus, so `end_task` rejects them.
- Every planner decision appears as a row in the chat's agent feed — `schedule`, `send`, `status`, `history`, `subscribe`, `finish`, plus "directive → target" and "ended task" markers.

## Create an LLM sub-agent

1. **Agents** → **+ New Agent**.
2. Type: `LLM`.
3. Name and description. The description appears in the main agent's available-agent list; state the responsibility clearly.
4. System prompt: role, output format, boundaries.
5. Model (optional): leave empty to inherit the main model; fill in to override.
6. Max completion tokens and temperature (optional).
7. Max iterations: default 10; `0` means unlimited (the external task state decides when to stop).
8. Allowed tools: check a subset of `file_actions` / `run_cmd` / `run_shell`; **All** / **None** shortcuts are available.
9. Save and enable.

The panel only offers those three execution tools; `timer` is always filtered. Every `llm` sub-agent also starts with the **`memory` tool** and the memory scopes are **shared with the main agent**: `session` is keyed by the main chat's `session_id` (main agent + every sibling task), `user` / `repo` live in `<workspace>/.data/memory`, and `evolution` exists in self-evolution mode. The sub-agent prompt tells it to search memory before starting and record one concise conclusion before finishing; it never runs `consolidate`. `cli` / `cloud` agents cannot call app tools, so they receive no memory digest. In **self-evolution mode** a sub-agent additionally receives `file_actions`, because it may evolve skills.

A task's static prompt also carries a **conversation summary** (`MAIN CONVERSATION`, at most 12 entries / 12 KB): the delegating `description` and `context` can be thin, so the summary lets the task read what the user actually asked. It is background only — the task description stays authoritative.

> `sub_agents.json` also has an `allowed_skills` field, but the panel does not expose it and the run path does not filter skills by it; skills are loaded by the main agent.

## Create a CLI sub-agent

A CLI agent launches a headless coding CLI installed on this machine, passes the task as a prompt and streams its output.

### Required

| Field | Description |
| --- | --- |
| Command | Executable name (resolved through PATH) or full path; Windows `.cmd` / `.bat` / `.ps1` are handled |
| Arguments | One per line; supports `{prompt}`, `{model}`, `{cwd}`, `{agent_name}` |
| Prompt delivery | **As argument (`{prompt}`)** or **Via stdin** |

### Optional

| Field | Description |
| --- | --- |
| Model arguments | One per line; appended on every run — write the model id directly (e.g. `-m` and the model on separate lines). Nothing is inherited from the main config |
| Environment variables | One `KEY=VALUE` per line; defaults include `NO_COLOR=1`, `CLICOLOR=0`, `FORCE_COLOR=0`, `TERM=dumb`; same-name entries override |
| Working directory | Workspace-relative directory; empty = workspace root; sandbox-checked |
| Default timeout | Seconds, default 600; **0 = no timeout** |
| Require approval | Ask before the first run; when off, the enable switch is standing authorization |

Every run starts a fresh CLI session — no session state is carried across calls, and a non-zero exit or timeout closes the mission as `failed`.

### Built-in headless invocations

If arguments are empty, known CLIs get their headless form automatically:

| Command | Default arguments |
| --- | --- |
| `opencode` | `run {prompt}` |
| `claude` | `-p {prompt}` |
| `codex` | `exec {prompt}` |
| `gemini` | `-p {prompt}` |
| `qwen` | `-p {prompt}` |
| `crush` | `run {prompt}` |
| `droid` | `exec {prompt}` |
| `cursor-agent` | `-p {prompt}` |
| `copilot` | `-p {prompt}` |
| `amp` | `-x {prompt}` |

Explicit arguments always win; the defaults are only applied when the list is empty.

### Example: opencode

```text
Command: opencode
Arguments:
run
{prompt}
Prompt delivery: As argument
Working directory: (empty)
Timeout: 900
```

Equivalent to:

```bash
opencode run "<task>"
```

### Test

In the editor, click **Test**:

- CLI: runs `<command> --version` and shows the resolved path, exit code and output;
- Cloud: sends a GET to the endpoint and shows reachability, HTTP status and details.

### Runtime behavior

- The child cwd is the workspace (or the configured workspace-relative `cwd`), and `PWD` is synced;
- output streams to the frontend and ANSI color codes are stripped;
- timeout, Stop and app exit kill the whole process tree (Unix process group / Windows job);
- a non-zero exit or a timeout marks the task `failed` (it never shows a green "done" badge);
- no console window on Windows;
- if the prompt may contain text that `cmd.exe` would expand (e.g. `%VAR%`), prefer `stdin`.

## Create a cloud sub-agent

### Request

| Field | Description |
| --- | --- |
| Endpoint URL | Receives the POST |
| Token | Optional; sent as `Authorization: Bearer <token>` |
| Extra headers | One `KEY=VALUE` per line |
| Body template | JSON; `{prompt}`, `{agent_name}`, `{cwd}` are replaced and JSON-escaped |
| Result path | Dot path to the text in the response, e.g. `choices.0.message.content` or `result.output` |
| Timeout | Seconds, default 1800 |

Default body:

```json
{"messages":[{"role":"user","content":"{prompt}"}]}
```

Default result path:

```text
choices.0.message.content
```

### Async polling (optional)

If the remote side is "submit job → poll result":

| Field | Description |
| --- | --- |
| Poll URL | e.g. `https://api.example.com/jobs/{id}`; `{id}` is replaced |
| Job id path | Dot path in the first response, default `id` |
| Status path | Dot path in the poll response, default `status` |
| Done values | One per line; defaults `completed` / `succeeded` / `success` / `done` / `finished` |
| Failed values | One per line; defaults `failed` / `error` / `cancelled` / `canceled` |
| Poll interval | Seconds, default 5 |

An empty poll URL means a single response.

## `call_subagent`

When orchestration is on, the main agent also gets `call_subagent`:

```json
{
  "agent_id": "exact agent id",
  "task_description": "self-contained task description",
  "context": "optional extra context",
  "run_in_background": false
}
```

Rules:

- Sub-agents **do not see the current conversation**; `task_description` must be self-contained.
- `context` is appended to the task description; use it for file paths, findings and constraints.
- The main agent waits for the result.
- With **Settings → Run → Background execution** on, `run_in_background: true` returns a `bg-…` id immediately: the sub-agent keeps running, the main agent continues, and the report is delivered before its next request (the turn stays open until then). Sub-agents themselves never receive the flag.
- Every call (and every planner mission) gets a **deliverable exchange directory**: `<workspace>/.data/deliver/<session-id>/<batch-id>/` with a `shared/` folder plus one folder per task. The sub-agent's prompt names its own and the shared folder; large outputs should be written there instead of being squeezed into the answer, and a dependent task is told to read `<exchange-root>/<task-id>/<file>`. The mission report and the `call_subagent` result list the produced files with sizes, so the main agent can open them with `file_actions`.
- The exchange area is **permanent** — nothing cleans it automatically. Open it via the title-bar **`⋯` menu → Open deliverables folder**, the button next to the mission monitor's refresh, or the mission panel's deliverables list, which reveals the files a finished mission left behind. It is part of a backup and is deleted together with its session. `cloud` agents cannot reach the local workspace and return deliverables inline; `llm` and `cli` agents use the folders.
- The final answer is still written by the main agent; sub-agents never reply to the user directly.

## Mission state and monitor

Each sub-agent task has a mission record in `<workspace>/.data/chat.db` (`agent_missions` / `agent_tasks`).

Sub-agents can use the task tools:

- `add_task`
- `update_task_status`
- `get_active_tasks`
- `mark_mission_accomplished`

Open **Settings → Debug → Agent Missions Monitor** to inspect:

- mission status, root task description, context;
- active tasks and their statuses;
- the deliverables the mission produced;
- episodic summary;
- final report;
- created/updated timestamps.

## Stop and failure

- Pressing **Stop** during generation:

  - planning phase: cancellation is checked between steps; no new sub-agents are launched;
  - execution phase: CLI process trees are killed; cloud polling checks the cancel flag between polls and stops waiting; remaining tasks are marked as skipped;
  - mission status becomes `stopped`, not `completed`;
  - the main agent does not continue to the final answer.

- A single sub-agent failure does not crash the mission; the error is recorded on that task and others continue.
- If max iterations is reached without `mark_mission_accomplished`, the mission closes as non-completed.

## Approval

`cli` and `cloud` agents default to **Require approval**:

- each agent asks once per **app run** before its first execution;
- after allowing, it does not ask again in that run;
- turning it off makes the enable switch the standing authorization;
- this is the `external_agent` confirmation kind and is **not** covered by **Tools → Auto-accept**, which only covers the L0–L3 built-in command confirmations.

External agents run outside the app: CLI agents run with your user permissions, cloud agents send the task to a remote endpoint. Only enable agents you trust.

## Best practices

- Write descriptions that state "what it owns, what it outputs, what it must not do".
- Grant the smallest LLM tool set; do not give `run_shell` to a read-only task.
- Set CLI timeouts per task; large refactors may need 1800–3600 seconds.
- Validate a cloud agent's result path with a test or `curl` first.
- Turn orchestration off for simple Q&A; enable it for complex multi-file work.
- Parallel tasks must not write the same files, or they will overwrite each other.

## FAQ

| Symptom | Check |
| --- | --- |
| Orchestration is on but the answer is direct | Any enabled agents? Did orchestration fail? Check logs/mission monitor |
| CLI test fails | Is the command in PATH? Does Windows need a shell wrapper? Read `--version` output |
| CLI output is empty | Are arguments passing the prompt? Try `stdin`; check timeout and exit code |
| Cloud 401/403 | Token and headers; other auth requirements |
| Cloud result is empty | Does `result_path` match the response JSON? |
| Polling never finishes | `poll_status_path`, done values and poll URL |
| Sub-agent cannot see files | cwd, permissions, tool allowlist |
| A new `agents/*.md` file is missing from the panel | Press **Refresh**; the panel reads `agents/index.yaml`, not the folder |
| A discovered agent is greyed out | It has no entry in `sub_agents.json` yet: enable or edit it once |
| Cannot delete a library agent | Only `agents/custom/**` is deletable; disable the agent instead |
| `run_in_background` is not offered | Background execution is off in Settings → Run |
| Processes remain after Stop | Normally trees are killed; check Task Manager and logs |
| Mission monitor is empty | No sub-agent ran in this session; did the workspace DB switch? |
