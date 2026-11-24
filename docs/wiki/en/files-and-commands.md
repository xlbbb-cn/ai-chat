These three tools operate at different levels:

| Tool | Good for | Not for |
| --- | --- | --- |
| `file_actions` | Read, write, search, patch, rename, delete workspace files | Access outside the workspace; running commands |
| `run_cmd` | Run one PATH-resolved program or a workspace script, e.g. `git status`, `npm test` | Pipes, redirection, `&&`, variable expansion, absolute paths |
| `run_shell` | PowerShell/Bash scripts, pipes, loops, builds, background serve | Bypassing risk assessment |

All three start in the **workspace root**. `run_shell` scripts can `cd` elsewhere themselves, but path operations are still subject to risk assessment; `file_actions` has no "outside the workspace" escape hatch.

## The three scopes at a glance

| Tool | Where it may reach | Audit |
| --- | --- | --- |
| `file_actions` | The workspace root, **including** the hidden `.data` folder (skills, memory, todos, skill status, scripts, MCP rosters). The workspace database `.data/chat.db` is the single exception and cannot be modified. | Path sandbox — no escape hatch |
| `run_cmd` | One program the OS resolves through `PATH` — a bare name like `python`, `python3`, `git` — or a script from the workspace script library. Absolute and relative paths are **rejected**. | Risk L0–L6 + confirmation + skill allowlist |
| `run_shell` | Anywhere, including outside the workspace | Risk L0–L6 + confirmation + syntax gate |

## `file_actions`

### Actions

| Action | Required | Description |
| --- | --- | --- |
| `read` | `path` | Read a file; optional `start_line` / `end_line` (1-based, inclusive) |
| `write` | `path`, `content` | Overwrite; creates parent directories |
| `list` | `path` | List a directory (`.data` included) |
| `search` | `path`, `query` | Built-in search engine; no external `rg` needed |
| `mkdir` | `path` | Create directories recursively |
| `patch` | `path`, `patch` | Apply a unified diff; located by content, not line numbers |
| `diff` | `path`, `new_path` | Compare two files; returns added/removed counts and a diff |
| `rename` | `path`, `new_path` | Rename a file/directory |
| `move` | `path`, `new_path` | Move a file/directory |
| `delete` | `path` | Delete a file or a whole directory (recursive) |

Paths are relative to the workspace root. Preferred forms:

```text
./src/App.tsx
./logs/build.log
.
```

Absolute paths are accepted only if they are inside the workspace root; `../`, `C:\Windows`, `/etc` and similar are rejected.

Temporary and scratch output belongs in `.data/tmp/` (files sitting directly in it expire after a day, so long-lived output belongs in a subdirectory); sub-agent deliverables arrive in `.data/deliver/<session-id>/`.

### Path and protection rules

- The workspace root itself cannot be written, renamed or deleted.
- `.data/` is **inside the workspace**, so it is listed, searched, read and written like any other folder — the assistant can look up its own skills, memories, scripts and todo lists, and manage them directly.
- The one exception is the workspace database (`.data/chat.db` plus its `-wal` / `-shm` / `-journal` sidecars): `file_actions` could only truncate or delete it, never rewrite it, so it is rejected. Losing it would destroy the chat history, the request monitor, the interaction log and the agent missions.
- The one skills root (`./.data/skills`) cannot be deleted as a whole — target a skill folder instead.
- Every file or folder under the skills root gets a `.bak.N` backup before it is overwritten or deleted: `skill.md.bak.1` for a file, `demo.bak.1` for a whole folder. Backups are a safety net, not a permission — they are created whatever the mode.
- Paths are canonicalized and symlink-checked; symlinks pointing outside the workspace are rejected. Resolution is lenient about the shape of a path (a `./` prefix, a doubled separator, a repetitive name such as `<root>/ai-chat/src`) but never about escaping the root.

### Search parameters

| Parameter | Default | Description |
| --- | --- | --- |
| `recursive` | `true` | Recurse into subdirectories |
| `case_sensitive` | `false` | Case-sensitive match |
| `smart_case` | `false` | Uppercase in the query switches to case-sensitive |
| `use_regex` | `false` | Treat `query` as a regular expression |
| `glob` | none | File filter, e.g. `**/*.rs`, `src/**/*.ts` |
| `include_hidden` | `true` | Include hidden files |
| `respect_gitignore` | `false` | Honor `.gitignore` / `.ignore` / git exclude |
| `max_results` | `1000` | Maximum matching lines; search stops early |

For large repositories:

```json
{
  "action": "search",
  "path": ".",
  "query": "TODO",
  "glob": "src/**/*.ts",
  "respect_gitignore": true,
  "max_results": 200
}
```

### Patch

`patch` uses unified diff:

```diff
@@ -10,7 +10,7 @@
 function greet(name: string) {
-  return `Hello ${name}`;
+  return `Hello, ${name}!`;
 }
```

Rules:

- **Located by content**: `@@` line numbers are hints; context lines decide where the hunk lands.
- Include about 3 lines of context around each change.
- Context/removed/added lines must match the file character-for-character, including indentation and spaces.
- One hunk per contiguous change; split distant edits into multiple hunks.
- `---` / `+++` / `diff --git` headers are optional.
- LF and CRLF both work.
- Matching tolerates leading/trailing whitespace differences only.
- On failure the error shows the closest actual content; re-read and fix the context.
- For large rewrites use `write`, not a hand-built patch.

### Diff

```json
{
  "action": "diff",
  "path": "./src/old.ts",
  "new_path": "./src/new.ts",
  "context_lines": 3,
  "ignore_whitespace": false
}
```

Returns `+N / -M lines across K hunk(s)` and the full diff.

## `run_cmd`

Runs one program directly, without a shell.

```json
{
  "action": "run",
  "command": "git status --short",
  "timeout_seconds": 30
}
```

Behavior:

- **The first word resolves in two steps**: first against the **workspace script library** `<workspace>/.data/scripts` — a bare name, extension optional (`build-frontend`, `deploy.py`); the library is prepended to the child's `PATH`, so a script wins over a same-named program. Otherwise it must be a bare program name that the OS resolves through `PATH` — `python`, `python3`, `git`, `npm`, `node`, `cargo`, `curl`. Absolute paths (`C:\Python\python.exe`, `/usr/bin/python3`) and relative paths (`.\build\tool.exe`, `sub/tool`) are **rejected** before the risk assessment even runs. Use `run_shell` when a path is really needed.
- The first word is the program; the rest are arguments.
- Quotes and backslash escapes are supported.
- **No** pipes, redirection, `&&`, `||`, `$VAR` or glob expansion — use `run_shell` for those.
- Working directory: workspace root.
- Default timeout 30 seconds, maximum 3600.
- When skills are active, the executable must be in the `allowed-commands` union.
- L0–L6 risk assessment runs before execution; L0–L3 require confirmation.

Good for: `git`, `npm`, `node`, `cargo`, `python`, `curl`.

### Workspace script library

`run_cmd` maintains a script library at `<workspace>/.data/scripts` (created at startup, shared by the whole workspace):

```json
{ "action": "list" }
```

- `list` shows every library script with its interpreter, the `@description` / `@usage` header comments and its use count. Metadata is read only from header comments (`#`, `//`, `REM` or `::`).
- The library is listed in the system prompt, so the model reuses an existing script instead of retyping a long command. Convention: save a multi-step command as a script once it has succeeded at least twice, or whenever you ask to keep it; prefer `.ps1` on Windows and `.sh` on Linux/macOS; keep it argument-driven and self-contained; never store credentials in it.
- A script whose body contains destructive or sensitive patterns asks for confirmation once; the approval is remembered for that exact content until the file changes.
- Scripts are ordinary files: manage them with `file_actions`, keep them in version control; profile backups include `workspace/scripts/`.

## `run_shell`

Runs a script in the build's shell. The tool schema states the host OS and the fixed language, so there is **no shell-type argument**: write PowerShell on Windows, Bash on Linux/macOS.

```json
{
  "run_type": "direct",
  "code": "Get-ChildItem -Recurse -Filter *.ts | Select-String 'TODO'",
  "timeout_seconds": 60
}
```

Parameters:

| Parameter | Description |
| --- | --- |
| `run_type` | `direct` (default, wait for exit) or `serve` (background process) |
| `action` | serve-only: `start` / `stop` / `list` |
| `serve_id` | serve process id; required for `stop`; use `"all"` to stop all |
| `code` | Script body; required for `direct` and `serve start` |
| `sudo` | Bash only: run the whole script through `sudo` (asks for credentials) |
| `elevated` | PowerShell only: request administrator elevation (UAC) |
| `timeout_seconds` | Direct timeout, default 30, max 3600 |

Behavior:

- The script starts with `Set-Location` / `cd` to the workspace root.
- **The script has full freedom**: it may `cd` anywhere and run anything. The only gate is the risk engine — every script is scored and, for L0–L3, confirmed before it runs.
- Output is returned as-is (ANSI color codes are **not** stripped for `run_shell`; CLI sub-agents do strip them).
- Process trees are killed on timeout, Stop and app exit.
- Hidden window on Windows; own process group on Unix.
- A syntax gate runs before execution: a mismatched brace or quote is blocked with the reported line/column instead of being executed.
- `sudo` / `elevated` are where privilege elevation belongs: a script that needs it declares it in the call instead of assuming the app runs privileged.

### Background serve

```json
{
  "run_type": "serve",
  "code": "npm run dev -- --host 127.0.0.1"
}
```

Returns a `serve_id`. Then:

```json
{ "run_type": "serve", "action": "list" }
```

```json
{ "run_type": "serve", "action": "stop", "serve_id": "serve-..." }
```

Important: **serve processes live only for the current chat turn.** When the turn ends (normal completion, Stop, error), the app kills the whole process tree and verifies exit. App exit also cleans them up.

Good for:

- starting a service for this turn, then using `run_cmd` to request it, run tests or collect logs;
- stopping it explicitly before the turn ends.

Not for:

- a dev server that must survive across user turns. Start that outside the app.

Running serve processes are shown in the transcript's **Background** card (pid, uptime, a Stop button), next to the background tasks of the same reply; the model can also list and stop them with the `serve` actions above.

## Background tasks (`run_in_background`)

`run_shell` serve keeps a process alive *for the turn*; a background task does the opposite — it lets the **model keep working** while one call finishes.

With **Settings → Run → Background execution** on, these accept `run_in_background: true`:

```json
{ "action": "run", "command": "cargo test", "run_in_background": true }
```

```json
{ "run_type": "direct", "code": "npm run build", "run_in_background": true }
```

```json
{ "action": "search", "path": ".", "query": "TODO", "glob": "**/*.ts", "run_in_background": true }
```

```json
{ "agent_id": "reviewer", "task_description": "Audit src-tauri/src/mcp.rs for blocking calls", "run_in_background": true }
```

| Tool | Supported background form |
| --- | --- |
| `run_cmd` | Every call |
| `run_shell` | `run_type: "direct"` only (`serve` has its own list/stop) |
| `file_actions` | `action: "search"` only |
| MCP tools | Every call; a stdio server gets its own short-lived process |
| `call_subagent` | Every call |

### What the model sees

1. The tool result is a placeholder with a `bg-…` id — the model is told to continue with other work.
2. The real output is injected before its **next request** as a `user` message starting with `[BACKGROUND TASK RESULT …]`, and stored on the assistant row so a reload or session switch keeps it.
3. If the model tries to produce the final answer while a task is still running, the turn is held open (max 3 rounds × 120 s); after that the remainder is cancelled.
4. The result is **not** appended to the chat transcript as a message. Progress and results are visible in the **Background** card at the end of the reply — and to the model.

### Managing tasks

The model gets the `background_task` tool:

| Action | Parameters | Effect |
| --- | --- | --- |
| `list` | — | Running and finished tasks of this session (id, tool, status, elapsed) |
| `wait` | `task_ids`?, `timeout_seconds`? | Block until the given tasks (or all) finish; returns the results available |
| `get` | `task_id` | Full result of one finished task |
| `cancel` | `task_id` or `"all"` | Kill the process tree and discard the result |

You can cancel from the **Background** card at the end of the reply — one Cancel button per task, Stop per serve process; `list_background_tasks` / `cancel_background_task` back that UI.

### Limits and cleanup

- Background `run_cmd` / `run_shell` use the timed runner: cancellable, process-tree kill, **600 s** default timeout.
- A background `file_actions` search aborts inside the parallel walker when cancelled.
- A background stdio MCP call uses an isolated short-lived process, because the shared pool serialises on one mutex and caps a round trip at 10 s.
- Confirmation dialogs are serialised; a cancelled task stops waiting for one.
- Tasks are **turn-scoped**: `end_turn_cleanup`, Stop and app exit set every cancel flag and kill each tree. A task is never resumed in a later turn.

### serve vs background task

| | `run_shell` serve | `run_in_background` |
| --- | --- | --- |
| Purpose | Keep a long-lived process (dev server) | Do slow work without blocking the model |
| Returns | `serve_id`, process keeps running | `bg-…` id, output arrives later |
| Lifetime | Until the turn ends | Until it finishes, is cancelled, or the turn ends |
| Interaction | `serve` list/stop | `background_task` list/wait/get/cancel |

## Examples

### Inspect a project

```text
file_actions list path="."
file_actions search path="." query="fn main" glob="**/*.rs" respect_gitignore=true
```

### Modify a function

1. `file_actions read ./src/utils.ts` to read the original.
2. `file_actions patch` with real context lines.
3. `run_cmd` to run `npm test`.
4. On failure, `read` the failing area and patch again.

### Start a service and verify it

1. `run_shell serve` to start `npm run dev -- --host 127.0.0.1`.
2. `run_cmd` to run `curl -s http://127.0.0.1:5173`.
3. `run_shell serve action="list"` to inspect output.
4. `run_shell serve action="stop"` to stop.

### Fetch a web page

```json
{ "action": "run", "command": "curl -s https://example.com", "timeout_seconds": 30 }
```

For multi-page scraping or article extraction, prefer the MCP fetch server (see [MCP Servers](MCP服务器)).

## Common errors

| Error | Cause / fix |
| --- | --- |
| `Path ... is outside the workspace root` | `file_actions` cannot leave the workspace; use `run_shell` and accept the risk prompt |
| `The workspace database (.data/chat.db) is managed by the application` | The chat database is the one file `file_actions` may not modify; use the app's own history/monitor panels |
| `The skills root itself cannot be modified or deleted` | Delete a skill folder (`.data/skills/<name>`), not the skills root |
| `run_cmd only runs programs that PATH resolves` | Use a bare program name (`python`), or switch to `run_shell` for an absolute path |
| `Command 'xxx' is not in this skill's allowed-commands list` | The skill command allowlist does not include the executable |
| `Syntax gate blocked` | Script syntax error; fix the reported line/column |
| `Execution rejected by user` | You denied the confirmation; the model sees the reason |
| Command timed out | Raise `timeout_seconds` (max 3600) or switch to serve |
| Pipes do not work in `run_cmd` | Use `run_shell` |
| serve process is gone next turn | By design: serve trees are killed at turn end |
| `run_in_background` is not in the tool schema | Turn on **Settings → Run → Background execution** |
| A background task never delivers a result | It was cancelled, or the turn ended; start it again in the current turn |
| The turn keeps waiting | A task is still running: wait for it, or cancel it from the Background card / `background_task action="cancel"` |
| Output is truncated | Tool results keep head 6,000 + tail 2,000 characters; narrow the query or read in ranges |

## Related pages

- [Tools Overview](Tools-Overview)
- [Security and Boundaries](Security-and-Boundaries)
- [Workspace and Data](Workspace-and-Data)