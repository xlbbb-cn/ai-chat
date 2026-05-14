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
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  allow_commands?: boolean;
}
