export interface ToolCallEntry {
  task_id: string;
  agent_name: string;
  /**
   * 1-based position of a planner-dispatched sub-agent inside this turn
   * (`#1`, `#2`, …), assigned by the frontend in event order. Absent for
   * planner/selector/aggregator rows, ordinary tool calls and old history.
   */
  agent_index?: number;
  /**
   * Row kind. `agent` is a sub-agent task row: it owns its own status and is
   * never settled by a main-chat tool event. Absent means an ordinary tool /
   * planner row (or a row persisted before the field existed).
   */
  kind?: "agent" | "planner" | "tool";
  description: string;
  status: "running" | "done" | "error" | "skipped";
  summary?: string;
  error?: string;
  /** Epoch ms when the row was created; drives the live elapsed timer. */
  started_at?: number;
  /**
   * Latest tool the sub-agent is running (`agent-task-tool`). Shown instead of
   * an empty summary while a round produces no content, so the row keeps
   * moving between model calls.
   */
  activity?: string;
  /**
   * Live reasoning preview for a sub-agent task. Only the character count is
   * kept — the reasoning text is never persisted in the mission report.
   */
  thinking?: { chars: number; started_at: number };
  /**
   * Set when this entry records a `use_skill` load. Persisted with the entry
   * (DB `tool_calls` column) and sent back to the backend as `loaded_skills`,
   * so a skill is loaded at most once per session.
   */
  skill_name?: string;
  /**
   * Set while this entry is blocked waiting for the user to allow or deny a
   * risky `run_shell` command (a `confirm-required` payload that needs no
   * credentials). ToolCallGroup renders the inline allow/deny row and the
   * response is routed back through `confirm_command` with `request_id`.
   */
  confirm?: {
    request_id: string;
    code: string;
    reason: string;
  };
  /**
   * Set while this entry tracks a background tool task (`run_in_background`).
   * The id is the backend `bg-…` task id, so the cancel button can stop it and
   * `background-task-state` events update the same row.
   */
  background?: boolean;
  /**
   * Workspace-relative directory the sub-agent was told to work in
   * (`agent-task-start`). Absent when it ran in the workspace root.
   */
  cwd?: string;
  /**
   * Live `ask_main_agent` exchange for a sub-agent task. `request` is the
   * sub-agent's question; `answer_chars` is the size of the material returned.
   */
  help?: ToolCallHelp;
  /**
   * Mission-bus (mbus) activity for this task row. Counts are live; the full
   * message log lives in the Agent Mission panel. `last.body` is clipped before
   * it is persisted with the tool-call entry.
   */
  mbus?: ToolCallMbus;
}

/** One sub-agent ↔ planner / peer message summary shown on a task row. */
export interface ToolCallMbus {
  sent: number;
  received: number;
  last?: {
    direction: "in" | "out";
    /** Display label: "Planner", a peer agent name, or "#topic". */
    label: string;
    body: string;
  };
}

/** One sub-agent → main-agent help exchange, mirrored from `agent-help-*`. */
export interface ToolCallHelp {
  request: string;
  status: "running" | "done" | "error";
  answer_chars?: number;
  error?: string;
}

/**
 * One background tool task, mirrored from the Rust `BackgroundTaskSnapshot`
 * (`background-task-state` events, `list_background_tasks` command).
 */
export interface BackgroundTaskState {
  id: string;
  session_id: string;
  tool: string;
  label: string;
  status: "running" | "done" | "failed" | "cancelled";
  started_at_ms: number;
  elapsed_ms: number;
  preview?: string;
}

/**
 * A pending delay timer created by the assistant through the `timer_set` tool.
 *
 * When it fires, the backend emits `timer-fired` and the frontend injects
 * `message` into the chat session identified by `session_id` as a new user turn,
 * so the assistant resumes the task. Mirrors the Rust `TimerEntry` in
 * `src-tauri/src/timer.rs`.
 */
export interface TimerEntry {
  id: string;
  session_id: string;
  /** Short human-readable label for the timer bar, e.g. "collect logs". */
  label: string;
  /** Instruction injected into the chat when the timer fires. */
  message: string;
  /** Creation time, milliseconds since the UNIX epoch. */
  created_at: number;
  /** Fire time, milliseconds since the UNIX epoch. */
  fire_at: number;
}

/**
 * Cross-platform "system awake" lock state. Mirrors `SleepLockState` in
 * `src-tauri/src/tools/timer.rs`: held while a timer is pending or a chat turn
 * is streaming, which keeps the OS from suspending mid-task.
 */
export interface SleepLockState {
  /** True while the OS has been asked not to idle-suspend. */
  active: boolean;
  /** Number of live holders (pending timers, running turns, …). */
  holders: number;
}

/**
 * One background process started by `run_shell` with `run_type: "serve"`.
 * Mirrors `ServeProcessInfo` in `src-tauri/src/tools/serve.rs`; the Tools panel
 * polls it to monitor running serve processes and stop them.
 */
export interface ServeProcessInfo {
  id: string;
  pid: number;
  /** Shell the script runs in (`bash`, `sh`, `powershell`, `pwsh`). */
  shell_type: string;
  /** Script preview, clamped by the backend. */
  code: string;
  cwd: string;
  /** `running`, `exited`, `lost`, `stopped` or `unknown`. */
  state: string;
  exit_code: number | null;
  /** Seconds since the process was started. */
  uptime_secs: number;
  /** Tail of stdout/stderr (last few lines). */
  recent_output: string;
}

/**
 * One part of a multimodal `Message.content`. Mirrors the OpenAI Chat
 * Completions `messages[].content` array element shape.
 *
 * - `text`      — plain text segment
 * - `image_url` — image, either a remote URL or a `data:` URL produced by the
 *                 `read_workspace_file_bytes` command for a `@file` mention
 * - `file`      — generic file (e.g. PDF) using the newer
 *                 `{ type: "file", file: { filename, file_data } }` shape
 * - `input_audio` — audio clip as base64 (`wav` / `mp3`, per the OpenAI
 *                 Chat Completions schema)
 */
export type ContentPart =
  | { type: "text"; text: string }
  | {
    type: "image_url";
    image_url: { url: string; detail?: "auto" | "low" | "high" };
  }
  | {
    type: "file";
    file: { filename: string; file_data: string };
  }
  | {
    type: "input_audio";
    input_audio: { data: string; format: "wav" | "mp3" };
  };

/**
 * `messages[].content` payload. A plain string is equivalent to a one-part
 * `[{ type: "text", text: <content> }]` array — the API accepts both.
 */
export type MessageContent = string | ContentPart[];

/**
 * Display-only metadata for a file the user attached to a user message.
 * Used to render pills/thumbnails and to keep file names visible after the
 * multimodal content is rendered.
 */
export interface Attachment {
  name: string;
  /**
   * How the attachment reaches the model: `text` is inlined into the prompt,
   * the others become a content part (`image_url` / `file` / `input_audio`) and
   * therefore need the matching model capability — see
   * `supportsAttachmentKind` in `attachments.ts`.
   */
  kind: "text" | "image" | "file" | "audio";
  /** MIME type when known (e.g. `image/png`, `application/pdf`). */
  mime?: string;
  /** `data:` URL for binary attachments (image / file). */
  data_url?: string;
  /** Original text for text attachments (also preserved in `content` parts). */
  text_content?: string;
  /**
   * Workspace-relative path of the file that holds this attachment's text when
   * it is too big to keep in the history row (a converted PDF mention caches its
   * Markdown under `.data/tmp/pdf/`).
   *
   * The stored copy drops `text_content` once this is set — the text still
   * reaches the model through the message `content`, so the attachment record
   * only has to remember where the file is. See `compactAttachmentsForDb` in
   * `attachments.ts`.
   */
  path?: string;
}

export interface Message {
  id: string;
  role: "user" | "assistant" | "system" | "tool_group";
  content: MessageContent;
  reasoning_content?: string;
  streaming?: boolean;
  tool_calls?: ToolCallEntry[];
  /** Display metadata for files attached to a user message. */
  attachments?: Attachment[];
  /**
   * Messages the user pushed into this assistant turn while it was already
   * running (`inject_user_message`). They belong to the assistant row — one
   * turn is one message — but were real user input the model answered after
   * seeing, so history replays them as user messages.
   */
  injections?: ChatInjection[];
  /**
   * UI-only failure text for an assistant turn that could not finish. It is
   * rendered under the partial reply but never persisted as model content and
   * never sent in the context; the partial `content`/`reasoning_content` can
   * still be replayed so a continuation request picks up where it stopped.
   */
  error?: string;
  /**
   * Outcome of a persisted assistant turn. `error` / `stopped` mark partial
   * turns whose visible output stays usable but which should not be treated
   * as a completed reply. `undefined` means a normal completed turn.
   */
  status?: "done" | "error" | "stopped";
  /** Database row id from history table (only for persisted messages). */
  dbId?: number;
}

/** One mid-turn message the user added while the assistant was working. */
export interface ChatInjection {
  /** Generated by the UI; `chat-injected` echoes it back once delivered. */
  id: string;
  text: string;
  /** Unix milliseconds, rendered as the "added at" clock label. */
  created_at_ms: number;
  /**
   * `background_result` marks the automatic delivery of a background tool
   * task's output. The UI labels it differently and clamps the preview; the
   * full text still travels into the conversation history.
   */
  kind?: "background_result";
}

/**
 * Confirmation kinds the backend can send with `confirm-required` — and the only
 * values `auto_accept_confirm_kinds` can meaningfully hold.
 *
 * The command kinds mirror `RiskLevel::confirm_kind()` in
 * `src-tauri/src/tools/risk.rs`: L0 (`dangerous`), L1 (`system_config`),
 * L2 (`user_software`) and L3 (`user_data`) require approval, while L4-L6 never
 * prompt at all. Keep this union in sync with the Rust side.
 */
export type ConfirmKind =
  | "dangerous"
  | "system_config"
  | "user_software"
  | "user_data"
  /** `cli` / `cloud` sub-agents (approved per agent in the Sub Agents panel). */
  | "external_agent";

/**
 * Values persisted in `auto_accept_confirm_kinds`. `"*"` is the
 * "auto-approve everything" wildcard: the backend matches it against every
 * confirmation kind — including kinds added in a later version — so the Tools
 * panel's one-click toggle keeps working after an upgrade.
 */
export type AutoAcceptKind = ConfirmKind | "*";

/** UI languages bundled with the frontend (see `src/i18n`). */
export type Language = "en" | "zh-CN" | "zh-TW";

/**
 * Light / dark resolution of the interface. `auto` follows the OS setting.
 * Applied as `data-theme` on `<html>`.
 */
export type ThemeMode = "auto" | "light" | "dark";

/**
 * Colour scheme, applied as `data-palette` on `<html>` on top of the
 * light / dark mode. Each palette ships a light *and* a dark token set,
 * so the two dimensions are independent.
 */
export type Palette = "default" | "fig" | "cream";

/**
 * One extra HTTP header attached to every upstream model request
 * (Settings → API → Custom request headers).
 */
export interface RequestHeader {
  name: string;
  value: string;
}

export interface AppConfig {
  api_base_url: string;
  api_key: string;
  model: string;
  model_catalog?: string[];
  /**
   * Header name that carries the chat session id on every upstream request.
   * Blank / missing = the default `x-conversation-id`.
   */
  conversation_id_header?: string;
  /**
   * Extra HTTP headers attached to every request this app sends to the model
   * endpoint (chat turns, sub-agent planning calls and the model listing).
   */
  custom_headers?: RequestHeader[];
  /**
   * Per-model context window (tokens). Auto-filled from `/models` when the
   * provider reports it (OpenRouter / Groq / vLLM…); otherwise set manually
   * in Settings as a fallback. Used for agent context budgeting.
   */
  model_context_lengths?: Record<string, number>;
  /**
   * Per-model capabilities learned from the public `basellm/llm-metadata`
   * catalogue when the model catalogue was refreshed ("Fetch Models From API").
   * Keyed by the remote model id; `model_name` inside the value is the
   * catalogue entry it was matched to. Informational only — never sent upstream.
   */
  model_metadata?: Record<string, ModelMetadata>;
  /**
   * Per-model "thinking depth", picked next to the model selector in the chat
   * toolbar and sent as `reasoning_effort` (`minimal` | `low` | `medium` |
   * `high`). Keyed by model id; absent = let the provider decide. Falls back to
   * `model_settings.reasoning_effort`.
   */
  model_reasoning_effort?: Record<string, string>;
  model_settings?: ModelSettings;
  /**
   * DS-Format (DeepSeek format): pass `reasoning_content` back on assistant
   * messages as DeepSeek-family thinking models require. DeepSeek-family
   * models always get it automatically; the switch only forces it for other
   * providers.
   */
  ds_format?: boolean;
  system_message?: string;
  selected_tools?: string[];
  self_evolution_mode?: boolean;
  /**
   * Allow `run_cmd` / `run_shell` / `file_actions search` to run as background
   * tasks (`run_in_background: true`). The turn stays open until their results
   * are delivered or the tasks are cancelled. Defaults to on.
   */
  background_tools?: boolean;
  /**
   * Allow `llm` sub-agents to call `ask_main_agent` and have the main agent
   * (main model, tools, skills, MCP and conversation history) collect material
   * for them. Defaults to on.
   */
  sub_agent_main_help?: boolean;
  workspace_dir?: string;
  logger_output?: "file" | "println";
  /**
   * Light / dark resolution of the interface. `auto` follows the OS.
   */
  theme?: ThemeMode;
  /**
   * Colour scheme layered on top of the light / dark mode: `default`
   * (Pure White + Electric Blue), `fig` (无花果, a pastel scheme on a
   * frost-purple canvas) or `cream` (乳白, the adjacent warm-white group
   * washed with oklab gradients).
   */
  palette?: Palette;
  /**
   * UI language. Kept in the config so it survives restarts and travels with
   * exported profiles; the frontend also caches it in `localStorage` so the
   * very first paint does not flash the wrong language.
   */
  language?: Language;
  auto_accept_confirm_kinds?: AutoAcceptKind[];
  check_updates_on_startup?: boolean;
  include_prerelease_updates?: boolean;
  /**
   * Keep a single running instance: launching the app again focuses the
   * window that is already open instead of starting a second process.
   * Read from `config.json` at process start, so a change applies after the
   * app is restarted.
   */
  single_instance?: boolean;
  /**
   * Retention window (days) for the request / interaction log tables. Older
   * rows are dropped at startup and when the monitor compacts the database.
   * 0 keeps logs forever; chat history is never pruned.
   */
  log_retention_days?: number;
}

/** A bundle attached to a GitHub release. */
export interface UpdateAsset {
  name: string;
  download_url: string;
  size: number;
  kind: string;
  recommended: boolean;
}

/** Result of a GitHub release update check. */
export interface UpdateInfo {
  current_version: string;
  latest_version: string;
  has_update: boolean;
  release_name: string;
  release_notes: string;
  release_url: string;
  published_at?: string;
  prerelease: boolean;
  assets: UpdateAsset[];
  has_platform_asset: boolean;
  platform: string;
}

/** Payload of the `update-download-progress` event. */
export interface UpdateDownloadProgress {
  asset_name: string;
  downloaded: number;
  total: number;
  done: boolean;
}

export interface ModelSettings {
  temperature?: number;
  top_p?: number;
  reasoning_effort?: string;
  max_complete_tokens?: number;
  max_tokens?: number;
}

/**
 * Capability metadata for one model, published by the public
 * [`basellm/llm-metadata`](https://github.com/basellm/llm-metadata) catalogue
 * (`fetch_model_metadata`) and — once matched — persisted per remote model id in
 * {@link AppConfig.model_metadata}.
 *
 * `model_name` is the catalogue entry the model was matched to, which is not
 * necessarily identical to the remote model id; `match_score` records how close
 * the two names were (1 = identical after normalisation). Mirrors the Rust
 * `ModelMetadata` struct in `src-tauri/src/model_metadata.rs`.
 */
export interface ModelMetadata {
  model_name: string;
  vendor?: string;
  description?: string;
  /** Raw catalogue tags, e.g. `["Tools", "Reasoning", "1M"]`. */
  tags?: string[];
  /** Context window in tokens, parsed from the catalogue size tag. */
  context_length?: number;
  supports_tools?: boolean;
  /** "Thinking" / chain-of-thought models. */
  supports_reasoning?: boolean;
  supports_vision?: boolean;
  supports_files?: boolean;
  supports_audio?: boolean;
  open_weights?: boolean;
  deprecated?: boolean;
  /** 0–1 name similarity; only present on persisted (matched) entries. */
  match_score?: number;
}

/**
 * One file offered by the composer's `@` mention popup
 * (`workspace_files::list_workspace_files`).
 */
export interface WorkspaceFileEntry {
  /** Workspace-relative path with `/` separators. */
  path: string;
  /** File name (basename) — what the popup filters on first. */
  name: string;
  /** Lowercase extension without the dot; `""` when there is none. */
  ext: string;
  size: number;
}

/** A text file sliced to a 1-based line range (`read_workspace_file_range`). */
export interface WorkspaceFileRange {
  text: string;
  total_lines: number;
  start_line: number;
  end_line: number;
  /** The requested range ran past the end of the file and was clamped. */
  truncated: boolean;
}

export interface Skill {
  name: string;
  description: string;
  system_prompt: string;
  /** Allowlist of executable names for direct execution (empty = unrestricted). */
  allowed_commands?: string[];
  allowed_tools?: string[];
  context?: string;
  agent?: string;
  license?: string;
  version?: string;
  author?: string;
  /** Evolution memory keys this skill archives (written by `memory consolidate`). */
  source_memory?: string[];
}

/** One ranked row of the read-only self-evolution memory panel. */
export interface EvolutionMemoryEntry {
  rank: number;
  key: string;
  title: string;
  kind: string;
  tags: string[];
  importance: number;
  confidence: number;
  evidence: number;
  uses: number;
  pinned: boolean;
  source: string;
  updated_at: string;
  chars: number;
  score: number;
  /** Whether the entry fits the prompt budget on the current ranking. */
  injected: boolean;
}

/** A skill that archives consolidated evolution-memory entries. */
export interface ArchivedSkill {
  name: string;
  entries: number;
  chars: number;
  skill_exists: boolean;
}

export type McpTransport = "stdio" | "sse" | "http" | "stream-http";

export interface McpServer {
  id: string;
  name: string;
  transport: McpTransport;
  /** stdio only */
  command: string;
  args: string[];
  env: Record<string, string>;
  /** sse / http / stream-http only */
  url: string;
  auth_token: string;
  enabled: boolean;
  /**
   * `""` / `"user"` for a panel-owned entry, `"self-evolution"` for a server
   * the assistant built with the `mcp_self` tool.
   */
  source?: string;
  /** Workspace-relative script path of a self-built server (`.data/mcp/<slug>/server.py`). */
  definition_path?: string;
}

export type McpLogLevel = "info" | "warn" | "error";

export interface McpLogEntry {
  /** Milliseconds since UNIX epoch. */
  ts: number;
  level: McpLogLevel;
  message: string;
}

export interface SubAgent {
  id: string;
  name: string;
  description: string;
  /** Execution backend; the other type's settings are ignored. */
  agent_type: AgentType;
  // ── `llm` backend ──
  system_prompt: string;
  model?: string;
  max_tokens?: number;
  max_complete_tokens?: number;
  temperature?: number;
  allowed_tools: string[];
  allowed_skills: string[];
  max_iterations: number;
  /**
   * `llm` only: inject the tools of every enabled MCP server. The task owns
   * its own MCP server processes and stops them when it ends.
   */
  enable_mcp?: boolean;
  // ── `llm` file-backed definition ──
  /** Display emoji from the md frontmatter (content, not a UI icon). */
  emoji?: string;
  /** Author color from the md frontmatter; stored, never painted as a UI accent. */
  color?: string;
  /** Relative path under `<app_data>/agents` holding the definition. */
  file?: string;
  /** Frontmatter parse error for a discovered file. */
  parse_error?: string;
  /** Whether `sub_agents.json` already has a config entry for this agent. */
  configured?: boolean;
  // ── `cli` backend ──
  cli: CliAgentSettings;
  // ── `cloud` backend ──
  cloud: CloudAgentSettings;
  /**
   * `cli` / `cloud` agents only: ask the user before this agent's first run in
   * each app session. Off = the enabled switch is the standing authorization.
   */
  require_approval: boolean;
  enabled: boolean;
}

export interface AgentOrchestration {
  use_agents: boolean;
  auto_configure: boolean;
  max_concurrent: number;
  mode: "parallel" | "sequential";
  /** Mission message bus + planner supervision (default true). */
  mbus_enabled: boolean;
  /**
   * Max mbus messages between any two sub-agents, both directions combined.
   * Reaching it blocks the pair and asks the planner to mediate. `0` disables
   * the per-pair cap.
   */
  mbus_pair_limit: number;
}

/** One key/value pair: a CLI agent's env var, or a cloud agent's header. */
export interface KeyValue {
  key: string;
  value: string;
}

/**
 * How a sub-agent is executed:
 * - `llm`   — the built-in autonomous LLM loop (tools + mission state);
 * - `cli`   — a headless coding CLI installed on this machine;
 * - `cloud` — a remote HTTP agent endpoint.
 */
export type AgentType = "llm" | "cli" | "cloud";

/**
 * Settings of a `cli` agent. `command` + `args` form the argv template:
 * `{prompt}`, `{model}`, `{cwd}` and `{agent_name}` are substituted before
 * spawning. With `prompt_via: "stdin"` the prompt is piped to the child.
 */
export interface CliAgentSettings {
  command: string;
  args: string[];
  prompt_via: "arg" | "stdin" | string;
  model_args: string[];
  env: KeyValue[];
  /** Run timeout in seconds; 0 = no timeout. */
  default_timeout_seconds: number;
}

/**
 * Settings of a `cloud` agent: an HTTP endpoint that accepts the task and
 * returns the result, optionally asynchronously through `poll_url`.
 */
export interface CloudAgentSettings {
  endpoint: string;
  token: string;
  headers: KeyValue[];
  /** JSON body template with `{prompt}` / `{agent_name}` / `{cwd}` placeholders. */
  body_template: string;
  /** Dot path into the response holding the text (e.g. `choices.0.message.content`). */
  result_path: string;
  /** Overall timeout in seconds covering polling; 0 = no timeout. */
  timeout_seconds: number;
  /** Async mode: poll this URL (`{id}` placeholder). Empty = single response. */
  poll_url: string;
  poll_id_path: string;
  poll_status_path: string;
  poll_done_values: string[];
  poll_failed_values: string[];
  poll_interval_seconds: number;
}

/** Result of `probe_agent_cli`: the resolved executable and its output. */
export interface CliProbe {
  command: string;
  resolved_path: string;
  exit_code?: number | null;
  output: string;
}

/** Result of `probe_agent_cloud`: endpoint reachability. */
export interface CloudProbe {
  endpoint: string;
  reachable: boolean;
  status?: number | null;
  detail: string;
}

/** `agent-task-token` payload: one streamed chunk of agent output. */
export interface AgentTaskTokenEvent {
  task_id: string;
  token: string;
}

/**
 * `agent-task-tool` payload: one tool call on a sub-agent task row.
 *
 * `phase: "progress"` carries only `text` (a one-line progress note emitted by
 * the tool itself); `"start"` / `"done"` bracket the actual call. The row's
 * status is never derived from this event — it only feeds the activity line.
 */
export interface AgentTaskToolEvent {
  task_id: string;
  phase: "start" | "progress" | "done";
  tool?: string;
  detail?: string;
  text?: string;
  duration_ms?: number;
  ok?: boolean;
}

/** `agent-task-reasoning` payload: one streamed reasoning delta. */
export interface AgentTaskReasoningEvent {
  task_id: string;
  token: string;
}

/** Legacy standalone CLI agent (pre agent-type profiles), kept for imports. */
export interface LegacyCliAgent {
  id: string;
  name: string;
  description: string;
  command: string;
  args: string[];
  prompt_via: string;
  model_args: string[];
  env: KeyValue[];
  default_timeout_seconds: number;
  enabled: boolean;
}

export interface AgentTaskEvent {
  task_id: string;
  agent_id: string;
  agent_name: string;
  description?: string;
  summary?: string;
  error?: string;
  /** Terminal outcome of a task (`agent-task-done`). */
  status?: "success" | "stopped" | "error";
  /** Why a planned task never ran (unknown agent, unmet dependency, stop). */
  reason?: string;
  /**
   * Workspace-relative directory the task was told to work in. `null` (or
   * absent) means the workspace root.
   */
  cwd?: string | null;
}

/** `agent-help-start` / `agent-help-done` / `agent-help-error` payload. */
export interface AgentHelpEvent {
  task_id: string;
  mission_id?: string;
  agent_id: string;
  agent_name?: string;
  request?: string;
  cwd?: string | null;
  answer_chars?: number;
  tokens?: number;
  tool_calls?: number;
  error?: string;
}

/** One live `agent-mbus-message` event (planner or sub-agent publish). */
export interface AgentMbusMessageEvent {
  bus_id: string;
  session_id: string;
  from_id: string;
  from_name: string;
  target_kind: "direct" | "topic";
  /** Task id for direct, topic name for topic. */
  target_id: string;
  topic: string;
  body: string;
  recipients: Array<{ id: string; name: string }>;
  created_at_ms: number;
}

/** `agent-mbus-delivered`: messages just injected at a sub-agent request boundary. */
export interface AgentMbusDeliveredEvent {
  bus_id: string;
  session_id: string;
  mission_id: string;
  task_id: string;
  count: number;
  messages: Array<{
    from_id: string;
    from_name: string;
    topic: string;
    body: string;
    created_at_ms: number;
  }>;
}

/** `agent-planner-state`: live supervision status for the planner row. */
export interface AgentPlannerStateEvent {
  session_id: string;
  bus_id: string;
  task_id: string;
  status: "thinking" | "monitoring" | "done" | "stopped" | "error";
  activation: number;
  task_count: number;
  activity: string;
}

/** `agent-planner-action`: one planner decision mirrored into the tool group. */
export interface AgentPlannerActionEvent {
  session_id: string;
  bus_id: string;
  task_id: string;
  action_id: string;
  activation: number;
  round: number;
  /** schedule | send | status | history | subscribe | unsubscribe | finish */
  action: string;
  status: "running" | "done" | "error";
  target: string;
  detail: string;
  created_at_ms: number;
}

/** Persisted mission-bus message rendered in the Agent Mission panel. */
export interface AgentBusMessage {
  id: number;
  bus_id: string;
  from_id: string;
  from_name: string;
  target_kind: "direct" | "topic";
  target_id: string;
  to_name: string;
  topic: string;
  body: string;
  recipient_count: number;
  created_at: string;
}

/**
 * Selector decision for one turn (`agent-selection` event). `delegate: false`
 * means the request is answered without sub-agents.
 */
export interface AgentSelectionEvent {
  delegate: boolean;
  confidence: number;
  agents: Array<{ id: string; name: string; role: string; why: string }>;
}

export interface AgentMissionTask {
  task_id: string;
  name: string;
  description: string;
  status: "pending" | "in_progress" | "completed" | string;
}

export interface AgentMissionSnapshot {
  mission_id: string;
  session_id: string;
  agent_id: string;
  agent_name: string;
  /** Mission bus of the orchestration batch; null for direct call_subagent runs. */
  bus_id?: string | null;
  root_task_description: string;
  root_task_context: string;
  status: string;
  mission_accomplished: boolean;
  episodic_summary: string;
  final_report: string;
  /** One line per `ask_main_agent` exchange, newest last. */
  help_notes?: string;
  active_tasks: AgentMissionTask[];
  active_task_count: number;
  created_at: string;
  updated_at: string;
}

export interface Profile {
  name: string;
  selected_tools: string[];
  agents: SubAgent[];
  orchestration: AgentOrchestration;
  /** Legacy standalone CLI agents (pre agent-type profiles). */
  cli_agents?: LegacyCliAgent[];
  mcp_servers: McpServer[];
  created_at: string;
  updated_at: string;
}

/**
 * Per-skill status metadata, stored per workspace in
 * `<workspace>/.data/skills.status`. Only `enabled` drives the chat request —
 * the rest is provenance shown in the Skills panel.
 */
export interface SkillStatusEntry {
  enabled: boolean;
  /** `user` (written through the Skills panel) or `self-evolution`. */
  source: string;
  enabled_at?: string;
  updated_at?: string;
  last_seen_at?: string;
  file_mtime?: string;
}

export interface SkillsStatus {
  version: number;
  updated_at?: string;
  skills: Record<string, SkillStatusEntry>;
}

/** One item of a session's todo list (`todos` tool / `<workspace>/.data/todos`). */
export interface TodoRecord {
  id: string;
  session_id: string;
  list_id: string;
  title: string;
  description: string;
  status: string;
  position: number;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
}

/** A whole todo list plus its derived counters, as stored on disk. */
export interface TodoListSummary {
  list_id: string;
  session_id: string;
  title: string;
  description: string;
  total: number;
  pending: number;
  in_progress: number;
  completed: number;
  cancelled: number;
  created_at: string;
  updated_at: string;
  todos: TodoRecord[];
}

/** Payload of the `todo-state` Tauri event. */
export interface TodoStateEvent {
  type: "todo_state" | "todo_updated" | "todo_cleared" | "todo_list_changed";
  list_id?: string;
  session_id?: string;
  summary?: TodoListSummary;
  todo?: TodoRecord;
}
