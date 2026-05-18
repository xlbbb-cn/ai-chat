import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import type { AppConfig, Message, Skill, McpServer } from "./types";

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function getWorkspaceDir(): Promise<string> {
  return invoke("get_workspace_dir");
}

export async function saveConfig(config: AppConfig): Promise<void> {
  return invoke("save_config", { config });
}

export async function fetchModels(): Promise<string[]> {
  return invoke("fetch_models");
}

export async function listSkills(): Promise<Skill[]> {
  return invoke("list_skills");
}

export async function saveSkill(skill: Skill): Promise<void> {
  return invoke("save_skill", { skill });
}

export async function deleteSkill(name: string): Promise<void> {
  return invoke("delete_skill", { name });
}

export async function stopChatCompletion(): Promise<void> {
  return invoke("stop_chat_completion");
}

export interface StreamCallbacks {
  onToken: (token: string) => void;
  onReasoningToken?: (token: string) => void;
  onDone: () => void;
  onError: (err: string) => void;
}

export async function chatCompletion(
  messages: Pick<Message, "role" | "content">[],
  skillIds: string[],
  sessionId: string,
  modelOverride: string | undefined,
  callbacks: StreamCallbacks
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];

  const cleanup = () => {
    unlisteners.forEach((fn) => fn());
    unlisteners.length = 0;
  };

  const unToken = await listen<string>("chat-token", (e) =>
    callbacks.onToken(e.payload)
  );
  const unReasoning = await listen<string>("chat-reasoning-token", (e) => {
    if (callbacks.onReasoningToken) {
      callbacks.onReasoningToken(e.payload);
    }
  });
  const unDone = await listen<void>("chat-done", () => {
    callbacks.onDone();
    cleanup();
  });
  const unError = await listen<string>("chat-error", (e) => {
    callbacks.onError(e.payload);
    cleanup();
  });

  unlisteners.push(unToken, unReasoning, unDone, unError);

  invoke("chat_completion", {
    messages: messages.map((m) => ({ role: m.role, content: m.content })),
    skillIds,
    sessionId,
    modelOverride,
  }).catch((err: string) => {
    callbacks.onError(err);
    cleanup();
  });

  return cleanup;
}

export async function saveHistory(sessionId: string, role: string, content: string): Promise<void> {
  return invoke("save_history", { sessionId, role, content });
}

export interface HistoryRecord {
  id: number;
  session_id: string;
  role: string;
  content: string;
  timestamp: string;
}

export async function loadHistory(): Promise<HistoryRecord[]> {
  return invoke("load_history");
}

export async function deleteHistory(sessionId: string): Promise<void> {
  return invoke("delete_history", { sessionId });
}

export async function listMcpServers(): Promise<McpServer[]> {
  return invoke("list_mcp_servers");
}

export async function saveMcpServer(server: McpServer): Promise<void> {
  return invoke("save_mcp_server", { server });
}

export async function deleteMcpServer(id: string): Promise<void> {
  return invoke("delete_mcp_server", { id });
}

export async function testMcpServer(server: McpServer): Promise<string> {
  return invoke("test_mcp_server", { server });
}

// ─── API Request Monitor ──────────────────────────────────────────────────────

export interface ApiRequestRecord {
  id: number;
  session_id: string;
  timestamp: string;
  model: string;
  finish_reason: string;
  prompt_tokens: number;
  completion_tokens: number;
  duration_ms: number;
  error: string;
  response_preview: string;
}

export interface ApiRequestDetail {
  id: number;
  session_id: string;
  timestamp: string;
  model: string;
  request_body: string;
  response_content: string;
  tool_calls: string;
  finish_reason: string;
  prompt_tokens: number;
  completion_tokens: number;
  duration_ms: number;
  error: string;
}

export async function listApiRequests(): Promise<ApiRequestRecord[]> {
  return invoke("list_api_requests");
}

export async function getApiRequest(id: number): Promise<ApiRequestDetail> {
  return invoke("get_api_request", { id });
}

export async function deleteApiRequest(id: number): Promise<void> {
  return invoke("delete_api_request", { id });
}

export async function clearApiRequests(): Promise<void> {
  return invoke("clear_api_requests");
}

