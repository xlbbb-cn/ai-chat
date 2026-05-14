export interface Message {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  streaming?: boolean;
}

export interface AppConfig {
  api_base_url: string;
  api_key: string;
  model: string;
  temperature?: number;
  enable_thinking?: boolean;
  reasoning_effort?: string;
  system_message?: string;
  selected_tools?: string[];
}

export interface Skill {
  name: string;
  description: string;
  system_prompt: string;
  allowed_tools?: string[];
  version?: string;
  author?: string;
}
