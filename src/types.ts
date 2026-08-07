export interface ToolCallEntry {
  task_id: string;
  agent_name: string;
  description: string;
  status: "running" | "done" | "error";
  summary?: string;
  error?: string;
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
  kind: "text" | "image" | "file";
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

export type ConfirmKind = "dangerous" | "sudo" | "elevation" | "external_path";

export interface AppConfig {
  api_base_url: string;
  api_key: string;
  model: string;
  model_catalog?: string[];
  model_settings?: ModelSettings;
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
  auto_accept_confirm_kinds?: ConfirmKind[];
}

export interface ModelSettings {
  temperature?: number;
  top_p?: number;
  reasoning_effort?: string;
  max_complete_tokens?: number;
  max_tokens?: number;
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

export type McpTransport = "stdio" | "sse";

export interface McpServer {
  id: string;
  name: string;
  transport: McpTransport;
  /** stdio only */
  command: string;
  args: string[];
  env: Record<string, string>;
  /** sse only */
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
