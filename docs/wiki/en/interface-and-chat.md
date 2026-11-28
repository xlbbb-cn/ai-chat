This page covers the window layout and a complete chat turn: how models are configured, how messages and attachments are sent, when context is compressed, and what each message button does.

## Window layout

The app uses a frameless window with a custom title bar:

- The left side shows the current workspace folder name with a thin red underline.
- The middle is the drag region (`data-tauri-drag-region`); double-click maximizes/restores.
- On macOS the window buttons are the platform's traffic lights — 12px red / amber / green dots in close / minimize / maximize order, with the native `×`, `−` and zoom-triangle glyphs appearing only on hover or keyboard focus — and they sit on the left. Windows and Linux keep the neutral minimize / maximize / close icon buttons on the right.
- The `⋯` menu contains: open app data directory, open workspace directory, open deliverables folder (the sub-agent exchange area `.data/deliver`), export backup, restore backup, Markdown editor, report an issue, About Rabbit Chat Studio.
- On macOS the app additionally shows the native menu bar (File / Edit / Tools / About). On Windows and Linux no native menu bar is drawn — the title-bar `⋯` menu above is the only menu, and the webview handles the clipboard shortcuts natively.
- With the glass appearance enabled, the window is transparent and the desktop behind it is blurred by the system (Acrylic, with Mica as the fallback); only the chrome turns translucent, `.messages` keeps the opaque canvas so reading is unaffected. Settings → Appearance drives the level live, and hosts without a backdrop keep the opaque canvas.
- An empty chat (no messages yet) shows a hopping smiley face; while the self-evolution "nap" pass runs it is replaced by its progress line (phase, round, tool calls), and a finished pass leaves a clickable result bubble.

Top toolbar:

| Button | Purpose |
| --- | --- |
| New Chat | Clear the current session and create a new `session_id` |
| Agents | Sub-agent list and orchestration settings |
| Skills | Workspace skills, enable/edit |
| MCP | MCP server management |
| Tools | Built-in tool toggles, auto-accept |
| History | Search, favorite, archive, rename, delete sessions |
| Settings | Appearance, API & Model, System Message, Advanced, Run, Debug, About & Updates |

The small number next to a button is the number of enabled items (skills, MCP servers, tools, agents). The side-panel lists share one row language: transparent rows, inset hairlines, a hover fill, relative timestamps, and the orchestration rows folded behind a single disclosure — only **Enable mode** stays visible.

## Appearance and chat background

Every choice in Settings → Appearance **previews immediately**: palette, glass and language apply the moment you pick them and are written to `config.json` on **Save Changes**; a chosen chat background is copied into the app data directory and shown right away, and is persisted the same way.

- **Color mode**: follow the system, or force light / dark.
- **Palette**: `default`, `cream`, `macaron` (pink-and-white macaron tints with a rose accent). All three carry light and dark token sets; only a palette tints the chat area with a faint same-family wash and retints every nested card (the running card, reply nesting, footer, composer top edge) — it paints the surfaces inside its own scope, so it works without the glass effect.
- **Glass**: `off` / `subtle` / `standard` / `strong`. With glass on the window frame turns transparent — the system backdrop shows the blurred desktop when it provides one (Acrylic on Windows, Mica as the fallback), other platforms show the app's own soft wash; `.messages` keeps the opaque canvas by default and only takes the photo when a background is set. At `off` the title bar and toolbar become opaque, which is lighter on a weak GPU or a remote desktop. The side panel and the Settings page are opaque at every level: they hold the 13px text and never show what is behind them.
- **Chat background**: lays only over the message area, never the toolbar or composer. The chosen photo is copied into the app data directory (`backgrounds/chat-background-<id>.<ext>`), so moving or deleting the original afterwards changes nothing; **Use the default wallpaper** brings back the built-in one (copied into `backgrounds/` on first use too). The photo is cover-cropped under a thin white veil that holds the contrast down; if the file is missing the UI simply has no background and offers to pick another. The background travels with configuration backups.

## The notification card

When a turn settles (success or failure), the assistant asks a question, or the app waits for an approval — and the main window is minimized or unfocused — a card slides into the bottom-right corner:

- It is the app's own frameless **webview window** (fixed 348 × 112), floating at the bottom-right of the work area. It **never takes focus**: your current application stays in front, and the card hides as soon as the main window gains focus.
- It dismisses after 6 s, paused while the pointer is over it; if the card's webview never answers it is force-hidden after 30 s.
- Its one interaction is a click: hide the card and bring the main window back (un-minimized, focused).
- The body is the first reply line or the error text, squashed to one line and capped at 160 characters; `done` / `error` / `ask` / `confirm` only color the title.
- Sound is a separate channel: success plays the *session finished* sound, failure and asks play the *message* sound. With the window in front a success is silent and a failure only sounds (no card); a turn the user stopped sends nothing at all.
- The master switch is **Settings → Run → Allow notifications** (`config.notify_when_hidden`, default on). The card window is prewarmed a few seconds after startup, so a very early notification waits for it to mount instead of showing blank.

## Model configuration

See [Quick Start](Quick-Start) for the first-run setup; this section explains what each field does.

### API & Model

| Field | Purpose |
| --- | --- |
| Base URL | OpenAI-compatible root, e.g. `https://api.openai.com/v1` |
| API key | Sent as `Authorization: Bearer <key>` |
| Model | Default model for the current chat |
| Model list | All ids available in the bottom model picker |
| Fetch Models From API | Calls `/models` and matches the capability catalogue (the button spins while both load) |
| Add Model Manually | Add an id when the provider has no `/models` |
| Context window | Token window for the selected model; used by the usage bar and compression |
| Model capabilities | Tools, reasoning, vision, files, audio, open weights, deprecated |

### Capability badges

The capability catalogue merges three public feeds (models.dev, OpenRouter and `basellm/llm-metadata`) into one flat snapshot at `<app_data>/model_metadata.json`, refreshed weekly and cached for 7 days; the first build downloads roughly 7 MB. Entries are stored under normalised names (lowercase, flattened), so a lookup is exact first and only falls back to the similarity scorer for unknown ids. Matches are shown as badges and stored in `config.model_metadata`. They affect:

- the icons in the model picker;
- which attachment types the picker allows;
- whether the **Thinking Depth** selector appears;
- the agent context budget.

Settings shows "Matched <name> · N% similarity"; use it to judge whether a fallback match is trustworthy. If nothing matches, the app stays permissive and does not block attachments.

### Advanced parameters

**Settings → Advanced**:

| Parameter | Description |
| --- | --- |
| Temperature | `temperature`, 0–2; empty = provider default |
| Top P | `top_p`, 0–1; empty = provider default |
| Max completion tokens | `model_settings.max_complete_tokens`, per-reply limit |
| DS-Format | Pass `reasoning_content` back to DeepSeek-family thinking models; keep off for other providers |

Two different limits:

- **Context window** (`model_context_lengths`) is "how much the whole conversation can hold";
- **Max completion tokens** is "how much this one reply can generate".

### Thinking depth

When the model is marked as reasoning-capable, or already has a stored effort, a brain-icon selector appears next to the input:

- Default (do not send `reasoning_effort`; let the provider decide)
- Low / Medium / High

The choice is stored per model in `config.model_reasoning_effort[model]` and sent as `reasoning_effort`. If the provider rejects the parameter, keep **Default**.

## Sending and streaming

- **Enter** sends; **Shift+Enter** inserts a newline.
- Type **`@`** to reference a workspace file and **`/`** to reference a workspace skill — both open a picker that never steals focus (see [Composer mentions](#composer-mentions-file-skill)).
- **`+agent`** embeds a sub-agent role in a new conversation, and **`@@name`** addresses a file in `.data/tmp`; pasting or dropping a file into the composer copies it there and attaches it as an `@` mention.
- **Background work is grouped, not sprinkled**: running background tasks and serve processes appear in one collapsible **Background** card at the end of the reply that started them, with running/waiting counters and cancel/stop buttons. Their results are delivered to the model — they are not printed into the transcript as chat bubbles.
- The newest assistant reply mirrors the session's **working plan**: a compact todo card with a completed/total counter and the running item. It restores with the session, so a reloaded conversation still shows the current plan; older replies stay clean. When the turn ends by waiting on a timer or an external event, the open todos are suspended instead of being reported as finished, and the `[goal:DONE]` control marker never appears in the bubble.
- Each turn footer reports what the turn cost: tokens and, when the provider prices them, the amount — next to the latency.
- Replies stream over SSE; the send button becomes **Stop** while generating. The live tail is revealed character by character — a continuous fade cascade that spans chunk boundaries, so streaming looks like one smooth typewriter pass instead of blocky updates.
- **The composer stays usable while a turn streams.** Pressing Enter then appends your text to the *running* turn instead of starting a new one: it is delivered as a `user` message at the next request boundary, announced in the transcript as an injected update. If the reply has already made its last request, the window is closed and your text is re-sent as a normal next turn.
- Stop cancels the turn: the main agent stops, sub-agent process trees are killed, and remaining tasks are marked as skipped.
- Reasoning models may show a collapsible **Thought Process** block; during streaming it previews the last few lines.
- Code blocks have a copy button; math is rendered with KaTeX.
- You **can switch away while generating**: starting a new chat or opening another history session hands the turn to a background driver — its history row shows a **Running** badge with elapsed time, switching back re-attaches the live transcript, **Stop** stops only that turn, and each conversation keeps its own draft. See [Sessions and History](Sessions-and-History#a-reply-keeps-running-in-the-background).

### When the stream breaks

Long thinking streams are not killed by a wall clock: requests run without a total timeout and only use a 120 s read timeout. On top of that:

- transient HTTP statuses (429, 5xx) are retried, honouring `Retry-After`;
- a mid-stream disconnect is retried even after tokens streamed — the dead attempt's tail is dropped first;
- a reply truncated by `finish_reason: "length"` is continued once or twice instead of ending silently;
- a stream that closes without `finish_reason`/`[DONE]` is treated as an error, not as a finished answer;
- after three attempts the turn fails with the error visible in the transcript.

## Composer mentions (`@file`, `/skill`)

The composer has two inline pickers; they replaced the old paperclip button:

- **`@` — reference a file.** Typing `@` at a word boundary opens a popup listing the files the model could be shown: workspace files, hidden and `.gitignore`-excluded paths skipped, capped at 20 000 entries. `↑`/`↓` move, `Enter`/`Tab` accept, `Esc` closes; the popup never takes focus. Accepting inserts `@path` into the text.
- **`/` — reference a skill.** Typing `/` lists the workspace skills (`.data/skills`). Accepting inserts the plain text `/skill-name` into the message — nothing is attached; the model resolves the token with its `use_skill` tool at run time.
- **`@@` — address a temp file.** Files under `.data/tmp` (pastes and drops) are reachable as `@@name`, so a long path never has to be typed.
- **`+` — embed a sub-agent role.** In a new conversation, `+agent` starts the turn with that sub-agent's role, without opening the Agents panel.
- Only one popup is open at a time: inside a word the `@` and `/` grammars are mutually exclusive, so no stacking rule is needed.

### Line ranges

A `#` suffix narrows a text attachment to specific lines:

| Mention | Attaches |
| --- | --- |
| `@src/main.rs` | the whole file |
| `@src/main.rs#12-24` | lines 12–24 |
| `@src/main.rs#12-` | line 12 to the end of the file |
| `@src/main.rs#12` | line 12 only |

Files that cannot be sliced by lines (images, binaries) attach the whole file and say so.

### PDF mentions count pages

A PDF has no lines, so its `#` suffix counts **pages**:

| Mention | Converts |
| --- | --- |
| `@paper.pdf` | the first 60 pages |
| `@paper.pdf#2` | page 2 |
| `@paper.pdf#2-5` | pages 2–5 |
| `@paper.pdf#2-` | page 2 to the last page |

The pages are rendered to **Markdown text** by the built-in `pdf2md-rs` converter and inlined into the prompt like any text attachment — no model capability is involved, so a PDF mention works with models that have no **Files** badge. Details:

- A progress strip above the input shows the conversion ("converting `paper.pdf` — page 3 of 30") and clears when it finishes or fails.
- Conversions are cached at `<workspace>/.data/tmp/pdf/<name>-<digest>.md`; the same mention reuses the cache, and a converter upgrade invalidates it.
- The prompt copy is capped at 60k characters; the cache file always holds the full conversion, so a tool can read the rest later.
- A failed conversion (encrypted, scanned, page past the end) is reported per mention and never blocks the message.

### What a mention becomes on send

On send, every `@path` is materialized into an attachment — the capability rules below still apply:

| Kind | How it is sent | Required model capability |
| --- | --- | --- |
| Text (`txt`/`md`/code/log/JSON/CSV…) | Inlined into the prompt | None; any model |
| Image (`png`/`jpg`/`webp`/`gif`/`svg`…) | `image_url` content part | Vision |
| PDF (`pdf`) | Converted to Markdown and inlined (see above) | None; any model |
| Audio (`wav`/`mp3`) | `input_audio` content part | Audio |

Details:

- The popup only offers what the current model could receive; a materialized attachment the new model does not support is removed with a message naming it. PDFs stay in the popup for every model, because they are sent as text.
- Text attachments keep their original text for display and are also inlined into the request.
- Images/PDFs/audio are sent as `data:` URLs or base64; large files significantly increase the request size. Size caps: 8 MiB for text, 24 MiB for binary/image mentions (before base64).
- Attachments belong to the **current input box**; a timer auto-resume does not pick up attachments you are still editing.
- In the transcript, sent messages highlight `@path` tokens (including `#line` ranges) and `/skill-name` tokens as accent chips; the attachments themselves are no longer listed separately under the message.

## Context usage and compression

A thin bar above the input shows the share of the context window used by the current request:

- While generating it shows a sweep animation; after usage arrives it shows the actual percentage.
- At 90% it turns into the warning color.
- The window is resolved from `model_context_lengths[model]` → `model_settings.max_tokens` → a default.

When the request gets close to the window (a predictive pre-check at ~4 characters per token, tools included, plus a saturation guard), the backend compresses automatically:

1. Keep roughly the last 12 messages, starting on a clean round boundary.
2. Summarize the older history with the main model into a 9-section compact prompt (intent, concepts, files, errors and fixes, problem solving, every user message, pending work, current work, next step); tool rounds collapse into a one-line ledger, and a mechanical line summarizer stays as the fallback so compression never fails a turn.
3. Inject the summary as a user message with continuation framing, before the remaining history — the prompt-cache prefix stays stable.
4. Persist the summary with the messages it covers; the next turn verifies the boundary and drops the summarized prefix, and a stale summary is discarded instead of applied.
5. If the provider answers with a context-overflow error anyway, the turn hard-compresses once and retries once.

This is not "history loss": the original messages remain in SQLite; they are simply not all sent to the model anymore.

## Drawings in replies

The assistant can draw, and a drawing arrives as a card in the transcript:

| Kind | Fence | Notes |
| --- | --- | --- |
| Diagram | `mermaid` | Flowchart, sequence, class, state, ER, gantt, pie, mindmap and timeline source, rendered by the transcript; the fence line becomes the caption |
| Flow chart | `flow` | JSON nodes/edges; the layout is computed (dagre) and colours come from the interface tokens, so light/dark and every accent follow |
| 3D scene | `carve` | Boxes, spheres, cylinders, planes, edges and labels with real shading and a contact shadow; drag to orbit |
| 2D SVG | `svg` | Inline vector diagram using the UI tokens |
| Dynamic figure | `html-fig` | A sandboxed frame for anything that needs motion or interaction; it is CSP-restricted, has no network access and only runs after you accept the execution gate |

- With the enhanced drawing toggle off, no drawing instructions are injected: the assistant stops drawing on its own, but a `mermaid` block you paste yourself still renders.
- Flow charts can animate (`flow` sends a packet along each edge, `step` reveals ranks in order) without running a script, and `prefers-reduced-motion` is honoured twice — at build time and inside the emitted style.
- Every figure has a save button: static drawings save as PNG, dynamic ones as HTML.
- A drawing never leaks into the code path: it is rendered in the card, not in a code block above the answer.

## Message actions

Hover over a message to see its buttons:

| Button | Where | Action |
| --- | --- | --- |
| Retry | Unfinished user message | Re-send that user message (e.g. after an error/interruption) |
| Export PDF | Assistant message | Write the rendered message to a PDF file (no print dialog) |
| Save | Drawing card | Save the figure as PNG (static) or HTML (dynamic) |
| Delete | Persisted message | Delete that row from the database |
| Fork | Persisted message | Create a new session ending at that message, keeping the earlier context |

User attachments show as filename pills; images show thumbnails.

## Timer bar

When the assistant creates a delayed task with `timer_set`, a timer bar appears above the input:

- Each chip shows a label and countdown.
- Click `×` to cancel.
- A timer belongs to the session that created it. If it fires while you are in another session, a notice offers to insert its prompt into the current input box.
- Pending timers are mirrored to `<workspace>/.data/timers.json` (per workspace); after an app restart they are re-armed, and overdue ones fire immediately.
- While timers are pending or a reply is streaming, the app asks the OS to stay awake; a **Sleep blocked** badge on this bar shows while that lock is held. It only covers idle system sleep — the display may still dim, and lid close or battery policy can still suspend.

See [Memory, Todos and Timers](Memory-Todos-and-Timers).

## Prompt ownership

Rabbit Chat Studio does not inject a default persona. `config.system_message` is your base prompt; the request only appends the minimum runtime guidance:

- OS information;
- explicit invocation syntax (`{tool:...}`, `{mcp:...}`, `{skill:...}`);
- available skills (name and description only; full instructions are loaded by `use_skill`);
- available sub-agents (only when orchestration is enabled);
- memory/timer/todo policy (when the corresponding tools are enabled);
- session summary and current todo list as dynamic context.

What you write is what the model sees as the base. The exact order is in [Prompts and Context](Prompts-and-Context).

## Common issues

| Symptom | Check |
| --- | --- |
| Send button is greyed out | Input is empty and there are no attachments, or a backup export is running |
| `@` popup shows "No file matches" | The popup only lists files the model could be shown (hidden and `.gitignore`-excluded paths are skipped) — type more of the path, or check the workspace |
| A `@` reference warns "cannot be sliced by lines" | Binary/image mentions attach the whole file; `#` line ranges only work on text files (a PDF's `#` counts pages) |
| A `@paper.pdf` mention fails | Encrypted or scanned PDFs cannot be converted; the message still sends, just without the attachment |
| Cannot pick images | Model has no **Vision** badge; fetch models again or switch models |
| No thinking-depth selector | Model is not matched as reasoning-capable, and no effort was saved for it |
| Usage bar keeps sweeping | Usage has not arrived; some providers do not return usage |
| Old content remains after Stop | Stop only interrupts generation; already-generated content stays |
| A long answer stops early | It was truncated by the provider: the app continues once or twice, then reports it — raise **Max completion tokens** |
