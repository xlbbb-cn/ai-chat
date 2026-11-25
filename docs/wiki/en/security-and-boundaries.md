Rabbit Chat Studio tools have real system permissions. The goal of the boundary design is not "make the model absolutely safe", but to make **what it can do, what it did, and why it was rejected** visible.

## Threat model

The main risks are not "a malicious model" but:

- misjudged paths deleting data outside the workspace or inside `.data`;
- prompt injection (web pages, files, MCP responses) inducing dangerous commands;
- auto-accept being left on, so dangerous commands run unchecked;
- third-party MCP servers executing code with your permissions;
- API keys, database passwords and business data ending up in logs or exports;
- the assistant rewriting its own instructions in self-evolution mode.

## Path boundaries

The three execution tools have deliberately different scopes:

| Tool | Where it may reach | How it is bounded |
| --- | --- | --- |
| `file_actions` | The workspace root, **including** the hidden `.data` folder | Path sandbox — no escape hatch |
| `run_cmd` | One PATH-resolved program | Name check + risk assessment |
| `run_shell` | Anywhere, including outside the workspace | Risk assessment + confirmation |

### `file_actions`

- can only operate inside the workspace root;
- accepts `./...`, `.`, and absolute paths inside the workspace;
- absolute paths outside the workspace, `../` escapes and rooted paths (`/etc`, `C:\`) are rejected;
- canonicalizes paths and checks symlinks;
- cannot write/delete/rename the workspace root itself;
- **`.data` is part of the workspace** — it is listed, searched, read and written like any other folder, so the assistant can look up and manage its own skills, memories and todo lists;
- the **one exception** is the workspace database (`.data/chat.db` and its `-wal` / `-shm` / `-journal` sidecars): `file_actions` could only truncate or delete it, never rewrite it, so it is rejected — it holds the chat history, the request monitor, the interaction log and the agent missions;
- the one skills root (`./.data/skills`) cannot be deleted as a whole;
- everything under the skills root gets a `.bak.N` backup before it is overwritten or deleted.

### `run_cmd` / `run_shell`

- processes start in the workspace root;
- `run_cmd` runs a **bare program name** that the OS resolves through `PATH` (`python`, `python3`, `git`, …); absolute and relative paths are rejected before the risk assessment runs;
- `run_shell` scripts can `cd` elsewhere; the tool does not prevent it;
- path boundaries for the shell rely on risk assessment and confirmation, not a sandbox;
- use `run_shell` for external paths and expect higher risk levels.

### MCP

- stdio MCP cwd is the workspace, but it is **not a sandbox**; it can access anything your user can;
- remote MCP servers receive the arguments you send;
- MCP tool arguments are treated as opaque; the app does not path-check them.

### CLI / cloud sub-agents

- CLI agents run with your user permissions; `cwd` is sandbox-checked as a workspace-relative directory, but the process itself can still access other paths;
- cloud agents send the task to a remote endpoint; data leaves the machine;
- both require approval on first run by default (`external_agent`).

## Tool permissions

Three layers stack:

1. **Global tool switches** (`config.selected_tools`): which built-in tools the main agent sees;
2. **Skill command allowlist** (`allowed-commands`): `run_cmd` executable names must be in the union of enabled/loaded skills;
3. **Sub-agent tool allowlist** (`allowed_tools`): `llm` sub-agents only get a subset of `file_actions` / `run_cmd` / `run_shell`.

Notes:

- `allowed-commands` is currently enforced only on `run_cmd`; `run_shell` still goes through risk confirmation;
- sub-agents' panel only offers the three execution tools; `timer` is always filtered;
- MCP tools are not controlled by `selected_tools`; only the server enable switch controls them;
- **background execution** (`config.background_tools`) does not add a new permission: it only lets an already-enabled tool run asynchronously, and the same risk assessment, confirmation, path checks and allowlists apply before the task starts;
- **self-evolution mode overrides the first layer**: `file_actions` and `memory` are forced on and cannot be switched off, by the panel, a backup restore or a hand-edited `config.json`.

## Risk levels and confirmations

`run_cmd` / `run_shell` run a static assessment:

| Level | Score | Behavior |
| --- | --- | --- |
| L0 | ≥85 | Must confirm (`dangerous`) |
| L1 | 70–84 | Must confirm (`system_config`) |
| L2 | 55–69 | Must confirm (`user_software`) |
| L3 | 40–54 | Must confirm (`user_data`) |
| L4 | 25–39 | Runs directly and is logged |
| L5 | 10–24 | Runs directly |
| L6 | 0–9 | Runs directly |

The dialog shows the level, score, matched rules and penalty items. Waiting more than **120 seconds** counts as a denial. A script that fails the syntax gate is rejected before it is scored at all.

### Auto-accept risk

**Tools → Auto-accept mode** can skip L0–L3 individually, or everything via `"*"`. When on:

- dangerous commands no longer ask;
- path checks, skill allowlists, risk assessment and logging still happen;
- but nothing stops an 85+ command anymore.

Recommendations:

- keep it off day to day;
- only enable it in an isolated workspace, briefly, for a clear task;
- never on a production machine, in a directory with secrets, or with unrecoverable data;
- turn it off immediately afterwards and inspect the interaction log.

## Skill isolation

The skill root is the folder containing `SKILL.md`:

- `skill_read` can only read inside that folder;
- `../` escapes are rejected;
- paths in the body are relative to the skill root;
- there is exactly **one** skill root per workspace, `<workspace>/.data/skills` — it is computed in a single place in the backend, so a second location cannot be introduced;
- **Self-evolution mode** lets the assistant write, extend and delete skills under that root, and creates `.bak.N` backups for everything it touches.

### Prompt injection

Skills, MCP responses, web pages and file contents can contain malicious instructions. The model may treat them as user requests. Reduce the risk:

- do not give untrusted content high-privilege tools;
- use least-privilege accounts for MCP servers;
- state in the system prompt that external content is not an instruction;
- keep human confirmation on for important operations;
- regularly inspect tool calls in the interaction log.

## Process cleanup

- stdio MCP, serve and CLI sub-agents are registered in `process_guard`;
- app exit and window close kill whole process trees;
- Windows uses a job object, so even an app crash does not leave children behind;
- Unix uses process groups so `kill(-pid)` reaches grandchildren;
- serve processes are killed at the **end of the current chat turn** by design;
- background tool tasks are turn-scoped too: Stop or turn end sets every cancel flag and each runner kills its process tree;
- CLI agent timeout/Stop kills the process tree.

## Secrets and data

- `api_key` in `config.json` is plaintext;
- `auth_token` and `env` in `mcp_servers.json` are plaintext;
- `interaction_log` in `chat.db` may contain full request bodies, file contents and key fragments;
- exported zips contain all of the above.

Recommendations:

- use full-disk encryption;
- do not put the app data directory in Git;
- add `.data/` to the workspace `.gitignore`;
- clear/compact logs periodically (Interaction Monitor);
- inspect zips before sharing;
- rotate leaked keys;
- use dedicated API keys and read-only database accounts in production.

## Recommended security baseline

- [ ] `config.workspace_dir` points at a dedicated project directory, not the whole disk or your home directory.
- [ ] The workspace `.gitignore` contains `.data/`.
- [ ] Only the tools needed for the current task are enabled.
- [ ] `run_shell` is enabled only when pipes/builds are needed.
- [ ] Auto-accept is off; if needed, use the smallest subset of L0–L3.
- [ ] MCP servers come from trusted sources and use least-privilege tokens.
- [ ] CLI/cloud sub-agents keep **Require approval** on.
- [ ] Production secrets are not placed in workspace files.
- [ ] Inspect `interaction_log` and exported zips regularly.

## Known limitations

- `run_shell` cwd is not restricted to the workspace; scripts can `cd` out.
- `file_actions` can write into `.data` (skills, memory, todos, skill status); only the workspace database is protected.
- MCP and CLI/cloud agents are not inside the workspace sandbox.
- `allowed-commands` is enforced only on `run_cmd`.
- Risk assessment is a static rule set, not a runtime sandbox; obfuscated commands may evade the score.
- Auto-accept `"*"` also covers future confirmation kinds.
- Release builds do not write `app-<date>.log`; use the Interaction Monitor and MCP logs.
- `config.json` / `mcp_servers.json` secrets are not stored in the OS keychain.

For stronger isolation, run Rabbit Chat Studio in a VM/container, mount only the project directory, and keep MCP/CLI in the same isolated environment.
