# AI Chat — Tauri 2.0 (Rust + React)

Desktop AI chat client (Tauri 2 + React 19 + TS + Vite, Rust backend) that talks to any OpenAI-compatible API and adds **Skills**, **Tools**, and **Sub-Agents**.

## UI design — mandatory

**Any change that affects the interface MUST follow [`DESIGN.md`](./DESIGN.md).** It is the single source of truth for colours, typography, spacing, components, motion and both themes. Do not invent visual language and do not copy a style from another project — if a value is not in the spec, add it to the spec first.

Non-negotiables, repeated here so they are impossible to miss:

- **Tokens only.** Colours, radii, shadows, durations and easings come from `src/App.css` (`:root` + `[data-theme="dark"]` + the palette blocks). A literal hex outside those blocks is a bug.
- **One accent colour.** Electric Blue (`--c-accent` = `#3E6AE1`) marks the primary action of a view — never decoration, never a second chromatic accent. Semantic `--c-danger` / `--c-success` / `--c-warning` are for status only. An accent used as a *label or line* is `--c-accent-text`; a label on an accent *fill* is `--c-on-accent`.
- **No elevation.** `--shadow-*` are `none`; layering is z-index, opacity and the frosted `--c-toolbar` surface. Never write `box-shadow: <literal>, var(--shadow-sm)` — the mixed declaration is invalid and is dropped silently.
- **4px radius** on interactive elements. No pills, no large radii. Toggles/badges square off at 4px too.
- **One motion curve:** `--t-dur` (`0.33s`) + `--t-ease`. Colour/border transitions only — no `scale`/`translate` on hover.
- **Hairlines separate.** 1px `--c-border` for structure, 1px `--c-border-i` for interactive outlines. No gradient surfaces.
- **Reuse the shared primitives.** `.close-btn`, `.btn-primary`, `.btn-secondary`, `.inline-edit-btn`, `.field-title-row` are defined once in `src/App.css` — never re-declare them in a component stylesheet (that is exactly how the Skills/MCP/Tools panels drifted apart).
- **Compact zone = 34px controls** (chat input row, sidebar panels, Settings). Settings also uses 13px text. Do not introduce 40px inputs.
- **Sentence case, weights 400/500 only, `letter-spacing: normal`.** No `text-transform: uppercase`, no negative tracking.
- **Check light *and* dark** (`data-theme` is driven by `config.theme` via `applyTheme()` in `src/App.tsx`), plus `prefers-reduced-motion` for any animation. The colour scheme is a second, independent dimension: `data-palette` (`default` / `fig` / `cream`) comes from `config.palette`, and every palette ships a light *and* a dark token set — check all four combinations of a change.

When a needed value has no token, **add a token** to `src/App.css` — do not hardcode it at the call site.

Verify style work with `npx tsc --noEmit && npx vite build` (not `npm run build`, which requires a git tag).

## Commands

```sh
# Dev (hot-reload). Runs `npm run dev` (Vite on :1420) then launches Tauri.
npm run tauri dev

# Frontend only (Vite on :1420, strictPort — must be free)
npm run dev

# Production build. NOTE: this runs `sync-version:git-tag` first, so the
# repo must have a git tag (e.g. v1.1.1) or the build aborts.
npm run tauri build

# Type-checks
npx tsc --noEmit                                  # frontend
cargo check --manifest-path src-tauri/Cargo.toml  # rust

# Version sync (package.json ↔ Cargo.toml ↔ tauri.conf.json)
node scripts/sync-version.cjs --from-git-tag
node scripts/sync-version.cjs --bump patch
npm run sync-version:git-tag
npm run sync-version:bump-patch

# Rust unit tests (inside the `tools` module: dispatcher, `tools/todos`, `tools/timer`)
cargo test --manifest-path src-tauri/Cargo.toml
```

### Cargo mirror (CN / restricted networks)

`~/.cargo/config` may point at the USTC git registry, which can be unreachable. If `cargo` fails to fetch deps, override the source:

```sh
cargo check --manifest-path src-tauri/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'
```

## Releases (CI)

`.github/workflows/release.yml` (trigger: `v*` tag push, or manual `workflow_dispatch`) builds via
`tauri-apps/tauri-action@v1` on 4 runners — `macos-latest` ×2 (`aarch64-apple-darwin`, `x86_64-apple-darwin`),
`ubuntu-22.04`, `windows-latest` — and uploads `.dmg` / `.AppImage` + `.deb` + `.rpm` / `.msi` + NSIS `.exe`
to a **published** (non-draft) GitHub Release for the tag (also kept as workflow artifacts).

- **The tag drives the version.** `tauri build` runs `beforeBuildCommand` → `npm run build` → `sync-version:git-tag`, which reads the newest tag; a workflow step fails fast when the pushed tag is not the newest one.
- CI runs `rm -f .cargo/config.toml` so the committed rsproxy.cn mirror is not used on GitHub runners.
- macOS uses `APPLE_SIGNING_IDENTITY='-'` (ad-hoc) — replace with real signing/notarization secrets when available.
- `.github/workflows/winget.yml` auto-submits each published release's NSIS `-setup.exe` to `microsoft/winget-pkgs` via `winget-releaser`; it first waits for the asset upload to finish and needs the `WINGET_TOKEN` secret (classic PAT, `public_repo` + `workflow`) plus a `microsoft/winget-pkgs` fork under `xlbbb-cn`.
- `releaseDraft: false` publishes the Release as soon as the first platform finishes; remaining assets upload afterwards, so downstream consumers (e.g. winget) must wait for the NSIS `.exe` to appear.

## Architecture

- **Entry points**: `src/main.tsx` → `src/App.tsx`; Rust `src-tauri/src/main.rs` → `src-tauri/src/lib.rs` (command registration, state, app setup).
- **Streaming chat**: `chat_completion` command opens an SSE connection (`reqwest` + `futures-util`) and emits to the frontend:
  `chat-token`, `chat-reasoning-token`, `chat-done`, `chat-error`, `chat-usage`.
- **Mid-turn injection** (`inject_user_message` + `AppState::turn_inbox`): the composer stays usable while a turn streams, and Enter then appends the text to the *running* turn instead of starting a new one. A turn is a sequence of requests, so the queue is drained at the top of the main tool loop — the only safe boundary, because an `assistant` message carrying `tool_calls` must stay adjacent to its `tool` results. The text is appended as a `user` message (never `system`: some OpenAI-compatible gateways reject mid-conversation system messages) and announced with `chat-injected`, so the UI can tell "delivered to a request" from "still queued". A reply that already made its last request has no boundary left: the window closes with the turn (`InjectionWindow`, keyed by turn id so a winding-down turn cannot touch a newer one) and the UI re-sends those items as a normal next turn. Deliveries are stored on the assistant row (`history.injections`) and replayed as user messages, since one turn is still one row.
- **Agent orchestration** (toggle via `use_agents` in config): `agent-plan-start`, `agent-task-start`, `agent-task-token`, `agent-task-done`, `agent-task-error`, `agent-aggregate-done`. The aggregation step is instantaneous (the mission report is only rendered for the main agent), so `agent-aggregate-done` reports an already-completed step — never render it as "running".
- **Turn cancellation** (`stop_chat_completion` + `AppState::chat_cancelled`): every `chat_completion` installs a fresh `Arc<AtomicBool>` as its own cancel flag (a `Mutex<Arc<…>>` slot); the previous turn's flag is set to `true` permanently, so a superseded turn can never be resurrected and Stop pressed while idle cannot leak into the next turn. The turn's flag is threaded through `orchestrate` → `execute_*` → `run_sub_agent` → `cli::run`/`cloud::run` and into the main agent's tool loop: `cli` agents poll it every 250 ms and kill their process tree, `llm` agents break their loop, remaining tasks report as skipped, missions close as "stopped" (not "completed"), and a stop during planning/execution aborts the turn before the final answer streams. Plan-phase LLM calls (`plan_tasks`, `auto_configure_agents`) are checked between steps, not cancelled mid-request.
- **Profiles / backup**: `src-tauri/src/backup.rs` owns profile export/import (app menu `save-profile` / `restore-profile`). The zip holds `config.json`, `profiles.json`, `mcp_servers.json`, `sub_agents.json`, `workspace/{memory,todos,skills}/`, `workspace/skills.status` and a `chat.db` snapshot; every `workspace/...` entry is read from and restored into the hidden `<workspace>/.data` directory of the (possibly switched) active workspace. Named profiles inside the app (`save_profile_config` / `apply_profile_config` / `delete_profile_config`) are separate and stay in `lib.rs`; a profile carries tools/agents/MCP but **no** skill selection (that is per-workspace state).
- **MCP warmup**: enabled stdio servers in `mcp_servers.json` are spawned at app startup (`mcp::start_stdio_process`). MCP stdio children and `cli` sub-agents both start with the **active workspace** as their working directory — MCP via `current_dir` + `PWD` in `build_stdio_cmd`, CLI agents via `resolve_agent_cwd` (which sandbox-checks the agent's optional workspace-relative `cwd`, defaulting to the workspace root). A `workspace_dir` switch stops running stdio children so the next tool call respawns them in the new workspace.
- **Delay timers** (`src-tauri/src/tools/timer.rs`): the `timer` tool (`timer_set` / `timer_list` / `timer_cancel`, toggled in the Tools panel) lets the assistant wait for slow work instead of polling — the LLM arms a delay and ends its turn; when it fires the backend emits `timer-fired` and the frontend injects the timer's `message` into the same session as a new user turn (`timer-state` keeps the timer bar above the input box in sync). Timers are mirrored to `timers.json` and re-armed at startup, where overdue ones fire immediately. Sub-agents never receive the timer tools (`agents.rs` filters them out). The same file hosts the cross-platform **sleep lock** (`SleepLock`, ref-counted RAII): it is held while any timer is pending **and** for the whole duration of a chat turn (`chat_completion` wrapper in `llm_complete.rs`, released after turn-end serve cleanup), preventing idle system suspend via `SetThreadExecutionState` (Windows), an IOPM assertion (macOS) or a D-Bus inhibit (Linux); `sleep-lock` events + `get_sleep_lock_state` drive the badge above the input box.
- **Model metadata** (`src-tauri/src/model_metadata.rs`): `fetch_model_metadata` pulls the public basellm/llm-metadata catalogue (6 h in-memory cache) and expands its `tags` into capability flags. `src/utils/modelMetadata.ts` fuzzy-matches remote model ids against it (vendor prefixes, date stamps, modality guards); "Fetch Models From API" stores the matches per model in `config.model_metadata` and uses the catalogue context window when the provider reports none. The chat toolbar renders those flags as icons inside the model picker's options, plus a thinking-depth selector for reasoning-capable models (`config.model_reasoning_effort` → `reasoning_effort`, resolved by `llm_complete::resolve_reasoning_effort` and applied to chat + sub-agent requests). The attachment picker follows the same flags: text is always accepted (inlined into the prompt), images need `Vision` (`image_url` parts), PDFs need `Files` (`file` parts), `wav`/`mp3` need `Audio` (`input_audio` parts); an unmatched model keeps the permissive default.
- **Update check** (`src-tauri/src/update.rs`): `check_update` reads the releases of `xlbbb-cn/ai-chat` from the GitHub API, keeps the highest semver (drafts are always skipped, pre-releases behind `include_prerelease_updates`), ranks the attached bundles for the current OS/arch and flags the preferred one as `recommended`. `download_update` streams into the downloads folder while emitting `update-download-progress`; `open_update_file` / `reveal_update_file` / `open_release_page` hand off to the OS (URLs are restricted to this repository). UI: startup probe + banner in `App.tsx`, dialog in `components/UpdatePanel.tsx`, toggles under Settings → About & Updates.
- **API layer**: `src/api.ts` wraps `invoke` + `listen`. `src/types.ts` is the canonical TS shape for `AppConfig`, `Skill`, `SkillsStatus`, `McpServer`, `SubAgent`, `AgentOrchestration`, `Profile`, `AgentMissionSnapshot`, `ModelMetadata`.
- **Theming**: two independent dimensions in `AppConfig` — `theme` (`auto` / `light` / `dark`) and `palette` (`default` / `fig` / `cream`). `applyTheme()` in `src/App.tsx` writes them to `<html>` as `data-theme` / `data-palette`; the token blocks in `src/App.css` do the rest. Settings → Appearance previews both immediately and persists them on "Save Changes". The palette field lives in the Rust `AppConfig` too, so it survives a config write.
- **Workspace state** (`src-tauri/src/workspace.rs`): single source of truth for the per-workspace layout. Everything workspace-scoped (SQLite database, memory, todos, skills, skill status) lives in the hidden `<workspace>/.data` directory, so switching `workspace_dir` in config swaps the whole state — `apply_config` closes and re-opens the SQLite connection at `<workspace>/.data/chat.db` when the workspace changes.
- **Persistence**: SQLite (`<workspace>/.data/chat.db`) for history, API request monitor, interaction logs, and agent missions. `app.log` stays in the app data dir.

## On-disk layout (runtime)

Application files live in the OS app data dir (`name` from `tauri.conf.json` identifier `com.leonard.aichat`):

- macOS: `~/Library/Application Support/ai-chat/`
- Linux: `~/.local/share/ai-chat/`
- Windows: `%APPDATA%/../Local/ai-chat/`

App-data contents: `config.json`, `mcp_servers.json`, `sub_agents.json`, `profiles.json`, `timers.json`, `app.log`, and a `workspace/` dir (default — overridable via `workspace_dir` in config).

Everything that belongs to the *current workspace* lives in a hidden `.data` directory inside it (see `src-tauri/src/workspace.rs`):

```text
<workspace>/.data/
  chat.db                  SQLite: history / API monitor / interaction log / agent missions
  memory/{user,repo}/…     memory tool file backend
  todos/<session>/…        todos tool
  skills/<name>/skill.md   the only skills root
  skills.status            per-workspace skill enablement + metadata (JSON)
```

- `.data` is hidden (Windows: file-system hidden attribute) but it is **part of the workspace**: `file_actions` lists, searches, reads and writes it like any other folder (skills, memory, todos, skill status). The one exception is the SQLite database — see the path policy below.
- **Self-evolution mode** (`config.self_evolution_mode`):
  - **Skill edits are restricted to `<workspace>/.data/skills`** — the one and only skills root, produced by `skills::self_evolution_skill_root` (a single `Option<PathBuf>`, never a list: a second skills location cannot be introduced). `tools/paths.rs` refuses to delete the skills root itself, and only that folder gets the automatic `.bak.<n>` backups.
  - **The main chat and the sub-agents both get that root.** `llm_complete.rs` / `agents.rs` pass it to `tools::execute_tool` as `protected_skill_root`; without it every skill write is rejected.
  - **`file_actions` and `memory` are forced on.** `AppConfig::effective_tools()` adds them whenever the mode is on, and `apply_config` / the startup load persist them — so the Tools panel, a profile, a backup restore or a hand-edited `config.json` cannot switch them off. The Tools panel shows them locked.
  - **A dedicated `evolution` memory scope** (`tools/memory.rs`, `EVOLUTION_SCOPE`) holds verified, reusable facts (default interpreter / runtime environment, the correct way to operate a tool, frequently used file locations). It is file-backed at `<workspace>/.data/memory/evolution`, only exists while the mode is on, is injected into the system prompt as "VERIFIED FACTS", and receives the automatic conclusion recording.
- `chat.db` moves with the workspace: `apply_config` re-opens the SQLite connection when `workspace_dir` changes, so history, monitor logs and missions are per workspace.
- `.data/skills` is **not** created automatically; skill listing and the memory store create what they need on demand.
- `skills.status` is the source of truth for which skills are enabled. It is per workspace and entries are never pruned automatically — a skill folder that disappears stays recorded (and simply stops reaching the model) until the folder returns. `config.selected_skills` no longer exists.

## Skills — path isolation rule

> Except for explicitly requested paths, any operation executed by a skill MUST treat the directory containing the skill's `SKILL.md` as its root. Operating outside this root is forbidden; all in-skill paths are evaluated relative to it.

This is enforced by the skills/tools layer. Don't add code that lets a skill escape its `SKILL.md` directory.

## Tools & dangerous commands

- Tools: `run_cmd`, `run_shell`, `file_actions`, `knowledge_graph` (Neo4j backend), plus the `memory`, `todos` and `timer` tools — all in `src-tauri/src/tools/` (`skill_read` is auto-injected when skills are active).
- **`run_shell` run types**: `direct` (default) waits for the script to exit and returns its output; `serve` starts a long-lived background child (HTTP server, watcher…) and returns a `serve_id` immediately. The same tool manages them: `action: "list"` shows running/finished entries with recent output, and `action: "stop"` with `serve_id` (or `"all"`) kills the whole tree. Serve state lives in `tools/serve.rs`; stopping a serve process never asks for confirmation because it only targets children this app started. **Serve processes live only for the chat turn that spawned them**: the `chat_completion` wrapper calls `ServeRegistry::shutdown_all` after every turn (final answer, error or Stop), which drains the registry, kills each tree, polls until the pids are confirmed gone — escalating with a second whole-tree kill once — and logs the verified summary (`exit NOT confirmed` on the pathological leftover).
- **Child-process cleanup** (`src-tauri/src/process_guard.rs`): every long-lived child (stdio MCP server, `serve` process, `cli` sub-agent) is registered by pid. `RunEvent::Exit` and the main-window `CloseRequested` hook call `kill_all()`, which terminates each process tree. On Windows children are also assigned to a job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so even a hard crash cannot leave MCP servers / CLI agents / serve processes running. On Unix children opt into their own process group (`process_group(0)`) so `kill(-pid)` reaches grandchildren.
- **Path policy** — the three execution tools have deliberately different scopes:
  - **`file_actions`** is strictly workspace-scoped — any path that resolves outside the workspace root is rejected outright, with no confirmation override. The workspace **includes the hidden `.data` state folder** (skills, memory, todos, skill status): it is listed, searched, read and written like any other folder. The single exception is the SQLite database (`.data/chat.db` and its `-wal`/`-shm`/`-journal` sidecars) — `file_actions` can only truncate or delete it, never rewrite it, so it would destroy the chat history, the API monitor, the interaction log and the agent missions; `tools/paths.rs::is_workspace_database` rejects it.
  - **`run_cmd`** runs a *program*, not a file: the program must be a bare name the OS resolves through `PATH` (`python`, `python3`, `git`, …). Absolute and relative paths are refused (`tools/process.rs::validate_path_resolved_program`, enforced both in the tool handler and in `build_command`). Use `run_shell` when a path is really needed. It starts in the workspace (`current_dir` + `PWD`).
  - **`run_shell`** has full freedom: it starts in the workspace but its working directory is **not** restricted — scripts may `cd` anywhere and run anything. Every script is audited by `tools/risk.rs` (risk score, syntax gate, confirmation for L0–L4) before it runs.
- **Sub-agents are typed** (`agent_type` in `sub_agents.json`): `llm` runs the built-in autonomous loop, `cli` launches a headless coding CLI installed on this machine (`agents/cli.rs`: PATH + `PATHEXT` resolution, Windows `.cmd`/`.bat`/`.ps1` wrapping, output streaming on `agent-task-token`, process-tree kill on timeout/Stop), `cloud` posts the task to a remote HTTP endpoint with optional job polling (`agents/cloud.rs`). All three are configured in the Sub Agents panel and reachable from the main agent through `call_subagent` **only while the `use_agents` (自动编排) switch is on** — with it off the tool is neither advertised nor callable, and the main agent answers directly. `cli`/`cloud` agents ask for approval once per app session unless their `require_approval` switch is off (kind `external_agent`). A `cli` agent's `model_args` are appended on every run (`{model}` comes from the agent's model field; literal `-m <id>` entries work without one), and its `continue_args` (`--continue`) are appended once the same agent has completed a successful run in the same chat and working directory — `AppState::cli_sessions` remembers that triple in memory, so a new chat or an app restart starts a fresh CLI session. Only a CLI process that exits 0 is a success: a non-zero exit or a timeout closes the mission as `failed` and emits `agent-task-error` instead of a green "done" badge.
- Some command patterns raise a frontend confirmation dialog. The Rust side blocks on `confirm_command` until the user replies. **If a tool call appears to hang, check the React tree for a pending `ConfirmDialog` / dangerous-command prompt before debugging the Rust side.**
- `ConfirmKind` values: `dangerous` (L0), `system_config` (L1), `user_software` (L2), `user_data` (L3), `external_agent` — the L0-L3 kinds mirror `RiskLevel::confirm_kind()` in `tools/risk.rs` (L4-L6 never prompt). Users can auto-accept kinds via `auto_accept_confirm_kinds` in config; the Tools panel only offers kinds the backend can actually emit (`external_agent` is approved per agent in the Sub Agents panel instead), plus a one-click master switch that stores the `"*"` wildcard (matches every kind, including future ones).

## Tauri 2 specifics for this repo

- **Identifier**: `com.leonard.aichat` (set in `tauri.conf.json`).
- **Window label**: `main` (the only window).
- **Window chrome is custom**: `decorations: false` + `shadow: true`; `src/components/TitleBar.tsx` draws the drag region (`data-tauri-drag-region`) and the window buttons via `@tauri-apps/api/window`. The required permissions live in `capabilities/default.json` (`allow-start-dragging`, `allow-minimize`, `allow-toggle-maximize`, `allow-close`, …). The titlebar is fixed above overlays; fullscreen pages start at `top: var(--titlebar-h)` (`src/App.css`). The native File/Tools/About menu stays for macOS, and the same 4 actions are exposed as commands (`app_open_data_dir`, `app_open_markdown`, `app_export_profile`, `app_import_profile`) for the titlebar's `⋯` menu.
- **Plugins registered in `lib.rs`**: `tauri-plugin-opener`, `tauri-plugin-dialog`, `tauri-plugin-clipboard-manager`. No `fs` / `shell` / `http` plugins. If you need them, add the crate to `Cargo.toml`, register with `.plugin(...)`, AND add the permission to `src-tauri/capabilities/default.json` (Tauri 2 denies by default).
- **App menu**: defined in `lib.rs` — items include `open-app-data-dir`, `save-profile`, `restore-profile`, `markdown-edit`, `about`.
- **Vite dev server**: port `1420`, `strictPort: true`. If the port is busy, Tauri dev fails. HMR uses `1421` if `TAURI_DEV_HOST` is set.
- **Vite watcher ignores** `**/src-tauri/**` (so Rust edits don't trigger Vite reloads — recompile via `cargo`).
- **Commands return** `Result<T, String>` (or another `Serialize` error). Frontend `invoke()` rejects on `Err` — always `await`.

## Project layout

```
src/                       # React 19 + TS frontend
  App.tsx                  # main UI shell
  api.ts                   # invoke/listen wrappers
  types.ts                 # shared TS types
  components/              # ChatMessage, HistoryPanel, SettingsPanel,
                           # SkillsPanel, ToolsPanel, McpPanel, AgentsPanel,
                           # AgentMissionPanel, ProfilePanel, MonitorPanel,
                           # MarkdownPreview, TodoList, ToolCallGroup, Portal,
                           # TitleBar

src-tauri/
  src/lib.rs               # state, menu, command registration, app setup
  src/llm_complete.rs      # chat_completion (SSE streaming)
  src/agents.rs            # sub-agent types, missions, orchestration
  src/agents/
    cli.rs                 #   `cli` backend: headless coding CLIs
    cloud.rs               #   `cloud` backend: remote HTTP agent endpoints
  src/skills.rs            # SKILL.md frontmatter parser, list/save/delete,
                           # workspace skill status (.data/skills.status)
  src/workspace.rs         # hidden `<workspace>/.data` layout (paths + hidden attr)
  src/process_guard.rs     # app-wide child registry + process-tree kill / Windows job
  src/tools/               # tool executor, split by feature:
    mod.rs                 #   dispatch/schemas/shared helpers (+ tests)
    memory.rs              #   memory tool (session/user/repo + "all" fan-out)
    file_actions.rs        #   file_actions handler, backups, diff compare
    patch.rs               #   LLM-hardened unified patch parser/applier
    paths.rs               #   workspace sandbox + shell cwd validation
    process.rs             #   command building, run_command, child output decoding
    serve.rs               #   `run_shell` serve processes (start/list/stop)
    risk.rs                #   L0-L6 risk assessment, syntax gate
    search.rs              #   integrated search engine
    timer.rs               #   delay timers (fire → resume a session, + tests)
    todos.rs               #   todo lists (+ tests)
    neo4j_db.rs            #   Neo4j client wrapper (pub, re-exported at root)
  src/mcp.rs               # MCP stdio/SSE/HTTP/Streamable-HTTP, warmup, test
  src/model_metadata.rs    # llm-metadata catalogue fetch (capabilities/context)
  src/agents.rs            # sub-agents, orchestration, missions
  src/db.rs                # SQLite: history, API monitor, interaction log
  src/logger.rs            # file + println AppLogger
  capabilities/default.json
  tauri.conf.json
  Cargo.toml

scripts/sync-version.cjs   # 3-way version sync (pkg / cargo / tauri.conf)
```

## Common gotchas

- **Production build fails on missing git tag.** `npm run build` → `sync-version:git-tag` calls `git tag --sort=-creatordate`; no tag = hard error. Tag first or use `--version x.y.z`.
- **Tauri 2 permissions are runtime, not compile-time.** A new `fs`/`shell`/`http` call will fail at runtime if the permission is missing from `capabilities/default.json`.
- **`async` commands returning `Result<T, E>`** — `E` must be `Serialize`. Use `String` for ad-hoc errors.
- **Vite `strictPort: 1420`** — don't change the port; Tauri expects it.
- **Workspace dir** is mutable at runtime via `get_workspace_dir` / config; skills, profiles, and the in-app browser operate on it. Hot-swap carefully — open handles may be invalidated.
- **Tauri codegen**: `src-tauri/gen/` is auto-generated (gitignored). If IPC types look stale, run `npm run tauri dev` once to regenerate.
- **Draft releases are invisible to the update check.** `GET /releases` never returns drafts, so an in-app “update available” only shows up after the draft created by CI is published.
