# Rust GUI Project — Tauri 2.0 (AI Chat)

This is an **OpenAI-compatible chat desktop app** built with Tauri 2.0 (Rust backend) + React + TypeScript frontend.
Users can configure any OpenAI-compatible API endpoint, switch models, and use **Skills** (named system prompts), **Tools**, and **Sub-Agents** to customize assistant behavior.

## Stack

| Layer | Technology |
|-------|-----------|
| Desktop shell | Tauri 2.0 |
| Backend logic | Rust — `src-tauri/src/lib.rs` |
| Frontend | React 19 + TypeScript + Vite |
| IPC | Tauri commands + streaming events |
| HTTP client | `reqwest` 0.12 (async + SSE streaming) |

## Key Architecture

- **`chat_completion`** Tauri command streams SSE from the API and emits `chat-token` / `chat-done` / `chat-error` / `chat-usage` events to the frontend.
- **Config** (`get_config`/`save_config`) stored as `config.json` in the OS app data dir (`~/.local/share/ai-chat/` on Linux, `~/Library/Application Support/ai-chat/` on macOS).
- **Skills** are configured locally. **Skill Path Isolation Rule**: Except for explicitly requested paths, any operation executed by a skill MUST use the directory containing the skill's `SKILL.md` as its root path. Operating on paths outside this root is strictly forbidden. All paths referenced within a skill are evaluated relative to this root path.
- **Sub-Agents** support task orchestration with dedicated events (`agent-task-start` / `agent-task-token` / `agent-task-done` / `agent-task-error`) and a top-level orchestration switch (`use_agents`).
- **Tools** include `run_cmd`, `run_shell`, `file_actions`, and `knowledge_graph`. Dangerous command patterns trigger a frontend confirmation flow before execution.
- Frontend entrypoint: `src/App.tsx`. API layer: `src/api.ts`. Types: `src/types.ts`.
- Main UI components: `ChatMessage`, `HistoryPanel`, `SettingsPanel`, `SkillsPanel`, `ToolsPanel`, `McpPanel`, `AgentsPanel`.

## Prerequisites

```sh
# Rust toolchain
rustup update stable

# Tauri CLI (v2)
cargo install tauri-cli --version "^2"

# Node.js deps (from project root)
npm install   # or pnpm install / yarn
```

## Build & Dev Commands

> **Note**: The default `~/.cargo/config` uses the USTC git registry which may be unavailable. Use the `--config` override below for any `cargo` command that fetches dependencies:

```sh
# Dev (hot-reload) — run from project root
npm run tauri dev
# If cargo needs to fetch deps, run with the mirror override:
cargo check --manifest-path src-tauri/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'

# Frontend only (Vite)
npm run dev

# Production build
npm run tauri build

# Rust-only type-check
cargo check --manifest-path src-tauri/Cargo.toml

# Frontend type-check
npx tsc --noEmit
```

## Project Layout

```
ai-chat/
├── src/                  # Frontend source (TypeScript/JS/HTML)
│   ├── App.tsx
│   ├── api.ts
│   ├── types.ts
│   └── components/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs       # Tauri app entry point
│   │   ├── lib.rs        # Command handlers & app state
│   │   ├── llm_complete.rs
│   │   ├── tools.rs
│   │   ├── skills.rs
│   │   ├── mcp.rs
│   │   ├── agents.rs
│   │   ├── db.rs
│   │   └── logger.rs
│   ├── Cargo.toml        # Rust dependencies
│   └── tauri.conf.json   # Tauri app config (identifier, bundle, permissions)
├── package.json
└── vite.config.ts
```

## Tauri 2.0 Key Conventions

- **Commands**: Expose Rust functions to the frontend with `#[tauri::command]` and register them in `tauri::Builder::invoke_handler`.
- **Permissions (v2 breaking change)**: Every capability (filesystem, shell, HTTP, etc.) must be declared in `src-tauri/capabilities/*.json`. Tauri 2.0 denies all by default — add required permissions explicitly.
- **Plugin system**: Core features (clipboard, dialog, fs, http, shell, notification) are now separate crates under `tauri-plugin-*`. Add them to `Cargo.toml` and register with `.plugin(tauri_plugin_xxx::init())`.
- **Events**: Use `app_handle.emit("event-name", payload)` (Rust→JS) and `listen("event-name", handler)` in JS (JS→Rust via `emit`).
- **State management**: Share app state across commands via `tauri::State<T>` — register with `.manage(MyState::new())`.
- **Window labels**: Multi-window apps address windows by label string defined in `tauri.conf.json` or created at runtime.

## Common Pitfalls

- `tauri.conf.json` field `identifier` must be a valid reverse-domain string (e.g. `com.example.rustgui`) — build fails otherwise.
- Capability JSON files must be placed in `src-tauri/capabilities/` and referenced; missing capabilities cause runtime permission errors, not compile errors.
- `async` Tauri commands must return `Result<T, String>` (or a serializable error type) — the frontend `invoke()` rejects on `Err`.
- Avoid blocking the async runtime in commands; use `tokio::task::spawn_blocking` for CPU-heavy work.
- Frontend `invoke()` calls are **async** — always `await` them.
- If command execution appears to stall, check whether a dangerous-command confirmation is pending in the frontend.

## Useful References

- [Tauri 2.0 docs](https://v2.tauri.app/)
- [Tauri 2.0 migration guide](https://v2.tauri.app/start/migrate/from-tauri-1/)
- [Plugin list](https://v2.tauri.app/plugin/)
- [Security model & capabilities](https://v2.tauri.app/security/)
