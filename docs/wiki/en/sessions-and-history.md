Every chat has a `session_id`. Sessions, messages, tool calls, attachments, reasoning content and session summaries are stored in the current workspace's SQLite database: `<workspace>/.data/chat.db`. Switching workspaces switches the history too.

## New chat

Click **New Chat** in the top toolbar:

- creates a new `session_id`;
- clears the current message list, attachments and input;
- does not delete the old session; it remains under **History**;
- keeps the current workspace, model and tool settings.

Switching the workspace also lands on a fresh conversation, so a new workspace never opens onto another directory's transcript.

## History panel

Click **History** in the top toolbar. The panel is one flat list of rows: a leading search glyph with a clear button, one segmented control for the tabs, and a relative timestamp on every row (hover it for the full date).

### Search

The search box matches:

- session titles;
- `session_id`;
- message content (SQLite content search; typing is debounced by 180 ms).

The default title comes from the first user message. For multimodal messages the text part is used; if there is only an attachment, it shows "(attachment)".

### Tabs

| Tab | Contents |
| --- | --- |
| All | Every non-deleted session |
| Favorites | Sessions marked as favorites |
| Archived | Sessions marked as archived |

Favorite and archive are session metadata; they do not move or delete messages.

### Session actions

The menu/buttons on each session:

| Action | Description |
| --- | --- |
| Rename | Change the display title; stored in `session_meta` |
| Favorite / Unfavorite | Add/remove from the Favorites tab |
| Archive / Unarchive | Add/remove from the Archived tab |
| Delete | Delete the session's messages and metadata; the adjacent session is loaded afterwards |

### A reply keeps running in the background

While a reply streams you can **switch conversations freely or start a new chat**: the running turn is handed to a background turn driver that keeps listening to its own session's events, keeps streaming and persists the assistant row when it settles, while the composer belongs to the new conversation immediately.

- The history row shows a **Running** badge with this turn's **elapsed time** (it ticks once a second; a reload mid-turn keeps counting from the turn's real start instead of restarting at zero).
- Switching back re-attaches the live transcript — content, reasoning, settled tool rows — and **Stop** stops that conversation's own turn.
- **Each conversation keeps its own input draft**, so unsent text is still there after switching back and forth.
- Confirmation dialogs and ask forms belong to the conversation that raised them, not to the one you are looking at.
- A webview reload re-attaches from the turn snapshot, so a reply that kept running still lands in the database. The one narrow window is a turn that settles between the snapshot and the re-attach.
- **Deleting** a running conversation is refused (stop it or wait for it to finish): the turn is still writing that session's rows and owns its deliverables directory.
- **Switching the workspace** is refused while any turn runs: a turn resolves tool paths against the workspace it started in and writes through that database handle.
- How many conversations may run at once is set by **Settings → Advanced → Max concurrent sessions** (`max_concurrent_turns`, default 4, `0` = unlimited, hard cap 16). A new turn inside the same session supersedes the old one and takes no extra slot.

Every turn-scoped event carries the `session_id` that emitted it and the frontend drops a payload addressed to another session, so parallel replies never bleed into each other.

## Message-level actions

### Retry

When the last message is a **user message** and the turn did not complete (request failure, stop, app restart), that message shows **Retry**. It re-sends the same content as a new turn; the failed attempt's tail is truncated first, so the conversation stays readable.

### Export PDF

The export button on an assistant message writes the reply to a **real PDF file** (no system print dialog): the rendered Markdown/HTML — code blocks, tables, math — becomes a PDF document you pick a destination for. The Markdown editor exports its draft the same way.

### Delete message

Persisted messages can be deleted individually. The row is removed from the database; later messages remain, but the context has a gap.

### Fork from message

The **Fork** button on a persisted message:

- creates a new session ending at that message;
- copies that message and all earlier history;
- leaves the original session untouched;
- useful for returning to a decision point and asking a different question.

## Session deliverables

When a turn runs sub-agents or other background work, the files they produce are kept as that session's **deliverables**; the mission panel lists them per session, and the title-bar `⋯` menu opens the deliverables folder (`.data/deliver`). Delivery does not delete them, so a long mission can be inspected afterwards.

## History and workspaces

- The history database is **workspace-scoped**: `<workspace>/.data/chat.db`.
- After switching workspaces the panel re-queries and does not show the previous workspace's sessions.
- The default workspace is `workspace/` in the app data directory; a custom workspace is selected from the **workspace name in the title bar**.
- If the saved workspace path is gone (moved, renamed, unmounted), the app falls back to the app-data workspace instead of failing; pick the directory again from Settings.
- Deleting the workspace directory deletes history, memory, todos and skills together. Export a backup first.

## Data retention

- **Chat history is never pruned automatically.**
- Request/interaction logs follow **Settings → Debug → Log retention days** (default 90; `0` keeps them forever).
- The Interaction Monitor's **Compact database** removes expired logs and runs `VACUUM`, but never touches chat history.

See [Monitoring and Logs](Monitoring-and-Logs).

## Backup and migration

Two options:

1. **Full export**: title-bar `⋯` → **Export backup…**. The zip contains a `chat.db` snapshot, config, MCP, sub-agents, skills, memory and todos. Best for moving machines or a full restore.
2. **Manual copy**: close the app and copy `<workspace>/.data/chat.db`. Enough for history only, but not for config or skills.

Full details: [Configuration and Backup](Configuration-and-Backup).

## FAQ

| Symptom | Cause |
| --- | --- |
| History is empty after switching workspaces | History is per workspace; switch back |
| Only titles, no content | The list stores a lightweight first-message summary; open the session for full messages |
| Deleted sessions still take disk space | SQLite does not shrink immediately; use **Compact database** |
| A workspace switch was refused | A turn is still running — wait for it or stop it; switching requires every running turn to end |
| Forking changes the original session | Fork should only copy; if it does not, check `session_id` and the database |
| Retry button does not appear | It only appears when the last message is an unfinished user message |
| A reply from another session showed up | It should not: turn events are session-scoped — report it with the request id from [Monitoring and Logs](Monitoring-and-Logs) |
