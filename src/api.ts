import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppConfig,
  Attachment,
  Message,
  MessageContent,
  Skill,
  McpServer,
  McpLogEntry,
  SubAgent,
  AgentOrchestration,
  AgentTaskEvent,
  AgentMissionSnapshot,
  Profile,
} from "./types";

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
  onAgentTaskStart?: (e: AgentTaskEvent) => void;
  onAgentTaskDone?: (e: AgentTaskEvent) => void;
  onAgentTaskError?: (e: AgentTaskEvent) => void;
  onAgentPlanStart?: (taskCount: number) => void;
  onAgentAggregateStart?: () => void;
}

export async function chatCompletion(
  messages: { role: Message["role"]; content: MessageContent; reasoning_content?: string }[],
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
  const unTaskStart = await listen<AgentTaskEvent>("agent-task-start", (e) => {
    callbacks.onAgentTaskStart?.(e.payload);
  });
  const unTaskDone = await listen<AgentTaskEvent>("agent-task-done", (e) => {
    callbacks.onAgentTaskDone?.(e.payload);
  });
  const unTaskError = await listen<AgentTaskEvent>("agent-task-error", (e) => {
    callbacks.onAgentTaskError?.(e.payload);
  });
  const unPlanStart = await listen<{ task_count: number }>("agent-plan-start", (e) => {
    callbacks.onAgentPlanStart?.(e.payload.task_count);
  });
  const unAggStart = await listen<void>("agent-aggregate-start", () => {
    callbacks.onAgentAggregateStart?.();
  });

  unlisteners.push(unToken, unReasoning, unDone, unError, unTaskStart, unTaskDone, unTaskError, unPlanStart, unAggStart);

  invoke("chat_completion", {
    messages: messages.map((m) => ({ role: m.role, content: m.content, reasoning_content: m.reasoning_content })),
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

export async function saveHistory(
  sessionId: string,
  role: string,
  content: string,
  toolCalls?: string,
  reasoningContent?: string,
  attachments?: Attachment[],
): Promise<number> {
  // Serialise the attachments array as JSON so it can be stored in a
  // single TEXT column. Pass `null` (not `undefined`) when absent so the
  // Tauri command receives an explicit `None` and writes NULL.
  const attachmentsJson = attachments && attachments.length > 0
    ? JSON.stringify(attachments)
    : null;
  return invoke("save_history", {
    sessionId,
    role,
    content,
    toolCalls: toolCalls ?? null,
    reasoningContent: reasoningContent ?? null,
    attachments: attachmentsJson,
  });
}

export interface HistoryRecord {
  id: number;
  session_id: string;
  role: string;
  content: string;
  timestamp: string;
  tool_calls?: string;
  reasoning_content?: string;
  /** JSON-serialized `Attachment[]` for user messages. */
  attachments?: string;
}

export async function loadHistory(): Promise<HistoryRecord[]> {
  return invoke("load_history");
}

export async function deleteHistory(sessionId: string): Promise<void> {
  return invoke("delete_history", { sessionId });
}

export async function deleteMessage(messageId: number): Promise<void> {
  return invoke("delete_message", { messageId });
}

export async function forkSession(
  sourceSessionId: string,
  newSessionId: string,
  upToMessageId: number
): Promise<number> {
  return invoke("fork_session", { sourceSessionId, newSessionId, upToMessageId });
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

export async function cancelMcpTest(id: string): Promise<void> {
  return invoke("cancel_mcp_test", { id });
}

// ─── MCP Diagnostic Log ───────────────────────────────────────────────────────

export async function getMcpLogs(id: string): Promise<McpLogEntry[]> {
  return invoke<McpLogEntry[]>("get_mcp_logs", { id });
}

export async function clearMcpLogs(id: string): Promise<void> {
  return invoke("clear_mcp_logs", { id });
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

export async function listAgentMissions(sessionId: string): Promise<AgentMissionSnapshot[]> {
  return invoke("list_agent_missions", { sessionId });
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

export async function saveMarkdownFile(path: string, content: string): Promise<void> {
  return invoke("save_markdown_file", { path, content });
}

// ─── Named Profiles ──────────────────────────────────────────────────────────

export async function listProfiles(): Promise<Profile[]> {
  return invoke("list_profiles");
}

export async function saveProfile(profile: Profile): Promise<void> {
  return invoke("save_profile_config", { profile });
}

export async function deleteProfile(name: string): Promise<void> {
  return invoke("delete_profile_config", { name });
}

export async function applyProfile(name: string): Promise<void> {
  return invoke("apply_profile_config", { name });
}
