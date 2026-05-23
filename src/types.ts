export interface Message {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  reasoning_content?: string;
  streaming?: boolean;
}

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
}

export interface ModelSettings {
  temperature?: number;
  top_p?: number;
  reasoning_effort?: string;
  max_tokens?: number;
}

export interface Skill {
  name: string;
  description: string;
  system_prompt: string;
  /** Allowlist of executable names for direct execution (empty = unrestricted). */
  allowed_commands?: string[];
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

export interface SubAgent {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  model?: string;
  max_tokens?: number;
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
