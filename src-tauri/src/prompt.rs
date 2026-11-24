//! Every concrete model-facing prompt/manual text of the manual system.
//!
//! The manuals module (`manuals.rs`) owns the file layout and the pointer
//! injection; this file owns the WORDS. Nothing here reads the filesystem —
//! the bodies are static (or delegate to an existing prompt builder such as
//! `figures::prompt_block`), so a manual's content is byte-stable across turns
//! and free to re-seed at any time.
//!
//! Design (see the recorded design note `design-subagent-prompt-injection`):
//! before the assistant performs a heavyweight action — drawing an enhanced
//! figure, creating a skill, creating an MCP server, explicitly creating a
//! sub-agent — the system prompt only ANNOUNCES that a manual exists (the
//! pointer block). The manual is read on demand (`file_actions` read), so the
//! full reference costs nothing until it is actually needed, and none of it is
//! registered as an explicit skill.

use crate::figures;

/// Workspace-relative folder the manuals are seeded into.
pub const MANUALS_DIR: &str = ".data/manuals";

// ─── Pointer block ───────────────────────────────────────────────────────────

/// The short announcement injected into the system prompt INSTEAD of the full
/// manual text. One line per manual, plus the shared "read before acting"
/// rule. Kept tiny: the whole point is the prompt budget.
///
/// `lines` are the pre-rendered per-manual lines (`manuals.rs` renders them so
/// it owns the name/path/description triple).
pub fn pointer_block(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "MANUALS: the following references exist as plain files in this workspace — they are \
         NOT skills and never appear in any skill list. Read the file with `file_actions` \
         (action `read`) BEFORE you perform the corresponding action for the first time in \
         this session; do not perform the action from guesswork:",
    );
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out
}

/// One manual line of the pointer block: `- <title>: <description> → <path>`.
pub fn pointer_line(title: &str, description: &str, path: &str) -> String {
    format!("- {title}: {description} → `{path}`")
}

// ─── Manual bodies ───────────────────────────────────────────────────────────

/// `enhanced-figures` manual: the full draw syntax. This is exactly the text
/// the old inline figures prompt block carried (`figures::prompt_block`), so
/// nothing is lost by pointing at the file instead.
pub fn enhanced_figures_manual() -> String {
    format!(
        "# Enhanced figures — draw syntax reference\n\n\
         The transcript renders the following fenced blocks; users can save drawings.\n\n{}\n",
        figures::prompt_block()
    )
}

/// `create-skill` manual: how to author/edit a workspace skill by hand.
pub fn skill_manual() -> String {
    "# Creating / editing a skill (self-evolution)\n\n\
     The ONE skill directory is `<workspace>/.data/skills` (workspace-relative \
     `./.data/skills`) — never create or look for a second one, and never write outside the \
     workspace.\n\n\
     ## Shape\n\
     A skill is a folder containing exactly one `skill.md`:\n\n\
     ```\n\
     ./.data/skills/<folder-name>/skill.md\n\
     ```\n\n\
     ```markdown\n\
     ---\n\
     name: my-skill          # same as the folder name\n\
     description: >-         # ONE sentence: when to load this skill and what it covers\n\
       What it is for and when to use it.\n\
     ---\n\n\
     The body is the full instruction set that `use_skill` injects into the\n\
     conversation when the skill is loaded. Keep it dense and operational:\n\
     exact commands, paths, file layouts, gotchas. Never a transcript of a\n\
     past conversation.\n\
     ```\n\n\
     ## Workflow (use `file_actions` with workspace-relative paths)\n\
     1. `mkdir` the folder `./.data/skills/<folder-name>`.\n\
     2. `write` the complete `skill.md` (frontmatter + body) in one call.\n\
     3. Register it in `<workspace>/.data/skills.status` so the Skills panel sees it and \
     `use_skill` treats it consistently: add (or update) an entry under `skills` — \
     `{ \"enabled\": true, \"source\": \"self-evolution\" }` — keeping `version` and every other \
     entry untouched. Read the file first and patch it; it is JSON.\n\
     4. Verify: read the written file back (or run the skill's first command) before telling \
     the user it exists.\n\n\
     ## Rules\n\
     - Every mutation of an existing skill file creates a `.bak.<n>` backup automatically — \
     do not create backups yourself.\n\
     - Only `.data/chat.db` is protected; everything else under `.data` is yours.\n\
     - `name` in the frontmatter must match the folder name.\n\
     - Description quality decides discoverability: it is what the skill list (and the \
     `/` mention popup) shows.\n\
     - Deleting a skill: `file_actions` `delete` on the folder, then remove its entry from \
     `skills.status`.\n\
     - Never claim a skill exists without having written AND registered it."
        .to_string()
}

/// `create-mcp-server` manual: building a self-owned MCP server with `mcp_self`.
pub fn mcp_manual() -> String {
    "# Creating a self-built MCP server (`mcp_self`)\n\n\
     A server is a small stdio program speaking line-delimited JSON-RPC 2.0. One job and a \
     few tools per server; prefer `python` unless the task needs Node; never put credentials \
     in a script (use the environment or an app tool).\n\n\
     ## Handshake (implement all three)\n\
     1. `initialize` — reply with `{\"jsonrpc\":\"2.0\",\"id\":<id>,\"result\":{\"protocolVersion\":\
     \"2024-11-05\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"<slug>\",\"version\":\
     \"0.1.0\"}}}`.\n\
     2. `tools/list` — reply with the tool catalogue: each entry \
     `{\"name\":..., \"description\":..., \"inputSchema\":{...JSON Schema...}}`.\n\
     3. `tools/call` — execute and reply `{\"content\":[{\"type\":\"text\",\"text\":\"<result>\"}]}`.\n\n\
     Write ONLY JSON-RPC to stdout — any other stdout byte corrupts the stream. Log to \
     stderr. The script starts with the workspace root as cwd, so workspace-relative paths \
     work; write scratch data to `.data/tmp/`.\n\n\
     ## Workflow\n\
     1. Write the script with `file_actions` to `./.data/mcp/<slug>/server.<ext>` \
     (`<ext>` = `py` / `js` / `sh`).\n\
     2. Register it with the `mcp_self` tool: action `write` picks the interpreter from \
     `runtime` (`python` | `node` | `shell` — never an arbitrary command), backs up the \
     script, runs the handshake and `tools/list`, and reports the tool names. Enable with \
     `enabled: true` so the tools are callable immediately.\n\
     3. New tools are callable in the SAME turn — no restart of the conversation needed. \
     If `tools/list` fails the server stays disabled: call `mcp_self` action `logs` to see \
     why, fix the script and `write` again (a `write` re-tests) or `restart` after editing.\n\n\
     ## Roster & lifecycle\n\
     - The registry is `./.data/mcp.conf` (workspace YAML). Do not hand-edit it: `mcp_self` \
     `write`/`enable`/`disable`/`remove` keep it and the running process in sync. The \
     app-data `mcp_servers.json` belongs to the user's panel — never write it.\n\
     - `test` is for diagnosis only; `remove` stops the process, deletes the folder and \
     drops the registry entry.\n\
     - Keep servers small and focused; drop the ones no longer needed."
        .to_string()
}

/// `create-subagent` manual: the reference the main agent reads when it
/// explicitly creates a sub-agent (skill-like injection: the pointer only
/// announces this file; the content loads on demand).
pub fn subagent_manual() -> String {
    "# Explicitly creating a sub-agent (`agent_admin`)\n\n\
     Available only while the sub-agent system AND self-evolution mode are on. Creating a \
     sub-agent means persisting a new roster entry — it survives the session, shows up in \
     the Agents panel and is selectable like any panel-created agent. Read this manual \
     BEFORE the first creation; after that the field shapes below are the reference.\n\n\
     ## Actions (`agent_admin`)\n\
     - `list` — current roster (id, type, enabled, name, description).\n\
     - `save` — create or update one agent (`agent` object below; omit `id` to create).\n\
     - `delete` — remove by `id` (library agents are protected; disable them instead).\n\n\
     ## The `agent` object\n\
     Common fields: `name` (required, non-empty), `description` (one line: what it is for), \
     `agent_type` (`\"llm\"` | `\"cli\"` | `\"cloud\"`), `enabled` (default true).\n\n\
     ### `\"llm\"` — built-in autonomous loop (tools + mission state)\n\
     - `system_prompt` (required): the agent's full instructions. It never sees this \
     conversation — everything it needs goes here or in the per-task description.\n\
     - `model`: optional model id; omit to use the main model.\n\
     - `allowed_tools`: restrict the tool set; empty = all tools.\n\
     - `allowed_skills`: skill names it may load; empty = all.\n\
     - `max_iterations`: 0 = unlimited (default; mission completion is driven by the \
     external task state, not a round count; panel ceiling 500).\n\
     - `enable_mcp`: inject the tools of every enabled MCP server. `llm` only.\n\
     - `temperature`, `max_tokens` / `max_complete_tokens`: optional overrides.\n\
     The definition markdown is written to `<app_data>/agents/custom/<slug>.md` and the \
     config entry references it — you never write those paths yourself.\n\n\
     ### `\"cli\"` — headless coding CLI on this machine\n\
     - `cli.command` (required): executable, bare name (resolved through PATH) or path.\n\
     - `cli.args`: argv template; `{prompt}`, `{model}`, `{cwd}`, `{agent_name}` are \
     substituted. With `cli.prompt_via = \"stdin\"` the prompt goes to stdin instead.\n\
     - `cli.model_args`: extra argv appended when a model is set. `cli.env`: \
     `[{\"key\":\"K\",\"value\":\"V\"}]`. `cli.timeout_seconds`: 0 = none.\n\n\
     ### `\"cloud\"` — remote HTTP agent endpoint\n\
     - `cloud.endpoint` (required): URL.\n\
     - `cloud.poll_interval_seconds`: polling interval (0 → default).\n\
     - `cloud.headers` / `cloud.body_template` etc. follow the Agents panel fields; \
     `{prompt}` / `{images_data_urls}` placeholders are supported where the panel \
     documents them.\n\n\
     ## Rules\n\
     - NEVER convert an `llm` agent's type after creation (the markdown definition would be \
     stranded); create a new agent instead.\n\
     - `require_approval` (cli/cloud, default true): keep it on unless the user asked for a \
     standing authorization.\n\
     - Give every agent a self-contained `description`: the planner and the main agent \
     pick agents by reading it.\n\
     - After creating, report the new `id` and how to delegate to it (`call_subagent` with \
     `agent_id`). The agent becomes available on the next request of this turn — the tool \
     loop rebuilds the roster each round."
        .to_string()
}

/// `create-plugin` manual: a workspace plugin and the message bus behind it.
pub fn plugin_manual() -> String {
    r#"# Creating a plugin (plugin message bus)

A plugin is a workspace folder that becomes tools the model can call: the tool publishes on
the plugin message bus, the bus routes the message to one of the plugin's instances, and the
instance's reply returns through the same request id. `./.data/plugins/echo/` is a working
example (Node, 2 instances, settings form).

## Shape

```
./.data/plugins/<id>/plugin.json          # manifest; <id> MUST equal the folder name
./.data/plugins/<id>/<entry>              # entry script (server.mjs / server.py / …)
./.data/plugins/<id>/config.schema.yaml   # optional settings form
```

```json
{
  "name": "Clock",
  "version": "1.0.0",
  "description": "One line for the panel.",
  "runtime": "node",
  "entry": "server.mjs",
  "instances": 2,
  "enabled": true,
  "call_timeout_ms": 30000,
  "transport": "spawn",
  "subscriptions": ["clock/#"],
  "tools": [
    { "name": "now", "description": "Current time.", "parameters": { "type": "object", "properties": {} } }
  ]
}
```

`runtime` is `auto` | `python` | `node` | `shell` | `exec` (derived from `entry` when `auto`);
`command` + `args` replace the interpreter for a compiled binary. `instances` is 1-8.
`transport` is `spawn` (default) | `service` | `both`. Every declared tool reaches the model
as `plugin_<id>_<tool>`; the `plugin` control tool has `list` / `call` / `publish` / `events` /
`status` / `logs` / `start` / `stop` / `reload`.

## Protocol (stdio, one JSON object per line)

```
host -> {"type":"hello","plugin":"clock","instance":0,"instances":2,"workspace":"…","data_dir":"…"}
node -> {"type":"ready","version":"1.0.0"}
host -> {"type":"request","id":"r1","topic":"clock","tool":"now","args":{},"session_id":"…","timeout_ms":30000}
node -> {"type":"reply","id":"r1","ok":true,"result":{"time":"12:00"}}
node -> {"type":"reply","id":"r1","ok":false,"error":"why it failed"}
```

`ready` must arrive within 15 s or the instance is failed. The host also sends `event`
(`{"type":"event","topic":"…","payload":{…}}`), `ping` and `shutdown`; an instance may send
`event` (audited, visible in the panel), `log` (`{"level":"info","message":"…"}`) and `pong`.
Write ONLY protocol lines to stdout — anything else is logged and ignored. Diagnostics go to
stderr, which the panel keeps in the plugin's log ring.

## Service plugins (attach to the bus)

`"transport": "service"` (or `both`) means the process is started by its own supervisor, not
by the app. It dials the endpoint shown in the plugin manager (Settings → Plugins → service
endpoint, `127.0.0.1:<port>`) and sends this as its first line:

```json
{"type":"hello","plugin":"clock","token":"<token from the panel>"}
```

Every connection becomes one instance — connect N times for N workers, and a dropped socket
only removes that instance. The app answers `{"type":"welcome","instance":"clock#link0",
"config":{…}}`, then speaks the same protocol as above.

## Settings

`config.schema.yaml` (`sections[]` → `fields[]`; types `text` / `multiline` / `number` /
`integer` / `boolean` / `enum` / `path` / `secret` / `json`; `default`, `required`, `min`,
`max`, `max_length`, `options`, `advanced`, `reveal_when`) makes the manager render a generic
form; values live in `config.yaml` beside it. A plugin needing a bespoke UI sets `config_page`
to its own HTML. Values reach the plugin in `welcome.config` and again as
`{"type":"config","config":{…}}` on every save — hot reload, no restart.

## Workflow

1. Create the folder and files with `file_actions` (`./.data/plugins/<id>/…`).
2. `plugin` action `reload` re-reads the roster, `list` confirms the new plugin, `start`
   spawns the pool, `call` invokes a tool, `logs` shows the ring, `events` drains instance
   events.
3. In chat the model calls `plugin_<id>_<tool>` directly, or `plugin` action `call` with
   `{"plugin":"<id>","tool":"<name>","args":{…}}`.
4. The system must be enabled first (`plugins_enabled`, Settings → Plugins), otherwise no
   plugin tool reaches the model.

## Rules

- The child starts in the plugin folder with its own interpreter — never a shell string.
- Never put credentials in the plugin folder; read them from the settings values.
- A `.js` entry inside a `"type": "module"` workspace is ESM: name it `server.mjs`, or use
  CommonJS with `.cjs`.
- One plugin, one purpose. Its tools share the instance pool and the settings page.
- A reply is text the model reads: keep it small, structured, free of ANSI noise.
"#
        .to_string()
}

// ─── Tool description ────────────────────────────────────────────────────────

/// The `agent_admin` tool schema description. Short on purpose — the full
/// creation reference lives in the sub-agent manual, which the pointer block
/// announces and the model reads on demand.
pub fn agent_admin_description() -> String {
    "Manage the sub-agent roster explicitly (self-evolution mode): create, update, list or \
     delete agents of type `llm` / `cli` / `cloud`. A created agent persists in the roster \
     and is selectable with `call_subagent` from the next request of this turn. Read the \
     manual `.data/manuals/create-subagent.md` BEFORE the first `save` of this session — it \
     carries the exact field shapes per type. Never guess the schema."
        .to_string()
}
