Look up the symptom. Each entry gives the most likely cause first, then the next step.

## Install and launch

| Symptom | Cause / fix |
| --- | --- |
| macOS says the developer cannot be verified | Builds are ad-hoc signed. Right-click → **Open**, or allow it in **System Settings → Privacy & Security** |
| Store build and installer build have separate settings | MSIX redirects `%APPDATA%` into `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\`, so each build keeps its own config; `.data` inside the workspace is still shared. Copy the old config into the new folder when switching |
| The Store build never shows "new version available" | By design: the Store updates it, and `is_store_packaged()` makes the in-app check answer "up to date". Use Microsoft Store → **Library → Get updates** |
| Windows says WebView2 is missing | Install the Microsoft Edge WebView2 Runtime |
| Linux AppImage does nothing on double-click | `chmod +x` and run it from a terminal to see the error |
| Linux is missing WebKitGTK | Install the Tauri 2 dependencies such as `libwebkit2gtk-4.1-dev` |
| Port 1420 is busy (development) | Vite uses fixed `strictPort`; free the port or stop the process |
| `npm run build` says no git tag | Create a tag first, or use `npx vite build` for the frontend only |
| Versions are inconsistent | `node scripts/sync-version.cjs --from-git-tag` |

## API and models

| Symptom | Cause / fix |
| --- | --- |
| `401 Unauthorized` | Wrong/expired key; organization headers; local services may require any non-empty key |
| `403 Forbidden` | Account permissions, region restrictions, model not enabled |
| `404 Not Found` | Wrong base URL; do not include `/chat/completions`; check whether `/v1` is needed |
| `429 Too Many Requests` | Rate limit; reduce concurrency/frequency or use another key. The app retries 429/5xx, honouring `Retry-After` |
| `5xx` | Provider outage; inspect the error in the Interaction Monitor |
| Empty model list | Provider may not implement `/models`; add the model id manually |
| Chat works but tools never run | Model does not support tools; switch to a function-calling model |
| Context window is wrong | Set it manually; providers do not always report it |
| Attachment skipped | Model lacks vision/files/audio; switch models or remove the file |
| `reasoning_content` 400 | DeepSeek thinking model; enable DS-Format |
| `reasoning_effort` 400 | Provider rejects it; set thinking depth to Default |
| Reply is cut off | Raise **Max completion tokens**; check the provider limit |
| Model matched with a low similarity | The merged catalogue does not know that id; capabilities may be partial — set the context window manually |

## Chat and history

| Symptom | Cause / fix |
| --- | --- |
| Send button is greyed out | Empty input and no attachments; backup export running; generation in progress |
| Cannot switch sessions while generating | By design; click **Stop** first |
| Content remains after Stop | Stop only interrupts the stream; generated content stays |
| History is empty | History is per workspace; check the active workspace |
| History shows only titles | The title comes from the first user message; open the session for full messages |
| Retry button is missing | Only shown when the last message is an unfinished user message |
| Context breaks after deleting a message | Deletion is physical; start a new session if needed |
| Fork fails | The message must have a `dbId` (be persisted); wait a moment and retry |
| Export PDF does nothing | The app writes a PDF file to the chosen path; check the folder's write permission and the Interaction Monitor for the error |
| Timer fired in another session | Normal; use **Insert into input** in the notice |
| Background result missing from the chat | By design: results go to the model, not the transcript — check the **Background** card at the end of the reply |
| The turn keeps going after a "final" answer | The todo list still has open items — that is the goal loop; finish or cancel them, or wait via a timer / `[WAIT_EVT: …]` |
| Turn ends although items are open | A timer is pending, the answer declared `[WAIT_EVT: …]`, or the 4-continuation budget ran out |
| Cost chip is missing | The provider returned no usage for that turn |

## Workspace and data

| Symptom | Cause / fix |
| --- | --- |
| `.data` is invisible | Windows hidden attribute; use a terminal or show hidden items |
| The assistant cannot find `.data` | The file tools list it like any other folder — ask it to `list ./.data` |
| Skills/history changed after switching workspaces | Workspace-level state switches with the directory |
| The app opened a new empty chat after switching | By design: a workspace switch lands on a fresh conversation, so the old transcript cannot mix with the new directory |
| First MCP call after switching is slow | stdio was stopped; it restarts in the new workspace |
| Database is locked | Close other processes using `chat.db`; the app waits for its own connection |
| Disk usage is high | Lower log retention; compact the database; delete old sessions |
| MCP/history missing after import | Imports restore `mcp_servers.json`; `chat.db` is still manual — copy it while the app is closed |
| `config.json` edits do not apply | Fully quit the app first; the Settings panel may overwrite |
| Second window lost my settings changes | With `single_instance` off, an extra window is a shadow on a forked config; only `workspace_history` is merged back — change settings in the primary window |
| The saved workspace path no longer exists | The app falls back to the default workspace under app data instead of refusing to start |

## Skills

| Symptom | Cause / fix |
| --- | --- |
| No skills listed | Does `.data/skills` exist? Is the file named `skill.md` / `SKILL.md`? |
| Enabled but never used | `description` lacks triggers, or the task does not need it |
| `use_skill` says not found | Folder/frontmatter `name` mismatch; current workspace? |
| Changes do not apply in the same session | Loaded skills are not reloaded; start a new chat or re-enable |
| Command rejected | The executable is not in the `allowed-commands` union |
| `run_cmd` refuses a program | It only runs PATH-resolved names; use `run_shell` for a path |
| The assistant rewrote a skill on its own | Self-evolution mode is on; check `.data/skills/*/*.bak.N` |
| File Actions / Memory are greyed out | Self-evolution mode owns these two tools |
| Wrong name after importing a zip | The frontmatter `name` is authoritative |
| Skill toggle is lost | `skills.status` migration or the directory was deleted |

## Tools

| Symptom | Cause / fix |
| --- | --- |
| Model never calls tools | Tool disabled; model lacks tool support; system prompt forbids it |
| Confirmation dialog never appears / hangs | Check for a pending `ConfirmDialog`; after 120 seconds it counts as denied |
| Confirmed but still not executed | Read the tool result; a path or allowlist may have blocked it |
| `Path ... outside the workspace` | `file_actions` cannot leave the workspace; use `run_shell` |
| `.data/chat.db` write rejected | The workspace database is protected — the file tools could only truncate or delete it |
| Pipes do not work in `run_cmd` | Use `run_shell` |
| `run_cmd` rejects an absolute path | It only runs PATH-resolved program names; use `run_shell` |
| Command timed out | Raise `timeout_seconds` (max 3600) or use serve |
| serve disappears next turn | Serve trees are killed at turn end; run long-lived services outside the app |
| Workspace script not found by `run_cmd` | The file must be directly in `.data/scripts` and called by bare name (extension optional) — no subfolders, no paths |
| Patch fails | Context lines do not match; re-read and copy them exactly |
| Tool output is truncated | Head 6,000 + tail 2,000; narrow the range |
| Auto-accept did not apply | Check `auto_accept_confirm_kinds` (L0–L3 kinds or `"*"`); `external_agent` is not in this panel |

## MCP

| Symptom | Cause / fix |
| --- | --- |
| `npx` / `uvx` not found | Node/Python missing; PATH; Windows wrapper; check logs |
| Test hangs | First package download; network/proxy; check logs |
| `initialize` fails | Command is not an MCP server; non-JSON banner output |
| Remote 401 | `auth_token`; required headers |
| Remote 404 | URL path; SSE vs Streamable HTTP endpoints differ |
| Empty tool list | Server lacks `tools/list`; not enabled |
| Tool name collision | Enable only one same-named server |
| Arguments contain spaces | UI splits on whitespace; use a wrapper or edit JSON |
| Tool fails once after switching workspaces | stdio stopped; next call restarts |
| Process left behind | Normal exit cleans up; Windows job object handles crashes |

## Sub-agents

| Symptom | Cause / fix |
| --- | --- |
| Orchestration on but direct answer | No enabled agents; orchestration failed; check logs |
| CLI test fails | Command not in PATH; Windows `.cmd`; read `--version` |
| CLI output empty | Prompt not passed; try stdin; timeout/exit code |
| Cloud 401/403 | Token/headers |
| Cloud result empty | `result_path` mismatch |
| Polling never finishes | Status path/done values/poll URL |
| Sub-agent cannot see files | cwd, permissions, tool allowlist |
| Processes remain after Stop | Normally killed; check Task Manager and logs |
| Mission monitor is empty | No sub-agent ran; workspace DB switched? |
| External agent did not prompt | `require_approval` off; already approved this run |
| Sub-agents keep messaging each other | They hit the per-pair cap and the planner was notified; raise **Messages per agent pair** if the exchange is legitimate |
| Deliverables folder is empty | The task returned everything inline, or it was a `cloud` agent (no workspace access) |

## Timers

| Symptom | Cause / fix |
| --- | --- |
| Timer never fires | App must be running; check `<workspace>/.data/timers.json`; session deleted? |
| Timers lost after restart | `timers.json` deleted/corrupted; it lives in the workspace's `.data`, not the app data directory |
| Timer gone after switching workspaces | Timers are per workspace; switch back or set it again |
| Fires immediately on startup | It expired while the app was closed |
| Fires in another session | Use the notice to insert the prompt into the current input |
| Duplicate timers | Call `timer_list` before `timer_set` |
| Sub-agent cannot set a timer | By design; only the main agent has timer tools |
| System still sleeps | The lock is best-effort: Linux needs a desktop D-Bus service; lid close and battery policy still suspend |

## Updates

| Symptom | Cause / fix |
| --- | --- |
| New version not shown | Draft not published; pre-release toggle; no platform asset; network |
| Download fails | Asset still uploading; proxy; download manually from the Release page |
| Installer will not run | macOS right-click Open; Windows SmartScreen; Linux execute permission |
| Want to install with winget | No package-manager build yet (winget / Homebrew / Scoop); use the Microsoft Store or the Release installer |
| Config lost after update | Check install method/user directory; updates should not delete app data |

## Build and release

| Symptom | Cause / fix |
| --- | --- |
| Build requires a git tag | `npm run build` version sync; create a tag first |
| Windows GNU link errors | Use the MSVC toolchain |
| Linux missing WebKitGTK | Install Tauri 2 dependencies |
| CI fails: tag is not newest | The pushed tag must be the newest tag |
| CI does not publish | Manual `workflow_dispatch` is a smoke test; check permissions/secrets |
| Wiki pages missing on the site | `node scripts/gen-wiki.mjs` failed or `docs/wiki/**` was not in the workflow's paths; check the Pages run log |
| winget not submitted | The package is not accepted into winget-pkgs yet, or `WINGET_TOKEN` / fork is missing; the workflow is inactive for now |

## Collecting debug information

When filing an issue, include:

- Rabbit Chat Studio version (**Settings → About & Updates**);
- OS and architecture;
- reproduction steps and expected/actual results;
- screenshots;
- the error details from the Interaction Monitor (**remove API keys, passwords and business data first**);
- MCP logs (for MCP issues);
- CLI agent `--version` output (for CLI issues).

Do not publish:

- API keys;
- MCP `auth_token` / `env`;
- full `config.json` / `mcp_servers.json` (redact first);
- `chat.db` containing user data.

## Where else to look

- Install/build: [Installation and Build](Installation-and-Build)
- Models/chat: [Interface and Chat](Interface-and-Chat)
- Tools/risk: [Tools Overview](Tools-Overview), [Files and Commands](Files-and-Commands)
- MCP: [MCP Servers](MCP-Servers)
- Sub-agents: [Sub-Agents](Sub-Agents)
- Logs: [Monitoring and Logs](Monitoring-and-Logs)
- Boundaries: [Security and Boundaries](Security-and-Boundaries)
