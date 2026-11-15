MCP (Model Context Protocol) servers bring external tools and data sources into the chat. Once enabled, a server's tools appear as ordinary functions in the model's tool list and can be called like built-in tools.

Configuration has two rosters, merged on read:

- servers you create in the panel live in `mcp_servers.json` in the app data directory and do not switch with the workspace;
- servers the assistant built itself (self-evolution mode only) live in `<workspace>/.data/mcp.conf` next to their scripts and **do** switch with the workspace.

On an id clash the panel entry wins. Either way, stdio servers run with the **current workspace** as their working directory.

## Panel and lifecycle

Open **MCP** in the top toolbar:

| Element | Purpose |
| --- | --- |
| Toggle | Enable/disable; enabling a stdio server starts it immediately, disabling stops it |
| Name | Display name and log identifier |
| Transport summary | `stdio: command args` or `transport: url` |
| `self-built` badge | Marks a server the assistant created in self-evolution mode; edit/delete writes the workspace `mcp.conf`, never app data |
| ⚡ Test connection | Sends a real `initialize` handshake; opens logs automatically |
| Logs | Diagnostic log for the server (refreshes every 1.5 s); can be cleared |
| Edit / Delete | Modify or remove |
| + Add server | Create a new one |

Other behavior:

- At startup, all **enabled stdio** servers are started automatically (warmup).
- Switching the workspace stops running stdio children; the next tool call respawns them in the new workspace.
- App exit, window close and crashes clean up stdio children (Windows job object; Unix process group).
- HTTP/SSE/Streamable HTTP servers do not run a persistent child; connections are made per call.

## Transports

| Transport | Use | Configuration |
| --- | --- | --- |
| `stdio` | Local executable / `npx` / `uvx` | command, args, env |
| `sse` | Legacy HTTP + Server-Sent Events | url, auth_token |
| `http` | Plain JSON-RPC POST | url, auth_token |
| `stream-http` | Streamable HTTP (recommended) | url, auth_token |

### stdio

| Field | Description |
| --- | --- |
| Command | Executable name or path, e.g. `npx`, `uvx`, `python`, `./server` |
| Arguments | **Space-separated**; e.g. `-y @modelcontextprotocol/server-filesystem .` |
| Environment variables | One `KEY=value` per line, e.g. `API_KEY=abc123` |

The arguments input splits on whitespace, so a single argument cannot contain spaces. For those, use a wrapper script or edit the JSON config.

Working directory:

- The child's `current_dir` is the current workspace.
- `PWD` is set too, so CLIs that derive the project root from `PWD` do not see the app's launch directory.
- `./server`, `./data.db` and `.` therefore resolve against the workspace.

On Windows, `npx` / `npm` / `pnpm` / `yarn` / `uvx` / `conda` and `.cmd` / `.bat` are wrapped through PowerShell so PATH resolves. On Unix, commands go through a login shell to pick up `nvm`, Homebrew, pyenv and similar PATH setup.

### SSE / HTTP / Streamable HTTP

| Field | Description |
| --- | --- |
| URL | SSE usually `http://host/sse`; HTTP is a JSON-RPC endpoint; Streamable HTTP usually `http://host/mcp` |
| Auth token | Optional; sent as `Authorization: Bearer <token>` |

- SSE: open the event stream, wait for the `endpoint` event, then POST JSON-RPC to that endpoint.
- HTTP: POST JSON-RPC directly and read the JSON response.
- Streamable HTTP: `initialize`, keep the session id, then `tools/list` / `tools/call`; responses may be JSON or SSE streams.

## Add a stdio server

Using the official filesystem server:

1. **MCP** → **+ Add server**.
2. Name: `filesystem`.
3. Transport: `stdio (subprocess)`.
4. Command: `npx`.
5. Arguments: `-y @modelcontextprotocol/server-filesystem .`
6. Environment: leave empty.
7. Save.
8. Turn on the toggle; the first run may download the package and take a while.
9. Click ⚡ **Test connection**; the log should show a successful initialization.
10. In chat, ask: "List the files in the workspace root."

> `.` is the current workspace root. The filesystem server stays within the directories you give it; do not point it at the whole disk unless you really mean it.

### Other examples

| Server | Command | Arguments |
| --- | --- | --- |
| Fetch (web pages) | `uvx` | `mcp-server-fetch` |
| SQLite | `uvx` | `mcp-server-sqlite --db-path ./data.db` |
| Memory (graph memory) | `npx` | `-y @modelcontextprotocol/server-memory` |
| GitHub | `npx` | `-y @modelcontextprotocol/server-github`; put `GITHUB_PERSONAL_ACCESS_TOKEN=...` in env |

Package names and arguments change upstream; check the server's README. Test the connection first.

## Self-built servers (self-evolution mode)

With **Settings → self-evolution mode** on, the assistant gets the `mcp_self` tool and can build a small stdio server itself:

- it writes a script under `<workspace>/.data/mcp/<slug>/` (a `.bak.<n>` copy is kept on update) and registers it in the workspace roster `<workspace>/.data/mcp.conf`;
- the backend picks the interpreter from a whitelist (`python` / `node` / `shell`) — the model can only turn workspace code into a server, never register an arbitrary command;
- actions: `list` / `write` / `enable` / `disable` / `test` / `restart` / `logs` / `remove`. Writing, enabling and removal ask for confirmation once (the `user_software` kind, auto-acceptable via Settings → auto-accept);
- an entry whose `tools/list` handshake fails stays disabled, so a broken server can never stall later requests — `logs` returns the log tail so the model can fix its own script;
- the tool registry is rebuilt at the next request boundary once the roster changes, so *write → enable → call* works within one turn;
- sub-agents never receive `mcp_self` — they would race on the roster and a confirmation dialog would block a mission.

The panel shows these rows with a `self-built` badge; editing or deleting one writes the workspace file. You can also hand-edit `<workspace>/.data/mcp.conf` and use the server's **restart** action (or the panel's edit) to pick the change up.

## Add a remote server

1. **+ Add server**.
2. Name: `my-remote`.
3. Transport: **Streamable HTTP (recommended)**.
4. URL: e.g. `https://mcp.example.com/mcp`.
5. Auth token: fill it if the server requires Bearer auth.
6. Save and enable.
7. Test the connection; on failure, read the logs for HTTP status and response.

## How tools reach the model

After enabling a server, the app calls `tools/list` and converts each tool to an OpenAI function:

```json
{
  "type": "function",
  "function": {
    "name": "<raw MCP tool name>",
    "description": "<description> [MCP: <server name>]",
    "parameters": { "...": "inputSchema" }
  }
}
```

Notes:

- Tool names are **not** prefixed with the server name; the MCP server's own name is used.
- If two servers expose the same tool name, calls may be ambiguous; enable only what you need, or choose servers without name collisions.
- The app routes calls to the right server and executes `tools/call`.
- Every call writes an MCP diagnostic log entry and an interaction log entry.

### Who owns the server process

| Caller | Process pool |
| --- | --- |
| Main chat | One warmup pool (`mcp_children`), spawned at startup and reused; a `workspace_dir` switch stops the stdio children so the next call respawns them in the new workspace |
| `llm` sub-agent with `enable_mcp` | Its own `AgentMcpSession` per task — spawned at task start, shut down on success, error, max iterations or Stop |
| A background MCP call (`run_in_background`) | A short-lived isolated process per call, because the shared pool serialises on one mutex and caps a round trip at 10 s |

The two pools never share children, so the main chat and parallel tasks cannot stall each other.

## Diagnostic logs

Click the **Logs** button on a server:

- records save, start, `initialize`, `tools/list`, `tools/call` and errors;
- testing opens the log automatically;
- refreshes every 1.5 seconds;
- can be refreshed or cleared;
- in-memory ring buffer; cleared on restart.

Typical entries:

```text
Stdio process started and initialized
LLM requested tools/list
tools/list returned 12 tools
LLM tools/call: read_file
tools/call failed: ...
```

## Security boundaries

- stdio servers run with your user permissions; they are not limited to the workspace. Pointing filesystem at the workspace is a convention, not a sandbox.
- Remote servers receive the arguments you send; do not put unnecessary secrets in arguments.
- `auth_token` and `env` are stored in plaintext in `mcp_servers.json`.
- Only enable servers you trust; third-party MCP servers may execute arbitrary code.
- A background MCP call runs in its own short-lived process; it is still bound to the current turn and is killed when the task is cancelled or the turn ends.
- Deleting a server does not delete data it wrote externally.
- The app registers and cleans up server processes, but remote server-side data is unaffected.

## FAQ

| Symptom | Check |
| --- | --- |
| Command not found (`npx` / `uvx`) | Are Node/Python installed? Is PATH correct on Windows? Check the MCP log |
| Test hangs | First `npx` download is slow; check the log and network/proxy |
| Log says `initialize` failed | Is the command a real MCP server? Does it print non-JSON banner output? |
| Remote 401 | `auth_token`; other required headers |
| Remote 404 | URL path; SSE and Streamable HTTP endpoints usually differ |
| Empty tool list | Does the server implement `tools/list`? Is it enabled? Check the log |
| Tool name collision | Enable only one same-named server, or choose a different implementation |
| Tool fails once after switching workspaces | stdio was stopped; the next call restarts it |
| Self-built server never enables | A start that fails `tools/list` leaves it disabled; open **Logs** and fix the script, then start again |
| Process left behind | Normal exit cleans up; Windows job object handles crashes. Check Task Manager |
| Arguments contain spaces | The UI splits on whitespace; use a wrapper script or edit `mcp_servers.json` |
