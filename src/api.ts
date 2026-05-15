import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import type { AppConfig, Message, Skill } from "./types";

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function saveConfig(config: AppConfig): Promise<void> {
  return invoke("save_config", { config });
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

export interface StreamCallbacks {
  onToken: (token: string) => void;
  onReasoningToken?: (token: string) => void;
  onDone: () => void;
  onError: (err: string) => void;
}

export async function chatCompletion(
  messages: Pick<Message, "role" | "content">[],
  skillIds: string[],
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
  }).catch((err: string) => {
    callbacks.onError(err);
    cleanup();
  });

  return cleanup;
}

export async function searchBy(engine: string, query: string): Promise<string> {
  return invoke("search_by", { engine, query });
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
