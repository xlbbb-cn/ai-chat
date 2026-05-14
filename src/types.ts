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
}

export interface Skill {
  name: string;
  description: string;
  system_prompt: string;
  allowed_tools?: string[];
  version?: string;
  author?: string;
}
