A plugin turns a **long-lived program** into tools the model can call directly. It fills the slot next to built-in tools, scripts and MCP: a skill is prompt text, a script is a one-shot command, MCP is another protocol — a plugin is a pool of resident instances in the workspace plus a message bus.

The chain is fixed: the model calls a tool → the tool publishes on the **plugin message bus** → the bus routes the request to one of the plugin's instances → the reply returns through the same request id.

## Enabling and entry points

| Where | What |
| --- | --- |
| Settings → Run → **Enable the plugin system** | `config.plugins_enabled`, off by default. Only then does the model receive the `plugin` tool and every declared `plugin_<id>_<tool>` |
| Settings → Run → **Plugin manager** | Opens a **separate window** (not a section inside the settings sheet); the row is hidden while the plugin system is off |
| Inside the manager | Plugin list with a filter box, instance table, settings form, log tail, bus audit strip; the book icon next to the title opens "Creating a plugin", **Export** sits beside Start / Stop and **Import package** in the toolbar (see *Export and import*) |

The **service endpoint** (`127.0.0.1:<port>`) and **token** a service plugin needs live in the manager window too, each with a copy button.

Plugins are **workspace content**: switching the workspace switches the whole set, and running instances are stopped when you leave (they are rooted in the old folder).

## Folder and manifest

```text
<workspace>/.data/plugins/<id>/plugin.json   # manifest; <id> MUST equal the folder name
<workspace>/.data/plugins/<id>/<entry>       # entry script (server.mjs / server.py / …)
<workspace>/.data/plugins/<id>/config.schema.yaml  # optional settings form
<workspace>/.data/plugins/<id>/config.yaml         # settings values (written by the manager)
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
  "auto_start": true,
  "call_timeout_ms": 30000,
  "transport": "spawn",
  "subscriptions": ["clock/#"],
  "tools": [
    { "name": "now", "description": "Current time.", "parameters": { "type": "object", "properties": {} } }
  ]
}
```

| Field | Meaning |
| --- | --- |
| `name` / `version` / `description` | Shown in the panel and in tool descriptions |
| `runtime` | `auto` / `python` / `node` / `shell` / `exec`; `auto` derives it from the `entry` extension |
| `entry` | Entry file relative to the plugin folder |
| `command` + `args` | Escape hatch for a compiled binary or a vendored interpreter; wins over `runtime` |
| `env` | Extra environment variables for the child |
| `instances` | Resident instance count, default 1, clamped to 8 |
| `enabled` | Whether the plugin loads at all; a disabled plugin holds no instances and exposes no tools |
| `auto_start` | Warm the instance pool when the workspace opens (without it the **first call** still starts them) |
| `call_timeout_ms` | Per-reply deadline; `0` = the 30 s default, capped at 600 s |
| `transport` | `spawn` (default) / `service` / `both`, see below |
| `subscriptions` | Bus topics this plugin listens to; `+` matches one level, a terminal `#` the rest. Empty = `<id>/#` |
| `tools` | Tools declared to the model; each becomes `plugin_<id>_<tool>` (an entry may carry its own `topic`; empty = the plugin id) |
| `config_page` | HTML file for a bespoke settings page (instead of the schema form) |

A broken manifest never takes the whole load down: that plugin becomes an entry under "Manifests that could not be loaded" and the others keep working.

## Export and import

The manager moves a plugin between workspaces as **one `.zip`**:

| Action | What happens |
| --- | --- |
| **Export** (beside Start / Stop) | Packs the selected plugin: `plugin.json` at the archive root, then the entry script, the schema and its assets. Hidden files, `*.bak.*`, `node_modules/` and the cache folders stay out. `config.yaml` travels only when the plugin declares a `config.schema.yaml`, and every `secret` field is removed first — re-enter a token after importing |
| **Import package** (toolbar) | Takes a `.zip` — a package from this app, or a folder zipped by hand holding one `plugin.json` — unpacks it into the workspace and selects the new plugin. Importing over an installed plugin stops its instances and moves the old folder to `.data/plugin-backups/<id>-<timestamp>/` |
| Refused, nothing written | An archive with no `plugin.json`, two plugins in one archive, a declared `entry` or `config_schema` the archive does not carry, or a workspace already holding 32 plugins |

A package is the migration path: [Configuration and Backup](Configuration-and-Backup) does not include `.data/plugins`, so export the plugins you need before moving a workspace.

## Protocol

stdio, **one JSON object per line** (no JSON-RPC framing):

```text
host -> {"type":"hello","plugin":"clock","name":"Clock","instance":0,"instances":2,
         "workspace":"…","dir":"…/.data/plugins/clock","topics":["clock/#"],"protocol":1}
node -> {"type":"ready","plugin":"clock","version":"1.0.0","tools":["now"]}
host -> {"type":"request","id":"r1","topic":"clock","tool":"now","args":{},"session_id":"…","timeout_ms":30000}
node -> {"type":"reply","id":"r1","ok":true,"result":{"time":"12:00"}}
node -> {"type":"reply","id":"r1","ok":false,"error":"why it failed"}
```

- An instance has **15 seconds** to answer `hello` with `ready`, or it is marked failed;
- the host also sends `event` (`{"type":"event","topic":"…","payload":{…}}`), `ping` and `shutdown`;
- an instance may send `event` on its own (audited, visible in the manager), `log` (`{"level":"info","message":"…"}`) and `pong`;
- **write ONLY protocol lines to stdout**: a non-JSON line is logged and ignored, so a `print` does not kill the plugin; diagnostics go to stderr and land in the plugin's own log ring (newest 200 entries).

## Instances, scheduling and lifecycle

- A plugin runs N resident instances; the one with the **least in-flight work** takes the request, round-robin breaks ties, and a request that cannot be written fails over once to a sibling before the call errors.
- Instances start **on demand** (`ensure_started`): missing ones are spawned, dead ones pruned, and the whole pool is retried on the next call.
- A child is spawned from an explicit interpreter + entry path — **never a shell string** — with the plugin folder as its working directory and `PLUGIN_ID` / `PLUGIN_INSTANCE` / `PLUGIN_DIR` / `PLUGIN_WORKSPACE` in the environment.
- Children join the process guard (Windows job object / Unix process group), so quitting the app kills the tree.
- A workspace switch runs `stop_all()`; app exit is covered by the guard.

## Service plugins

With `transport` set to `service` (or `both`) the process runs under your own supervisor, not the app's:

1. read the **service endpoint** and **token** from the manager (the token lives in `.data/plugins/.link-token`);
2. have the process dial `127.0.0.1:<port>` and send as its first line:

```json
{"type":"hello","plugin":"clock","token":"<token from the panel>"}
```

The app answers `{"type":"welcome","instance":"clock#link0","config":{…}}`, then speaks the same protocol as above.

- **One connection = one instance**: connect three times for three workers, and a dropped socket only removes that instance.
- The manifest's `instances` governs the spawned pool only, not service connections.
- Shutting down an attached instance just drops the socket — it **never** kills a process the app did not start.

## Settings

Two ways, pick one:

| Mode | How | UI |
| --- | --- | --- |
| Declarative form | Add `config.schema.yaml` (`sections[] → fields[]`) | The manager renders the field types: `text` / `multiline` / `number` / `integer` / `boolean` / `enum` / `path` / `secret` / `json`, with `default`, `required`, `min` / `max` / `max_length`, `options`, `advanced`, `reveal_when` |
| Custom page | Set `config_page` to an HTML file in the plugin folder | The manager embeds that page |

**Every section and every field carries `key`** — it is the name stored in `config.yaml` and the anchor in the form; `title` is the text a user reads. This is a shape that loads:

```yaml
title: Clock settings          # optional form header
sections:
  - key: main                  # required on the section
    title: Clock
    fields:
      - key: timezone          # required on the field — the name in config.yaml
        type: text
        title: Time zone
        default: UTC
      - key: format
        type: enum
        title: Format
        options:
          - { value: 24h, label: 24-hour }
          - { value: 12h, label: 12-hour }
      - key: token
        type: secret           # masked in the form, stripped from an exported package
        title: API token
        advanced: true
```

A schema that does not parse is shown **verbatim** in the manager instead of the form, and the plugin keeps running. The message names the exact path (`sections[<i>].fields[<j>]`):

| Message | Meaning |
| --- | --- |
| `sections[0]: missing field 'key'` | The first section object has no `key:` line |
| `sections[1].fields[2]: missing field 'key'` | That field has no `key`; `title` alone is not an identity |
| `... .type: unknown variant 'textbox'` | `type` is not one of the nine above |
| `... .options: invalid type: string` | `options` is a list of `{value, label}` mappings |
| `did not find expected key` | YAML indentation: two spaces per level, `fields:` under its section, no tabs |

`label` / `help` are accepted as aliases of `title` / `description`.

- Values live in `config.yaml` inside the plugin folder, so a plugin stays **self-contained and copyable**;
- a `secret` field reads back as a mask after saving; submitting the same mask means "unchanged";
- a save validates field by field and hands field-level errors back to the form;
- a successful save **hot-pushes** `{"type":"config","config":{…}}` to every ready instance — no restart. Instances also see the values in `welcome.config`, or read `config.yaml` / `PLUGIN_DIR` themselves.
- **The model has no way to write settings**: the `plugin` tool deliberately has no action that edits config, so a prompt-injected turn cannot retarget a plugin.

## What the model sees

Two tool surfaces, one bus:

- the **`plugin` control tool**: `list` (start here), `call` (`{plugin, tool, args}`, optional `topic` / `timeout_ms`), `publish` (fan one event out to every instance subscribed to a topic, no reply), `events` (read instance-pushed events, optionally `drain`), `status` (per-instance state, pid, in-flight count, restarts, last error), `logs` (the plugin log ring) and `start` / `stop` / `reload` (lifecycle);
- **each declared tool** appears directly as `plugin_<id>_<tool>` (capped at 64, sorted by function name because the tool list sits in the cached request prefix). Both routes end in `PluginHost::call`.
- A manifest written **mid-turn**, or reloaded, is callable on the next request (the same rebuild condition as MCP's `mcp_revision`).
- **No confirmation prompt**: a plugin is workspace content you installed, at the same trust level as a skill's scripts, and it never receives a shell — `run_cmd` / `run_shell` remain the risk-scored path.

## Writing a minimal plugin

```json
{ "name": "Echo", "version": "1.0.0", "runtime": "node", "entry": "server.mjs",
  "tools": [{ "name": "echo", "description": "Return the text unchanged.",
              "parameters": { "type": "object", "properties": { "text": { "type": "string" } } } }] }
```

```js
// server.mjs — protocol lines on stdout only, diagnostics on stderr
import { createInterface } from "node:readline";

const send = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");
createInterface({ input: process.stdin }).on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.type === "hello") return send({ type: "ready", version: "1.0.0" });
  if (msg.type === "request") {
    const text = msg.args?.text ?? "";
    return send({ type: "reply", id: msg.id, ok: true, result: { text } });
  }
  if (msg.type === "ping") send({ type: "pong" });
});
```

Workflow:

1. create the folder and files with `file_actions` (workspace-relative `./.data/plugins/<id>/…`);
2. `plugin` action `reload` re-reads the manifests, `list` confirms the plugin, `start` spawns the pool;
3. in chat, call `plugin_<id>_<tool>` directly, or `plugin` action `call`;
4. enable the plugin system first (Settings → Run), otherwise no plugin tool reaches the model.

## Boundaries and rules

- **Never put credentials in the plugin folder**: read them from settings values (a `secret` field).
- A child is not a shell: put shell semantics in a script the plugin calls itself.
- A `.js` entry inside a `"type": "module"` workspace is ESM — name it `server.mjs`, or use CommonJS with `server.cjs`.
- One plugin, one purpose: its tools share one instance pool and one settings page.
- A reply is text the model reads: keep it small, structured and free of ANSI noise.
- **Backups do not include `.data/plugins`**: export the plugins you need as packages before migrating a workspace (see *Export and import*).

## FAQ

| Symptom | Cause and fix |
| --- | --- |
| The model sees no plugin tools at all | The plugin system is off in Settings → Run; turning it on needs no restart, it applies to the next request |
| A plugin shows "could not be loaded" | `plugin.json` failed to parse, or `<id>` does not match the folder name; check the manager's issues and the log |
| An instance stays "Starting" and then "Failed" | No `ready` within 15 s: the entry script crashed, or stdout is polluted — read the plugin log |
| Calls time out | The reply deadline is 30 s by default (`call_timeout_ms`, max 600 s); report progress or split long work |
| A service plugin cannot attach | Stale endpoint/token or nothing listening; copy the token again and confirm the process dials the current workspace's `127.0.0.1:<port>` |
| Settings changed but the plugin ignores them | The plugin does not listen for `{"type":"config"}`, or no instance was ready; `stop` then `start` once |
| Every plugin disappeared after a workspace switch | Expected: plugins are workspace-scoped, and the new workspace needs its own copy |

## Related pages

- [Tools Overview](Tools-Overview) — tool switches and risk levels
- [MCP Servers](MCP-Servers) — the other way to bring in external tools
- [Workspace and Data](Workspace-and-Data) — the `.data` layout
- [Configuration and Backup](Configuration-and-Backup) — what a backup contains and how to migrate
