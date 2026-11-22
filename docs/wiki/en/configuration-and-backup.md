Rabbit Chat Studio configuration has two layers:

- **Application-level**: `config.json`, `mcp_servers.json`, `sub_agents.json`, `profiles.json` in the OS app data directory.
- **Workspace-level**: `<workspace>/.data/` with the chat database, skills, memory, todos, timers, skill status, the `run_cmd` script library and the workspace MCP roster.

## App data directory

| Platform | Path |
| --- | --- |
| Windows | `%APPDATA%\com.leonard.aichat\` (Roaming) |
| macOS | `~/Library/Application Support/com.leonard.aichat/` |
| Linux | `~/.local/share/com.leonard.aichat/` |

> Store (MSIX) installs are redirected by Windows to
> `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\Roaming\com.leonard.aichat\`.

| File | Contents |
| --- | --- |
| `config.json` | Model, API, tools, workspace, appearance, log retention, update toggles |
| `mcp_servers.json` | MCP server definitions |
| `sub_agents.json` | Sub-agents + orchestration |
| `profiles.json` | In-app **Profiles** |
| `agents/` | `llm` agent definitions (Markdown) + the `index.yaml` discovery manifest |
| `model_metadata.json` | Merged model catalogue (~2 900 entries, ~1.2 MB); refreshed at most weekly |
| `shadow/` | Primary-instance lock + one forked `config-<timestamp>-<id>/` per extra window (see [Multiple windows](#multiple-windows)) |
| `app-<date>.log` | Debug-build logs, one file per day (release builds do not write them by default) |
| `workspace/` | Default workspace |

## `config.json` fields

### Model and API

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `api_base_url` | string | `https://api.openai.com/v1` | OpenAI-compatible root URL |
| `api_key` | string | empty | Bearer key; stored in plaintext |
| `model` | string | `gpt-4o-mini` | Default model |
| `model_catalog` | string[] | `["gpt-4o-mini"]` | Model picker entries |
| `model_context_lengths` | object | `{}` | Per-model context window, e.g. `{"gpt-4o-mini": 128000}` |
| `model_metadata` | object | `{}` | Capabilities matched from the merged catalogue; local only |
| `model_reasoning_effort` | object | `{}` | Per-model thinking depth: `minimal` / `low` / `medium` / `high` |
| `model_settings` | object | `{}` | Temperature, Top P, completion limit, etc. |
| `ds_format` | boolean | `false` | Fill `reasoning_content` for DeepSeek thinking models (auto-enabled when the model id matches DeepSeek) |
| `system_message` | string | empty | Your base system prompt |
| `conversation_id_header` | string? | empty | Extra header that carries the session id, e.g. `X-Conversation-Id` |
| `custom_headers` | `{name, value}[]` | `[]` | Extra request headers added to every chat request |

### The model catalogue

Fetching models does not talk to one feed. The app downloads **three** and merges them into one entry per *normalised* name:

| Order | Source | Why |
| --- | --- | --- |
| 1 | `models.dev` | Widest coverage and the only feed with exact token limits |
| 2 | OpenRouter | Extra ids and pricing/context hints |
| 3 | `llm-metadata` | The only feed that ships `tags` (`Audio` / `Files` / `Open Weights` / `Deprecated`) |

Rules worth knowing:

- a lower-priority source only fills fields the higher-priority one left empty, and contributes models the others miss;
- a source that fails or no longer parses is skipped — the others still merge; the command only errors when nothing answered **and** no local file exists;
- names are normalised to lowercase and flattened, so a pulled id is matched against the catalogue by direct comparison; the similarity scorer only covers ids the catalogue does not know (the settings row then shows `Matched <name> · N% similarity`);
- the merged result is cached in `<app data>/model_metadata.json` and refreshed at most **weekly**, so a restart, a restart without network, or a slow mirror still serves the catalogue;
- the picker shows a spinner while a fetch runs.

`model_settings` fields:

| Field | Description |
| --- | --- |
| `temperature` | 0–2 |
| `top_p` | 0–1 |
| `reasoning_effort` | Global fallback; `model_reasoning_effort[model]` wins |
| `max_complete_tokens` | Per-reply completion limit (recommended) |
| `max_tokens` | Legacy field; also a fallback for the context window |

### Tools and workspace

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `selected_tools` | string[] | `[]` | Enabled tool ids |
| `self_evolution_mode` | boolean | `false` | Let the assistant evolve its own skills; forces `file_actions` + `memory` on |
| `background_tools` | boolean | `false` | Advertise `run_in_background` on `run_cmd` / `run_shell` / `file_actions` search / MCP tools / `call_subagent` (Settings → Runtime & Debug → Background execution) |
| `parallel_tools` | boolean | `true` | Run the independent tool calls of one round at the same time (Tools sidebar → Parallel tool calls, at the bottom); `false` = strictly one call at a time |
| `max_parallel_tools` | number | `4` | How many calls of one round may run at once, clamped to 1–8; `1` behaves like `parallel_tools: false` |
| `workspace_dir` | string? | empty | Absolute workspace path; empty = default |
| `workspace_history` | string[] | `[]` | Recently used workspaces (newest first, max 10); rendered as a picker in Settings → Workspace |
| `main_agent_max_rounds` | number | `40` | Requests the main agent may spend per turn (Settings → Advanced); `0` = unlimited, clamped to ≤ 1000 |
| `main_agent_max_tool_calls` | number | `120` | Tool calls per turn; `0` = unlimited, clamped to ≤ 5000 |
| `tool_failure_limit` | number | `3` | Consecutive failures before a tool is disabled for the rest of the session; `0` = off |
| `auto_accept_confirm_kinds` | string[] | `[]` | Auto-accept kinds; `"*"` = all |

`selected_tools` values:

```text
file_actions, run_cmd, run_shell, memory, todo_list, timer
```

Alias: `todo`. `todo_list` is exposed to the model as `todos`. Retired ids (for example `knowledge_graph`) are dropped from `selected_tools` on load and on save.

> `self_evolution_mode: true` **forces** `file_actions` and `memory` into `selected_tools`. The app rewrites the selection on every save and on startup, so the Tools panel, a profile, a backup restore or a hand-edited `config.json` cannot switch them off while the mode is on. It also unlocks the `evolution` memory scope and lets the assistant install, edit and delete skills under `<workspace>/.data/skills`.

`auto_accept_confirm_kinds` values:

```text
dangerous, system_config, user_software, user_data, external_agent, *
```

A built-in command at risk level L0–L3 asks before it runs; auto-accepting its kind (or `*`) skips that question for the rest of the session. `external_agent` is not covered by the Tools panel's shortcuts — it is set here.

### Appearance, logs and updates

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `theme` | string? | `auto` | `auto` / `light` / `dark` |
| `palette` | string? | `default` | Colour scheme: `default` / `cream` / `macaron` (马卡龙, pink / white pastels with a rose-pink accent); each ships a light and a dark token set. Older values of `sakura` are migrated to `default` on load |
| `chat_background` | string? | none | Optional chat background photo, stored as an app-data-relative path such as `backgrounds/chat-background-<id>.png`. Set by Settings → Appearance → Chat background, which copies the file into `<app data>/backgrounds/` immediately |
| `single_instance` | boolean | `false` | Release builds only. On: a second launch focuses the running window. Off: every launch becomes a window (see [Multiple windows](#multiple-windows)). Applies on next launch |
| `language` | string? | auto | `en` / `zh-CN` / `zh-TW` |
| `logger_output` | string | `file` | `file` / `println`; debug builds only |
| `log_retention_days` | number | `90` | Request/interaction log retention; `0` = forever |
| `check_updates_on_startup` | boolean | `true` | Check for updates on startup |
| `include_prerelease_updates` | boolean | `false` | Include pre-releases |

### Full example

```json
{
  "api_base_url": "https://api.openai.com/v1",
  "api_key": "sk-...",
  "model": "gpt-4o-mini",
  "model_catalog": ["gpt-4o-mini", "gpt-4.1-mini"],
  "model_context_lengths": {
    "gpt-4o-mini": 128000
  },
  "model_settings": {
    "temperature": 0.7,
    "top_p": 0.95,
    "max_complete_tokens": 4096
  },
  "system_message": "You are a helpful assistant.",
  "selected_tools": ["file_actions", "memory", "todo_list", "timer"],
  "background_tools": true,
  "parallel_tools": true,
  "max_parallel_tools": 4,
  "workspace_dir": "D:/projects/my-repo",
  "main_agent_max_rounds": 40,
  "main_agent_max_tool_calls": 120,
  "tool_failure_limit": 3,
  "theme": "auto",
  "language": "en",
  "log_retention_days": 90,
  "check_updates_on_startup": true,
  "include_prerelease_updates": false
}
```

> Fully quit the app before editing `config.json`; otherwise the Settings panel may overwrite the file with its in-memory values.

## Profiles

**Settings → Workspace & Profiles → Profiles** stores a **capability set**:

| Included | Not included |
| --- | --- |
| `selected_tools` | API URL/key |
| Sub-agent list | Default model |
| Orchestration | System prompt |
| MCP servers | Workspace directory |
| — | Skill enablement |

Good for:

- switching between "read-only analysis", "full coding" and "MCP-enabled workflow";
- preparing different agent/MCP sets for different projects;
- backing up MCP and sub-agent configuration.

Actions:

- enter a name → **Save Current**;
- **Apply** → overwrite the current tools/agents/MCP/orchestration;
- delete → remove from `profiles.json`.

Skill enablement is per workspace (`.data/skills.status`) and is not touched by profiles. This is intentional.

## Full export and restore

Entry points:

- title-bar `⋯` → **Export Profile…** / **Restore Profile…**;
- macOS native File menu;
- commands `app_export_profile` / `app_import_profile`.

Exporting locks the send button and shows progress, so the database is not written during the snapshot.

### Zip contents

```text
config.json             AppConfig
profiles.json           named profiles
mcp_servers.json        MCP servers
sub_agents.json         sub-agents + orchestration
agents/...              `llm` agent definitions (app-wide, Markdown)
backgrounds/...         chat background photo (app-wide)
workspace/memory/...    current workspace memory
workspace/todos/...     current workspace todos
workspace/skills/...    current workspace skills
workspace/scripts/...   run_cmd script library
workspace/mcp/...       self-built MCP server scripts (self-evolution mode)
workspace/mcp.conf      roster of those servers
workspace/deliver/...   sub-agent deliverables, one directory per session
workspace/skills.status skill enablement
chat.db                 SQLite snapshot (VACUUM INTO)
```

The sub-agent deliverable exchange area (`<workspace>/.data/deliver/<session-id>/`) **is** exported; it is pruned when the session is deleted.

`chat.db` is a consistent `VACUUM INTO` snapshot; the app briefly blocks sending while it is created.

### Import behavior

Import:

1. reads and applies `config.json` (which may switch the workspace);
2. writes `sub_agents.json`;
3. writes `profiles.json` (only if it is a real profiles file);
4. **merges `agents/**/*.md`** into the app-global `agents/` store (an identical file is left untouched);
5. restores `backgrounds/` (replacing the folder) so the background path in `config.json` still resolves;
6. restores `workspace/memory`, `workspace/todos`, `workspace/skills`, `workspace/scripts`, `workspace/mcp` and `workspace/mcp.conf` into the **current** workspace's `.data`;
7. restores `workspace/skills.status`.

It does **not** currently restore:

- `chat.db`

The zip contains it, but the import path does not write it back. For history: quit the app and copy `chat.db` to `<workspace>/.data/chat.db`.

The import also writes the app-data `mcp_servers.json`.

> This is the current import behavior; later releases may change it. Verify the import result after upgrading.

### Manual full backup

The most reliable method is to quit the app and copy both locations:

```text
<app data>/                     → config.json, mcp_servers.json, sub_agents.json,
                                  profiles.json, agents/, backgrounds/,
                                  model_metadata.json, shadow/
<workspace>/.data/              → chat.db, memory/, todos/, skills/, skills.status,
                                  timers.json, scripts/, mcp/, mcp.conf
```

Quit first so SQLite has no unflushed WAL.

## Multiple windows

`single_instance` decides whether a second launch survives at all:

- **On**: the second launch hands its arguments to the running instance and the existing window is shown and focused.
- **Off**: every launch becomes a window. The first process is the **primary** — it keeps using the shared `config.json` unchanged. Every later window is a **shadow**:

  - at startup it copies the current config into `<app data>/shadow/config-<timestamp>-<id>/config.json`;
  - `workspace_dir` in that copy is rewritten to a fork under its own `workspace/` — **two windows never share a workspace**, so they never contend for one SQLite database;
  - a shadow writes only its fork; on exit the copy is deleted and only `workspace_history` is merged back into the shared config (the shadow's temporary workspace is filtered out);
  - clearing `workspace_dir` in a shadow resets it to its own fork, never the shared default workspace;
  - a forked window says so in Settings → Workspace; a copy left behind by a crashed window is swept at the next startup.

Settings → Workspace lists the recently used directories (`workspace_history`, newest first, max 10). Picking one only *stages* it — **Save Changes** applies the switch. The workspace name in the titlebar carries a thin red underline, so a window running on a fork is easy to spot.

## Moving to a new machine

1. Old machine: fully quit the app.
2. Old machine: copy the app data directory and the current workspace `.data`.
3. New machine: install the same or a newer version.
4. New machine: launch once to create the default directories.
5. New machine: quit the app.
6. New machine: overwrite the app data files; place `.data` in the new workspace.
7. Start the app and check **Settings → Workspace** points at the right directory.
8. Check model/API key, MCP servers, sub-agents and skill toggles.
9. Open History and confirm `chat.db` was read.

If the workspace path changed, select it again in Settings or edit `config.workspace_dir`.

## Security

- API keys in `config.json` are **plaintext**.
- `auth_token` and `env` in `mcp_servers.json` are plaintext too.
- Do not commit the app data directory to Git.
- Use full-disk encryption on shared machines; protect exported zips.
- Rotate keys periodically; revoke immediately if leaked.
- Do not put production secrets in workspace files; the model can read them.

## FAQ

| Symptom | Check |
| --- | --- |
| Config changes do not apply | Fully quit and restart; did the Settings panel overwrite? |
| MCP servers disappeared after import | Copy `mcp_servers.json` manually or re-export with a current build |
| History disappeared after import | `chat.db` is not restored automatically; copy it manually |
| Skill toggles are wrong after import | Is `skills.status` in the zip? Was it restored into the right workspace? |
| Applying a profile does not change the model | Profiles do not include model/API; they only switch tools, agents, MCP and orchestration |
| The exported zip is large | `chat.db` contains request bodies and logs; compact or lower retention first |
| Profiles have no skills | By design; skills are workspace state |
| Model capabilities are missing | `model_metadata.json` may predate the model; the cache refreshes weekly — fetch models again to force a merge |
