For people who want to read, modify or release the code. Regular users do not need this page.

## Stack

| Layer | Technology |
| --- | --- |
| Frontend | React 19, TypeScript, Vite |
| Desktop shell | Tauri 2 (`decorations: false`, custom title bar) |
| Backend | Rust, Tauri command system |
| HTTP / streaming | `reqwest` + `futures-util` (SSE) |
| Database | `rusqlite` (SQLite) |
| Serialization | `serde` / `serde_json` / `serde_yaml` |
| PDF → Markdown | `pdf2md-rs` (workspace member crate, in-process) |
| Knowledge graph client (library, not a tool) | `neo4rs` (Neo4j Bolt) |
| Packaging | Tauri bundler, MSIX (`makeappx`), GitHub Actions |

Crate/binary name: `rabbit-chat-studio` (`src-tauri/Cargo.toml`), library target `rustgui_lib`; the npm package and the app id stay `rabbit-chat-studio`.

## Architecture

```text
┌────────────────────────────── React 19 (src/) ──────────────────────────────┐
│  App.tsx  ChatMessage  HistoryPanel  SettingsPanel  SkillsPanel  ToolsPanel  │
│  McpPanel  AgentsPanel  MonitorPanel  ProfilePanel  UpdatePanel  TitleBar    │
└───────────────┬───────────────────────────────────────────────┬─────────────┘
                │ invoke(command, args)                         │ listen(event)
                ▼                                               ▲
┌──────────────────────────── Tauri 2 (src-tauri/src/) ─────────┴─────────────┐
│ lib.rs             command registration / AppState / workspace / menu / update│
│ llm_complete.rs    Chat Completions (SSE), tool loop, compression, stop      │
│ llm_complete/      turn loop, context compressor, stream, conclusion          │
│ turn_guard.rs      turn budget (rounds/tool calls) + tool-failure guard      │
│ agents.rs          sub-agents, planning, execution, mission state            │
│ agents/cli.rs      local CLI child processes                                  │
│ agents/cloud.rs    remote HTTP endpoints + polling                            │
│ agents/files.rs    file-backed `llm` definitions (Markdown frontmatter)       │
│ agents/exchange.rs `.data/deliver` deliverable exchange area                      │
│ agents/mbus.rs     mission bus: pub/sub + live status registry                │
│ agents/supervisor.rs planner supervisor (event-driven, mbus-only tools)        │
│ instance.rs        single instance / shadow windows on forked configs         │
│ pdf_mentions.rs    `@paper.pdf#pages` → Markdown via pdf2md-rs                │
│ mcp.rs             stdio / SSE / HTTP / Streamable HTTP                       │
│ skills.rs          SKILL.md parsing, skill status                             │
│ workspace.rs       <workspace>/.data layout                                   │
│ backup.rs          export / import zip                                        │
│ model_metadata.rs  merged model catalogue (models.dev + OpenRouter + llm-metadata)
│ update.rs          update check / download                                    │
│ logger.rs          file / terminal logging                                    │
│ process_guard.rs   child registry and process-tree cleanup                    │
│ tools/             files, commands, patches, search, risk, memory, todos,     │
│                    timers, background tasks, self-built MCP (mcp_self)       │
│ db.rs              SQLite: history / api_requests / interaction_log / missions│
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
                         <workspace>/.data/chat.db + file state
```

### Lifecycle of a chat turn

1. The frontend calls `invoke("chat_completion", …)`.
2. The backend installs a fresh `Arc<AtomicBool>` cancel flag for **this turn**; the previous flag stays `true` forever.
3. If orchestration is on and agents are enabled: `agents::orchestrate` plans → executes sub-agents → injects the mission report into the main context.
4. Messages are assembled: system prompt, loaded skills, history, session summary, todos, mission report, last user message.
5. Predictive compression may run first (see [Prompts and Context](Prompts-and-Context)).
6. `POST {base_url}/chat/completions` with `stream: true`.
7. SSE is parsed; the backend emits `chat-token` / `chat-reasoning-token` / `chat-usage`.
8. If the model returns `tool_calls`, tools run; results are appended as `tool` messages and the loop continues.
9. The **goal loop** may continue the turn while the todo list has open items (max 4 continuations), unless a pending timer or `[WAIT_EVT: …]` suspends it.
10. `chat-done` on success; `chat-error` on failure. Control markers are stripped before the row is stored.
11. Messages, tool calls and tokens are written to SQLite.

### Stop

- `stop_chat_completion` sets **this turn's** flag.
- CLI agents poll every 250 ms and kill their process tree.
- `llm` agents exit at loop boundaries.
- Cloud requests stop waiting.
- Remaining tasks are marked skipped; the mission becomes `stopped`.
- Planning/execution check between steps; they do not cancel an in-flight LLM request.
- A new turn installs a new flag, so pressing Stop while idle cannot affect the next turn.

### Main events

| Event | Direction | Purpose |
| --- | --- | --- |
| `chat-token` | backend → frontend | Answer stream |
| `chat-reasoning-token` | backend → frontend | Reasoning stream |
| `chat-usage` | backend → frontend | Tokens (the turn cost chip is derived from these) |
| `chat-retry` | backend → frontend | A dropped stream is being retried |
| `chat-done` / `chat-error` | backend → frontend | End / error |
| `chat-injected` | backend → frontend | A mid-turn update or background result was attached to the assistant row |
| `chat-notice` | backend → frontend | Floating warning/error notice (budget exhausted, tool disabled, …) |
| `tool-call` | backend → frontend | Tool status text |
| `todo-state` | backend → frontend | Live todo updates |
| `skill-loaded` | backend → frontend | Skill loaded |
| `timer-state` / `timer-fired` | backend → frontend | Timers |
| `sleep-lock` | backend → frontend | System-awake lock state (badge above the input) |
| `agent-plan-start` / `agent-task-start` / `agent-task-token` / `agent-task-reasoning` / `agent-task-tool` / `agent-task-done` / `agent-task-error` / `agent-planner-state` / `agent-planner-action` / `agent-aggregate-done` | backend → frontend | Orchestration progress — task rows own their status, planner decisions stream as feed rows |
| `confirm-required` | backend → frontend | Risk confirmation |
| `workspace-changed` | backend → frontend | Workspace switch |
| `update-download-progress` | backend → frontend | Update download |

## Design principles

The core intent: reason from first principles, return freedom to the user (tools, LLM API, prompts), and keep the interface minimal and professional.

1. **Minimal dependency closure**: all model calls go through OpenAI-compatible APIs.
2. **Capabilities decoupled from permissions**: chat by default; tools opt-in; skills narrow them.
3. **Local-first observability**: config, history and request logs stay local.
4. **Explicit boundaries**: skill directory isolation, tool allowlists, workspace path constraints.
5. **Replaceable**: frontend, command layer, model service and graph backend are modular.

In code:

- `get_all_tools(selected_tools)` decides the tool list;
- `execute_tool(..., allowed_commands, ...)` is the single execution entry point;
- `resolve_safe_path` / `ensure_mutation_target_allowed` are path boundaries;
- `process_guard` is the process boundary;
- `workspace.rs` is the state boundary.

## Project structure

```text
ai-chat/
├─ src/                         React frontend
│  ├─ App.tsx                   main UI shell
│  ├─ api.ts                    invoke/listen wrappers
│  ├─ types.ts                  shared TS types
│  ├─ fileMention.ts            `@file` popup query/scoring (line ranges)
│  ├─ pathRef.ts                `&path` reference grammar (no attachment)
│  ├─ slashMention.ts           `/skill` popup query/scoring
│  ├─ attachments.ts            materializes `@path` mentions on send
│  ├─ components/               Chat/History/Settings/Skills/Tools/MCP/Agents/
│  │                            Profile/Monitor/Update/TitleBar/FileMentionPopup/
│  │                            SlashMentionPopup/BackgroundTaskGroup…
│  ├─ i18n/                     en / zh-CN / zh-TW
│  └─ utils/                    model catalogue matching (modelMetadata.ts), etc.
├─ src-tauri/
│  ├─ src/lib.rs                entry, commands, AppState, menu
│  ├─ src/llm_complete.rs       SSE chat, tool loop, compression, stop
│  ├─ src/llm_complete/         turn.rs / context.rs / stream.rs / conclusion.rs
│  ├─ src/agents.rs             sub-agents, orchestration, missions
│  ├─ src/agents/cli.rs         CLI backend
│  ├─ src/agents/cloud.rs       cloud backend
│  ├─ src/agents/exchange.rs    `.data/deliver` deliverable exchange area
│  ├─ src/agents/files.rs       `llm` definitions (Markdown parse/serialize/scan)
│  ├─ src/agents/mbus.rs        mission bus (pub/sub + status registry)
│  ├─ src/agents/supervisor.rs  planner supervisor
│  ├─ src/instance.rs           single instance / shadow windows
│  ├─ src/turn_guard.rs         turn budget + tool-failure guard
│  ├─ src/pdf_mentions.rs       `@paper.pdf#pages` conversion + cache
│  ├─ src/mcp.rs                MCP client
│  ├─ src/skills.rs             SKILL.md parsing, skill status
│  ├─ src/workspace_files.rs    workspace file listing for `@` mentions
│  ├─ src/workspace.rs          `.data` layout
│  ├─ src/backup.rs             export/import
│  ├─ src/model_metadata.rs     merged capability catalogue
│  ├─ src/update.rs             update check/download
│  ├─ src/logger.rs             file/terminal logging
│  ├─ src/process_guard.rs      child cleanup
│  ├─ src/tools/                tool implementations
│  │  ├─ mod.rs                 dispatch/schema/tests
│  │  ├─ file_actions.rs
│  │  ├─ patch.rs
│  │  ├─ paths.rs
│  │  ├─ risk.rs
│  │  ├─ scripts.rs             run_cmd script library (resolve, metadata, approvals)
│  │  ├─ search.rs
│  │  ├─ serve.rs
│  │  ├─ background.rs
│  │  ├─ memory.rs
│  │  ├─ todos.rs
│  │  ├─ timer.rs
│  │  └─ mcp_self.rs            self-built MCP servers (self-evolution mode)
│  ├─ utils/pdf2md-rs/          PDF → Markdown library crate (workspace member)
│  ├─ utils/knowledge_graph/    knowledge graph client library (not wired into the tool set)
│  ├─ capabilities/default.json Tauri permissions
│  └─ tauri.conf.json
├─ scripts/
│  ├─ sync-version.cjs          3-way version sync
│  ├─ gen-changelog.cjs         CHANGELOG.md from git tags/commits
│  ├─ gen-wiki.mjs              docs/wiki/*.md → docs/site/wiki/**.html
│  ├─ build-msix.ps1            MSIX packaging
│  └─ capture-store-screenshots.ps1
├─ docs/
│  ├─ wiki/                     wiki source: nav.json + en/ + zh/
│  ├─ site/                     GitHub Pages (static, no build step)
│  ├─ gen.py                    generates site/architecture.html in CI
│  ├─ store/                    Store listing assets
│  ├─ DESIGN.md                 UI design spec
│  └─ agents/                   developer docs: architecture / tech stack / modules / interface / workflow
├─ .github/workflows/           pages.yml / release.yml / store.yml / winget.yml
├─ AGENTS.md                    docs index — the rules live in docs/agents/
└─ CHANGELOG.md                 generated, do not hand-edit
```

## Database

`<workspace>/.data/chat.db` main tables:

| Table | Contents |
| --- | --- |
| `history` | Session messages, tool calls, attachments, reasoning |
| `session_meta` | Title, favorite, archived |
| `session_summaries` | Compressed session summaries |
| `api_requests` | Metrics for each LLM request |
| `interaction_log` | LLM/tool/MCP interaction details |
| `agent_missions` | Sub-agent missions |
| `agent_tasks` | Tasks inside a mission |
| `agent_bus_messages` | Mission-bus publish history (replay for the Mission panel) |

## Development commands

```bash
# Install dependencies
npm install
cargo install tauri-cli --version "^2"

# Development (Vite :1420 + Tauri)
npm run tauri dev

# Frontend only
npm run dev

# Type check
npx tsc --noEmit

# Frontend build (no git tag needed)
npx vite build

# Rust check
cargo check --manifest-path src-tauri/Cargo.toml

# Rust unit tests
cargo test --manifest-path src-tauri/Cargo.toml

# Production build (requires the newest git tag)
npm run tauri build
```

Documentation and packaging:

```bash
python3 docs/gen.py              # architecture.html into docs/site
node scripts/gen-wiki.mjs        # docs/wiki → docs/site/wiki
node scripts/gen-changelog.cjs   # regenerate CHANGELOG.md from git
pwsh scripts/build-msix.ps1      # MSIX bundle (Windows + makeappx)
```

Version sync:

```bash
node scripts/sync-version.cjs --from-git-tag
node scripts/sync-version.cjs --bump patch
npm run sync-version:git-tag
npm run sync-version:bump-patch
```

## Release flow

1. Merge into `main`.
2. Tag and push:

   ```bash
   git tag v3.3.3
   git push origin v3.3.3
   ```

3. `.github/workflows/release.yml` builds on 4 runners.
4. The first platform publishes the Release; the rest upload afterwards.
5. `store.yml` builds the `.msix` workflow artifact; it is not attached to the Release.
6. `winget.yml` is inactive for now: it submits only once the package is accepted into `winget-pkgs` and `WINGET_TOKEN` exists.

See [Installation and Build](Installation-and-Build) and [Updates](Updates).

## Roadmap

From the README:

- add model-provider presets (OpenAI / Azure / local gateways);
- tool invocation auditing and replay;
- skill signing and publishing;
- end-to-end tests and security baseline scans.

## Contributing

- Read `docs/DESIGN.md` before changing UI: use only the tokens in `src/App.css`; 4px radii, a single Electric Blue accent, no shadows, one motion curve.
- Update the schema and tests when changing Rust tools.
- When adding a Tauri plugin, add the crate, register `.plugin(...)`, and add the permission to `capabilities/default.json` (Tauri 2 denies by default).
- Wiki pages live in `docs/wiki/{en,zh}/*.md`; the HTML under `docs/site/wiki/` is generated and must not be committed. Run `node scripts/gen-wiki.mjs` after editing.
- Before committing, run `npx tsc --noEmit` and `cargo check`; run `cargo test` for tool logic.
- Never commit real API keys or user data.

## License

**MIT**. See `LICENSE` in the repository.

## Related documents

- `README.md` / `README.zh.md`: project introduction and quick start
- `AGENTS.md`: the docs index — which file to read for architecture, stack, modules, interface and workflow
- `docs/agents/`: the developer docs themselves (architecture, tech stack, modules, interface design, workflow)
- `docs/DESIGN.md`: UI design spec
- `config.example.md`: config field examples
- `CHANGELOG.md`: generated release notes
