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
  UpdateInfo,
  UpdateDownloadProgress,
} from "./types";

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function confirmCommand(
  requestId: string,
  confirmed: boolean,
  opts?: { username?: string; password?: string },
): Promise<void> {
  return invoke("confirm_command", {
    requestId,
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

/**
 * One entry of the remote `/models` listing.
 * `context_length` is only present when the provider reports a context
 * window (OpenRouter, Groq, vLLM, LM Studio…); OpenAI's own endpoint
 * does not — set it manually in Settings in that case.
 */
export interface RemoteModel {
  id: string;
  context_length?: number;
}

export async function fetchModels(): Promise<RemoteModel[]> {
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

/**
 * Return the subset of `names` that still resolve to a loadable skill.
 * Used to prune `selected_skills` after a workspace switch or profile load.
 */
export async function filterExistingSkills(names: string[]): Promise<string[]> {
  return invoke("filter_existing_skills", { names });
}

// ─── Update check (GitHub Releases) ──────────────────────────────────────────

/** Fetch the newest published GitHub release and compare it with this build. */
export async function checkUpdate(includePrerelease?: boolean): Promise<UpdateInfo> {
  return invoke("check_update", { includePrerelease });
}

/** Download a release asset; resolves with the path it was saved to. */
export async function downloadUpdate(assetName: string, downloadUrl: string): Promise<string> {
  return invoke("download_update", { assetName, downloadUrl });
}

/** Open the downloaded installer with the OS default handler. */
export async function openUpdateFile(path: string): Promise<void> {
  return invoke("open_update_file", { path });
}

/** Reveal the downloaded installer in the file manager. */
export async function revealUpdateFile(path: string): Promise<void> {
  return invoke("reveal_update_file", { path });
}

/** Open the release page in the default browser. */
export async function openReleasePage(url: string): Promise<void> {
  return invoke("open_release_page", { url });
}

/** Version of the running app bundle. */
export async function getAppVersion(): Promise<string> {
  return invoke("get_app_version");
}

/** Subscribe to update download progress events. */
export async function onUpdateDownloadProgress(
  cb: (progress: UpdateDownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<UpdateDownloadProgress>("update-download-progress", (e) => cb(e.payload));
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
  messages: {
    role: Message["role"];
    content: MessageContent;
    reasoning_content?: string;
    /** Skills loaded earlier in the session (assistant messages only). */
    loaded_skills?: string[];
  }[],
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
    messages: messages.map((m) => ({
      role: m.role,
      content: m.content,
      reasoning_content: m.reasoning_content,
      loaded_skills: m.loaded_skills,
    })),
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

/** One row of the history sidebar — no message bodies. */
export interface HistorySessionSummary {
  session_id: string;
  message_count: number;
  created_at: string;
  /** First user message text (clamped), used as the default title. */
  first_user_content: string;
}

/**
 * List history sessions (newest activity first). `keyword` filters by session
 * id, message content or the first user message; omit it to list everything.
 * The payload never contains full message bodies.
 */
export async function listHistorySessions(keyword?: string): Promise<HistorySessionSummary[]> {
  return invoke("list_history_sessions", { keyword: keyword?.trim() ? keyword : null });
}

/** Load every message of one session, oldest first (no truncation). */
export async function loadSessionMessages(sessionId: string): Promise<HistoryRecord[]> {
  return invoke("load_session_messages", { sessionId });
}

export async function deleteHistory(sessionId: string): Promise<void> {
  return invoke("delete_history", { sessionId });
}

export interface SessionMeta {
  session_id: string;
  title?: string;
  favorite: boolean;
  archived: boolean;
}

export async function listSessionMeta(): Promise<SessionMeta[]> {
  return invoke("list_session_meta");
}

export async function updateSessionMeta(
  sessionId: string,
  fields: { title?: string; favorite?: boolean; archived?: boolean }
): Promise<void> {
  return invoke("update_session_meta", {
    sessionId,
    title: fields.title ?? null,
    favorite: fields.favorite ?? null,
    archived: fields.archived ?? null,
  });
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

/** Result of a database compaction run. */
export interface CompactDbResult {
  /** Expired log rows dropped by the retention policy during this run. */
  rows_pruned: number;
  /** Retention window (days) that was applied; `0` = pruning disabled. */
  retention_days: number;
  bytes_before: number;
  bytes_after: number;
}

/**
 * Delete every request/interaction log row. Message history is untouched.
 * Returns the number of removed rows; run `compactDatabase` to reclaim the
 * space on disk.
 */
export async function clearLogs(): Promise<number> {
  return invoke("clear_logs");
}

/**
 * Apply the configured log retention window (Settings → Runtime & Debug) and
 * `VACUUM` the database file so deleted rows actually free disk space.
 */
export async function compactDatabase(): Promise<CompactDbResult> {
  return invoke("compact_database");
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
