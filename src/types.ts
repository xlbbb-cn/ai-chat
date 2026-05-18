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
  search_engine?: string;
  kg_engine?: string;
  neo4j_uri?: string;
  neo4j_user?: string;
  neo4j_password?: string;
  workspace_dir?: string;
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
  allowed_tools?: string[];
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
