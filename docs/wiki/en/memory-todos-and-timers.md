These three tools let the agent continue beyond a single turn: remember conclusions, maintain a plan and wait for slow work. Each is toggled separately under **Tools**.

## Memory (`memory`)

### Scopes

| Scope | Storage | Lifetime | Good for |
| --- | --- | --- | --- |
| `session` | Process memory | Current session; cleared on app restart | Task context, in-progress notes |
| `user` | `<workspace>/.data/memory/user/<key>.md` | Across sessions, per workspace | User preferences, general insights |
| `repo` | `<workspace>/.data/memory/repo/<key>.md` | Across sessions, per workspace | Codebase conventions, build commands, project facts |
| `evolution` | `<workspace>/.data/memory/evolution/<key>.md` | Across sessions, per workspace | Verified, reusable facts — see below |

> `user` is "user-level" but still stored inside the current workspace's `.data`. It does **not** cross workspaces. Switching workspaces switches memory.

### The `evolution` scope (self-evolution mode)

The fourth scope exists **only while self-evolution mode is on**. It is the assistant's own long-term notebook for facts it has *verified* and that must survive across sessions:

- the **runtime environment** — which interpreter is the default one (`python` vs `python3`), the package manager, the shell, the path to a project interpreter;
- the **correct way to operate a tool** — how to drive the browser, how to run the project's tests, the exact command that works;
- the **file locations you use often** — important paths, output directories, config files.

Two things make it more than a folder:

1. **The prompt points at it, not preloads it.** The system prompt only tells the assistant to search this scope when a problem needs earlier experience; the model queries it on demand with `memory list` / `get` / `search`.
2. **Automatic conclusion recording** goes here instead of `repo` while the mode is on.

The scope is rejected when the mode is off:

```text
Error: the 'evolution' memory scope is only available while self-evolution mode is on.
```

#### Ranking and the budget

Entries are not listed alphabetically. Each one carries YAML front matter (`key`, `title`, `kind`, `tags`, `importance`, `confidence`, `evidence`, `uses`, `pinned`, `source`, timestamps) and is scored by:

```text
2 × importance + confidence + 1.5 × ln(1 + evidence) + 1.2 × ln(1 + uses) + 3 × e^(−age/30)
```

`pinned` (or `importance ≥ 5`) floats an entry above everything else. That ranking is what `list` / `search` return — experience-weighted rather than by key — and what the Memory panel uses to mark top-tier entries.

Re-adding an existing key is an **upsert** (`evidence += 1`), a similar title merges into the existing entry, and identical bodies are collapsed at read time. Legacy files (`# key` + free text) are upgraded lazily; the prompt builder never writes.

Inspect the result in **Settings → Debug → Memory** (the row only appears while the mode is on): it shows the same ranking and marks the top tier.

#### Consolidating memory into a skill

When the notebook grows, `memory action="consolidate"` folds a hot cluster into one skill:

| Phase | What it does |
| --- | --- |
| `plan` | Clusters the ranked entries by tags, title tokens and kind; reports the blocks that occupy ≥20% of the budget and have at least 4 entries, with the keys and a suggested skill name |
| `apply` | Takes `keys` + the full `skill_md`: validates the name, parses the skill, verifies every key is covered, writes the skill (with `.bak.n`), enables it in `skills.status`, and moves the entries to `<workspace>/.data/memory/evolution/archive/<skill>/` |
| `restore` | Moves a `skill`'s archived entries back into the active notebook |
| `list` | Lists the archives and their entry counts |

`apply` is a single transaction in that order. The archive is excluded from `list` and `search`, while `memory get` still resolves an archived key. A consolidated skill is enabled automatically so its one-line description reaches the prompt; its body is loaded on demand with `use_skill`.

### Actions

| Action | Parameters | Description |
| --- | --- | --- |
| `add` | `key`, `content`, optional `tags`, `importance`, `title` | Add or overwrite the entry with the same key (upsert bumps `evidence`) |
| `get` | `key` | Read by key (also resolves archived entries) |
| `list` | — | List entries in the scope, ranked |
| `search` | `query` | Search keys and content |
| `delete` | `key` | Delete an entry |
| `consolidate` | `phase` (`plan` / `apply` / `restore` / `list`), `keys`, `skill_md`, `skill` | Fold evolution entries into a skill archive — self-evolution mode only |

`scope: "all"` runs `get` / `list` / `search` across all scopes and returns a merged view; `add` and `delete` **must** use a concrete scope.

### Automatic conclusion recording

When the memory tool is enabled, the backend also checks the final answer at the end of each turn. If it contains clear conclusion markers (e.g. "the root cause", "the fix is", "we decided", or Chinese equivalents) and the model did not call `memory` itself, the app distils the answer (first sentence plus up to three bullets, ≤320 characters) and writes it to a file-backed scope:

```text
<workspace>/.data/memory/repo/conclusion-<session>-<timestamp>.md      ← normal mode
<workspace>/.data/memory/evolution/conclusion-<session>-<timestamp>.md ← self-evolution mode
```

It records a summary, not the whole answer, and caps the number of `source: auto` entries — so the notebook does not grow with every turn. Review it with `memory list scope="repo"` and delete stale entries.

### Memory files and file_actions

Memory files live in `<workspace>/.data/memory/`, which is inside the workspace — so `file_actions` can read and write them like any other file. Prefer the `memory` tool anyway: it keeps the key → file mapping, the tags and the merged listing consistent. The one thing `file_actions` must never touch is `.data/chat.db`.

### Examples

```text
memory add scope="user" key="response-style" content="The user prefers concise Chinese answers: conclusion first, then reasons."
memory add scope="repo" key="build" content="Frontend check: npx tsc --noEmit && npx vite build. Rust: cargo check."
memory add scope="evolution" key="python-default" content="The default interpreter on this machine is `python` (3.12), not `python3`."
memory search scope="all" query="build"
```

## Todos (`todos`)

### Scope

Todos are **session-scoped**:

```text
<workspace>/.data/todos/<session_id>/
  .active                 current list id
  lists/<list_id>.json    list contents
```

The main agent and parallel sub-agents in the same session share one list. A new chat gets a new `session_id`, so it does not inherit the old list.

### Actions

| Action | Parameters | Description |
| --- | --- | --- |
| `list` | — | Return the current list, item ids and statuses |
| `add` | `title`, optional `description` | Append an item, initially `pending` |
| `update_status` | `todo_id`, `status` | `pending` → `in_progress` → `completed` / `cancelled` |
| `clear_completed` | — | Remove completed items |
| `archive` | optional `new_title` | Archive the current list and start a new one (default title `Working plan`) |
| `ask` | `questions` (1–8), optional `topic` | Open an interactive question form above the input and block until the user answers |

### Question form (`ask`)

When a decision needs the user's choice — an approach, a name, a trade-off — the model can ask instead of guessing. `questions` is an array of 1–8 items, each `{ title (required), description?, options?, allow_custom? }`; `topic` is an optional header for the form. Limits: ≤12 options per question, ≤500-char title, ≤2000-char description, ≤300-char option text. Without `options` a question is free-text only; with options, a manual input is offered beside them only when `allow_custom` is true.

The call **blocks the turn** until the user answers: the form appears as a card **inside the assistant's reply**, on the line right after the reply's output — one tab per question, one card per question, and one submit button for the whole form. Typing clears the selected option and selecting clears the typed text. Dismissing the form, pressing Stop, or 15 minutes without an answer ends the wait too. (When the reply that asked is not on screen — e.g. the app was reloaded mid-question — the form falls back to a panel above the input box, so the waiting turn can always be unblocked.)

Every outcome returns structured JSON — never an error — so a user who was away or stopped the turn is not counted as a tool failure:

```json
{ "status": "answered", "answers": [ { "id": "q1", "question": "…", "answer": "…", "source": "option" } ] }
{ "status": "dismissed", "note": "…continue with your best judgement…" }
{ "status": "timeout", "note": "…you may ask again later…" }
{ "status": "cancelled", "note": "The turn was stopped while the question was pending." }
```

`source` is `option` when the answer was a listed choice and `custom` for typed text. Only malformed arguments (`questions` missing, empty or over 8 items) return an `Error:` result. One form is open at a time — parallel sub-agents queue theirs — and the panel stays visible when you switch to another chat, so the waiting turn cannot deadlock.

### UI

The session's working plan is mirrored under the **newest** assistant reply and updates live via `todo-state` events. The card reads the session's active list when it mounts, so a restored or reloaded conversation still shows the current plan. Collapsed it is two lines: the list title with a completed/total counter, then the running item (or the first pending one). When nothing is pending or running the card is hidden, and only the newest reply mirrors the list — historic replies stay clean. It is display-only; the model changes statuses through the `todos` tool.

### Goal loop

An unfinished list is not decoration: while the list still has `pending` / `in_progress` items, the end of a reply does **not** end the turn. The backend re-injects the fresh list (rendered from the current state, not the snapshot taken at turn start) with an instruction to keep working, and the model continues — up to **4 continuations** per turn, then the loop stops so a confused model cannot spin forever.

Two things end the turn without forcing the goal:

- **A pending delay timer.** The remaining items are intentionally on hold until the notification arrives.
- **An explicit `[WAIT_EVT: what you are waiting for]`** in the final answer — the same wait, for a signal the app cannot schedule (a CI run, a colleague's reply, a deployment window).

Both are logged as *Todo goal suspended*; the plan resumes by itself on the next turn, when the timer fires or the user replies. Markers (`[goal:DONE]`, `[WAIT_EVT: …]`) are control signals, not text: they are stripped from the bubble before it is stored, and a reply containing nothing else does not leave an empty bubble in the transcript.

### Tips

- Have the model `list` before planning to avoid duplicate plans.
- Write actionable, verifiable items, not "handle it".
- Use `clear_completed` or `archive` at the end of a long task; leave a long-running plan open only when you want the loop to keep going.
- Todos are a model-facing plan, not a project-management system; use memory or external tools for cross-session work.
- Use `ask` when a decision needs the user's choice instead of guessing — but do not ask what the conversation already answers.

## Timers (`timer`)

### Why

Some tasks cannot finish in one turn: a 30-minute log collection, a long build, a download, a deployment, a rate-limit window. Sleeping or polling in a shell command wastes a turn and tokens. A timer lets the model end the turn and come back later — and, together with the goal loop above, it is the sanctioned way to wait.

### Tools

| Tool | Parameters | Description |
| --- | --- | --- |
| `timer_set` | `delay_seconds`, `message`, optional `label` | Schedule a one-shot timer |
| `timer_list` | — | List pending timers (id, label, fire time, remaining seconds, message) |
| `timer_cancel` | `timer_id` | Cancel |

`delay_seconds` range: 1 second – 30 days (2,592,000 seconds).

### Firing behavior

1. When due, the backend emits `timer-fired`.
2. The frontend injects `message` as a new user message into the session that created it:

   ```text
   ⏱️ [Timer fired: <label>]
   <message>
   ```

3. If a reply is streaming, the message is queued and sent when idle.
4. If you moved to another session, it does not hijack the current chat; a notice offers to insert the prompt into the current input box.
5. Timers survive an app restart: pending entries are mirrored to `<workspace>/.data/timers.json`, re-armed at startup, and overdue ones fire immediately. The mirror is **per workspace**, so switching the workspace flushes the current set back to the old file and arms the new workspace's set.
6. While a timer is pending — or any chat turn is still running — the app holds a shared `sleep-lock` that asks the OS not to idle-suspend (Windows `SetThreadExecutionState`, macOS IOPM assertion; on Linux it takes every inhibitor the session offers and releases each the way it was acquired). The lock is released once the last timer is gone and the turn has finished; failures are logged once and never block the timer.

### Timer bar

Above the input:

- label;
- countdown (`mm:ss` or `h:mm:ss`);
- cancel button;
- a **Sleep blocked** badge while the sleep lock is held.

### Write a good message

`message` is the **only new instruction** in that future turn; it must be self-contained. Do not write "continue". State what to check, where the output is and what to do next.

Good:

```text
The log collection started at 15:07 should be finished. Check ./logs/collect-2026-09-24.log, summarize the errors by root cause, and continue with fix suggestions. If the file is missing, inspect the serve process output and explain why.
```

Bad:

```text
Continue the previous task.
```

### Limits

- Sub-agents never get the timer tools; only the main agent can schedule.
- Timers are one-shot; schedule another for a repeated wait.
- Cancelling a timer does not delete chat history.
- The app must be running to fire; timers that expire while the app is closed fire on next startup.

## Combined workflow

A typical long task:

1. `run_shell serve` starts a log collection script.
2. `todos add` records the follow-up as an open item.
3. `timer_set delay_seconds=1800` with a message describing what to check.
4. Tell the user it will continue in 30 minutes and end the turn — the goal loop stays suspended meanwhile.
5. The timer fires; the model resumes with full history and continues the plan.
6. If the user says "stop waiting", the model calls `timer_cancel` and cancels the chip.

## FAQ

| Symptom | Check |
| --- | --- |
| Memory not found | Scope; workspace switched; session memory cleared on restart |
| `scope: "all"` cannot add | By design; add/delete need a concrete scope |
| `evolution` scope rejected | Self-evolution mode is off; the scope only exists while it is on |
| The notebook keeps filling with auto conclusions | Expected: each is distilled and `source: auto` entries are capped; delete stale ones or run `memory action="consolidate"` |
| `consolidate` reports nothing to do | Needs a cluster of ≥4 entries occupying ≥20% of the budget |
| An entry disappeared from the prompt after `apply` | It was archived under `.data/memory/evolution/archive/<skill>/`; use `phase: "restore"` to bring it back |
| Todo card not updating | Is the Todos tool enabled? Did the model call `todos`? |
| The turn keeps going after a "final" answer | The list still has open items — that is the goal loop; finish or cancel them, or wait via a timer / `[WAIT_EVT]` |
| Turn ends even though items are open | A timer is pending, the answer declared `[WAIT_EVT: …]`, or the 4-continuation budget ran out |
| The turn seems stuck with a form open | That is `todos ask` waiting for your answers — fill the form in the assistant's reply (or above the input box) and submit, or press Stop; it also ends by itself after 15 minutes |
| Timer did not fire | Is the app running? Does `<workspace>/.data/timers.json` exist? Was the session deleted? Did the workspace switch since it was set? |
| Timer fired in another session | Normal; use **Insert into input** in the notice |
| Duplicate timers | Call `timer_list` first, then `timer_set` |
| Model keeps sleeping/polling | Enable the Timer tool and emphasize using a timer instead of `sleep` |
