import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { compactAttachmentsForDb } from "./attachments";
import type {
  AppConfig,
  Attachment,
  ArchivedSkill,
  EvolutionMemoryEntry,
  InstanceInfo,
  Message,
  MessageContent,
  ModelMetadata,
  Skill,
  McpServer,
  McpLogEntry,
  SubAgent,
  AgentMention,
  AgentOrchestration,
  AgentTaskEvent,
  AgentMissionSnapshot,
  AgentBusMessage,
  CliProbe,
  CloudProbe,
  KeyValue,
  Profile,
  SkillsStatus,
  ServeProcessInfo,
  SavedFigure,
  SleepLockState,
  TimerEntry,
  BackgroundTaskState,
  EvolutionMaintenanceState,
  GlassEffect,
  TodoListSummary,
  UpdateInfo,
  UpdateDownloadProgress,
  WorkspaceFileEntry,
  WorkspaceFileRange,
} from "./types";

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

/**
 * Which window the backend is: the primary that owns the shared config, or a
 * shadow running on a temporary copy for this window only.
 */
export async function getInstanceInfo(): Promise<InstanceInfo> {
  return invoke("get_instance_info");
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

/**
 * Files the composer's `@` mention popup can offer (hidden files and
 * `.gitignore`d entries are already filtered out by the backend).
 */
export async function listWorkspaceFiles(): Promise<WorkspaceFileEntry[]> {
  return invoke("list_workspace_files");
}

/**
 * Read a workspace text file, optionally sliced to a 1-based line range:
 * `startLine`/`endLine` both set → that range, `endLine` omitted → to EOF.
 */
export async function readWorkspaceFileRange(
  path: string,
  opts?: { startLine?: number; endLine?: number },
): Promise<WorkspaceFileRange> {
  return invoke("read_workspace_file_range", {
    path,
    startLine: opts?.startLine ?? null,
    endLine: opts?.endLine ?? null,
  });
}

/** Base64 payload of an image/binary workspace file (attached whole). */
export async function readWorkspaceFileBytes(path: string): Promise<string> {
  return invoke("read_workspace_file_bytes", { path });
}

/**
 * Store one file pasted into the composer (`Ctrl+V`, or a drop of file bytes)
 * under `<workspace>/.data/tmp/` and return its workspace-relative path.
 *
 * The path is what the composer inserts as an `@.data/tmp/…` mention, so the
 * file is attached by the normal mention pipeline when the turn is sent — no
 * separate attachment channel, and nothing typed by the user is turned into
 * HTML.
 */
export async function savePastedFile(
  name: string,
  dataBase64: string,
): Promise<string> {
  return invoke("save_pasted_file", { name, dataBase64 });
}

/**
 * Copy files dropped onto the composer (real OS paths, from Tauri's native
 * drag-drop event) into `<workspace>/.data/tmp/`.
 *
 * `imported` holds the workspace-relative paths of the copies that were made
 * (the composer turns them into `@.data/tmp/…` mentions) and `problems` holds one
 * line per refused path — a folder, an oversized file, an unreadable one — so a
 * partial drop still attaches what arrived.
 */
export interface ImportedWorkspacePaths {
  imported: string[];
  problems: string[];
}

export async function importWorkspacePaths(
  paths: string[],
): Promise<ImportedWorkspacePaths> {
  return invoke("import_workspace_paths", { paths });
}

/**
 * Write one rendered figure into `<workspace>/figures/`.
 *
 * `pngBase64` is the rasterised image without the `data:` prefix (the backend
 * rejects anything that is not a PNG), and `id` is the file-name stem: the
 * model's `#name` when it gave one, else `fig-<n>`.
 */
export async function saveFigure(request: {
  id: string;
  pngBase64: string;
}): Promise<SavedFigure> {
  return invoke("save_figure", { id: request.id, pngBase64: request.pngBase64 });
}

/** Open `<workspace>/figures` in the OS file manager. */
export async function openFiguresDir(): Promise<void> {
  return invoke("app_open_figures_dir");
}

/**
 * Markdown of one PDF mention (`@paper.pdf#2`).
 *
 * The page range is converted in-process by the `pdf2md-rs` library and cached
 * under `<workspace>/.data/tmp/pdf/`; `markdown` is what goes into the prompt
 * and `path` is where the full conversion stayed on disk.
 */
export interface PdfMentionResult {
  markdown: string;
  /** Workspace-relative path of the cached Markdown. */
  path: string;
  /** 1-based first converted page. */
  start_page: number;
  /** 1-based last converted page (clamped to the document). */
  end_page: number;
  total_pages: number;
  /** The page range or the prompt budget dropped content. */
  truncated: boolean;
}

/** One progress tick of a PDF mention conversion, streamed while it runs. */
export interface PdfMentionProgress {
  /**
   * Pipeline stage: `reading`, `extracting`, `analysing`, `parsing`,
   * `cleaning`, `rendering` or `done`.
   */
  stage: string;
  /** Page being worked on; 0 outside the per-page stages. */
  page: number;
  /** Pages in the requested range; 0 outside the per-page stages. */
  total: number;
  /** Overall completion, 0–100, as the library computes it. */
  percent: number;
}

/**
 * Render the page range of a workspace PDF (`#2`, `#2-5`, `#2-`) to Markdown.
 *
 * Omitted bounds mean "no range in the mention": the backend then converts a
 * bounded preview (60 pages) instead of a whole book; `endPage` omitted *with*
 * a `startPage` means "to the last page".
 *
 * `onProgress` receives the conversion's ticks (`page 12 of 30`, then the
 * finishing stages) over a Tauri channel while the command runs on a blocking
 * thread — the composer has nothing else to show during that wait.
 */
export async function convertPdfMention(
  path: string,
  opts?: { startPage?: number; endPage?: number },
  onProgress?: (update: PdfMentionProgress) => void,
): Promise<PdfMentionResult> {
  const progress = new Channel<PdfMentionProgress>();
  progress.onmessage = (update) => onProgress?.(update);
  return invoke("convert_pdf_mention", {
    path,
    startPage: opts?.startPage ?? null,
    endPage: opts?.endPage ?? null,
    onProgress: progress,
  });
}

export async function saveConfig(config: AppConfig): Promise<void> {
  return invoke("save_config", { config });
}

/**
 * Store (or clear with `null`) the context window for one model id.
 *
 * Shared per model: every sub-agent that runs the model uses the same value.
 * Clearing it falls back to the built-in default (128K for sub-agents).
 */
export async function setModelContextLength(
  model: string,
  contextLength: number | null,
): Promise<void> {
  return invoke("set_model_context_length", { model, contextLength });
}

/**
 * Apply the window-level backdrop behind a frosted-glass level.
 *
 * The desktop seen through the window is blurred by the compositor, not by CSS:
 * Mica on Windows 11, Acrylic on Windows 10. Returns the kind now in effect, or
 * `null` when the platform has no backdrop — the caller writes it to
 * `<html data-vibrancy>`, which is the only switch that turns the translucent
 * window tokens on (`src/App.css`). A `null` answer therefore keeps the app on
 * its opaque canvas rather than over a raw, unblurred desktop.
 */
export async function setWindowVibrancy(
  level: GlassEffect,
  dark: boolean,
): Promise<WindowVibrancy | null> {
  return invoke("set_window_vibrancy", { level, dark });
}

/** Native backdrop actually applied to the window. */
export type WindowVibrancy = "mica" | "acrylic";

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

/**
 * Model capability catalogue (tools / reasoning / vision / files, context
 * windows …) from the public `basellm/llm-metadata` feed. Entries are returned
 * unfiltered — match them against the remote model ids with
 * `matchModelMetadata` from `utils/modelMetadata`.
 *
 * The backend serves a 6-hour memory cache; pass `force` to re-download.
 */
export async function fetchModelMetadata(force = false): Promise<ModelMetadata[]> {
  return invoke("fetch_model_metadata", { force });
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
 *
 * Read-only: the persisted selection (`<workspace>/.data/skills.status`) keeps
 * names whose folder is temporarily gone, so this only decides which of them
 * can be sent to the model right now — it never edits the selection.
 */
export async function filterExistingSkills(names: string[]): Promise<string[]> {
  return invoke("filter_existing_skills", { names });
}

/**
 * Read the workspace skill status (`<workspace>/.data/skills.status`), folding
 * in any skill that appeared on disk. Entries are never removed.
 */
export async function getSkillsStatus(): Promise<SkillsStatus> {
  return invoke("get_skills_status");
}

/** Persist the workspace skill status (enablement + metadata). */
export async function saveSkillsStatus(status: SkillsStatus): Promise<void> {
  return invoke("save_skills_status", { status });
}

// ─── Self-evolution memory ───────────────────────────────────────────────────

/**
 * Ranked evolution memory entries, in exactly the order the prompt injects
 * them. Read-only: the panel never mutates memory.
 */
export async function listEvolutionMemory(): Promise<EvolutionMemoryEntry[]> {
  return invoke("list_evolution_memory");
}

/** Full markdown of one entry (front matter + body), hot or archived. */
export async function readEvolutionEntry(key: string): Promise<string> {
  return invoke("read_evolution_entry", { key });
}

/** Skills that currently own an archive of consolidated memory entries. */
export async function listArchivedSkills(): Promise<ArchivedSkill[]> {
  return invoke("list_archived_skills");
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

// ─── Evolution maintenance (the empty-state "nap face") ─────────────────────

/**
 * Start a silent maintenance pass. Returns as soon as the backend claimed the
 * run; the outcome arrives through `onEvolutionMaintenanceState`.
 */
export async function startEvolutionMaintenance(modelOverride?: string): Promise<void> {
  return invoke("start_evolution_maintenance", { modelOverride });
}

/** Stop the running maintenance pass (double-click on the napping face). */
export async function stopEvolutionMaintenance(): Promise<void> {
  return invoke("stop_evolution_maintenance");
}

/** Whether a maintenance pass is running (used to restore the napping face). */
export async function getEvolutionMaintenanceState(): Promise<boolean> {
  return invoke("get_evolution_maintenance_state");
}

/** Subscribe to the maintenance turn's start / terminal state. */
export async function onEvolutionMaintenanceState(
  cb: (state: EvolutionMaintenanceState) => void,
): Promise<UnlistenFn> {
  return listen<EvolutionMaintenanceState>("evolution-maintenance-state", (e) =>
    cb(e.payload),
  );
}

// ─── Delay timers ────────────────────────────────────────────────────────────

/**
 * Pending delay timers (soonest first). Used once on mount; afterwards the
 * `timer-state` event keeps the list current.
 */
export async function listTimers(): Promise<TimerEntry[]> {
  return invoke("list_timers");
}

/** Cancel a pending timer so it never resumes the conversation. */
export async function cancelTimer(id: string): Promise<TimerEntry> {
  return invoke("cancel_timer", { id });
}

// ─── Background tool tasks ───────────────────────────────────────────────────

/** Background tasks of one chat session (used on mount / session switch). */
export async function listBackgroundTasks(
  sessionId: string,
): Promise<BackgroundTaskState[]> {
  return invoke("list_background_tasks", { sessionId });
}

// ─── Todo list ───────────────────────────────────────────────────────────────

/**
 * The session's active todo list, or `null` when it has none. The panel is
 * mounted per assistant message and therefore has no state of its own: this is
 * what makes a list created in an earlier turn (or before a reload) visible as
 * soon as the user sends the next message.
 */
export async function getActiveTodoList(
  sessionId: string,
): Promise<TodoListSummary | null> {
  return invoke("get_active_todo_list", { sessionId });
}

/** Stop one background task; its whole process tree is terminated. */
export async function cancelBackgroundTask(
  taskId: string,
): Promise<BackgroundTaskState> {
  return invoke("cancel_background_task", { taskId });
}

/**
 * Current sleep-lock state (badge in the timer bar). Used once on mount;
 * afterwards the `sleep-lock` event reports every change.
 */
export async function getSleepLockState(): Promise<SleepLockState> {
  return invoke("get_sleep_lock_state");
}

// ─── run_shell serve processes ───────────────────────────────────────────────

/**
 * Every background process started by `run_shell` with `run_type: "serve"`,
 * including recently finished ones. Polled by the Tools panel monitor.
 */
export async function listServeProcesses(): Promise<ServeProcessInfo[]> {
  return invoke("list_serve_processes");
}

/** Stop one serve process (or remove a finished entry). */
export async function stopServeProcess(serveId: string): Promise<string> {
  return invoke("stop_serve_process", { serveId });
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
  /** The mission report was assembled — the step is already complete. */
  onAgentAggregateDone?: (taskCount: number) => void;
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
  /**
   * Composer `+agent` selections for a session's FIRST turn. The backend
   * snapshots their system prompts into `session_meta.embedded_roles`; later
   * turns pass nothing and read the snapshot instead.
   */
  embeddedAgentIds?: string[],
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];

  const cleanup = () => {
    unlisteners.forEach((fn) => fn());
    unlisteners.length = 0;
  };

  // `chat_completion` can both emit `chat-error` and reject the invoke promise
  // (the same failure travels through both paths). Callbacks must run once per
  // turn — a doubled `onError` would persist the partial assistant row twice.
  let settled = false;
  const finish = (error?: string) => {
    if (settled) return;
    settled = true;
    if (error === undefined) {
      callbacks.onDone();
    } else {
      callbacks.onError(error);
    }
    cleanup();
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
    finish();
  });
  const unError = await listen<string>("chat-error", (e) => {
    finish(e.payload);
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
  const unAggDone = await listen<{ task_count: number }>("agent-aggregate-done", (e) => {
    callbacks.onAgentAggregateDone?.(e.payload?.task_count ?? 0);
  });

  unlisteners.push(unToken, unReasoning, unDone, unError, unTaskStart, unTaskDone, unTaskError, unPlanStart, unAggDone);

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
    embeddedAgentIds: embeddedAgentIds ?? [],
  }).catch((err: string) => {
    finish(err);
  });

  // Mark the turn closed before removing listeners: a late invoke rejection
  // after Stop must not run the error path (and persist a second partial row).
  return () => {
    settled = true;
    cleanup();
  };
}

export async function saveHistory(
  sessionId: string,
  role: string,
  content: string,
  toolCalls?: string,
  reasoningContent?: string,
  attachments?: Attachment[],
  /** JSON-serialized `ChatInjection[]` for an assistant turn. */
  injections?: string,
  /** UI-only failure text; never part of the model context. */
  error?: string,
  /** `done` / `error` / `stopped`; omitted for a normal completed turn. */
  status?: string,
): Promise<number> {
  // Serialise the attachments array as JSON so it can be stored in a
  // single TEXT column. Pass `null` (not `undefined`) when absent so the
  // Tauri command receives an explicit `None` and writes NULL.
  //
  // A converted PDF mention is stored by path instead of by text: its Markdown
  // is already in `content`, and the two copies would double the row.
  const stored = attachments && attachments.length > 0
    ? compactAttachmentsForDb(attachments)
    : undefined;
  const attachmentsJson = stored && stored.length > 0
    ? JSON.stringify(stored)
    : null;
  return invoke("save_history", {
    sessionId,
    role,
    content,
    toolCalls: toolCalls ?? null,
    reasoningContent: reasoningContent ?? null,
    attachments: attachmentsJson,
    injections: injections ?? null,
    error: error ?? null,
    status: status ?? null,
  });
}

/**
 * Push a message into the assistant turn that is currently running.
 *
 * Returns `accepted: false` when no turn is running for this session — the
 * caller must then send the text as a normal next-turn message, because a turn
 * that already made its last request will never read the queue.
 */
export async function injectUserMessage(
  sessionId: string,
  id: string,
  text: string,
): Promise<{ accepted: boolean }> {
  return invoke("inject_user_message", { sessionId, id, text });
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
  /** JSON-serialized `ChatInjection[]` for assistant turns. */
  injections?: string;
  /** UI-only failure text restored for display. */
  error?: string;
  /** `done` / `error` / `stopped` for partial assistant turns. */
  status?: string;
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

export async function startStdioMcp(id: string): Promise<void> {
  return invoke("mcp_start_stdio", { serverId: id });
}

export async function stopStdioMcp(id: string): Promise<void> {
  return invoke("mcp_stop_stdio", { serverId: id });
}

// ─── MCP Diagnostic Log ───────────────────────────────────────────────────────

export async function getMcpLogs(id: string): Promise<McpLogEntry[]> {
  return invoke<McpLogEntry[]>("get_mcp_logs", { id });
}

export async function clearMcpLogs(id: string): Promise<void> {
  return invoke("clear_mcp_logs", { id });
}

/**
 * One registry mutation performed by the assistant's `mcp_self` tool
 * (self-evolution mode): create / update / enable / disable / remove.
 *
 * The open MCP panel refreshes its list from this event, so a server the model
 * built mid-chat shows up without reopening the panel.
 */
export interface McpServersChanged {
  id: string;
  name: string;
  action: string;
  enabled: boolean;
  source?: string;
  definition_path?: string;
}

export async function onMcpServersChanged(
  cb: (change: McpServersChanged) => void,
): Promise<UnlistenFn> {
  return listen<McpServersChanged>("mcp-servers-changed", (e) => cb(e.payload));
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
  /** Cached prompt-prefix tokens; `null` when the provider does not report caching. */
  cached_tokens: number | null;
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
  /** Cached prompt-prefix tokens; `null` when the provider does not report caching. */
  cached_tokens: number | null;
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

/**
 * Lightweight `+agent` popup roster: enabled `llm` agents with a system prompt,
 * already carrying the token that gets inserted after `+`.
 */
export async function listAgentMentions(): Promise<AgentMention[]> {
  return invoke("list_agent_mentions");
}

/**
 * Re-scan the agents/ markdown tree, replace `agents/index.yaml`, and return
 * the merged roster. The Sub Agents panel's Refresh button is the only
 * discovery trigger.
 */
export async function refreshSubAgentIndex(): Promise<SubAgent[]> {
  return invoke("refresh_agent_index");
}

export async function saveSubAgent(agent: SubAgent): Promise<SubAgent> {
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

export async function listMissionBusMessages(
  missionId: string,
  limit = 200,
): Promise<AgentBusMessage[]> {
  return invoke("list_mission_bus_messages", { missionId, limit });
}

// ─── Agent probes (Sub Agents panel "Test" buttons) ──────────────────────────

/**
 * Resolve a `cli` agent's command (PATH + PATHEXT) and try to run it (`args`,
 * e.g. `["--version"]`).
 */
export async function probeAgentCli(
  command: string,
  args?: string[],
): Promise<CliProbe> {
  return invoke("probe_agent_cli", { command, args: args ?? null });
}

/** Check that a `cloud` agent's endpoint is reachable with the given token. */
export async function probeAgentCloud(
  endpoint: string,
  token?: string,
  headers?: KeyValue[],
): Promise<CloudProbe> {
  return invoke("probe_agent_cloud", {
    endpoint,
    token: token ?? null,
    headers: headers ?? null,
  });
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
  /** Metadata JSON (`"{}"` when absent); `llm_api` rows carry the cache-hit metrics. */
  metadata: string;
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

/**
 * One tool's traffic inside a session (Monitor → session report ranking).
 * Mirrors `ToolUsageStat` in `src-tauri/src/db.rs`.
 */
export interface ToolUsageStat {
  name: string;
  calls: number;
  /** Calls that ended in a `tool_error` / `mcp_call` row. */
  failures: number;
  duration_ms: number;
}

/**
 * Aggregated analysis of one chat session, rendered in the Monitor panel's
 * bottom strip. Mirrors `SessionStats` in `src-tauri/src/db.rs`.
 */
export interface SessionStats {
  requests: number;
  failed_requests: number;
  prompt_tokens: number;
  completion_tokens: number;
  /** Prompt tokens served from the provider cache — the report's "hit" amount. */
  hit_tokens: number;
  /** Prompt tokens processed from scratch (`prompt_tokens - hit_tokens`). */
  miss_tokens: number;
  /** Requests that reported cache metrics at all (`0` hit ≠ no feedback). */
  cache_reported_requests: number;
  cache_hit_requests: number;
  /** Prompt tokens of the newest successful request — the context in use. */
  context_tokens: number;
  /** Context window of the model behind it (`0` = unknown). */
  context_window: number;
  tool_calls: number;
  tool_failures: number;
  llm_errors: number;
  /** Interaction rows recorded for the session, all types. */
  interactions: number;
  total_request_ms: number;
  total_tool_ms: number;
  /** Rounds that streamed a thinking phase before answering. */
  reasoning_requests: number;
  /** Summed thinking phase of those rounds (ms). */
  reasoning_ms_total: number;
  /** Rounds that reported a first-token latency, and its total (ms). */
  first_token_requests: number;
  first_token_ms_total: number;
  tools: ToolUsageStat[];
}

/** Session analysis report: token usage, cache hit/miss, context and tools. */
export async function getSessionStats(sessionId: string): Promise<SessionStats> {
  return invoke("get_session_stats", { sessionId });
}

export async function saveMarkdownFile(path: string, content: string): Promise<void> {
  return invoke("save_markdown_file", { path, content });
}

// ─── App menu (custom titlebar) ──────────────────────────────────────────────

/** Open the app data directory in the OS file manager. */
export async function openAppDataDir(): Promise<void> {
  return invoke("app_open_data_dir");
}

/** Open the active workspace directory in the OS file manager. */
export async function openWorkspaceDir(): Promise<void> {
  return invoke("app_open_workspace_dir");
}

/** Open `<workspace>/.data/tmp` — the sub-agent deliverable exchange area. */
export async function openWorkspaceTmpDir(): Promise<void> {
  return invoke("app_open_workspace_tmp_dir");
}

/** Open `<app_data>/agents` (the `llm` agent definition folder). */
export async function openAgentsDir(): Promise<void> {
  return invoke("app_open_agents_dir");
}

/** Open the GitHub new-issue page in the system browser. */
export async function openIssuePage(): Promise<void> {
  return invoke("app_open_issue_page");
}

/**
 * Read a user-picked Markdown file and hand it to the built-in editor
 * (`markdown-edit-open` / `markdown-edit-error` events).
 */
export async function openMarkdownFile(path: string): Promise<void> {
  return invoke("app_open_markdown", { path });
}

/** Export the profile archive to a path the caller picked. */
export async function exportProfileTo(path: string): Promise<void> {
  return invoke("app_export_profile", { path });
}

/** Restore a profile archive from a path the caller picked. */
export async function importProfileFrom(path: string): Promise<void> {
  return invoke("app_import_profile", { path });
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
