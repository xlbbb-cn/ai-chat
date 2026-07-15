# AI Chat — Tauri 2.0 (Rust + React)

Desktop AI chat client (Tauri 2 + React 19 + TS + Vite, Rust backend) that talks to any OpenAI-compatible API and adds **Skills**, **Tools**, and **Sub-Agents**.

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

# Rust unit tests (only `todos` and `tools` modules have tests)
cargo test --manifest-path src-tauri/Cargo.toml
```

### Cargo mirror (CN / restricted networks)

`~/.cargo/config` may point at the USTC git registry, which can be unreachable. If `cargo` fails to fetch deps, override the source:

```sh
cargo check --manifest-path src-tauri/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'
```

## Architecture

- **Entry points**: `src/main.tsx` → `src/App.tsx`; Rust `src-tauri/src/main.rs` → `src-tauri/src/lib.rs` (command registration, state, app setup).
- **Streaming chat**: `chat_completion` command opens an SSE connection (`reqwest` + `futures-util`) and emits to the frontend:
  `chat-token`, `chat-reasoning-token`, `chat-done`, `chat-error`, `chat-usage`.
- **Agent orchestration** (toggle via `use_agents` in config): `agent-plan-start`, `agent-task-start`, `agent-task-token`, `agent-task-done`, `agent-task-error`, `agent-aggregate-start`.
- **Profiles**: `save_profile_config` / `apply_profile_config` / `delete_profile_config` zip skills + config for export/import. Triggered from the app menu (`save-profile` / `restore-profile`).
- **MCP warmup**: enabled servers in `mcp_servers.json` are spawned at app startup (`mcp::spawn_warmup`).
- **API layer**: `src/api.ts` wraps `invoke` + `listen`. `src/types.ts` is the canonical TS shape for `AppConfig`, `Skill`, `McpServer`, `SubAgent`, `AgentOrchestration`, `Profile`, `AgentMissionSnapshot`.
- **Persistence**: SQLite (`chat.db`) for history, API request monitor, interaction logs, and agent missions. `app.log` is written next to the DB.

## On-disk layout (runtime)

Config and DB live in the OS app data dir (`name` from `tauri.conf.json` identifier `com.leonard.aichat`):

- macOS: `~/Library/Application Support/ai-chat/`
- Linux: `~/.local/share/ai-chat/`
- Windows: `%APPDATA%/../Local/ai-chat/`

Contents: `config.json`, `chat.db`, `mcp_servers.json`, `agents.json`, `app.log`, `profiles/`, and a `workspace/` dir (default — overridable via `workspace_dir` in config). `workspace/skills/` is created on first launch.

## Skills — path isolation rule

> Except for explicitly requested paths, any operation executed by a skill MUST treat the directory containing the skill's `SKILL.md` as its root. Operating outside this root is forbidden; all in-skill paths are evaluated relative to it.

This is enforced by the skills/tools layer. Don't add code that lets a skill escape its `SKILL.md` directory.

## Tools & dangerous commands

- Tools: `run_cmd`, `run_shell`, `file_actions`, `knowledge_graph` (Neo4j backend), plus `web_search` / `fetch_web` in `tools.rs`.
- Some command patterns raise a frontend confirmation dialog. The Rust side blocks on `confirm_command` until the user replies. **If a tool call appears to hang, check the React tree for a pending `ConfirmDialog` / dangerous-command prompt before debugging the Rust side.**
- `ConfirmKind` values: `dangerous`, `sudo`, `elevation`, `external_path`. Users can auto-accept some kinds via `auto_accept_confirm_kinds` in config.

## Tauri 2 specifics for this repo

- **Identifier**: `com.leonard.aichat` (set in `tauri.conf.json`).
- **Window label**: `main` (the only window).
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
                           # MarkdownPreview, TodoList, ToolCallGroup, Portal

src-tauri/
  src/lib.rs               # state, menu, command registration, app setup
  src/llm_complete.rs      # chat_completion (SSE streaming)
  src/skills.rs            # SKILL.md frontmatter parser, list/save/delete
  src/tools.rs             # run_cmd/run_shell/file_actions/kg/web_search
  src/mcp.rs               # MCP stdio/SSE servers, warmup, test
  src/agents.rs            # sub-agents, orchestration, missions
  src/todos.rs             # todo lists (also has unit tests)
  src/db.rs                # SQLite: history, API monitor, interaction log
  src/neo4j_db.rs          # Neo4j client wrapper
  src/search.rs            # web search / fetch helpers
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
