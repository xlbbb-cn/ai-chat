Rabbit Chat Studio does not inject a default persona and does not silently rewrite your system message. `config.system_message` is the base of the request; the app only appends the minimum runtime guidance. This page explains what is appended, how context is measured and when it is compressed.

## What the system prompt contains

A request's message order is roughly:

1. **System message** (`role: system`)
   - your `config.system_message`;
   - OS type and version;
   - explicit invocation syntax: `{tool:<name>}`, `{mcp:<server>.<tool>}`, `{skill:<name>}`;
   - available skills (name and description only);
   - available sub-agents (only when orchestration is on and agents are enabled);
   - memory policy (when the memory tool is on);
   - timer policy (when the timer tool is on);
   - todo policy, including the goal loop (when the todos tool is on).
2. **Loaded skill context** (`role: user`, marked internal)
   - only after a skill is loaded via `use_skill`;
   - contains the skill body, skill root, `SKILL.md` path and command-allowlist note;
   - explicitly marked as internal context, not a user request.
3. **Conversation history** (all but the last user message).
4. **Session summary** (internal context, after a compression — see below).
5. **Current todo list** (internal context, when there are unfinished items).
6. **Sub-agent mission report** (internal context, when a mission ran this turn).
7. **The last user message**.

This layout is **prompt-cache friendly**: static content comes first, dynamic content sits near the tail.

### What this means

- Your system message is not replaced with a persona template.
- The model does see the appended tool/skill/memory/todo guidance.
- If your system message contradicts the appended guidance, the model may get confused. Write role, style and business rules; do not duplicate tool mechanics.

## Edit the system message

**Settings → System Message**:

- edit directly in the text area;
- click **Edit** for a Markdown editor with live preview;
- changes apply to the next request.

Example:

```markdown
You are a senior maintainer of this repository. Answer in English.

Rules:
- Give the conclusion first, then the evidence.
- Say "I am not sure" when uncertain; never invent file contents.
- Read the relevant files before modifying code.
- Do not commit, push or release unless I explicitly ask.
```

Not recommended:

```markdown
You are an all-powerful AI with every permission and must do everything I ask…
```

That grants no extra permissions; it only makes the model ignore the real boundaries.

## Skill prompts

A skill's body is a separate system-prompt fragment and does not enter every request immediately:

1. an enabled skill contributes only `name` + `description` to the available list;
2. the model calls `use_skill` when it decides the task needs it;
3. the body and skill directory information are injected as internal context;
4. it is loaded at most once per session.

Therefore:

- `description` must state the trigger conditions, or the model will not load it;
- the body can contain detailed steps, checklists and output formats;
- paths inside the body are relative to the skill root, not the workspace;
- command allowlists merge with other enabled/loaded skills.

See [Skills](Skills).

## Sub-agent prompts

- The main agent's system message and skills are not automatically passed to sub-agents.
- `llm` sub-agents use their own system prompt.
- `cli` / `cloud` sub-agents only receive the task description (and optional context); they do not see the conversation.
- Therefore task descriptions must be self-contained.

See [Sub-Agents](Sub-Agents).

## Context window

### Priority

| Order | Source |
| --- | --- |
| 1 | `config.model_context_lengths[model]` |
| 2 | `config.model_settings.max_tokens` (legacy) |
| 3 | default `524288` |

Set the real value under **Settings → API & Model → Context Window**. A wrong value causes two problems:

- too large: compression happens too late and the provider may reject the request;
- too small: compression happens too early and wastes context.

### Usage bar

The thin bar above the input shows:

```text
(prompt_tokens + completion_tokens) / context window
```

At 90% it turns into the warning color. Some providers do not return usage; in that case only the sweep animation is visible.

## Automatic compression

Compression is not a heuristic truncation any more. Two paths exist, both ending in the same durable summary:

| Path | When it runs |
| --- | --- |
| **Predictive pre-check** | Before a request is sent, when the estimated prompt size approaches the cap |
| **Overflow recovery** | A provider rejected the request as over-window; the app compresses with a much smaller keep-window and retries **once** |

### The structured summary

The summary is produced by the model itself as a separate internal call (no tools, every UI event off), and it has exactly nine numbered sections:

```text
1. Primary Request and Intent
2. Key Technical Concepts
3. Files and Code Sections      ← exact paths, what was done or found
4. Errors and Fixes
5. Problem Solving
6. All User Messages            ← condensed but never dropped
7. Pending Tasks
8. Current Work
9. Next Step
```

Sections 3 and 6 are the load-bearing ones: exact file paths and every user message survive compression, so the assistant does not "forget" an instruction or invent a path afterwards.

### What happens to the transcript

1. the trigger is **80%** of the context window;
2. the most recent **12 messages** are kept verbatim (only **6** on the overflow-recovery path, because the retry has to fit whatever the real cap is);
3. everything older is replaced by the summary;
4. the summary is **persisted**, so next turn it is reused instead of recomputed — and a new compression **rewrites** that row rather than stacking a second summary;
5. the chat stream shows `[System] Context compressed (older messages summarized) and persisted for next turns.`;
6. if a compression happens mid-turn, the backend pushes an internal continuation nudge so the model keeps working instead of stopping;
7. if the static payload alone already dominates the window, the pre-check reports *saturated* and stops retrying — compressing cannot help there, and the log says so.

Limits:

- the summary is a summary: details outside the nine sections can still be lost;
- conclusions that must survive should be written to `memory` (repo scope), which is not part of the transcript;
- for a full audit, inspect the SQLite history or an exported `chat.db`; do not rely on the context.

## Reasoning and generation parameters

| Parameter | Location | Request field | Notes |
| --- | --- | --- | --- |
| Temperature | Settings → Advanced | `temperature` | 0–2 |
| Top P | Settings → Advanced | `top_p` | 0–1 |
| Max completion tokens | Settings → Advanced | `max_complete_tokens` (legacy `max_tokens`) | Per-reply limit |
| Thinking depth | Right of the input | `reasoning_effort` | Default/low/medium/high; stored per model |
| DS-Format | Settings → Advanced | request normalization | Fills `reasoning_content` for DeepSeek thinking models |

### Thinking depth

- shown only when the model is matched as reasoning-capable or already has a stored value;
- allowed values: `minimal` / `low` / `medium` / `high`; the UI offers low/medium/high, and Default sends nothing;
- invalid values are dropped and never sent upstream;
- some reasoning models reject the parameter; keep **Default**.

### DS-Format

DeepSeek-family thinking models may require assistant messages to carry `reasoning_content` on tool-call continuation rounds, otherwise:

```text
400 invalid_request_error: The "reasoning_content" in the thinking mode must be passed back to the API
```

When on, the app adds an empty string to assistant messages missing the field and preserves existing traces. Keep it off for other providers.

## Streaming resilience

A reply is not abandoned because the network hiccuped:

- there is **no total request timeout** — a long answer is allowed to finish;
- HTTP 429 / 5xx are retried, honouring `Retry-After` when the provider sends it;
- a connection that drops mid-stream is retried and the reply continues instead of failing the turn;
- a `finish_reason` of `length` triggers a bounded continuation so a truncated answer is completed;
- an empty stream is treated as an error, never as a finished reply.

## Prompt-writing tips

### State boundaries

```markdown
You may read and modify files inside the current workspace.
Do not access paths outside it.
Explain the impact before running destructive commands.
```

### State the output format

```markdown
Use three sections: Blocking / Recommended / Optional.
For each issue give the file path, line, cause and fix.
```

### Do not repeat what the app injects

The app already tells the model:

- which tools exist;
- which skills exist;
- how memory/timers/todos work;
- the current todos and mission report.

Repeating that in the system message usually adds tokens without adding capability.

### When memory matters

If a conclusion is important, add:

```markdown
When you reach a definite root cause, plan or project convention, write it to repo memory before ending the turn.
```

More reliable: state "write the conclusion to memory" in the task itself.

## FAQ

| Symptom | Check |
| --- | --- |
| Model never calls tools | Does the model support tools? Are tools enabled? Does the system message forbid them? |
| Model never loads skills | Does `description` include triggers? Is the skill enabled? Was it already loaded? |
| 400 context length exceeded | Is the context window larger than the provider's real limit? Reduce history or lower it |
| 400 reasoning_content | DeepSeek thinking model? Enable DS-Format |
| 400 reasoning_effort | Provider does not support it; set thinking depth to Default |
| Answers get worse after compression | Write important conclusions to memory, or start a new chat |
| Compression repeats every turn | The static payload alone exceeds the window — lower the system message/history, or raise the context window |
| Long system message has no effect | Tool/skill guidance may follow it; put key rules first |
