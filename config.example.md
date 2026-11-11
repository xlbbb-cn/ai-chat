# config.json Example

This project stores runtime configuration in app data as config.json.

Typical locations:
- Windows: %APPDATA%/../Local/ai-chat/config.json (depends on Tauri app data path)
- macOS: ~/Library/Application Support/ai-chat/config.json
- Linux: ~/.local/share/ai-chat/config.json

## Example

```json
{
  "api_base_url": "https://api.openai.com/v1",
  "api_key": "sk-...",
  "model": "gpt-4o-mini",
  "model_catalog": ["gpt-4o-mini", "gpt-4.1-mini"],
  "model_settings": {
    "temperature": 0.7,
    "top_p": 0.95,
    "reasoning_effort": "medium",
    "max_tokens": 4096
  },
  "system_message": "You are a helpful assistant.",
  "selected_tools": ["run_cmd", "file_actions"],
  "check_updates_on_startup": true,
  "include_prerelease_updates": false,
  "glass_effect": "standard",
  "log_retention_days": 90
}
```

## Field Notes

- api_base_url: OpenAI-compatible API base URL.
- api_key: API key for the model provider.
- model: Default model name used for chat completion.
- model_catalog: Model options shown in the UI model selector.
- model_context_lengths: Per-model context window in tokens. Filled from `/models` when the provider reports one (OpenRouter, Groq, vLLM…), otherwise from the matched llm-metadata catalogue entry; can be set manually in Settings.
- model_metadata: Per-model capabilities (tools / reasoning / vision / files / audio, context window, vendor) learned from the public basellm/llm-metadata catalogue when the model list is refreshed in Settings. Keyed by remote model id; `model_name` inside the value is the catalogue entry the id was matched to and `match_score` how similar the names were (1 = identical). Informational only — never sent upstream.
- model_reasoning_effort: Per-model "thinking depth" picked next to the model selector in the chat toolbar, forwarded as `reasoning_effort`. Keyed by model id; allowed values are `minimal`, `low`, `medium`, `high` (anything else is dropped so a typo can never be sent upstream). A missing/empty entry means "let the provider decide" and falls back to `model_settings.reasoning_effort`. Only sent for the model that was configured — reasoning models that reject the parameter can be left on Default.
- model_settings: Advanced model parameters forwarded to chat completion.
- system_message: Global system prompt appended to each conversation.
- selected_tools: Enabled tools list. Supported values: run_cmd, run_shell, file_actions, memory, todo_list (alias todo) and timer. A retired id (for example `knowledge_graph`) is dropped on load and on save.
- check_updates_on_startup: Check the GitHub Releases API for a newer published release when the app starts (default true).
- include_prerelease_updates: Also consider releases flagged as pre-release; draft releases are never visible to the public API.
- glass_effect: Frosted-glass strength for the titlebar, toolbar, sidebar, panel headers and modal scrims — `off` (opaque bars, no blur), `subtle` (12px), `standard` (20px, also the default when the field is absent) or `strong` (32px). The backend never interprets it; the frontend writes it to `<html>` as `data-glass`, which selects the `--glass-*` token block in `src/App.css`. On Windows the same level also drives the window's Acrylic backdrop (the DWM system backdrop on 11, the v1809 accent blur on 10): the frame is tinted by `--c-window` so the blurred desktop shows through it, while the transcript stays opaque. `off` clears the backdrop and returns the window to its opaque canvas; a platform without a backdrop does the same on its own.
- log_retention_days: Retention window (days, default 90) for the request/interaction log tables (`api_requests`, `interaction_log`). Older rows are deleted at startup and when the Interaction Monitor compacts the database; 0 keeps logs forever. Chat history is never pruned. Each request stores its full prompt body, so this is the main driver of `chat.db` growth.

## Security

- Do not commit real keys or passwords.
- Prefer environment-specific config files for production deployment.
