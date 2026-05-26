import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import type { AppConfig, Message, Skill, McpServer, SubAgent, AgentOrchestration, AgentTaskEvent } from "./types";

interface SessionTokenEvent {
  session_id: string;
  token: string;
}

interface SessionDoneEvent {
  session_id: string;
}

interface SessionErrorEvent {
  session_id: string;
  error: string;
}

interface AgentPlanEvent {
  session_id: string;
  task_count: number;
}

interface AgentAggregateEvent {
  session_id: string;
}

export interface SessionRuntimeRecord {
  session_id: string;
  status: "working" | "idle";
}

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function confirmCommand(
  confirmed: boolean,
  opts?: { username?: string; password?: string },
): Promise<void> {
  return invoke("confirm_command", {
    confirmed,
    username: opts?.username,
    password: opts?.password,
  });
}

export async function getWorkspaceDir(): Promise<string> {
  return invoke("get_workspace_dir");
}

export async function getSkillRoots(): Promise<string[]> {
  return invoke("get_skill_roots");
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

export async function stopChatCompletion(sessionId: string): Promise<void> {
  return invoke("stop_chat_completion", { sessionId });
}

export async function listSessionRuntimeStates(): Promise<SessionRuntimeRecord[]> {
  return invoke("list_session_runtime_states");
}

export interface StreamCallbacks {
  onToken: (token: string) => void;
  onReasoningToken?: (token: string) => void;
  onDone: () => void;
  onError: (err: string) => void;
  onAgentTaskStart?: (e: AgentTaskEvent) => void;
  onAgentTaskDone?: (e: AgentTaskEvent) => void;
  onAgentTaskError?: (e: AgentTaskEvent) => void;
  onAgentPlanStart?: (taskCount: number) => void;
  onAgentAggregateStart?: () => void;
}

export async function chatCompletion(
  messages: Pick<Message, "role" | "content">[],
  skillIds: string[],
  sessionId: string,
  modelOverride: string | undefined,
  callbacks: StreamCallbacks,
  useAgents?: boolean,
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];

  const cleanup = () => {
    unlisteners.forEach((fn) => fn());
    unlisteners.length = 0;
  };

  const unToken = await listen<SessionTokenEvent>("chat-token", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onToken(e.payload.token);
  });
  const unReasoning = await listen<SessionTokenEvent>("chat-reasoning-token", (e) => {
    if (e.payload.session_id !== sessionId) return;
    if (callbacks.onReasoningToken) {
      callbacks.onReasoningToken(e.payload.token);
    }
  });
  const unDone = await listen<SessionDoneEvent>("chat-done", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onDone();
    cleanup();
  });
  const unError = await listen<SessionErrorEvent>("chat-error", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onError(e.payload.error);
    cleanup();
  });
  const unTaskStart = await listen<AgentTaskEvent>("agent-task-start", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onAgentTaskStart?.(e.payload);
  });
  const unTaskDone = await listen<AgentTaskEvent>("agent-task-done", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onAgentTaskDone?.(e.payload);
  });
  const unTaskError = await listen<AgentTaskEvent>("agent-task-error", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onAgentTaskError?.(e.payload);
  });
  const unPlanStart = await listen<AgentPlanEvent>("agent-plan-start", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onAgentPlanStart?.(e.payload.task_count);
  });
  const unAggStart = await listen<AgentAggregateEvent>("agent-aggregate-start", (e) => {
    if (e.payload.session_id !== sessionId) return;
    callbacks.onAgentAggregateStart?.();
  });

  unlisteners.push(unToken, unReasoning, unDone, unError, unTaskStart, unTaskDone, unTaskError, unPlanStart, unAggStart);

  invoke("chat_completion", {
    messages: messages.map((m) => ({ role: m.role, content: m.content })),
    skillIds,
    sessionId,
    modelOverride,
    useAgents: useAgents ?? false,
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

// ─── Sub-Agent Management ─────────────────────────────────────────────────────

export async function listSubAgents(): Promise<SubAgent[]> {
  return invoke("list_sub_agents");
}

export async function saveSubAgent(agent: SubAgent): Promise<void> {
  return invoke("save_sub_agent", { agent });
}

export async function deleteSubAgent(id: string): Promise<void> {
  return invoke("delete_sub_agent", { id });
}

export async function getAgentOrchestration(): Promise<AgentOrchestration> {
  return invoke("get_agent_orchestration");
}

export async function saveAgentOrchestration(orchestration: AgentOrchestration): Promise<void> {
  return invoke("save_agent_orchestration", { orchestration });
}

// ─── Interaction Log Monitor ──────────────────────────────────────────────────

export interface InteractionLogRecord {
  id: number;
  session_id: string;
  interaction_type: string;
  timestamp: string;
  actor: string;
  action_name: string;
  error_message: string;
  duration_ms: number;
  input_preview: string;
  output_preview: string;
}

export interface InteractionLogDetail {
  id: number;
  session_id: string;
  interaction_type: string;
  timestamp: string;
  actor: string;
  action_name: string;
  input_data: string;
  output_data: string;
  error_message: string;
  duration_ms: number;
  metadata: string;
}

export async function listInteractions(sessionId: string): Promise<InteractionLogRecord[]> {
  return invoke("list_interactions", { sessionId });
}

export async function getInteraction(id: number): Promise<InteractionLogDetail> {
  return invoke("get_interaction", { id });
}

export async function clearInteractions(sessionId: string): Promise<void> {
  return invoke("clear_interactions", { sessionId });
}
