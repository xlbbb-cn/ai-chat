export interface ToolCallEntry {
  task_id: string;
  agent_name: string;
  description: string;
  status: "running" | "done" | "error";
  summary?: string;
  error?: string;
  /**
   * Set when this entry records a `use_skill` load. Persisted with the entry
   * (DB `tool_calls` column) and sent back to the backend as `loaded_skills`,
   * so a skill is loaded at most once per session.
   */
  skill_name?: string;
}

/**
 * One part of a multimodal `Message.content`. Mirrors the OpenAI Chat
 * Completions `messages[].content` array element shape.
 *
 * - `text`      — plain text segment
 * - `image_url` — image, either a remote URL or a `data:` URL produced by
 *                 reading an attached file via `FileReader.readAsDataURL`
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
   * `supportsAttachmentKind` in `App.tsx`.
   */
  kind: "text" | "image" | "file" | "audio";
  /** MIME type when known (e.g. `image/png`, `application/pdf`). */
  mime?: string;
  /** `data:` URL for binary attachments (image / file). */
  data_url?: string;
  /** Original text for text attachments (also preserved in `content` parts). */
  text_content?: string;
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
  /** Database row id from history table (only for persisted messages). */
  dbId?: number;
}

/**
 * Confirmation kinds the backend can send with `confirm-required` — and the only
 * values `auto_accept_confirm_kinds` can meaningfully hold.
 *
 * The command kinds mirror `RiskLevel::confirm_kind()` in
 * `src-tauri/src/tools.rs`: L0 (`dangerous`), L1 (`system_config`),
 * L2 (`user_software`) and L3 (`user_data`) require approval, while L4-L6 never
 * prompt at all. `external_path` covers `file_actions` targeting an absolute
 * path outside the workspace. Keep this union in sync with the Rust side.
 */
export type ConfirmKind =
  | "dangerous"
  | "system_config"
  | "user_software"
  | "user_data"
  | "external_path";

/**
 * Values persisted in `auto_accept_confirm_kinds`. `"*"` is the
 * "auto-approve everything" wildcard: the backend matches it against every
 * confirmation kind — including kinds added in a later version — so the Tools
 * panel's one-click toggle keeps working after an upgrade.
 */
export type AutoAcceptKind = ConfirmKind | "*";

/** UI languages bundled with the frontend (see `src/i18n`). */
export type Language = "en" | "zh-CN" | "zh-TW";

export interface AppConfig {
  api_base_url: string;
  api_key: string;
  model: string;
  model_catalog?: string[];
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
   * messages as DeepSeek-family thinking models require. Off by default —
   * other providers receive plain messages.
   */
  ds_format?: boolean;
  system_message?: string;
  selected_tools?: string[];
  selected_skills?: string[];
  self_evolution_mode?: boolean;
  kg_engine?: string;
  neo4j_uri?: string;
  neo4j_user?: string;
  neo4j_password?: string;
  workspace_dir?: string;
  logger_output?: "file" | "println";
  theme?: "auto" | "light" | "dark";
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
  system_prompt: string;
  model?: string;
  max_tokens?: number;
  max_complete_tokens?: number;
  temperature?: number;
  allowed_tools: string[];
  allowed_skills: string[];
  max_iterations: number;
  enabled: boolean;
}

export interface AgentOrchestration {
  use_agents: boolean;
  auto_configure: boolean;
  max_concurrent: number;
  mode: "parallel" | "sequential";
}

export interface AgentTaskEvent {
  task_id: string;
  agent_id: string;
  agent_name: string;
  description?: string;
  summary?: string;
  error?: string;
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
  root_task_description: string;
  root_task_context: string;
  status: string;
  mission_accomplished: boolean;
  episodic_summary: string;
  final_report: string;
  active_tasks: AgentMissionTask[];
  active_task_count: number;
  created_at: string;
  updated_at: string;
}

export interface Profile {
  name: string;
  selected_skills: string[];
  selected_tools: string[];
  agents: SubAgent[];
  orchestration: AgentOrchestration;
  mcp_servers: McpServer[];
  created_at: string;
  updated_at: string;
}
