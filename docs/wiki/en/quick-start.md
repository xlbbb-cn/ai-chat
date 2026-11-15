This page takes you from "just downloaded" to "the model can answer, and it knows which workspace it is in". Model parameters, tools and skills have their own pages.

## 1. Install

### Microsoft Store (Windows, recommended)

Open <https://apps.microsoft.com/detail/9MV82J9G95GS> (Store ID `9MV82J9G95GS`) and click **Get**; the protocol link
`ms-windows-store://pdp/?productid=9MV82J9G95GS` opens the Store page directly.

The Store build is an MSIX package: the Store installs it and keeps it updated, it needs neither administrator rights nor a manual WebView2 install (Windows 10 2004+ / Windows 11 required). Two differences from the GitHub installer build:

- Its app config is redirected to `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\Roaming\com.leonard.aichat`, so it is **not shared** with the installer build; the `.data` folder inside your workspace is shared by both.
- The in-app update check answers "up to date" for the Store build — version updates come from the Store (see [Updates](Updates)).

### From the GitHub Release

Open [Latest release](https://github.com/xlbbb-cn/ai-chat/releases/latest) and pick your platform:

| Platform | File |
| --- | --- |
| Windows | `Rabbit-Chat-Studio-Windows-x64-setup.exe` (NSIS) |
| macOS Apple Silicon | `Rabbit-Chat-Studio-macOS-Apple-Silicon.dmg` |
| macOS Intel | `Rabbit-Chat-Studio-macOS-Intel.dmg` |
| Linux x86_64 | `Rabbit-Chat-Studio-Linux-x86_64.AppImage`, `.deb` or `.rpm` |

### Package managers (not available yet)

There is no package-manager channel yet: the winget, Homebrew and Scoop manifests are not published, so `winget install`, `brew install --cask` and `scoop install` cannot find the package. Use the Microsoft Store or the GitHub Release installer above; the commands will be added here once the manifests exist.

### First-launch notes

- **Microsoft Store build**: nothing extra to do; it works on first launch. If you previously used the installer build you will see a "first run", because the two keep separate configs.
- **macOS**: builds are ad-hoc signed (no Apple Developer certificate). Right-click the app → **Open**, or use **System Settings → Privacy & Security → Open Anyway**.
- **Linux AppImage**: make it executable first.

  ```bash
  chmod +x Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ./Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ```

- **Windows**: if WebView2 is missing, install the [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/). If you build from source and hit GNU linker errors, switch to the MSVC toolchain.

## 2. Configure a model

Open **Settings → API & Model**.

### Required fields

| Field | Description | Example |
| --- | --- | --- |
| Base URL | Root of the OpenAI-compatible API, usually ending in `/v1` | `https://api.openai.com/v1` |
| API key | Key from the provider; local services may accept any value | `sk-...` |
| Model | Model id used by the current chat | `gpt-4o-mini` |

Common base URLs:

| Service | Base URL |
| --- | --- |
| OpenAI | `https://api.openai.com/v1` |
| DeepSeek | `https://api.deepseek.com/v1` |
| OpenRouter | `https://openrouter.ai/api/v1` |
| Ollama (local) | `http://localhost:11434/v1` |
| LM Studio (local) | `http://localhost:1234/v1` |
| vLLM (local) | `http://localhost:8000/v1` |

> Stop at `/v1`; do not append `/chat/completions`. The app adds the endpoint itself.

### Fetch the model list

Click **Fetch Models From API** (the button shows a spinner while both requests are in flight):

1. The app calls `GET {base_url}/models` and merges the returned ids into **Model List**.
2. It also loads the capability catalogue — three public feeds (models.dev, OpenRouter and `basellm/llm-metadata`) merged into one flat file, `<app_data>/model_metadata.json`, refreshed weekly. The first build downloads roughly 7 MB and is cached for 7 days.
3. Matched capabilities (tools, reasoning, vision, files, audio, context window) appear as badges and are stored in `config.model_metadata`. Names are normalised (lowercase, flattened) and matched exactly first, then by similarity.
4. If the provider reports a context window in `/models`, it wins; otherwise the catalogue value is used.

A failed fetch does not block manual use: enter a model id under **Add Model Manually** and click **Add**.

### Context window

The context window is used by the usage bar and the agent's context budget. Priority:

1. `config.model_context_lengths[model]` (auto-filled from `/models` or entered manually);
2. legacy `model_settings.max_tokens`;
3. a conservative global default.

If the provider does not report it and the catalogue has no match, set the real value manually (for example `128000`); otherwise long chats will compress earlier than necessary.

### Save

Click **Save Changes** in the top-right corner. The new configuration is used by the next turn; no restart is needed.

## 3. First chat

1. Return to the main window and type in the input box.
2. **Enter** sends; **Shift+Enter** inserts a newline.
3. While the reply streams, the send button becomes **Stop**; click it to cancel the turn.
4. Reasoning-capable models show a collapsible **Thought Process** block above the answer.
5. The turn footer reports what the turn cost (tokens and, when known, price) next to the latency.
6. Type `@` in the input to attach a workspace file (add `#12-24` for a line range) and `/` to insert a skill reference, `+agent` to embed a sub-agent role, and `@@name` to address a file in `.data/tmp`. Images, PDFs and audio only attach cleanly when the selected model has the matching capability badge.
7. Paste or drop a file into the composer and it is copied to `.data/tmp` and attached as an `@` mention.
8. A finished reply can be saved as Markdown or exported to a PDF file; drawings in the reply save as PNG (static) or HTML (dynamic).

More detail: [Interface and Chat](Interface-and-Chat).

## 4. Choose a workspace

Open **Settings → Workspace & Profiles** and click **Browse…** next to **Workspace Directory**.

- If unset, the default workspace is `workspace/` inside the app data directory.
- Once selected, the app creates a hidden `.data/` folder there and stores the chat database, skills, memory and todos inside it.
- Switching the workspace switches all of that state: history, skill enablement, memory and todos, and it lands on a fresh conversation.
- If the saved path no longer exists (a moved or unmounted directory), the app falls back to the workspace in the app data directory instead of failing to start; pick the directory again from Settings.
- If the workspace is a Git repository, add `.data/` to `.gitignore`.

Full layout: [Workspace and Data](Workspace-and-Data).

## 5. Enable capabilities as needed

None of these is required for the first chat:

| Capability | Entry point | Tutorial |
| --- | --- | --- |
| File access and command execution | **Tools** | [Files and Commands](Files-and-Commands) |
| Reusable workflows (skills) | **Skills** | [Skills](Skills) |
| External MCP tools | **MCP** | [MCP Servers](MCP-Servers) |
| Sub-agents and orchestration | **Agents** | [Sub-Agents](Sub-Agents) |
| Memory / todos / timers | **Tools** | [Memory, Todos and Timers](Memory-Todos-and-Timers) |

## 6. Where the data lives

Application-level files (config, MCP, sub-agents, profiles, logs) live in the OS app data directory:

| Platform | Path |
| --- | --- |
| Windows | `%APPDATA%\com.leonard.aichat\` (Roaming) |
| macOS | `~/Library/Application Support/com.leonard.aichat/` |
| Linux | `~/.local/share/com.leonard.aichat/` |

Workspace-level files live in `<workspace>/.data/`:

```text
<workspace>/.data/
  chat.db                 chats / request monitor / interaction log / agent missions
  memory/{user,repo}/…    memory files
  todos/<session>/…       todo lists
  skills/<name>/skill.md  skills
  skills.status           skill enablement for this workspace
  timers.json             this workspace's pending delay timers
  scripts/                run_cmd script library
  tmp/                    composer pastes + PDF mention cache
  deliver/<session>/…     sub-agent deliverables, one directory per batch
```

## 7. Next steps

- To let the model read and write code: read [Tools Overview](Tools-Overview) first, then enable **File Actions**.
- To run builds/tests: see `run_cmd` / `run_shell` in [Files and Commands](Files-and-Commands).
- To make it follow a fixed workflow: write a [Skill](Skills).
- To connect external systems: configure [MCP Servers](MCP-Servers).
- To parallelize large tasks: configure [Sub-Agents](Sub-Agents).

## Quick troubleshooting

| Symptom | Check |
| --- | --- |
| `401 Unauthorized` | API key correctness/expiry; organization headers |
| `404 Not Found` | Base URL has too many or too few `/v1` segments; did you include `/chat/completions`? |
| Empty model list | Provider may not implement `/models`; add the model manually |
| Fetch Models spins for a while the first time | The capability catalogue is downloading (~7 MB, cached for 7 days) |
| Attachment skipped | The selected model lacks vision/files/audio capability |
| Slow or interrupted replies | Provider rate limits, proxy, or context length; see [Monitoring and Logs](Monitoring-and-Logs) |
