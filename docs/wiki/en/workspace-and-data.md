The workspace is Rabbit Chat Studio's core boundary: skills, memory, todos, chat history and the relative paths used by file tools all resolve against it. Switching the workspace switches the whole state.

## What a workspace is

- A normal directory on disk, usually your project root.
- File tools, command execution, CLI sub-agents and stdio MCP servers start there.
- The app creates a hidden `.data/` folder inside it for workspace-level state.
- The title bar shows the folder name with a thin red underline; the native window title is `Rabbit Chat Studio — <full path>`.
- Title-bar `⋯` → **Open workspace directory** reveals it in the file manager.

## Default location and changing it

| Case | Path |
| --- | --- |
| `workspace_dir` unset | `<app data>/workspace/` |
| Custom | **Settings → Workspace & Profiles → Workspace Directory** |

The directory field is next to a picker of recently used workspaces (`workspace_history`, newest first, max 10). Picking one only stages it — **Save Changes** applies the switch. Entries are recorded on every applied change and travel with a profile backup.

If the saved path no longer exists when the app starts (the directory was moved, renamed or unmounted), the app falls back to `<app data>/workspace/` instead of failing to start; the title bar then shows that folder, and you can pick the real directory again from Settings.

App data directories:

| Platform | Path |
| --- | --- |
| Windows | `%APPDATA%\com.leonard.aichat\` (Roaming) |
| macOS | `~/Library/Application Support/com.leonard.aichat/` |
| Linux | `~/.local/share/com.leonard.aichat/` |

> A build installed from the Microsoft Store is an MSIX package, so Windows redirects the Windows path above to `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\Roaming\com.leonard.aichat\`. The Store build and the GitHub installer build therefore **cannot see each other's app config**, while the workspace `.data` folder stays shared — switching install method looks like a first launch.

**Reset** clears `config.workspace_dir` and returns to the default workspace. In a shadow window (see [Multiple windows](Configuration-and-Backup)) the reset returns to that window's own fork, never the shared default workspace.

### What happens on switch

After saving a new workspace:

1. The app closes the current SQLite connection and opens `<new workspace>/.data/chat.db`.
2. History, request monitor, interaction logs and agent missions switch to the new database.
3. Skills and their enablement switch to the new workspace's `.data/skills` and `skills.status`.
4. Memory and todos switch to the new workspace's `.data/memory` and `.data/todos`.
5. Pending timers are flushed back to the old workspace's `.data/timers.json` and the new workspace's set is armed; overdue ones fire immediately.
6. Running stdio MCP children are stopped; the next tool call respawns them in the new workspace.
7. The history panel re-queries, the title bar updates, and the app lands on a fresh conversation — a new workspace never opens onto the previous transcript.

> Switching does not move the old `.data` folder. To migrate, copy it or use **Export Profile / Restore Profile**.

## `.data` layout

```text
<workspace>/
  .data/                       (hidden; part of the workspace)
    chat.db                    SQLite: history / api_requests / interaction_log / agent_missions …
    memory/
      user/<key>.md            user-scope long-term memory
      repo/<key>.md            repo-scope long-term memory
      evolution/<key>.md       evolution-scope memory (self-evolution mode only)
    todos/
      <session_id>/
        .active                active list id
        lists/<list_id>.json   list contents
    skills/
      <folder>/skill.md        the one skills root (also accepts SKILL.md)
    skills.status              skill enablement and metadata for this workspace
    timers.json                pending delay timers of this workspace
    scripts/                   run_cmd script library (bare-name scripts, created at startup)
    mcp/
      <slug>/server.*          self-built MCP server scripts (self-evolution mode)
    mcp.conf                   roster of those servers (merged with the app-data mcp_servers.json)
    deliver/
      <session-id>/<batch-id>/ sub-agent deliverable exchange: shared/ + one folder per task
    tmp/
      pdf/<stem>-<digest>.md   cached @-mention PDF→Markdown conversions
```

`scripts/`, `tmp/` and `deliver/` are created at startup. `deliver/` lives as long as its session — it is part of the profile backup and disappears when the session is deleted; open it via the title-bar `⋯` menu (**Open deliverables folder**) or the button next to the mission monitor's refresh. `tmp/` is pure scratch: a plain file sitting directly in it expires after a day. The script library is listed to the model and included in profile backups.

On Windows the folder has the hidden attribute; on macOS/Linux the dot prefix hides it.

### `.data` is inside the workspace

The assistant's file tools reach `.data` like any other folder:

- `file_actions` lists, searches, reads **and writes** it — the assistant can look up its own skills, memories and todo lists, and manage them directly;
- the **one exception** is the workspace database (`.data/chat.db` plus its `-wal` / `-shm` / `-journal` sidecars). `file_actions` could only truncate or delete it, never rewrite it, so it is rejected — it holds the chat history, the request monitor, the interaction log and the agent missions;
- the workspace root itself still cannot be deleted or overwritten.

Use the app's own panels (history, monitor, memory, todos, skills) for those files where they exist — they keep the formats and indices consistent. The database has no such panel, which is exactly why it is protected.

## App data directory

Application-level files do not live in the workspace:

| File | Contents |
| --- | --- |
| `config.json` | Model, API, tools, workspace, appearance, log retention |
| `mcp_servers.json` | MCP server definitions |
| `sub_agents.json` | Sub-agents + orchestration |
| `profiles.json` | In-app **Profiles** |
| `agents/` | `llm` agent definitions (Markdown) + the `index.yaml` discovery manifest |
| `shadow/` | Primary-instance lock + one forked config per extra window |
| `app.log` | Debug-build logs (release builds do not write it by default) |
| `workspace/` | Default workspace |

Full config fields: [Configuration and Backup](Configuration-and-Backup).

## Workspaces and Git

If the workspace is a Git repository:

```gitignore
.data/
```

`.data` holds the chat database, memory, todos and skill enablement — local state that should not be committed. If you want to share skill sources, keep them in the repository and copy them into `.data/skills`, or distribute them through a profile export.

## Workspace vs. profiles vs. backup

| Mechanism | Scope | Includes skills? | Use |
| --- | --- | --- | --- |
| Workspace | One directory | Yes (`.data/skills`) | Daily project isolation |
| Profiles | App-level `profiles.json` | **No** | Quickly switch tools, sub-agents, MCP |
| Export zip | App + current workspace | Yes | Move machines, full backup |

Profiles store `selected_tools`, sub-agents, orchestration and MCP servers. Skill selection is workspace state and is not part of a profile.

## Migrating a workspace

### Option A: full export/import (recommended)

1. Open the app in the old workspace.
2. Title-bar `⋯` → **Export Profile…**, save the zip.
3. Open the app on the new machine/workspace.
4. `⋯` → **Restore Profile…**, choose the zip.
5. Import applies `config.json` first (which may switch the workspace), then restores `workspace/` state into the new workspace.

See [Configuration and Backup](Configuration-and-Backup) for the zip contents and current import limitations.

### Option B: manual copy

Close the app, then:

```text
copy <old workspace>/.data  →  <new workspace>/.data
```

Open the app in the new workspace. Useful when you only want state, not app config.

## Disk usage

Main sources:

- chat history in `chat.db`;
- `api_requests` and `interaction_log` (full request bodies can be large);
- memory and skill files (usually small).

Controls:

- **Settings → Runtime & Debug → Log retention days** (30/90 days; `0` = forever);
- Interaction Monitor → **Compact database** prunes expired logs and runs `VACUUM`;
- delete unneeded history sessions.

Chat history is never pruned automatically.

## FAQ

| Symptom | Explanation |
| --- | --- |
| `.data` is invisible in Explorer/Finder | Windows hidden attribute; show hidden items or use a terminal |
| The assistant says `.data` does not exist | It lists it like any other folder — ask it to `list ./.data` |
| `file_actions` refuses `.data/chat.db` | The workspace database is protected: the file tools could only truncate or delete it |
| Skills disappeared after switching workspaces | Skills are per workspace; switch back or copy them |
| First MCP tool call after switching is slow | stdio servers were stopped and are respawned in the new workspace |
| The app opened a different workspace than last time | The saved path was gone, so it fell back to the app-data workspace; pick the directory again |
| Can I recover a deleted workspace? | Only from an export zip or backup; back up first |
