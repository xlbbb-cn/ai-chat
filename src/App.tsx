import { useState, useRef, useEffect, useLayoutEffect, useCallback, useMemo } from "react";
import { listen } from "@tauri-apps/api/event";
import { chatCompletion, checkUpdate, getConfig, getSkillsStatus, getAgentOrchestration, injectUserMessage, listMcpServers, listSubAgents, saveSkillsStatus, saveConfig, saveHistory, stopChatCompletion, confirmCommand, saveMarkdownFile, deleteMessage, forkSession, filterExistingSkills, listTimers, cancelTimer, getSleepLockState, listBackgroundTasks, cancelBackgroundTask, listWorkspaceFiles, listSkills } from "./api";

import { ChatMessage } from "./components/ChatMessage";
import { ToolCallGroup } from "./components/ToolCallGroup";
import { SettingsPanel } from "./components/SettingsPanel";
import { ModelSelect } from "./components/ModelSelect";
import { SkillsPanel } from "./components/SkillsPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { ToolsPanel } from "./components/ToolsPanel";
import { McpPanel } from "./components/McpPanel";
import { AgentsPanel } from "./components/AgentsPanel";
import { MarkdownPreview } from "./components/MarkdownPreview";
import { Portal } from "./components/Portal";
import { TitleBar } from "./components/TitleBar";
import { ChatNotice } from "./components/ChatNotice";
import type {
  ChatNoticeAction,
  ChatNoticeState,
  ChatNoticeTone,
} from "./components/ChatNotice";
import { UpdatePanel } from "./components/UpdatePanel";
import { FileMentionPopup } from "./components/FileMentionPopup";
import { SlashMentionPopup } from "./components/SlashMentionPopup";
import type {
  AppConfig,
  Message,
  AgentTaskEvent,
  AgentHelpEvent,
  AgentSelectionEvent,
  AgentTaskTokenEvent,
  AgentTaskToolEvent,
  AgentTaskReasoningEvent,
  AgentMbusMessageEvent,
  AgentPlannerStateEvent,
  AgentPlannerActionEvent,
  ChatInjection,
  ToolCallEntry,
  Attachment,
  ContentPart,
  MessageContent,
  ModelMetadata,
  Palette,
  SkillsStatus,
  SleepLockState,
  ThemeMode,
  TimerEntry,
  BackgroundTaskState,
  UpdateInfo,
  WorkspaceFileEntry,
} from "./types";
import { extensionOf, supportsMentionName } from "./attachments";
import { activeMentionQuery, resolveFileMentions, scoreWorkspaceFile } from "./fileMention";
import type { MentionProgress, MentionResolution } from "./fileMention";
import { activeSlashQuery, scoreSkillOption, toSkillOptions } from "./slashMention";
import type { SkillMentionOption } from "./slashMention";
import { isGenerationStopped, isLocale, stripGenerationStopped, useI18n } from "./i18n";
import { formatCountdown } from "./utils/time";
import {
  FontAwesomeIcon,
  faBrain,
  faClockRotateLeft,
  faGear,
  faPenToSquare,
  faPlug,
  faRobot,
  faRotateRight,
  faScrewdriverWrench,
  faSpinner,
  faStopwatch,
  faTriangleExclamation,
  faWandMagicSparkles,
  faXmark,
} from "./icons";
import "./App.css";

type Sidebar = "settings" | "skills" | "history" | "tools" | "mcp" | "agents" | null;

/**
 * Paint the current theme + palette on `<html>`.
 *
 * The two are independent dimensions: `data-theme` resolves the light /
 * dark half (with `auto` following the OS), `data-palette` selects the
 * colour scheme. Both are read by the token blocks in `src/App.css`.
 */
function applyTheme(theme: ThemeMode | undefined, palette: Palette | undefined) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const resolved = theme === "dark" || (theme !== "light" && prefersDark) ? "dark" : "light";
  document.documentElement.setAttribute("data-theme", resolved);
  document.documentElement.setAttribute("data-palette", palette ?? "default");
}

// ─── Delay timers ────────────────────────────────────────────────────────────

/**
 * The user turn a fired timer produces. The prefix makes an automatic resume
 * recognisable in the transcript (and to the model) instead of looking like
 * something the user typed.
 */
function timerPromptText(timer: TimerEntry): string {
  const label = timer.label?.trim();
  return label
    ? `⏱️ [Timer fired: ${label}]\n${timer.message}`
    : `⏱️ [Timer fired]\n${timer.message}`;
}

// ─── Attachment handling ─────────────────────────────────────────────────────

/** Strip the `data:<mime>;base64,` prefix — `input_audio` wants the payload. */
function dataUrlBase64(dataUrl: string): string {
  const comma = dataUrl.indexOf(",");
  return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
}

/**
 * Code-fence language for an inlined text attachment.
 *
 * The name may carry a range (`kk.pdf#2`, `src/App.tsx#12-24`), which would
 * otherwise leak into the fence tag, and a converted PDF mention is Markdown
 * no matter what its extension says.
 */
function fenceLanguage(att: Attachment): string {
  if (att.mime === "text/markdown") return "markdown";
  const ext = extensionOf(att.name.split("#")[0]);
  return /^[a-z0-9]+$/.test(ext) ? ext : "";
}

/** Render a user message's attachments into a structured `content` payload. */
function buildMessageContent(
  text: string,
  attachments: Attachment[],
): { content: MessageContent } {
  if (attachments.length === 0) {
    return { content: text };
  }

  const parts: ContentPart[] = [];
  const trimmedText = text.trim();
  if (trimmedText.length > 0) {
    parts.push({ type: "text", text: trimmedText });
  }

  for (const att of attachments) {
    if (att.kind === "image" && att.data_url) {
      parts.push({
        type: "image_url",
        image_url: { url: att.data_url },
      });
    } else if (att.kind === "file" && att.data_url) {
      parts.push({
        type: "file",
        file: { filename: att.name, file_data: att.data_url },
      });
    } else if (att.kind === "audio" && att.data_url) {
      parts.push({
        type: "input_audio",
        input_audio: {
          data: dataUrlBase64(att.data_url),
          // The schema only accepts `wav` and `mp3` (enforced by the picker).
          format: extensionOf(att.name) === "wav" ? "wav" : "mp3",
        },
      });
    } else if (att.kind === "text" && att.text_content !== undefined) {
      parts.push({
        type: "text",
        text: `\n<details><summary>Attached File: ${att.name}</summary>\n\n\`\`\`${fenceLanguage(att)}\n${att.text_content}\n\`\`\`\n</details>`,
      });
    }
  }

  return { content: parts };
}

/** Persist a multimodal content payload as a single text column. */
function serializeContentForDb(content: MessageContent): string {
  if (typeof content === "string") return content;
  return JSON.stringify(content);
}

/**
 * Skills loaded earlier in the session, derived from the per-message
 * tool-call entries. Sent to the backend as explicit `loaded_skills` state,
 * so a skill is loaded at most once per session (and the backend never has
 * to inspect message text).
 */
function loadedSkillsOfMessage(m: Message): string[] | undefined {
  const names = (m.tool_calls ?? [])
    .map((tc) => tc.skill_name)
    .filter((n): n is string => typeof n === "string" && n.length > 0);
  if (names.length === 0) return undefined;
  return Array.from(new Set(names));
}

/**
 * Content replayed to the model. The stop marker is a UI affordance, so it is
 * stripped here instead of ever reaching the provider. User content may stay
 * multimodal.
 */
function contextContent(m: Message): MessageContent {
  return m.role === "assistant" && typeof m.content === "string"
    ? stripGenerationStopped(m.content)
    : m.content;
}

/**
 * A failed/stopped assistant turn is still useful context when it produced
 * visible text or reasoning. Empty interrupted turns (including rows that only
 * carry an error notice) are skipped instead of becoming blank assistant turns.
 */
function hasContextContent(m: Message): boolean {
  if (m.role !== "assistant") return true;
  const content = typeof m.content === "string" ? stripGenerationStopped(m.content) : "";
  return content.trim().length > 0 || Boolean(m.reasoning_content?.trim());
}

/** An assistant turn that ended with an error or a user stop. */
function isInterruptedAssistant(m: Message): boolean {
  return (
    m.role === "assistant" &&
    (Boolean(m.error) || m.status === "error" || m.status === "stopped")
  );
}

/**
 * The action a failed turn offers in its notice. Resuming only makes sense when
 * the turn left something usable behind — an immediate rate limit with no
 * tokens has nothing to continue.
 */
function resumeAction(
  label: string,
  content: string,
  reasoning: string,
  toolCallCount: number,
  run: () => void,
): ChatNoticeAction | undefined {
  const resumable =
    content.trim() !== "" || reasoning.trim() !== "" || toolCallCount > 0;
  return resumable ? { label, run } : undefined;
}

interface AgentStatus {
  status: "idle" | "running" | "done" | "error";
  description?: string;
  summary?: string;
  error?: string;
  tokens?: number;
}

/** How long the `@file` popup trusts its cached workspace listing. */
const WORKSPACE_FILES_TTL_MS = 15_000;
/** Rows the `@file` popup shows at once — enough to be useful, short enough
 *  that arrow-key navigation stays predictable. */
const MENTION_MAX_ITEMS = 8;
/** How long the `/skill` popup trusts its cached skill list. */
const SKILL_MENTIONS_TTL_MS = 15_000;

interface MarkdownEditPayload {
  path: string;
  content: string;
}

export default function App() {
  const { t, locale, setLocale } = useI18n();
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  /**
   * Composer `@file` popup. `start` is the index of the `@` that opened it —
   * the whole token is replaced when an item is accepted.
   */
  const [mentionState, setMentionState] = useState<{
    open: boolean;
    query: string;
    start: number;
  }>({ open: false, query: "", start: 0 });
  const [mentionIndex, setMentionIndex] = useState(0);
  /** Workspace files for the popup; `null` until the first `@` triggers a load. */
  const [workspaceFiles, setWorkspaceFiles] = useState<WorkspaceFileEntry[] | null>(null);
  const [workspaceFilesLoading, setWorkspaceFilesLoading] = useState(false);
  /** Timestamped cache of `workspaceFiles`, so the walk runs at most every 15s. */
  const workspaceFilesRef = useRef<{ at: number; files: WorkspaceFileEntry[] } | null>(null);
  const workspaceFilesPendingRef = useRef(false);
  /**
   * Composer `/skill` popup, the Claude-Code style sibling of the `@file` one.
   * `start` is the index of the `/` that opened it — the whole token is
   * replaced when a skill is accepted.
   */
  const [slashState, setSlashState] = useState<{
    open: boolean;
    query: string;
    start: number;
  }>({ open: false, query: "", start: 0 });
  const [slashIndex, setSlashIndex] = useState(0);
  /** Workspace skills for the popup; `null` until the first `/` triggers a load. */
  const [slashSkills, setSlashSkills] = useState<SkillMentionOption[] | null>(null);
  const [slashSkillsLoading, setSlashSkillsLoading] = useState(false);
  /** Timestamped cache of `slashSkills`, so the list is read at most every 15s. */
  const slashSkillsRef = useRef<{ at: number; options: SkillMentionOption[] } | null>(null);
  const slashSkillsPendingRef = useRef(false);
  const [streaming, setStreaming] = useState(false);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
  const [sidebarMotion, setSidebarMotion] = useState<"opening" | "closing" | null>(null);
  /** Geometry of the toolbar's sliding selection fill (measured from the active button). */
  const [toolbarIndicator, setToolbarIndicator] = useState<{ left: number; width: number } | null>(null);
  const [activeSkillIds, setActiveSkillIds] = useState<string[]>([]);
  /**
   * Per-workspace skill status (`<workspace>/.data/skills.status`): the
   * persisted enablement plus provenance. `activeSkillIds` is the *derived*
   * subset that actually resolves right now and is sent to the model — the
   * persisted selection is never pruned by a workspace switch.
   */
  const [skillStatus, setSkillStatus] = useState<SkillsStatus | null>(null);
  /** Gate: the status file is only written back after the first load. */
  const [skillStatusLoaded, setSkillStatusLoaded] = useState(false);
  /** Bumped after a workspace switch so HistoryPanel re-reads its DB. */
  const [historyRefreshKey, setHistoryRefreshKey] = useState(0);
  // Warnings and errors are transient notices, never transcript rows — see
  // `components/ChatNotice.tsx`. `setError` keeps its original call shape
  // (`setError(null)` clears) and takes an optional tone and action — the
  // failed turn's "continue" is only reachable through the notice, now that
  // the transcript carries no failure row. Every call bumps a nonce so a
  // repeated message restarts the dwell instead of reading as "no change" and
  // leaving a half-faded bar on screen.
  const [notice, setNotice] = useState<ChatNoticeState | null>(null);
  const noticeNonceRef = useRef(0);
  const setError = useCallback(
    (
      message: string | null,
      tone: ChatNoticeTone = "error",
      action?: ChatNoticeAction,
    ) => {
      if (!message) {
        setNotice(null);
        return;
      }
      noticeNonceRef.current += 1;
      setNotice({ text: message, tone, nonce: noticeNonceRef.current, action });
    },
    [],
  );
  const dismissNotice = useCallback(() => setNotice(null), []);
  // A failed turn reports itself from inside `sendMessage`, which is defined
  // before the resume action exists; the notice reads the action through this
  // ref so a click always runs the current one instead of a stale render's.
  const continueUnfinishedRef = useRef<() => void>(() => {});
  // Newest published release; shared by the startup banner and the update dialog.
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [updatePanelOpen, setUpdatePanelOpen] = useState(false);
  const [updateBannerDismissed, setUpdateBannerDismissed] = useState(false);
  const [usage, setUsage] = useState<{
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens?: number;
    max_tokens?: number;
    usage_ratio?: number;
    /** Cached prompt-prefix tokens of the current request (provider-reported). */
    cached_tokens?: number;
    /** Cached / prompt for the current request; absent when the endpoint
     *  does not report caching at all, so `0` stays distinguishable. */
    hit_ratio?: number;
  } | null>(null);
  /** True once `chat-usage` arrived for the request currently streaming. */
  const [usageReady, setUsageReady] = useState(false);
  /**
   * PDF mention conversion in flight: `sendMessage` resolves mentions before the
   * turn starts, and a long document would otherwise be a silent pause. Drives
   * the strip above the input row; `null` whenever nothing is converting.
   */
  const [mentionProgress, setMentionProgress] = useState<MentionProgress | null>(null);
  const [maxTokens, setMaxTokens] = useState<number | null>(null);
  const [availableModels, setAvailableModels] = useState<string[]>(["gpt-4o-mini"]);
  const [selectedModel, setSelectedModel] = useState("gpt-4o-mini");
  /** Capabilities per model, learned from the llm-metadata catalogue. */
  const [modelMetadata, setModelMetadata] = useState<Record<string, ModelMetadata>>({});
  /**
   * Capabilities of the model currently selected in the toolbar. Declared here
   * (not next to its other uses further down) because `sendMessage` and the
   * `@file` popup both need it in their dependency lists.
   */
  const selectedMetadata = modelMetadata[selectedModel];
  /** Thinking depth per model (`reasoning_effort`); absent = provider default. */
  const [reasoningEfforts, setReasoningEfforts] = useState<Record<string, string>>({});
  const [activeToolCount, setActiveToolCount] = useState(0);
  const [activeMcpCount, setActiveMcpCount] = useState(0);
  const [activeAgentCount, setActiveAgentCount] = useState(0);
  const [useAgentsEnabled, setUseAgentsEnabled] = useState(false);
  const [agentStatuses, setAgentStatuses] = useState<Record<string, AgentStatus>>({});
  const [confirmDialog, setConfirmDialog] = useState<{
    request_id: string;
    reason: string;
    cmd_type: string;
    code: string;
    confirm_kind?: "dangerous" | "sudo" | "elevation" | "system_config" | "user_software" | "user_data" | "sensitive_read" | "general_query" | "external_agent";
    requires_auth?: "none" | "sudo" | "elevation";
    risk_level?: string;
    risk_score?: number;
    disposition?: string;
    blacklist_hits?: Array<{ rule_id: string; severity: string; matched: string; contribution: number }>;
    penalty_items?: Array<{ name: string; points: number }>;
  } | null>(null);
  const [confirmUsername, setConfirmUsername] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [riskAssessment, setRiskAssessment] = useState<{
    request_id?: string;
    cmd_type?: string;
    code?: string;
    risk_level?: string;
    risk_score?: number;
    disposition?: string;
    recommendation?: string;
    blacklist_hits?: Array<{ rule_id: string; severity: string; matched: string; contribution: number }>;
    penalty_items?: Array<{ name: string; points: number }>;
    requires_confirmation?: boolean;
  } | null>(null);
  const [profileExporting, setProfileExporting] = useState(false);
  const [profileExportPhase, setProfileExportPhase] = useState<"idle" | "preparing" | "status">("idle");
  const [profileExportStatus, setProfileExportStatus] = useState("");
  const [pendingRetryMessageId, setPendingRetryMessageId] = useState<string | null>(null);
  const [markdownEditorOpen, setMarkdownEditorOpen] = useState(false);
  const [markdownPath, setMarkdownPath] = useState("");
  const [markdownDraft, setMarkdownDraft] = useState("");
  const [markdownSaving, setMarkdownSaving] = useState(false);
  /** Pending delay timers created by the `timer_set` tool (soonest first). */
  const [timers, setTimers] = useState<TimerEntry[]>([]);
  /** Cross-platform "system awake" lock, shown as a badge above the input. */
  const [sleepLock, setSleepLock] = useState<SleepLockState>({ active: false, holders: 0 });
  /**
   * Wall-clock start of the running turn, so the sleep-lock badge can also
   * show how long this turn has been going. `null` while the chat is idle.
   */
  const [turnStartedAt, setTurnStartedAt] = useState<number | null>(null);
  /** 1 s heartbeat that drives the countdowns in the timer bar. */
  const [timerTick, setTimerTick] = useState(() => Date.now());
  /** Timer that fired for a session the user is not currently in. */
  const [timerNotice, setTimerNotice] = useState<TimerEntry | null>(null);
  /** Background tool tasks of the active turn (`run_in_background`). */
  const [backgroundTasks, setBackgroundTasks] = useState<BackgroundTaskState[]>([]);
  /** >0 while the backend is holding the turn open for background results. */
  const [backgroundWaiting, setBackgroundWaiting] = useState(0);
  /** Transcript scroll container; its position decides whether the view
   *  follows incoming content or stays where the user left it. */
  const messagesRef = useRef<HTMLDivElement>(null);
  /** `true` while the transcript follows new content at the bottom; cleared
   *  when the user scrolls up, re-armed when they come back down. */
  const followBottomRef = useRef(true);
  /** `scrollTop` last applied by the tail-follow effect above. The scrollbar
   *  logic needs to tell that automatic pin-to-bottom apart from a scroll the
   *  user actually made (wheel / thumb drag), otherwise the bar would stay
   *  visible for the whole duration of a streaming reply. */
  const programmaticScrollRef = useRef<number | null>(null);
  /** The two bars the transcript can scroll under. Their edge shade is
   *  toggled imperatively — it tracks scroll position, not render state, so a
   *  class flip beats re-rendering the whole tree on every scroll event. */
  const toolbarRef = useRef<HTMLElement>(null);
  const toolbarActionsRef = useRef<HTMLDivElement>(null);
  const inputAreaRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const themeRef = useRef<ThemeMode>("auto");
  const paletteRef = useRef<Palette>("default");
  const currentAssistantMessageIdRef = useRef<string | null>(null);
  const hasRunningToolCallRef = useRef(false);
  const currentToolCallsRef = useRef<ToolCallEntry[]>([]);
  /** Mission bus of the current turn; old bus events must not touch new rows. */
  const currentMbusIdRef = useRef<string | null>(null);
  /**
   * Buffered sub-agent stream state per task, flushed on a timer:
   * `text` is `agent-task-token` content, `reasoning` counts thinking chars.
   */
  const pendingAgentTokensRef = useRef<
    Map<string, { text: string; reasoning: number }>
  >(new Map());
  /** Resume prompts from fired timers, plus mid-turn injections that arrived
   *  after the turn's last request. Sent one at a time once the chat is idle,
   *  so the turn order stays intact. */
  const [pendingAutoPrompts, setPendingAutoPrompts] = useState<string[]>([]);
  /** Injections accepted by the running turn that have not reached a request
   *  yet, shown as queued chips above the input. */
  const [pendingInjections, setPendingInjections] = useState<ChatInjection[]>([]);
  /** Authoritative copy of `pendingInjections` for the once-registered event
   *  listener and the turn-end callback, neither of which may read stale state. */
  const pendingInjectionsRef = useRef<ChatInjection[]>([]);
  /** Injections already delivered to a request during the turn that is
   *  streaming now — persisted with the assistant row in `onDone`. */
  const currentInjectionsRef = useRef<ChatInjection[]>([]);
  /** Latest active session for the timer listener (registered once). */
  const sessionIdRef = useRef(sessionId);

  /**
   * Load the workspace skill status and derive the enabled list.
   *
   * The status file is the source of truth and lives next to the workspace's
   * skills, so switching workspaces loads a different selection instead of
   * pruning a global one. Names whose folder has gone missing are kept in the
   * file but filtered out here, so only resolvable skills reach the model.
   */
  const loadSkillStatus = useCallback(async (): Promise<void> => {
    const status = await getSkillsStatus();
    const enabled = Object.entries(status.skills)
      .filter(([, entry]) => entry.enabled)
      .map(([name]) => name);
    let resolvable = enabled;
    try {
      resolvable = await filterExistingSkills(enabled);
    } catch (err) {
      // If validation itself fails (IPC error), keep the selection rather than
      // silently dropping the user's active skills.
      console.error("Failed to validate active skills", err);
    }
    setSkillStatus(status);
    setActiveSkillIds(resolvable);
    setSkillStatusLoaded(true);
  }, []);

  /** Reload persisted config into the UI, pruning skills that no longer exist. */
  const applyConfigToUi = useCallback(async (): Promise<AppConfig> => {
    const cfg = await getConfig();
    const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
    setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
    setSelectedModel(cfg.model || "gpt-4o-mini");
    setMaxTokens(cfg.model_context_lengths?.[cfg.model] ?? cfg.model_settings?.max_tokens ?? null);
    setModelMetadata(cfg.model_metadata ?? {});
    setReasoningEfforts(cfg.model_reasoning_effort ?? {});
    setActiveToolCount((cfg.selected_tools ?? []).length);
    // The config is the source of truth for the UI language (localStorage is
    // only a first-paint cache), so a profile import can switch it too.
    if (isLocale(cfg.language)) setLocale(cfg.language);
    return cfg;
  }, [setLocale]);

  // Silent update probe on startup: it never blocks the chat and never surfaces
  // errors — Settings has an explicit "Check for updates" action instead.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const cfg = await getConfig();
        if (cfg.check_updates_on_startup === false) return;
        const info = await checkUpdate();
        if (!cancelled && info.has_update) setUpdateInfo(info);
      } catch (err) {
        console.error("Update check failed", err);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const updateActiveAssistantToolCalls = useCallback((toolCalls: ToolCallEntry[]) => {
    const assistantId = currentAssistantMessageIdRef.current;
    if (!assistantId) return;

    setMessages((prev) =>
      prev.map((message) =>
        message.id === assistantId
          ? { ...message, tool_calls: toolCalls }
          : message
      )
    );
  }, []);

  /**
   * Apply the buffered sub-agent stream to the task rows.
   *
   * The 400 ms timer keeps a fast stream from re-rendering the transcript per
   * token, while the first chunk of a task is applied immediately (the caller
   * flushes on the leading edge) so a row never sits blank after it starts.
   * A task row keeps its planner description and its own status: content goes
   * to `summary`, reasoning to the `thinking` counter, tool calls to
   * `activity`.
   */
  const flushAgentTokenBuffers = useCallback(() => {
    const pending = pendingAgentTokensRef.current;
    if (pending.size === 0) return;
    const chunks = new Map(pending);
    pending.clear();

    currentToolCallsRef.current = currentToolCallsRef.current.map((entry) => {
      const chunk = chunks.get(entry.task_id);
      if (!chunk) return entry;

      if (entry.kind === "agent") {
        const reasoning = (entry.thinking?.chars ?? 0) + chunk.reasoning;
        return {
          ...entry,
          summary: chunk.text
            ? ((entry.summary ?? "") + chunk.text).slice(-1200)
            : entry.summary,
          // A new reasoning burst follows the previous tool: drop its activity
          // line so the live thinking counter is what the row shows. Streamed
          // content ends the thinking phase, so the answer becomes visible.
          activity: chunk.reasoning > 0 ? undefined : entry.activity,
          thinking: chunk.text
            ? undefined
            : reasoning > 0
              ? {
                  chars: reasoning,
                  started_at: entry.thinking?.started_at ?? Date.now(),
                }
              : entry.thinking,
        };
      }

      const output = ((entry.summary ?? "") + chunk.text).slice(-1200);
      const lastLine = chunk.text
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line.length > 0)
        .pop();
      return {
        ...entry,
        summary: output,
        description: lastLine ? lastLine.slice(0, 140) : entry.description,
      };
    });
    updateActiveAssistantToolCalls(currentToolCallsRef.current);
  }, [updateActiveAssistantToolCalls]);

  /**
   * Adds a stream delta to a task's buffer and applies it immediately when the
   * row has nothing visible yet (the first content / reasoning chunk of that
   * task). Later chunks stay batched in the 400 ms flush.
   */
  const appendAgentBuffer = useCallback(
    (taskId: string, delta: { text?: string; reasoning?: number }) => {
      const buffered = pendingAgentTokensRef.current.get(taskId) ?? {
        text: "",
        reasoning: 0,
      };
      pendingAgentTokensRef.current.set(taskId, {
        text: delta.text
          ? (buffered.text + delta.text).slice(-4000)
          : buffered.text,
        reasoning: buffered.reasoning + (delta.reasoning ?? 0),
      });
      const entry = currentToolCallsRef.current.find(
        (item) => item.task_id === taskId
      );
      if (entry && !entry.summary && !entry.thinking) flushAgentTokenBuffers();
    },
    [flushAgentTokenBuffers]
  );

  /**
   * Paint the scroll-edge shade on a boundary only while the transcript hides
   * content past it — top shade when the first message has scrolled out above,
   * bottom shade when more messages remain below. At the very top / bottom the
   * corresponding shade is gone, so the edges "rest" against the bars.
   */
  const syncScrollEdges = useCallback(() => {
    const el = messagesRef.current;
    if (!el) return;
    const scrollable = el.scrollHeight - el.clientHeight > 1;
    const atTop = el.scrollTop <= 0;
    const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 1;
    toolbarRef.current?.classList.toggle("is-scrolled", scrollable && !atTop);
    inputAreaRef.current?.classList.toggle("is-scrolled", scrollable && !atBottom);
  }, []);

  /**
   * Follow the tail only while the transcript is already at the bottom.
   * Streaming updates `messages` many times per second; a view the user
   * scrolled up to re-read must not be yanked back down by incoming tokens.
   *
   * `scrollTop` (not `scrollIntoView({ behavior: "smooth" })`) pins the view
   * instantly — a smooth animation lags behind a fast stream, and that lag
   * would look like "scrolled away from the bottom" to the scroll handler.
   */
  useEffect(() => {
    const el = messagesRef.current;
    if (!el) return;
    if (followBottomRef.current) {
      el.scrollTop = el.scrollHeight;
      // Read back immediately: the browser clamps to the real maximum.
      programmaticScrollRef.current = el.scrollTop;
    }
    syncScrollEdges();
  }, [messages, syncScrollEdges]);

  /** Re-arm the follow once the user scrolls back to (near) the bottom. */
  const handleMessagesScroll = useCallback((event: React.UIEvent<HTMLDivElement>) => {
    const el = event.currentTarget;
    followBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 48;
    syncScrollEdges();
  }, [syncScrollEdges]);

  // A resize (window, banner, sidebar) changes how much of the transcript fits
  // without scrolling, so the edge shades must be re-evaluated too.
  useEffect(() => {
    const el = messagesRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => syncScrollEdges());
    observer.observe(el);
    return () => observer.disconnect();
  }, [syncScrollEdges]);

  /**
   * Sliding toolbar selection.
   *
   * One fill travels between the panel buttons instead of every `.active`
   * button painting its own background. Geometry comes from the button, so a
   * locale change or a count badge widening a button is picked up; closing the
   * sidebar only fades the fill out, keeping its last position so reopening
   * the same panel does not make it jump in from the left.
   */
  useLayoutEffect(() => {
    const container = toolbarActionsRef.current;
    if (!container) return;

    const measure = () => {
      const active = container.querySelector<HTMLButtonElement>(".toolbar-btn.active");
      if (active) {
        setToolbarIndicator({ left: active.offsetLeft, width: active.offsetWidth });
      }
    };

    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    container.querySelectorAll<HTMLButtonElement>(".toolbar-btn").forEach((button) => {
      observer.observe(button);
    });
    return () => observer.disconnect();
  }, [
    sidebar,
    locale,
    activeAgentCount,
    activeMcpCount,
    activeSkillIds.length,
    activeToolCount,
    useAgentsEnabled,
  ]);

  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = "auto";
      textareaRef.current.style.height = textareaRef.current.scrollHeight + "px";
    }
  }, [input]);

  useEffect(() => {
    const unlisten = listen<{
      prompt_tokens: number;
      completion_tokens: number;
      total_tokens?: number;
      max_tokens?: number;
      usage_ratio?: number;
      cached_tokens?: number;
      hit_ratio?: number;
    }>("chat-usage", (e) => {
      // This request's token figures are known from here on; the usage bar
      // above the input switches from its indeterminate sweep to a real ratio.
      setUsageReady(true);
      setUsage((prevUsage) => {
        // If this is the first usage data, return it as-is
        if (!prevUsage) return e.payload;

        // Accumulate token counts within the same session
        // (for session history), but preserve current request's max_tokens and usage_ratio
        const prevTotal = prevUsage.total_tokens ?? prevUsage.prompt_tokens + prevUsage.completion_tokens;
        const currentTotal = e.payload.total_tokens ?? e.payload.prompt_tokens + e.payload.completion_tokens;

        return {
          // Accumulated token counts for session history display
          prompt_tokens: prevUsage.prompt_tokens + e.payload.prompt_tokens,
          completion_tokens: prevUsage.completion_tokens + e.payload.completion_tokens,
          total_tokens: prevTotal + currentTotal,
          // Keep the current request's max_tokens for ratio calculation (not accumulated)
          max_tokens: e.payload.max_tokens ?? prevUsage.max_tokens,
          // Keep the current request's usage_ratio (not accumulated)
          // This correctly reflects the current request's token usage within its max_tokens limit
          usage_ratio: e.payload.usage_ratio,
          // Accumulated cache savings, plus the current request's hit ratio
          // (like usage_ratio, hit_ratio describes this request only). It is
          // overwritten, never kept from the previous request: an endpoint
          // without cache reporting must drop the stale figure instead of
          // keeping the last provider's hit rate in the tooltip.
          cached_tokens: (prevUsage.cached_tokens ?? 0) + (e.payload.cached_tokens ?? 0),
          hit_ratio: e.payload.hit_ratio,
        };
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Context menu is now allowed so users can right-click to copy/paste.

  useEffect(() => {
    const panelScrollableSelector = [
      ".messages",
      ".history-list",
      ".settings-body",
      ".skills-list",
      ".skill-editor",
      ".tools-list",
      ".agents-list",
      ".agent-editor",
      ".mcp-list",
      ".mcp-form",
    ].join(", ");
    const hideDelayMs = 900;
    const minVisibleMs = 260;
    const minScrollDelta = 5;
    const hideTimers = new Map<HTMLElement, number>();
    const lastShownAt = new Map<HTMLElement, number>();
    const lastScrollTop = new Map<HTMLElement, number>();

    const hideScrollbar = (el: HTMLElement) => {
      el.classList.remove("scrolling-active");
      hideTimers.delete(el);
      lastShownAt.delete(el);
    };

    const scheduleHide = (el: HTMLElement) => {
      const existingTimer = hideTimers.get(el);
      if (existingTimer !== undefined) {
        window.clearTimeout(existingTimer);
      }

      const nextTimer = window.setTimeout(() => {
        const shownAt = lastShownAt.get(el) ?? Date.now();
        const elapsed = Date.now() - shownAt;
        if (elapsed < minVisibleMs) {
          const holdTimer = window.setTimeout(() => hideScrollbar(el), minVisibleMs - elapsed);
          hideTimers.set(el, holdTimer);
          return;
        }
        hideScrollbar(el);
      }, hideDelayMs);

      hideTimers.set(el, nextTimer);
    };

    const handlePanelScroll = (event: Event) => {
      const target = event.target;
      if (!(target instanceof HTMLElement)) return;
      if (!target.matches(panelScrollableSelector)) return;
      if (target.scrollHeight <= target.clientHeight) return;

      const previousTop = lastScrollTop.get(target) ?? target.scrollTop;
      const delta = Math.abs(target.scrollTop - previousTop);
      lastScrollTop.set(target, target.scrollTop);

      // The transcript pins itself to the newest content while a reply
      // streams. That follow is not the user scrolling, so it never reveals
      // the bar — only a real wheel / drag (a position the effect did not
      // write) counts as activity. The marker belongs to `.messages` alone,
      // so a panel scrolling first cannot swallow it.
      if (target === messagesRef.current) {
        const programmaticTop = programmaticScrollRef.current;
        programmaticScrollRef.current = null;
        if (programmaticTop !== null && Math.abs(target.scrollTop - programmaticTop) < 2) {
          return;
        }
      }

      const isActive = target.classList.contains("scrolling-active");
      if (!isActive && delta < minScrollDelta) return;

      if (!isActive) {
        target.classList.add("scrolling-active");
        lastShownAt.set(target, Date.now());
      }

      scheduleHide(target);
    };

    document.addEventListener("scroll", handlePanelScroll, true);
    return () => {
      document.removeEventListener("scroll", handlePanelScroll, true);
      hideTimers.forEach((timerId) => window.clearTimeout(timerId));
      hideTimers.clear();
      lastShownAt.clear();
      lastScrollTop.clear();
    };
  }, []);

  useEffect(() => {
    const unlisten = listen("request-set-workspace-dir", () => {
      setSidebar("settings");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // A workspace switch swaps the whole workspace-scoped state: config reload,
  // skill status (the previous workspace's selection must not leak into the
  // new one), and the database that backs the history sidebar.
  useEffect(() => {
    const unlisten = listen<string>("workspace-changed", async () => {
      try {
        await applyConfigToUi();
        await loadSkillStatus();
        setHistoryRefreshKey((key) => key + 1);
      } catch (err) {
        console.error(err);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [applyConfigToUi, loadSkillStatus]);

  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      listen<MarkdownEditPayload>("markdown-edit-open", (event) => {
        setMarkdownPath(event.payload.path);
        setMarkdownDraft(event.payload.content ?? "");
        setMarkdownEditorOpen(true);
      }),
      listen<string>("markdown-edit-error", (event) => {
        setError(event.payload);
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  // Dangerous-command confirmation dialog
  useEffect(() => {
    // Risk assessment event (emitted before confirmation for L0-L4 commands)
    const unlistenRisk = listen<{
      request_id?: string;
      cmd_type?: string;
      code?: string;
      risk_level?: string;
      risk_score?: number;
      disposition?: string;
      recommendation?: string;
      blacklist_hits?: Array<{ rule_id: string; severity: string; matched: string; contribution: number }>;
      penalty_items?: Array<{ name: string; points: number }>;
      requires_confirmation?: boolean;
    }>("risk-assessment", (e) => {
      setRiskAssessment(e.payload);
    });

    const unlisten = listen<{
      request_id: string;
      reason: string;
      cmd_type: string;
      code: string;
      confirm_kind?: "dangerous" | "sudo" | "elevation" | "system_config" | "user_software" | "user_data" | "sensitive_read" | "general_query" | "external_agent";
      requires_auth?: "none" | "sudo" | "elevation";
      risk_level?: string;
      risk_score?: number;
      disposition?: string;
      blacklist_hits?: Array<{ rule_id: string; severity: string; matched: string; contribution: number }>;
      penalty_items?: Array<{ name: string; points: number }>;
    }>(
      "confirm-required",
      (e) => {
        const { request_id, reason, cmd_type, code, confirm_kind, requires_auth, risk_level, risk_score, disposition, blacklist_hits, penalty_items } = e.payload;
        // `run_shell` confirmations that need no credentials are answered
        // inline in the tool-call group (allow/deny buttons on the card);
        // everything else — run_cmd, sudo/elevation, external agents — keeps
        // the standalone dialog. `run_shell` carries the shell type as
        // `cmd_type`, `run_cmd` sends "direct".
        const inlineShell =
          (requires_auth ?? "none") === "none" &&
          confirm_kind !== "external_agent" &&
          ["bash", "sh", "powershell", "pwsh"].includes(cmd_type);
        if (inlineShell) {
          const entry: ToolCallEntry = {
            task_id: request_id,
            agent_name: "run_shell",
            description: code.split("\n")[0].slice(0, 140),
            status: "running",
            confirm: { request_id, code, reason },
          };
          currentToolCallsRef.current = [...currentToolCallsRef.current, entry];
          updateActiveAssistantToolCalls(currentToolCallsRef.current);
          hasRunningToolCallRef.current = true;
          return;
        }
        setConfirmDialog((current) =>
          current ?? { request_id, reason, cmd_type, code, confirm_kind, requires_auth, risk_level, risk_score, disposition, blacklist_hits, penalty_items }
        );
        setConfirmUsername("");
        setConfirmPassword("");
      }
    );
    return () => {
      unlisten.then((fn) => fn());
      unlistenRisk.then((fn) => fn());
    };
  }, [updateActiveAssistantToolCalls]);

  const respondToConfirm = useCallback((confirmed: boolean) => {
    const requiresSudo = confirmDialog?.requires_auth === "sudo";
    confirmCommand(confirmDialog?.request_id ?? "", confirmed, requiresSudo ? { username: confirmUsername, password: confirmPassword } : undefined).catch(console.error);
    setConfirmDialog(null);
    setConfirmUsername("");
    setConfirmPassword("");
    setRiskAssessment(null);
  }, [confirmDialog, confirmUsername, confirmPassword]);

  /**
   * Answer an inline `run_shell` confirmation from the tool-call group: reply
   * to the backend (it is blocked inside the tool call), then settle the
   * pending entry — "running" on allow (the follow-up `tool-call` event marks
   * it done), "error" on deny (the tool returns without emitting anything).
   */
  const respondToolCallConfirm = useCallback((requestId: string, allowed: boolean) => {
    confirmCommand(requestId, allowed).catch(console.error);
    currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
      entry.confirm?.request_id === requestId
        ? {
            ...entry,
            confirm: undefined,
            status: allowed ? ("running" as const) : ("error" as const),
            error: allowed ? undefined : t("toolGroup.rejected"),
          }
        : entry
    );
    updateActiveAssistantToolCalls(currentToolCallsRef.current);
  }, [updateActiveAssistantToolCalls, t]);

  useEffect(() => {
    applyConfigToUi()
      .then((cfg) => {
        themeRef.current = cfg.theme ?? "auto";
        paletteRef.current = cfg.palette ?? "default";
        applyTheme(cfg.theme, cfg.palette);
      })
      .catch(console.error);
    // The skill selection is workspace-scoped, so it is loaded from the
    // workspace status file instead of the config.
    loadSkillStatus().catch(console.error);
  }, [applyConfigToUi, loadSkillStatus]);

  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => { if (themeRef.current === "auto") applyTheme("auto", paletteRef.current); };
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

  useEffect(() => {
    listMcpServers()
      .then((servers) => setActiveMcpCount(servers.filter((s) => s.enabled).length))
      .catch(console.error);
  }, []);

  useEffect(() => {
    listSubAgents()
      .then((agents) => setActiveAgentCount(agents.filter((a) => a.enabled).length))
      .catch(console.error);
    getAgentOrchestration()
      .then((orch) => setUseAgentsEnabled(orch.use_agents))
      .catch(console.error);
  }, []);

  useEffect(() => {
    const unlisten = listen("profile-restored", async () => {
      try {
        // A restored profile can reference skills that are absent from the
        // current workspace, so re-validate before activating them.
        const [, servers] = await Promise.all([applyConfigToUi(), listMcpServers()]);
        setActiveMcpCount(servers.filter((s) => s.enabled).length);
      } catch (err) {
        console.error(err);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [applyConfigToUi]);

  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      listen("profile-export-start", () => {
        setProfileExporting(true);
        setProfileExportPhase("preparing");
      }),
      listen<string>("profile-export-status", (e) => {
        setProfileExportPhase("status");
        setProfileExportStatus(e.payload);
      }),
      listen("profile-export-done", () => {
        setProfileExporting(false);
        setProfileExportPhase("idle");
        setProfileExportStatus("");
      }),
      listen<string>("profile-export-error", (e) => {
        setProfileExporting(false);
        setProfileExportPhase("idle");
        setProfileExportStatus("");
        setError(t("app.profileExportFailed", { error: e.payload }));
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, [t]);

  // The Skills panel lists what is on disk; reloading the status when it opens
  // keeps the two in sync (e.g. after a skill was created by self-evolution).
  useEffect(() => {
    if (sidebar === "skills") {
      loadSkillStatus().catch(console.error);
    }
  }, [sidebar, loadSkillStatus]);

  // Persist skill enablement to the workspace status file. Waiting for the
  // initial load avoids writing an empty selection over the stored one.
  useEffect(() => {
    if (!skillStatusLoaded || !skillStatus) return;

    saveSkillsStatus(skillStatus).catch(console.error);
  }, [skillStatus, skillStatusLoaded]);

  // Agent task progress tracking
  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    // Appearance-order numbering for sub-agent rows (`#1`, `#2`, …), so the
    // tool group matches the planner's task ids without exposing long uuids.
    const nextAgentIndex = () =>
      currentToolCallsRef.current.reduce(
        (max, entry) => Math.max(max, entry.agent_index ?? 0),
        0
      ) + 1;
    unlisteners.push(
      // Turn-guard notices from the backend (`turn_guard`): a tool disabled for
      // the rest of the session after repeated failures, or the turn's round /
      // tool-call budget being spent. Both are informational — the turn keeps
      // running — so they surface as a warning notice, never as a failed turn.
      listen<{
        kind: "tool_disabled" | "budget_exhausted";
        tool?: string;
        failures?: number;
        limit?: number;
        rounds?: number;
        tool_calls?: number;
      }>("chat-notice", (e) => {
        const payload = e.payload;
        if (!payload) return;
        if (payload.kind === "tool_disabled") {
          setError(
            t("app.toolDisabledNotice", {
              tool: payload.tool ?? "",
              limit: payload.limit ?? 0,
            }),
            "warning",
          );
          return;
        }
        setError(
          t("app.budgetExhaustedNotice", {
            rounds: payload.rounds ?? 0,
            toolCalls: payload.tool_calls ?? 0,
          }),
          "warning",
        );
      }),
      listen<AgentTaskEvent>("agent-task-start", (e) => {
        const { agent_id, agent_name, description, task_id, cwd } = e.payload;
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "running", description },
        }));
        const entry: ToolCallEntry = {
          task_id,
          agent_name,
          kind: "agent",
          agent_index: nextAgentIndex(),
          description: description ?? "",
          status: "running",
          cwd: cwd ?? undefined,
          started_at: Date.now(),
        };
        currentToolCallsRef.current = [...currentToolCallsRef.current, entry];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<AgentTaskEvent>("agent-task-done", (e) => {
        const { agent_id, summary, task_id, status } = e.payload;
        // Flush the last buffered chunks first: the final tokens of a task must
        // not be dropped when the row settles.
        flushAgentTokenBuffers();
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "done", summary: summary ?? "" },
        }));
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === task_id
            ? {
                ...entry,
                // `stopped` is neutral, never a green success checkmark.
                status:
                  status === "stopped"
                    ? ("skipped" as const)
                    : status === "error"
                      ? ("error" as const)
                      : ("done" as const),
                summary: summary ?? entry.summary,
                thinking: undefined,
              }
            : entry
        );
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<AgentTaskEvent>("agent-task-error", (e) => {
        const { agent_id, error, task_id } = e.payload;
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "error", error: error ?? "" },
        }));
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === task_id
            ? {
                ...entry,
                status: "error" as const,
                error: error ?? undefined,
                thinking: undefined,
              }
            : entry
        );
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      // A planned task that never ran (unknown agent, unmet dependency, user
      // stop). No `agent-task-start` was emitted for it, so the row is created
      // here; otherwise the report would silently be shorter than the plan.
      listen<AgentTaskEvent>("agent-task-skipped", (e) => {
        const { agent_id, agent_name, task_id, reason } = e.payload;
        const text = reason ?? "";
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "idle" },
        }));
        const exists = currentToolCallsRef.current.some(
          (entry) => entry.task_id === task_id
        );
        currentToolCallsRef.current = exists
          ? currentToolCallsRef.current.map((entry) =>
              entry.task_id === task_id
                ? { ...entry, status: "skipped" as const, description: entry.description || text }
                : entry
            )
          : [
              ...currentToolCallsRef.current,
              {
                task_id,
                agent_name,
                kind: "agent" as const,
                agent_index: nextAgentIndex(),
                description: text,
                status: "skipped" as const,
              },
            ];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      // Sub-agent → main-agent help exchange (`ask_main_agent`). The task row
      // shows the question and, once it returns, the size of the material.
      listen<AgentHelpEvent>("agent-help-start", (e) => {
        const { task_id, request } = e.payload;
        if (!task_id || !request) return;
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === task_id
            ? { ...entry, help: { request, status: "running" as const } }
            : entry
        );
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<AgentHelpEvent>("agent-help-done", (e) => {
        const { task_id, answer_chars } = e.payload;
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === task_id && entry.help
            ? {
                ...entry,
                help: {
                  ...entry.help,
                  status: "done" as const,
                  answer_chars: answer_chars ?? 0,
                },
              }
            : entry
        );
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<AgentHelpEvent>("agent-help-error", (e) => {
        const { task_id, error } = e.payload;
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === task_id && entry.help
            ? {
                ...entry,
                help: { ...entry.help, status: "error" as const, error: error ?? "" },
              }
            : entry
        );
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      // Selector chip: which agents were chosen for this turn (soft narrowing,
      // the main agent can still call any enabled agent).
      listen<AgentSelectionEvent>("agent-selection", (e) => {
        const { delegate, agents } = e.payload;
        if (!delegate || !agents?.length) return;
        const entry: ToolCallEntry = {
          task_id: `selection-${Date.now()}`,
          agent_name: t("app.agents.selector"),
          kind: "planner",
          description: t("app.agents.selectionDone", {
            count: agents.length,
            names: agents.map((agent) => agent.name).join(", "),
          }),
          status: "done",
        };
        currentToolCallsRef.current = [...currentToolCallsRef.current, entry];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<{ task_count: number; mbus?: boolean; bus_id?: string | null }>(
        "agent-plan-start",
        (e) => {
          if (e.payload.task_count <= 0) return;
          const mbus = e.payload.mbus === true;
          currentMbusIdRef.current = mbus ? (e.payload.bus_id ?? null) : null;
          const entry: ToolCallEntry = {
            task_id: "planner",
            agent_name: t("app.agents.planner"),
            kind: "planner",
            description: mbus
              ? t("app.agents.plannerMonitoring", { count: e.payload.task_count })
              : t("app.agents.planDone", { count: e.payload.task_count }),
            status: mbus ? "running" : "done",
            started_at: Date.now(),
          };
          const exists = currentToolCallsRef.current.some(
            (item) => item.task_id === "planner"
          );
          currentToolCallsRef.current = exists
            ? currentToolCallsRef.current.map((item) =>
                item.task_id === "planner"
                  ? { ...item, ...entry, mbus: item.mbus }
                  : item
              )
            : [...currentToolCallsRef.current, entry];
          updateActiveAssistantToolCalls(currentToolCallsRef.current);
        }
      ),
      // Planner supervisor: same row as the plan step, now with live activity.
      listen<AgentPlannerStateEvent>("agent-planner-state", (e) => {
        const payload = e.payload;
        if (payload.session_id && payload.session_id !== sessionIdRef.current) return;
        if (!currentMbusIdRef.current || payload.bus_id !== currentMbusIdRef.current) return;
        const status: ToolCallEntry["status"] =
          payload.status === "error"
            ? "error"
            : payload.status === "stopped"
              ? "skipped"
              : payload.status === "done"
                ? "done"
                : "running";
        const entry: ToolCallEntry = {
          task_id: "planner",
          agent_name: t("app.agents.planner"),
          description:
            payload.activity ||
            t("app.agents.plannerMonitoring", { count: payload.task_count }),
          status,
        };
        const exists = currentToolCallsRef.current.some(
          (item) => item.task_id === "planner"
        );
        currentToolCallsRef.current = exists
          ? currentToolCallsRef.current.map((item) =>
              item.task_id === "planner"
                ? { ...item, ...entry, mbus: item.mbus }
                : item
            )
          : [...currentToolCallsRef.current, entry];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      // Planner actions (schedule, mbus send/status/history, subscribe, finish)
      // become their own rows in the tool group. `action_id` is stable across
      // the running → done/error pair, so the row settles in place.
      listen<AgentPlannerActionEvent>("agent-planner-action", (e) => {
        const payload = e.payload;
        if (payload.session_id && payload.session_id !== sessionIdRef.current) return;
        if (!currentMbusIdRef.current || payload.bus_id !== currentMbusIdRef.current) return;
        const description = (() => {
          switch (payload.action) {
            case "schedule":
              return t("app.agents.plannerActionSchedule");
            case "send":
              return t("app.agents.plannerActionSend", { target: payload.target });
            case "status":
              return t("app.agents.plannerActionStatus");
            case "history":
              return t("app.agents.plannerActionHistory");
            case "subscribe":
              return t("app.agents.plannerActionSubscribe", { target: payload.target });
            case "unsubscribe":
              return t("app.agents.plannerActionUnsubscribe", { target: payload.target });
            case "direct":
              return t("app.agents.plannerActionDirect", { target: payload.target });
            case "end_task":
              return t("app.agents.plannerActionEndTask", { target: payload.target });
            case "finish":
              return t("app.agents.plannerActionFinish");
            default:
              return t("app.agents.plannerActionUnknown", { action: payload.action });
          }
        })();
        const entry: ToolCallEntry = {
          task_id: payload.action_id,
          agent_name: t("app.agents.planner"),
          kind: "planner",
          description,
          status: payload.status,
          summary: payload.status === "error" ? undefined : payload.detail || undefined,
          error: payload.status === "error" ? payload.detail || undefined : undefined,
        };
        const exists = currentToolCallsRef.current.some(
          (item) => item.task_id === payload.action_id
        );
        currentToolCallsRef.current = exists
          ? currentToolCallsRef.current.map((item) =>
              item.task_id === payload.action_id ? { ...item, ...entry } : item
            )
          : [...currentToolCallsRef.current, entry];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      // Mission-bus traffic: mirror counts and the last message on the sender
      // and recipient task rows. The full log lives in the Mission panel.
      listen<AgentMbusMessageEvent>("agent-mbus-message", (e) => {
        const payload = e.payload;
        if (payload.session_id && payload.session_id !== sessionIdRef.current) return;
        if (!currentMbusIdRef.current || payload.bus_id !== currentMbusIdRef.current) return;
        const body =
          payload.body.length > 120 ? `${payload.body.slice(0, 120)}…` : payload.body;
        const fromLabel =
          payload.from_id === "planner" ? t("app.agents.planner") : payload.from_name;
        const targetLabel =
          payload.target_kind === "topic"
            ? `#${payload.target_id}`
            : payload.target_id === "planner"
              ? t("app.agents.planner")
              : payload.recipients.map((recipient) => recipient.name).join(", ") ||
                payload.target_id;
        const recipientIds = new Set(payload.recipients.map((recipient) => recipient.id));
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) => {
          if (entry.task_id === payload.from_id) {
            return {
              ...entry,
              mbus: {
                sent: (entry.mbus?.sent ?? 0) + 1,
                received: entry.mbus?.received ?? 0,
                last: { direction: "out" as const, label: targetLabel, body },
              },
            };
          }
          if (recipientIds.has(entry.task_id)) {
            return {
              ...entry,
              mbus: {
                sent: entry.mbus?.sent ?? 0,
                received: (entry.mbus?.received ?? 0) + 1,
                last: { direction: "in" as const, label: fromLabel, body },
              },
            };
          }
          return entry;
        });
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<{ task_count: number }>("agent-aggregate-done", (e) => {
        // The backend emits this *after* the mission report is assembled, so
        // the step is complete the moment it arrives — rendering it as
        // "running" would spin until the whole final answer is streamed.
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === "planner" && entry.status === "running"
            ? { ...entry, status: "done" as const }
            : entry
        );
        const entry: ToolCallEntry = {
          task_id: `aggregate-${Date.now()}`,
          agent_name: t("app.agents.aggregator"),
          kind: "tool",
          description: t("app.agents.aggregated", { count: e.payload?.task_count ?? 0 }),
          status: "done",
        };
        currentToolCallsRef.current = [...currentToolCallsRef.current, entry];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<string>("tool-call", (e) => {
        const text = e.payload;
        // Parse label from *italics* and optional code block detail
        const firstLine = text.split('\n')[0];
        // Capture emoji before asterisks and the label inside asterisks
        const labelMatch = firstLine.match(/^(.*?)\*(.+?)\*$/);
        let label: string;
        if (labelMatch) {
          const emoji = labelMatch[1].trim();
          const text = labelMatch[2];
          label = emoji ? `${emoji} ${text}` : text;
        } else {
          label = firstLine.replace(/[*]/g, '').trim();
        }
        const codeMatch = text.match(/```(?:\w+)?\n([\s\S]+?)\n```/);
        const detail = codeMatch ? codeMatch[1].trim() : undefined;

        const entry: ToolCallEntry = {
          task_id: crypto.randomUUID(),
          agent_name: t("app.agents.tool"),
          kind: "tool",
          description: label,
          status: "running",
          summary: detail,
        };

        currentToolCallsRef.current = [
          ...currentToolCallsRef.current.map((tc) =>
            // Only the main chat's own steps settle here. A sub-agent row and
            // the planner have their own terminal events; settling them on the
            // next main tool call is what showed a green checkmark while a task
            // was still running.
            tc.status === "running" && tc.kind !== "agent" && tc.kind !== "planner"
              ? { ...tc, status: "done" as const }
              : tc
          ),
          entry,
        ];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
        hasRunningToolCallRef.current = true;
      }),
      listen<AgentTaskTokenEvent>("agent-task-token", (e) => {
        // Sub-agent output (llm / cli / cloud) arrives token by token; buffer
        // it and let the flush interval update the entry, so a long stream does
        // not re-render the whole transcript per token. The first chunk of a
        // task is applied immediately: it is the row's first visible text.
        const { task_id, token } = e.payload;
        if (!task_id || !token) return;
        appendAgentBuffer(task_id, { text: token });
      }),
      listen<AgentTaskReasoningEvent>("agent-task-reasoning", (e) => {
        // Reasoning is streamed separately from content: it is usually the
        // very first thing the model produces, so it makes a blank row show
        // "thinking" within a second instead of waiting for the answer.
        const { task_id, token } = e.payload;
        if (!task_id || !token) return;
        appendAgentBuffer(task_id, { reasoning: token.length });
      }),
      listen<AgentTaskToolEvent>("agent-task-tool", (e) => {
        // A tool call inside a sub-agent task. It updates only the activity
        // line — never the row status (the task's own `agent-task-done` /
        // `agent-task-error` owns that).
        const { task_id, phase, tool, detail, text, duration_ms, ok } = e.payload;
        if (!task_id) return;
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) => {
          if (entry.task_id !== task_id || entry.kind !== "agent") return entry;
          if (phase === "progress") {
            return text
              ? { ...entry, activity: text.trim().slice(0, 200), thinking: undefined }
              : entry;
          }
          if (phase === "start") {
            const label = [tool, detail].filter(Boolean).join(" · ").trim();
            return label
              ? { ...entry, activity: label.slice(0, 200), thinking: undefined }
              : entry;
          }
          // done: keep the tool name visible; the duration is informational.
          const seconds =
            duration_ms !== undefined ? ` · ${(duration_ms / 1000).toFixed(1)}s` : "";
          const label = [ok === false ? `⚠ ${tool}` : tool, detail]
            .filter(Boolean)
            .join(" · ");
          return {
            ...entry,
            activity: (label + seconds).trim().slice(0, 200),
          };
        });
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<string>("skill-loaded", (e) => {
        // Structured skill-load event. The entry carries `skill_name`, which
        // is what later turns send back as `loaded_skills`.
        const skillName = e.payload;
        if (!skillName) return;
        const entry: ToolCallEntry = {
          task_id: crypto.randomUUID(),
          agent_name: t("app.agents.tool"),
          kind: "tool",
          description: t("app.agents.loadingSkill", { name: skillName }),
          status: "running",
          skill_name: skillName,
        };
        currentToolCallsRef.current = [
          ...currentToolCallsRef.current.map((tc) =>
            tc.status === "running" && tc.kind !== "agent" && tc.kind !== "planner"
              ? { ...tc, status: "done" as const }
              : tc
          ),
          entry,
        ];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
        hasRunningToolCallRef.current = true;
      }),
      listen<{ items: ChatInjection[] }>("chat-injected", (e) => {
        // The backend drained these into the next request, so the model has
        // seen them. Anything still queued stays a chip until it either lands
        // here or the turn ends (see `onDone`, which re-sends the leftovers).
        const items = e.payload?.items ?? [];
        if (items.length === 0) return;
        const delivered = new Set(items.map((item) => item.id));
        pendingInjectionsRef.current = pendingInjectionsRef.current.filter(
          (item) => !delivered.has(item.id)
        );
        setPendingInjections(pendingInjectionsRef.current);
        currentInjectionsRef.current = [...currentInjectionsRef.current, ...items];
        const targetId = currentAssistantMessageIdRef.current;
        if (!targetId) return;
        setMessages((prev) =>
          prev.map((m) =>
            m.id === targetId
              ? { ...m, injections: [...(m.injections ?? []), ...items] }
              : m
          )
        );
      }),
      // Background tool task lifecycle. The entry is upserted by the `bg-…`
      // task id, so a task keeps one row from start to finish and the cancel
      // button can target it.
      listen<BackgroundTaskState>("background-task-state", (e) => {
        const task = e.payload;
        if (!task) return;
        // A late event from a previous session must not leak into this one.
        if (task.session_id && task.session_id !== sessionIdRef.current) return;
        setBackgroundTasks((prev) => {
          const next = [...prev];
          const index = next.findIndex((item) => item.id === task.id);
          if (index >= 0) next[index] = task;
          else next.push(task);
          return next;
        });

        const status: ToolCallEntry["status"] =
          task.status === "running"
            ? "running"
            : task.status === "failed"
              ? "error"
              : "done";
        const entry: ToolCallEntry = {
          task_id: task.id,
          agent_name: t("app.background.toolName"),
          description: task.label,
          status,
          summary: task.preview,
          background: true,
        };
        const exists = currentToolCallsRef.current.some(
          (item) => item.task_id === task.id
        );
        currentToolCallsRef.current = exists
          ? currentToolCallsRef.current.map((item) =>
              item.task_id === task.id ? { ...item, ...entry } : item
            )
          : [...currentToolCallsRef.current, entry];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<{ pending?: number; session_id?: string }>("background-waiting", (e) => {
        if (
          e.payload?.session_id &&
          e.payload.session_id !== sessionIdRef.current
        ) {
          return;
        }
        setBackgroundWaiting(e.payload?.pending ?? 0);
      }),
    );
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, [
    updateActiveAssistantToolCalls,
    flushAgentTokenBuffers,
    appendAgentBuffer,
    t,
  ]);

  // Flush buffered sub-agent output into its task row a few times per second
  // (see the `agent-task-token` / `agent-task-reasoning` listeners above). The
  // first chunk of a task bypasses this timer (leading edge).
  useEffect(() => {
    const interval = window.setInterval(flushAgentTokenBuffers, 400);
    return () => window.clearInterval(interval);
  }, [flushAgentTokenBuffers]);

  // The timer listener is registered once, so it reads the active session from
  // a ref instead of re-subscribing whenever the user switches chats.
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  // Background tool tasks are turn-scoped, but a session switch (or a reloaded
  // window) must never show another chat's tasks.
  useEffect(() => {
    setBackgroundTasks([]);
    setBackgroundWaiting(0);
    if (!sessionId) return;
    listBackgroundTasks(sessionId)
      .then(setBackgroundTasks)
      .catch((err) => console.error("Failed to load background tasks", err));
  }, [sessionId]);

  // Delay timers: initial list, live updates, and the resume hook.
  useEffect(() => {
    listTimers()
      .then(setTimers)
      .catch((err) => console.error("Failed to load timers", err));
    getSleepLockState()
      .then(setSleepLock)
      .catch((err) => console.error("Failed to load sleep-lock state", err));

    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      listen<TimerEntry[]>("timer-state", (e) => setTimers(e.payload ?? [])),
      // The sleep lock is held while a timer is pending or a chat turn is
      // streaming; the badge above the input mirrors it.
      listen<SleepLockState>("sleep-lock", (e) =>
        setSleepLock(e.payload ?? { active: false, holders: 0 }),
      ),
      listen<TimerEntry>("timer-fired", (e) => {
        const timer = e.payload;
        if (!timer) return;
        // A timer always resumes its own session. If the user moved on to
        // another chat, hand the prompt over instead of hijacking it.
        if (timer.session_id && timer.session_id !== sessionIdRef.current) {
          setTimerNotice(timer);
          return;
        }
        // Queue the resume prompt; the effect below sends it once the chat is
        // idle. Queuing (instead of sending directly) means a timer that fires
        // mid-reply still continues the task when the reply finishes.
        setPendingAutoPrompts((prev) => [...prev, timerPromptText(timer)]);
      }),
    );
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  // Remember when the running turn started, so the sleep-lock badge can count
  // it up. `streaming` flips false→true at the start of a turn (send / retry)
  // and back to false only when the turn ends, is stopped, or the chat is
  // switched — one transition is one turn, so a chain of mid-turn requests
  // keeps its original start.
  useEffect(() => {
    if (streaming) {
      setTurnStartedAt((prev) => prev ?? Date.now());
    } else {
      setTurnStartedAt(null);
    }
  }, [streaming]);

  // Tick the countdowns — only while something is actually pending (or a turn
  // is running, which is what the badge's elapsed time follows).
  useEffect(() => {
    if (timers.length === 0 && !streaming) return;
    const id = window.setInterval(() => setTimerTick(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [timers.length, streaming]);

  const handleCancelTimer = useCallback(async (id: string) => {
    try {
      await cancelTimer(id);
      setTimers((prev) => prev.filter((timer) => timer.id !== id));
    } catch (err) {
      setError(String(err));
    }
  }, []);

  /** Stop one background tool task (its process tree is terminated). */
  const handleCancelBackgroundTask = useCallback(
    async (taskId: string) => {
      try {
        const updated = await cancelBackgroundTask(taskId);
        setBackgroundTasks((prev) => {
          const next = [...prev];
          const index = next.findIndex((task) => task.id === updated.id);
          if (index >= 0) next[index] = updated;
          return next;
        });
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === updated.id
            ? { ...entry, status: "done" as const, summary: updated.preview }
            : entry
        );
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      } catch (err) {
        setError(String(err));
      }
    },
    [updateActiveAssistantToolCalls]
  );

  /**
   * Push the composed text into the turn that is currently running.
   *
   * It joins the model at the next request boundary (before the next tool
   * round or the final answer), which is the only place a turn can accept new
   * input — a stream already in flight cannot be appended to. When no turn
   * accepts it (the reply ended in the meantime), the text is queued as a
   * normal next-turn message instead of being dropped.
   */
  const appendInjection = useCallback(async () => {
    const text = input.trim();
    if (!text || !streaming || profileExporting) return;

    const item: ChatInjection = {
      id: crypto.randomUUID(),
      text,
      created_at_ms: Date.now(),
    };
    setInput("");
    pendingInjectionsRef.current = [...pendingInjectionsRef.current, item];
    setPendingInjections(pendingInjectionsRef.current);

    try {
      const { accepted } = await injectUserMessage(sessionId, item.id, text);
      if (accepted) return;
    } catch (err) {
      console.error("Failed to inject mid-turn message", err);
    }

    // The turn had already made its last request: send it as the next turn.
    pendingInjectionsRef.current = pendingInjectionsRef.current.filter(
      (pending) => pending.id !== item.id
    );
    setPendingInjections(pendingInjectionsRef.current);
    setPendingAutoPrompts((prev) => [...prev, text]);
  }, [input, streaming, sessionId, profileExporting]);

  /**
   * Turn-end flush: an injection still queued when the turn ended never
   * reached a request (the model never saw it), so queue its text as the next
   * user turn instead of letting it disappear with the window.
   */
  const flushUndeliveredInjections = useCallback(() => {
    const undelivered = pendingInjectionsRef.current;
    if (undelivered.length === 0) return;
    pendingInjectionsRef.current = [];
    setPendingInjections([]);
    setPendingAutoPrompts((prev) => [...prev, ...undelivered.map((item) => item.text)]);
  }, []);

  const sendMessage = useCallback(async (overrideText?: string) => {
    if (profileExporting) return;

    // `overrideText` is the timer resume path: it bypasses the input box
    // entirely, so it must not carry a `@file` mention either.
    const autoResume = overrideText !== undefined;
    const rawText = (overrideText ?? input).trim();
    if (!rawText || streaming) return;

    // Sending is an explicit "show me what happens next": re-arm the follow so
    // the new turn is pinned into view even if the user was reading further
    // up. A timer resume is not a user action and leaves the view alone.
    if (!autoResume) followBottomRef.current = true;

    // `@path#lines` mentions are materialised here: the token stays in the body
    // (the model then sees which file was quoted) while the content travels
    // through the normal attachment pipeline.
    //
    // Resolving a PDF mention is a real conversion, so its progress is shown
    // while this await runs — the turn only appears once everything is read.
    let mentionResult: MentionResolution = { attachments: [], problems: [] };
    if (!autoResume) {
      try {
        mentionResult = await resolveFileMentions(
          rawText,
          selectedMetadata,
          selectedModel,
          setMentionProgress,
        );
      } finally {
        setMentionProgress(null);
      }
    }
    const messageAttachments = mentionResult.attachments;

    const { content: apiContent } = buildMessageContent(rawText, messageAttachments);
    const userAttachments = messageAttachments.length > 0 ? messageAttachments : undefined;
    setPendingRetryMessageId(null);

    const userMsg: Message = {
      id: crypto.randomUUID(),
      role: "user",
      // Send the structured multimodal content (text + image_url + file parts)
      // so the LLM request includes the attachments. `ChatMessage.tsx` reads
      // the text portion via `contentToText` and renders attachment chips
      // from `message.attachments`, so no separate "display content" is
      // needed here.
      content: apiContent,
      ...(userAttachments ? { attachments: userAttachments } : {}),
    };
    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };
    currentAssistantMessageIdRef.current = assistantId;
    currentToolCallsRef.current = [];
    pendingAgentTokensRef.current.clear();
    currentMbusIdRef.current = null;
    currentInjectionsRef.current = [];
    hasRunningToolCallRef.current = false;
    setBackgroundTasks([]);
    setBackgroundWaiting(0);

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    // An auto-resume must not discard a draft the user is still composing.
    if (!autoResume) {
      setInput("");
    }
    setStreaming(true);
    setError(null);
    if (mentionResult.problems.length > 0) {
      // Unresolvable mentions never block the message: the text is still sent,
      // the notice says which references were not attached.
      setError(mentionResult.problems.join("\n"), "warning");
    }
    // A new request starts with no token figures yet — the usage bar above the
    // input must sweep until this request's `chat-usage` arrives.
    setUsageReady(false);

    const history = [...messages, userMsg]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-") && m.role !== "tool_group" && hasContextContent(m))
      .flatMap((m) => {
        const entry = {
          role: m.role,
          content: contextContent(m),
          reasoning_content: m.reasoning_content,
          loaded_skills: loadedSkillsOfMessage(m),
        };
        // A mid-turn injection was a real user message the model answered
        // after seeing. History replays it as its own user turn directly
        // before the assistant reply, because one stored assistant row
        // covers the whole multi-request turn.
        const injections = m.role === "assistant" ? m.injections ?? [] : [];
        return [
          ...injections.map((injection) => ({
            role: "user" as const,
            content: injection.text,
            reasoning_content: undefined,
            loaded_skills: undefined,
          })),
          entry,
        ];
      });

    saveHistory(
      sessionId,
      "user",
      serializeContentForDb(apiContent),
      undefined,
      undefined,
      userAttachments,
    ).then((dbId) => {
      setMessages((prev) =>
        prev.map((m) => (m.id === userMsg.id ? { ...m, dbId } : m))
      );
    }).catch(console.error);

    let accumulatedContent = "";
    let accumulatedReasoning = "";

    const cleanup = await chatCompletion(history, activeSkillIds, sessionId, selectedModel, {
      onToken(token) {
        accumulatedContent += token;
        const shouldMarkDone = hasRunningToolCallRef.current;
        if (shouldMarkDone) hasRunningToolCallRef.current = false;
        setBackgroundWaiting(0);
        setMessages((prev) =>
          prev.map((m) => {
            if (m.id !== assistantId) return m;

            const nextToolCalls = shouldMarkDone
              ? (m.tool_calls ?? []).map((e) =>
                // The answer starting means the main agent's own step is
                // done. A sub-agent row / planner row keeps its own status
                // until its terminal event arrives.
                e.status === "running" &&
                e.kind !== "agent" &&
                e.kind !== "planner"
                  ? { ...e, status: "done" as const }
                  : e
              )
              : m.tool_calls;

            return {
              ...m,
              content: accumulatedContent,
              ...(nextToolCalls ? { tool_calls: nextToolCalls } : {}),
            };
          })
        );
      },
      onReasoningToken(token) {
        accumulatedReasoning += token;
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, reasoning_content: accumulatedReasoning } : m
          )
        );
      },
      onDone() {
        setBackgroundWaiting(0);
        // Settle the still-running steps before persisting: the transcript
        // must never carry a "running" entry into the history (it would spin
        // forever once the chat is reloaded). A sub-agent row that was still
        // running when the turn ended was torn down by the backend, so it is
        // neutral ("skipped") — never a success checkmark.
        const finalToolCalls = currentToolCallsRef.current.map((e) => {
          if (e.status !== "running") return e;
          if (e.kind === "agent") {
            return { ...e, status: "skipped" as const, thinking: undefined };
          }
          return { ...e, status: "done" as const };
        });
        // Deliveries are stored on the assistant row: one turn is one row, and
        // the reply was generated after the model saw them.
        const deliveredInjections = currentInjectionsRef.current;
        saveHistory(
          sessionId,
          "assistant",
          accumulatedContent,
          finalToolCalls.length > 0 ? JSON.stringify(finalToolCalls) : undefined,
          accumulatedReasoning || undefined,
          undefined,
          deliveredInjections.length > 0 ? JSON.stringify(deliveredInjections) : undefined,
        )
          .then((dbId) => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, dbId } : m))
            );
          })
          .catch(console.error);
        setMessages((prev) => {
          return prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => {
              if (m.id === assistantId) return { ...m, streaming: false, ...(finalToolCalls?.length ? { tool_calls: finalToolCalls } : {}) };
              return m;
            });
        });
        setAgentStatuses({});
        hasRunningToolCallRef.current = false;
        // Whatever never reached a request is sent as the next user turn.
        flushUndeliveredInjections();
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        pendingAgentTokensRef.current.clear();
        setStreaming(false);
        cleanupRef.current = null;
      },
      onError(err) {
        // A step that was still running when the turn failed never completed:
        // settle it so the group does not keep a spinner spinning forever.
        const settledToolCalls = currentToolCallsRef.current.map((e) => {
          if (e.status !== "running") return e;
          // A task killed by a failing turn is not itself a failed task.
          if (e.kind === "agent") {
            return {
              ...e,
              status: "skipped" as const,
              confirm: undefined,
              thinking: undefined,
            };
          }
          return { ...e, status: "error" as const, confirm: undefined };
        });
        // The transcript holds no failure row any more, so a turn that can
        // still be pushed forward offers "continue" from the notice.
        setError(
          err,
          "error",
          resumeAction(
            t("chat.continue"),
            accumulatedContent,
            accumulatedReasoning,
            settledToolCalls.length,
            () => continueUnfinishedRef.current(),
          ),
        );
        setMessages((prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) =>
              m.id === assistantId
                ? {
                    ...m,
                    error: err,
                    status: "error" as const,
                    streaming: false,
                    ...(settledToolCalls.length ? { tool_calls: settledToolCalls } : {}),
                  }
                : m
            )
        );
        // Persist the partial turn (if any) so refreshing or reopening the
        // session does not discard what was already generated. The error text
        // is stored separately and is never replayed as model content.
        const deliveredInjections = currentInjectionsRef.current;
        if (accumulatedContent.trim() || accumulatedReasoning.trim() || settledToolCalls.length > 0) {
          saveHistory(
            sessionId,
            "assistant",
            accumulatedContent,
            settledToolCalls.length > 0 ? JSON.stringify(settledToolCalls) : undefined,
            accumulatedReasoning || undefined,
            undefined,
            deliveredInjections.length > 0 ? JSON.stringify(deliveredInjections) : undefined,
            err,
            "error",
          )
            .then((dbId) => {
              setMessages((prev) =>
                prev.map((m) => (m.id === assistantId ? { ...m, dbId } : m))
              );
            })
            .catch(console.error);
        }
        setPendingRetryMessageId(userMsg.id);
        setAgentStatuses({});
        hasRunningToolCallRef.current = false;
        // Whatever never reached a request is sent as the next user turn.
        flushUndeliveredInjections();
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        pendingAgentTokensRef.current.clear();
        setStreaming(false);
        cleanupRef.current = null;
      },
    }, useAgentsEnabled);

    cleanupRef.current = cleanup;
  }, [input, messages, streaming, profileExporting, activeSkillIds, sessionId, selectedModel, selectedMetadata, useAgentsEnabled, t, flushUndeliveredInjections]);

  /**
   * Resume a partial reply. The normal send path keeps interrupted assistant
   * messages that still have visible output, so the model sees what it had
   * already reasoned/generated and continues instead of starting over.
   */
  const continueUnfinishedReply = useCallback(() => {
    if (streaming) return;
    void sendMessage(t("app.continueUnfinished"));
  }, [streaming, sendMessage, t]);

  // Handed to the failure notice, which is built before this callback exists.
  useEffect(() => {
    continueUnfinishedRef.current = continueUnfinishedReply;
  }, [continueUnfinishedReply]);

  // Continue the conversation from a fired timer — or deliver an injection
  // that arrived after the turn's last request — once the chat is idle. The
  // prompt goes out as a new user turn, so the assistant continues with the
  // full history. Sending one at a time keeps the turn order intact when
  // several prompts queue together.
  useEffect(() => {
    if (streaming || profileExporting || pendingAutoPrompts.length === 0) return;
    const [next, ...rest] = pendingAutoPrompts;
    setPendingAutoPrompts(rest);
    void sendMessage(next);
  }, [pendingAutoPrompts, streaming, profileExporting, sendMessage]);

  const retryPendingUserMessage = useCallback(async () => {
    if (streaming || !pendingRetryMessageId) return;

    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };
    currentAssistantMessageIdRef.current = assistantId;
    currentToolCallsRef.current = [];
    pendingAgentTokensRef.current.clear();
    currentMbusIdRef.current = null;
    currentInjectionsRef.current = [];
    hasRunningToolCallRef.current = false;
    setBackgroundTasks([]);
    setBackgroundWaiting(0);

    followBottomRef.current = true;
    setMessages((prev) => [...prev, assistantMsg]);
    setStreaming(true);
    setError(null);
    // A new request starts with no token figures yet — the usage bar above the
    // input must sweep until this request's `chat-usage` arrives.
    setUsageReady(false);

    // A retry regenerates the last answer: drop the trailing interrupted
    // assistant turn(s), plus transient progress rows, so the request ends at
    // the user message. Partial turns from earlier in the session stay.
    const retryBase = [...messages];
    while (retryBase.length > 0) {
      const last = retryBase[retryBase.length - 1];
      if (last.id.startsWith("agent-progress-") || last.role === "tool_group") {
        retryBase.pop();
        continue;
      }
      if (isInterruptedAssistant(last)) {
        retryBase.pop();
        continue;
      }
      break;
    }

    const history = retryBase
      .filter((m) => !m.streaming && m.role !== "tool_group" && hasContextContent(m))
      .flatMap((m) => {
        const entry = {
          role: m.role,
          content: contextContent(m),
          reasoning_content: m.reasoning_content,
          loaded_skills: loadedSkillsOfMessage(m),
        };
        const injections = m.role === "assistant" ? m.injections ?? [] : [];
        return [
          ...injections.map((injection) => ({
            role: "user" as const,
            content: injection.text,
            reasoning_content: undefined,
            loaded_skills: undefined,
          })),
          entry,
        ];
      });

    let accumulatedContent = "";
    let accumulatedReasoning = "";

    const cleanup = await chatCompletion(history, activeSkillIds, sessionId, selectedModel, {
      onToken(token) {
        accumulatedContent += token;
        const shouldMarkDone = hasRunningToolCallRef.current;
        if (shouldMarkDone) hasRunningToolCallRef.current = false;
        setBackgroundWaiting(0);
        setMessages((prev) =>
          prev.map((m) => {
            if (m.id !== assistantId) return m;

            const nextToolCalls = shouldMarkDone
              ? (m.tool_calls ?? []).map((e) =>
                // The answer starting means the main agent's own step is
                // done. A sub-agent row / planner row keeps its own status
                // until its terminal event arrives.
                e.status === "running" &&
                e.kind !== "agent" &&
                e.kind !== "planner"
                  ? { ...e, status: "done" as const }
                  : e
              )
              : m.tool_calls;

            return {
              ...m,
              content: accumulatedContent,
              ...(nextToolCalls ? { tool_calls: nextToolCalls } : {}),
            };
          })
        );
      },
      onReasoningToken(token) {
        accumulatedReasoning += token;
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, reasoning_content: accumulatedReasoning } : m
          )
        );
      },
      onDone() {
        setBackgroundWaiting(0);
        const finalToolCalls = currentToolCallsRef.current.map((e) => {
          if (e.status !== "running") return e;
          if (e.kind === "agent") {
            return { ...e, status: "skipped" as const, thinking: undefined };
          }
          return { ...e, status: "done" as const };
        });
        // Deliveries are stored on the assistant row: one turn is one row, and
        // the reply was generated after the model saw them.
        const deliveredInjections = currentInjectionsRef.current;
        saveHistory(
          sessionId,
          "assistant",
          accumulatedContent,
          finalToolCalls.length > 0 ? JSON.stringify(finalToolCalls) : undefined,
          accumulatedReasoning || undefined,
          undefined,
          deliveredInjections.length > 0 ? JSON.stringify(deliveredInjections) : undefined,
        )
          .then((dbId) => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, dbId } : m))
            );
          })
          .catch(console.error);
        setMessages((prev) => {
          return prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => {
              if (m.id === assistantId) return { ...m, streaming: false, ...(finalToolCalls?.length ? { tool_calls: finalToolCalls } : {}) };
              return m;
            });
        });
        setPendingRetryMessageId(null);
        setAgentStatuses({});
        // Whatever never reached a request is sent as the next user turn.
        flushUndeliveredInjections();
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        pendingAgentTokensRef.current.clear();
        setStreaming(false);
        cleanupRef.current = null;
      },
      onError(err) {
        const settledToolCalls = currentToolCallsRef.current.map((e) => {
          if (e.status !== "running") return e;
          // A task killed by a failing turn is not itself a failed task.
          if (e.kind === "agent") {
            return {
              ...e,
              status: "skipped" as const,
              confirm: undefined,
              thinking: undefined,
            };
          }
          return { ...e, status: "error" as const, confirm: undefined };
        });
        // Same as the first-turn handler: the failure is reported through the
        // notice, and the notice carries the resume action when there is
        // anything left to resume.
        setError(
          err,
          "error",
          resumeAction(
            t("chat.continue"),
            accumulatedContent,
            accumulatedReasoning,
            settledToolCalls.length,
            () => continueUnfinishedRef.current(),
          ),
        );
        setMessages((prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) =>
              m.id === assistantId
                ? {
                    ...m,
                    error: err,
                    status: "error" as const,
                    streaming: false,
                    ...(settledToolCalls.length ? { tool_calls: settledToolCalls } : {}),
                  }
                : m
            )
        );
        // Same as the first-turn handler: keep the partial reply on disk so a
        // reload or continuation can use it; the error itself stays UI-only.
        const deliveredInjections = currentInjectionsRef.current;
        if (accumulatedContent.trim() || accumulatedReasoning.trim() || settledToolCalls.length > 0) {
          saveHistory(
            sessionId,
            "assistant",
            accumulatedContent,
            settledToolCalls.length > 0 ? JSON.stringify(settledToolCalls) : undefined,
            accumulatedReasoning || undefined,
            undefined,
            deliveredInjections.length > 0 ? JSON.stringify(deliveredInjections) : undefined,
            err,
            "error",
          )
            .then((dbId) => {
              setMessages((prev) =>
                prev.map((m) => (m.id === assistantId ? { ...m, dbId } : m))
              );
            })
            .catch(console.error);
        }
        setAgentStatuses({});
        hasRunningToolCallRef.current = false;
        // Whatever never reached a request is sent as the next user turn.
        flushUndeliveredInjections();
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        pendingAgentTokensRef.current.clear();
        setStreaming(false);
        cleanupRef.current = null;
      },
    }, useAgentsEnabled);

    cleanupRef.current = cleanup;
  }, [messages, streaming, pendingRetryMessageId, activeSkillIds, sessionId, selectedModel, useAgentsEnabled, t, flushUndeliveredInjections]);

  const handleDeleteMessage = useCallback(async (messageId: string) => {
    const msg = messages.find((m) => m.id === messageId);
    if (!msg || msg.dbId === undefined) return;

    try {
      await deleteMessage(msg.dbId);
      setMessages((prev) => prev.filter((m) => m.id !== messageId));
    } catch (err) {
      setError(t("app.deleteFailed", { error: String(err) }));
    }
  }, [messages, t]);

  const handleForkMessage = useCallback(async (messageId: string) => {
    const msg = messages.find((m) => m.id === messageId);
    if (!msg || msg.dbId === undefined) return;

    const newSessionId = crypto.randomUUID();
    try {
      await forkSession(sessionId, newSessionId, msg.dbId);
      // Reload history panel will pick up the new session on next open
      // Switch to the new session
      const forkedMessages = messages
        .filter((m) => m.dbId !== undefined && m.dbId <= msg.dbId!)
        .map((m) => ({ ...m, id: crypto.randomUUID() }));
      followBottomRef.current = true;
      setMessages(forkedMessages);
      setSessionId(newSessionId);
      setPendingRetryMessageId(null);
      setError(null);
    } catch (err) {
      setError(t("app.forkFailed", { error: String(err) }));
    }
  }, [messages, sessionId, t]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (profileExporting) {
      e.preventDefault();
      return;
    }

    // An open mention popup (`@file` or `/skill`) owns the arrow keys,
    // Enter/Tab and Escape while it is visible. Their token grammars are
    // mutually exclusive, so at most one is open and it decides which list,
    // index and accept handler the keys act on.
    if (mentionState.open || slashState.open) {
      const rowCount = mentionState.open ? mentionItems.length : slashItems.length;
      if (e.key === "Escape") {
        e.preventDefault();
        closeMention();
        closeSlash();
        return;
      }
      if (rowCount > 0) {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          const delta = e.key === "ArrowDown" ? 1 : -1;
          if (mentionState.open) {
            setMentionIndex((prev) => (prev + delta + rowCount) % rowCount);
          } else {
            setSlashIndex((prev) => (prev + delta + rowCount) % rowCount);
          }
          return;
        }
        if (e.key === "Enter" || e.key === "Tab") {
          e.preventDefault();
          if (mentionState.open) {
            const item = mentionItems[mentionActiveIndex];
            if (item) acceptMention(item);
          } else {
            const item = slashItems[slashActiveIndex];
            if (item) acceptSlash(item);
          }
          return;
        }
      }
    }

    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      // While a turn is running the composer is an injection channel, not a
      // new-message channel: the text joins the running turn at its next
      // request boundary instead of starting a second turn.
      if (streaming) {
        void appendInjection();
      } else {
        sendMessage();
      }
    }
  }

  /**
   * Store the thinking depth of the selected model and persist it right away:
   * it is a per-model preference that must apply to the very next request, so
   * it must not wait for "Save Changes" in Settings.
   *
   * The config is re-read before writing because the skills / tools panels own
   * other parts of it and must not be rolled back by this change.
   */
  const handleReasoningEffortChange = useCallback(
    async (effort: string) => {
      setReasoningEfforts((prev) => {
        const next = { ...prev };
        if (effort) next[selectedModel] = effort;
        else delete next[selectedModel];
        return next;
      });

      try {
        const cfg = await getConfig();
        const next = { ...(cfg.model_reasoning_effort ?? {}) };
        if (effort) next[selectedModel] = effort;
        else delete next[selectedModel];
        await saveConfig({ ...cfg, model_reasoning_effort: next });
      } catch (err) {
        console.error("Failed to persist the reasoning effort", err);
      }
    },
    [selectedModel],
  );

  function toggleSidebar(panel: Sidebar) {
    // Clicking the open panel's button closes it; clicking it while the close
    // fade is still running reopens instead of waiting for it to finish.
    if (sidebar === panel && sidebarMotion !== "closing") {
      closeSidebar();
      return;
    }

    // Reopening from closed (or mid-close) fades in. Switching to another
    // panel while the sidebar is already visible swaps the shell in place:
    // replaying the fade on every tab click would flash the whole surface.
    setSidebar(panel);
    setSidebarMotion(
      sidebar === null || sidebarMotion === "closing" ? "opening" : sidebarMotion,
    );
  }

  function closeSidebar() {
    if (!sidebar || sidebarMotion === "closing") return;
    setSidebarMotion("closing");
  }

  /**
   * The fade owns the mount lifetime: `sidebar` stays in state until the
   * closing animation reports completion, so no timer has to guess the CSS
   * duration (the old 180 ms timer cut a 220 ms slide short and made it snap).
   * Child animations bubble through the aside, hence the target guard.
   */
  function handleSidebarAnimationEnd(event: React.AnimationEvent<HTMLElement>) {
    if (event.target !== event.currentTarget) return;
    if (sidebarMotion === "closing") setSidebar(null);
    setSidebarMotion(null);
  }

  function handleChatAreaClick(event: React.MouseEvent<HTMLDivElement>) {
    if (!sidebar) return;
    if ((event.target as HTMLElement).closest('.toolbar')) return;
    closeSidebar();
  }

  async function clearChat() {
    if (streaming) {
      await stopStreaming();
    }
    setMessages([]);
    setError(null);
    setUsage(null);
    setUsageReady(false);
    setPendingRetryMessageId(null);
    setSessionId(crypto.randomUUID());
  }

  const renderConfirmDialog = () => {
    if (!confirmDialog) {
      return null;
    }
    const requiresSudo = confirmDialog.requires_auth === "sudo";
    const requiresElevation = confirmDialog.requires_auth === "elevation";
    const isExternalAgent = confirmDialog.confirm_kind === "external_agent";
    // Prefer structured risk data from the risk-assessment event, falling back
    // to fields carried on the confirm-required payload itself.
    const riskLevel = riskAssessment?.risk_level ?? confirmDialog.risk_level;
    const riskScore = riskAssessment?.risk_score ?? confirmDialog.risk_score;
    const disposition = riskAssessment?.disposition ?? confirmDialog.disposition;
    const blacklistHits = riskAssessment?.blacklist_hits ?? confirmDialog.blacklist_hits;
    const penaltyItems = riskAssessment?.penalty_items ?? confirmDialog.penalty_items;
    const hasRisk = riskLevel !== undefined && riskScore !== undefined;

    const title = requiresSudo
      ? t("app.confirm.titleSudo")
      : requiresElevation
        ? t("app.confirm.titleElevation")
        : isExternalAgent
          ? t("app.confirm.titleExternalAgent")
          : hasRisk
            ? t("app.confirm.titleRisk", { level: riskLevel ?? "" })
            : t("app.confirm.titleDangerous");
    const badge = requiresSudo
      ? t("app.confirm.badgeSudo")
      : requiresElevation
        ? t("app.confirm.badgeAdmin")
        : isExternalAgent
          ? t("app.confirm.badgeExternalAgent")
          : hasRisk
            ? riskLevel ?? t("app.confirm.badgeRisk")
            : t("app.confirm.badgeDangerous");
    const preview =
      confirmDialog.code.length > 400
        ? confirmDialog.code.slice(0, 400) + "…"
        : confirmDialog.code;

    return (
      <div className="confirm-panel" role="alertdialog" aria-label={title}>
        <div className="confirm-panel-body">
          <div className="confirm-panel-header">
            <FontAwesomeIcon icon={faTriangleExclamation} className="confirm-panel-icon" />
            <span className="confirm-panel-title">{title}</span>
            <span className="confirm-panel-badge">{badge}</span>
          </div>
          <div className="confirm-panel-meta">
            <span>
              <strong>{t("app.confirm.reason")}</strong> {confirmDialog.reason}
            </span>
            <span>
              <strong>{t("app.confirm.type")}</strong> {confirmDialog.cmd_type}
            </span>
          </div>
          <div className="confirm-panel-preview">
            <pre>{preview}</pre>
          </div>
          {requiresSudo && (
            <div className="confirm-panel-credentials">
              <label>
                {t("app.confirm.username")}
                <input
                  value={confirmUsername}
                  onChange={(e) => setConfirmUsername(e.target.value)}
                  placeholder={t("app.confirm.usernamePlaceholder")}
                />
              </label>
              <label>
                {t("app.confirm.password")}
                <input
                  type="password"
                  value={confirmPassword}
                  onChange={(e) => setConfirmPassword(e.target.value)}
                  placeholder={t("app.confirm.passwordPlaceholder")}
                  autoFocus
                />
              </label>
              <p className="confirm-panel-hint">
                {t("app.confirm.sudoHint")}
              </p>
            </div>
          )}
          {requiresElevation && (
            <p className="confirm-panel-hint">
              {t("app.confirm.elevationHint")}
            </p>
          )}
          {isExternalAgent && (
            <p className="confirm-panel-hint">
              {t("app.confirm.externalAgentHint")}
            </p>
          )}
          {hasRisk && (
            <div className="confirm-panel-risk">
              <div className="confirm-risk-header">
                <span className={`confirm-risk-level confirm-risk-level-${String(riskLevel).toLowerCase()}`}>
                  {riskLevel}
                </span>
                <span className="confirm-risk-score">{t("app.confirm.score", { score: riskScore ?? 0 })}</span>
                <span className="confirm-risk-disposition">{disposition}</span>
              </div>
              {blacklistHits && blacklistHits.length > 0 && (
                <div className="confirm-risk-section">
                  <strong>{t("app.confirm.blacklistHits")}:</strong>
                  <ul>
                    {blacklistHits.map((hit, i) => (
                      <li key={i}>
                        [{hit.rule_id}] ({hit.severity}) {hit.matched} — +{hit.contribution}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {penaltyItems && penaltyItems.length > 0 && (
                <div className="confirm-risk-section">
                  <strong>{t("app.confirm.penaltyItems")}:</strong>
                  <ul>
                    {penaltyItems.map((item, i) => (
                      <li key={i}>{item.name} — +{item.points}</li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          )}
        </div>
        <div className="confirm-panel-actions">
          <button
            type="button"
            className="btn-secondary"
            onClick={() => respondToConfirm(false)}
          >
            {t("app.confirm.deny")}
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => respondToConfirm(true)}
            disabled={requiresSudo && confirmPassword.trim().length === 0}
          >
            {t("app.confirm.allow")}
          </button>
        </div>
      </div>
    );
  };

  async function stopStreaming() {
    try {
      await stopChatCompletion();
    } catch (err) {
      setError(String(err));
    }

    if (cleanupRef.current) {
      cleanupRef.current();
      cleanupRef.current = null;
    }

    // A step that was still running when the user hit Stop never completed:
    // settle it so the group does not keep a spinner spinning forever.
    const assistantId = currentAssistantMessageIdRef.current;
    const activeMessage = assistantId
      ? messages.find((m) => m.id === assistantId)
      : undefined;
    const stopSuffix = "\n\n" + t("app.generationStopped");
    // A stopped turn also drops any pending inline confirmation: clear the
    // `confirm` payload so no stale allow/deny row survives the settle (the
    // 120 s backend timeout already answers it as "not confirmed").
    const settledToolCalls = currentToolCallsRef.current.map((e) => {
      if (e.status !== "running") return e;
      // A sub-agent torn down by Stop was cancelled, not completed and not a
      // failure of its own: settle it neutrally so the row cannot keep spinning
      // or show a success checkmark.
      if (e.kind === "agent") {
        return {
          ...e,
          status: "skipped" as const,
          confirm: undefined,
          thinking: undefined,
        };
      }
      return { ...e, status: "error" as const, confirm: undefined };
    });
    // The turn is over: sub-agents were torn down by the backend and their
    // `agent-task-done` events arrive after the listeners are gone, so settle
    // the per-agent spinners and drop the in-turn progress rows here.
    setAgentStatuses({});
    hasRunningToolCallRef.current = false;
    currentAssistantMessageIdRef.current = null;
    currentToolCallsRef.current = [];
    pendingAgentTokensRef.current.clear();
    setMessages((prev) =>
      prev
        .filter((m) => !m.id.startsWith("agent-progress-"))
        .map((m) => {
          if (!m.streaming) return m;
          // `content` may be a string OR a multimodal array — only mutate
          // string content (assistant streamed text). Leave other shapes
          // untouched.
          if (typeof m.content !== "string") {
            return {
              ...m,
              streaming: false,
              ...(m.id === assistantId ? { status: "stopped" as const } : {}),
            };
          }
          const content = isGenerationStopped(m.content)
            ? m.content
            : (m.content || "") + stopSuffix;
          return {
            ...m,
            content,
            streaming: false,
            ...(m.id === assistantId
              ? {
                  status: "stopped" as const,
                  ...(settledToolCalls.length ? { tool_calls: settledToolCalls } : {}),
                }
              : {}),
          };
        })
    );
    // Keep the partial stopped reply on disk; the stop marker is UI-only and
    // gets stripped from the model context when the turn is replayed.
    if (assistantId && activeMessage) {
      const rawContent = typeof activeMessage.content === "string" ? activeMessage.content : "";
      const savedContent = rawContent
        ? (isGenerationStopped(rawContent) ? rawContent : rawContent + stopSuffix)
        : "";
      const deliveredInjections = currentInjectionsRef.current;
      if (savedContent.trim() || activeMessage.reasoning_content?.trim() || settledToolCalls.length > 0) {
        saveHistory(
          sessionId,
          "assistant",
          savedContent,
          settledToolCalls.length > 0 ? JSON.stringify(settledToolCalls) : undefined,
          activeMessage.reasoning_content || undefined,
          undefined,
          deliveredInjections.length > 0 ? JSON.stringify(deliveredInjections) : undefined,
          undefined,
          "stopped",
        )
          .then((dbId) => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, dbId } : m))
            );
          })
          .catch(console.error);
      }
    }
    setStreaming(false);
  }

  async function handleSaveMarkdownEditor() {
    if (!markdownPath.trim()) {
      setError(t("app.markdownPathEmpty"));
      return;
    }

    setMarkdownSaving(true);
    try {
      await saveMarkdownFile(markdownPath, markdownDraft);
      setMarkdownEditorOpen(false);
    } catch (err) {
      setError(t("app.markdownSaveFailed", { error: String(err) }));
    } finally {
      setMarkdownSaving(false);
    }
  }

  /** The thinking-depth control only makes sense for reasoning-capable models. */
  const reasoningEffort = reasoningEfforts[selectedModel] ?? "";
  // Shown for catalogue-matched reasoning models, and kept for any model that
  // already has a depth configured (e.g. an unmatched model set by hand).
  const reasoningSupported = selectedMetadata?.supports_reasoning === true || reasoningEffort !== "";

  // ─── Composer `@file` mentions ────────────────────────────────────────────

  /**
   * Files the `@file` popup offers: only the attachment kinds the selected
   * model can consume (`filter_by_externname` — text is always fine, images
   * need vision, documents need `supports_files`), ranked by file name first
   * and path second.
   */
  const mentionItems = useMemo(() => {
    if (!mentionState.open || !workspaceFiles) return [];
    const scored: { item: WorkspaceFileEntry; score: number }[] = [];
    for (const file of workspaceFiles) {
      // `@paper.pdf#2` reaches the model as Markdown text, so PDFs are offered
      // to every model — even one without `supports_files`.
      if (!supportsMentionName(selectedMetadata, file.name)) continue;
      const score = scoreWorkspaceFile(file, mentionState.query);
      if (score === null) continue;
      scored.push({ item: file, score });
    }
    scored.sort(
      (a, b) =>
        a.score - b.score ||
        a.item.path.length - b.item.path.length ||
        a.item.path.localeCompare(b.item.path),
    );
    return scored.slice(0, MENTION_MAX_ITEMS).map((entry) => entry.item);
  }, [mentionState.open, mentionState.query, workspaceFiles, selectedMetadata]);

  /** Index stays valid while the filtered list shrinks under the cursor. */
  const mentionActiveIndex = Math.min(mentionIndex, Math.max(0, mentionItems.length - 1));

  const closeMention = useCallback(() => {
    setMentionState((prev) => (prev.open ? { open: false, query: "", start: 0 } : prev));
  }, []);

  /**
   * Load the workspace file list for the popup (cached for
   * `WORKSPACE_FILES_TTL_MS`, so switching workspaces is picked up without an
   * explicit invalidation hook).
   */
  const ensureWorkspaceFiles = useCallback(async () => {
    if (workspaceFilesPendingRef.current) return;
    const cached = workspaceFilesRef.current;
    if (cached && Date.now() - cached.at < WORKSPACE_FILES_TTL_MS) return;

    workspaceFilesPendingRef.current = true;
    setWorkspaceFilesLoading(true);
    try {
      const files = await listWorkspaceFiles();
      workspaceFilesRef.current = { at: Date.now(), files };
      setWorkspaceFiles(files);
    } catch (err) {
      setError(t("app.fileMention.listFailed", { error: String(err) }));
    } finally {
      workspaceFilesPendingRef.current = false;
      setWorkspaceFilesLoading(false);
    }
  }, [t]);

  /**
   * Open/close the popup for the token under the caret. Only a real user edit
   * reaches this — accepting an item inserts text programmatically, which must
   * not immediately reopen the popup.
   */
  const syncMentionFromInput = useCallback(
    (value: string, caret: number) => {
      // While a turn is running the composer is an injection channel: injected
      // text carries no attachments, so a mention would only look like one.
      if (streaming) {
        closeMention();
        return;
      }
      const active = activeMentionQuery(value, caret);
      if (!active) {
        closeMention();
        return;
      }
      setMentionState((prev) =>
        prev.open && prev.start === active.start && prev.query === active.query
          ? prev
          : { open: true, query: active.query, start: active.start },
      );
      setMentionIndex(0);
      void ensureWorkspaceFiles();
    },
    [closeMention, ensureWorkspaceFiles, streaming],
  );

  /**
   * Insert `@<path>` for `item` and leave the caret right behind it, so a
   * `#12-24` range can be typed straight away.
   */
  const acceptMention = useCallback(
    (item: WorkspaceFileEntry) => {
      const el = textareaRef.current;
      const caret = el?.selectionStart ?? input.length;
      const token = `@${item.path}`;
      const before = input.slice(0, mentionState.start);
      const after = input.slice(Math.max(caret, mentionState.start));
      setInput(`${before}${token}${after}`);
      closeMention();
      requestAnimationFrame(() => {
        const position = before.length + token.length;
        el?.focus();
        el?.setSelectionRange(position, position);
      });
    },
    [closeMention, input, mentionState.start],
  );

  // ─── Composer `/skill` mentions ───────────────────────────────────────────

  /**
   * Skills the `/skill` popup offers, ranked by relevance.
   *
   * Nothing is filtered out: `use_skill` resolves any skill in the workspace,
   * so mentioning a disabled one is still meaningful — enabling only preloads
   * its description into the system prompt. Within one relevance tier the
   * enabled skills come first, because those are the ones the model already
   * knows about.
   */
  const slashItems = useMemo(() => {
    if (!slashState.open || !slashSkills) return [];
    const scored: { item: SkillMentionOption; score: number; rank: number }[] = [];
    for (const option of slashSkills) {
      const score = scoreSkillOption(option, slashState.query);
      if (score === null) continue;
      const enabled = skillStatus?.skills[option.name]?.enabled === true;
      scored.push({ item: option, score, rank: enabled ? 0 : 1 });
    }
    scored.sort(
      (a, b) => a.score - b.score || a.rank - b.rank || a.item.name.localeCompare(b.item.name),
    );
    return scored.slice(0, MENTION_MAX_ITEMS).map((entry) => entry.item);
  }, [slashState.open, slashState.query, slashSkills, skillStatus]);

  /** Index stays valid while the filtered list shrinks under the cursor. */
  const slashActiveIndex = Math.min(slashIndex, Math.max(0, slashItems.length - 1));

  const closeSlash = useCallback(() => {
    setSlashState((prev) => (prev.open ? { open: false, query: "", start: 0 } : prev));
  }, []);

  /**
   * Load the workspace skill list for the popup (cached for
   * `SKILL_MENTIONS_TTL_MS`, so switching workspaces is picked up without an
   * explicit invalidation hook — same contract as `ensureWorkspaceFiles`).
   */
  const ensureSlashSkills = useCallback(async () => {
    if (slashSkillsPendingRef.current) return;
    const cached = slashSkillsRef.current;
    if (cached && Date.now() - cached.at < SKILL_MENTIONS_TTL_MS) return;

    slashSkillsPendingRef.current = true;
    setSlashSkillsLoading(true);
    try {
      const options = toSkillOptions(await listSkills());
      slashSkillsRef.current = { at: Date.now(), options };
      setSlashSkills(options);
    } catch (err) {
      setError(t("app.slashMention.listFailed", { error: String(err) }));
    } finally {
      slashSkillsPendingRef.current = false;
      setSlashSkillsLoading(false);
    }
  }, [t]);

  /**
   * Open/close the `/skill` popup for the token under the caret. Only a real
   * user edit reaches this — accepting a skill inserts text programmatically,
   * which must not immediately reopen the popup.
   */
  const syncSlashFromInput = useCallback(
    (value: string, caret: number) => {
      // While a turn is running the composer is an injection channel; injected
      // text cannot carry a reference, so the popup would be misleading.
      if (streaming) {
        closeSlash();
        return;
      }
      const active = activeSlashQuery(value, caret);
      if (!active) {
        closeSlash();
        return;
      }
      setSlashState((prev) =>
        prev.open && prev.start === active.start && prev.query === active.query
          ? prev
          : { open: true, query: active.query, start: active.start },
      );
      setSlashIndex(0);
      void ensureSlashSkills();
    },
    [closeSlash, ensureSlashSkills, streaming],
  );

  /**
   * Insert `/<skill>` for `item`, replacing the `/` + query that opened the
   * popup, and leave the caret right behind the token.
   */
  const acceptSlash = useCallback(
    (item: SkillMentionOption) => {
      const el = textareaRef.current;
      const caret = el?.selectionStart ?? input.length;
      const token = `/${item.name}`;
      const before = input.slice(0, slashState.start);
      const after = input.slice(Math.max(caret, slashState.start));
      setInput(`${before}${token}${after}`);
      closeSlash();
      requestAnimationFrame(() => {
        const position = before.length + token.length;
        el?.focus();
        el?.setSelectionRange(position, position);
      });
    },
    [closeSlash, input, slashState.start],
  );

  /**
   * Skill names known in this workspace. A sent `/name` token is only
   * highlighted when it names a real skill, so ordinary text such as
   * `/usr/bin` stays plain.
   */
  const knownSkillNames = useMemo(() => {
    const names = new Set<string>();
    for (const option of slashSkills ?? []) names.add(option.name);
    for (const name of Object.keys(skillStatus?.skills ?? {})) names.add(name);
    return Array.from(names).sort();
  }, [slashSkills, skillStatus]);

  // The todo panel belongs to the newest assistant message only (like the
  // background-task group). A turn can end on a tool_group message, so scan
  // backwards for the last assistant message instead of taking the tail.
  const todoMessageId = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      if (messages[i].role === "assistant") return messages[i].id;
    }
    return null;
  }, [messages]);

  const usageTotal = usage ? (usage.total_tokens ?? usage.prompt_tokens + usage.completion_tokens) : 0;
  const fallbackMaxTokens = 131072; // 128k tokens as a hard upper bound for usage ratio calculations when no explicit max is provided
  const usageMax = usage?.max_tokens ?? maxTokens ?? fallbackMaxTokens;
  const usageRatio = usage
    ? (usage.usage_ratio ?? (usageMax > 0 ? usageTotal / usageMax : 0))
    : 0;
  const usagePercent = Math.max(0, Math.min(100, Math.round(usageRatio * 100)));
  // Provider-reported prompt-cache hit of the current request. Absent when the
  // endpoint does not report caching at all (the backend omits the field).
  const usageCachePercent =
    usage?.hit_ratio !== undefined ? Math.round(usage.hit_ratio * 100) : null;
  const usageCacheTooltip =
    usageCachePercent !== null
      ? t("app.contextUsageWithCache", {
          percent: usagePercent,
          cachePercent: usageCachePercent,
        })
      : t("app.contextUsage", { percent: usagePercent });

  return (
    <div className="app-layout">
      <TitleBar
        sleepLock={sleepLock}
        turnStartedAt={turnStartedAt}
        now={timerTick}
      />
      {markdownEditorOpen && (
        <Portal>
          <div className="markdown-editor-overlay" role="dialog" aria-modal="true" aria-label={t("app.markdownEditorAria")}>
            <div className="markdown-editor-shell">
              <div className="markdown-editor-header">
                <h3>{t("app.markdownEditorTitle")}</h3>
                <div className="markdown-editor-file" title={markdownPath}>{markdownPath}</div>
                <div className="markdown-editor-actions">
                  <button
                    type="button"
                    className="toolbar-btn"
                    onClick={() => setMarkdownEditorOpen(false)}
                    disabled={markdownSaving}
                  >
                    {t("common.cancel")}
                  </button>
                  <button
                    type="button"
                    className="send-btn"
                    onClick={handleSaveMarkdownEditor}
                    disabled={markdownSaving}
                  >
                    {markdownSaving ? t("common.saving") : t("common.save")}
                  </button>
                </div>
              </div>
              <div className="markdown-editor-body">
                <div className="markdown-editor-column">
                  <span>{t("app.markdownColumn")}</span>
                  <textarea
                    className="markdown-editor-textarea"
                    value={markdownDraft}
                    onChange={(e) => setMarkdownDraft(e.target.value)}
                    placeholder={t("app.markdownPlaceholder")}
                  />
                </div>
                <div className="markdown-editor-column">
                  <span>{t("app.previewColumn")}</span>
                  <div className="markdown-editor-preview">
                    {markdownDraft.trim()
                      ? <MarkdownPreview content={markdownDraft} />
                      : <p className="markdown-editor-preview-empty">{t("app.markdownPreviewEmpty")}</p>}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </Portal>
      )}
      {/* Update dialog — layered above the chat so a sidebar can stay open */}
      {updatePanelOpen && (
        <Portal>
          <UpdatePanel
            initialInfo={updateInfo}
            onInfo={setUpdateInfo}
            onClose={() => setUpdatePanelOpen(false)}
          />
        </Portal>
      )}
      {/* Settings — fullscreen page layered above the chat */}
      {sidebar === "settings" && (
        <Portal>
          <div className="settings-overlay">
            <SettingsPanel
              sessionId={sessionId}
              onClose={closeSidebar}
              onThemePreview={(theme) => {
                themeRef.current = theme ?? "auto";
                applyTheme(theme, paletteRef.current);
              }}
              onPalettePreview={(palette) => {
                paletteRef.current = palette;
                applyTheme(themeRef.current, palette);
              }}
              onConfigSaved={(cfg) => {
                const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
                setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
                setSelectedModel(cfg.model || "gpt-4o-mini");
                setMaxTokens(cfg.model_context_lengths?.[cfg.model] ?? cfg.model_settings?.max_tokens ?? null);
                // A "Fetch Models From API" in Settings refreshes the capability
                // metadata (and with it the badges in the input row).
                setActiveToolCount((cfg.selected_tools ?? []).length);
                setModelMetadata(cfg.model_metadata ?? {});
                setReasoningEfforts(cfg.model_reasoning_effort ?? {});
                themeRef.current = cfg.theme ?? "auto";
                paletteRef.current = cfg.palette ?? "default";
                applyTheme(cfg.theme, cfg.palette);
                if (isLocale(cfg.language)) setLocale(cfg.language);
              }}
            />
          </div>
        </Portal>
      )}
      {/* Sidebar */}
      {sidebar && (
        <aside
          className={`sidebar ${sidebarMotion === "opening" ? "sidebar-opening" : ""} ${sidebarMotion === "closing" ? "sidebar-closing" : ""}`}
          onAnimationEnd={handleSidebarAnimationEnd}
        >
          <div className={`sidebar-shell ${sidebar === "settings" ? "panel-settings" : ""} ${sidebar === "skills" ? "panel-skills" : ""} ${sidebar === "history" ? "panel-history" : ""} ${sidebar === "tools" ? "panel-tools" : ""} ${sidebar === "mcp" ? "panel-mcp" : ""} ${sidebar === "agents" ? "panel-agents" : ""}`} key={sidebar}>
            {sidebar === "skills" && (
              <SkillsPanel
                activeSkillIds={activeSkillIds}
                onToggle={(name, active) => {
                  // Two pieces of state: the persisted status (kept complete,
                  // so a missing folder can come back) and the derived list
                  // that is actually sent to the model.
                  setSkillStatus((prev) => {
                    if (!prev) return prev;
                    // Provenance is owned by the backend; the placeholder is
                    // only there to satisfy the type until the next reload.
                    const entry = prev.skills[name] ?? { enabled: false, source: "" };
                    return {
                      ...prev,
                      skills: { ...prev.skills, [name]: { ...entry, enabled: active } },
                    };
                  });
                  setActiveSkillIds((prev) =>
                    active
                      ? prev.includes(name)
                        ? prev
                        : [...prev, name]
                      : prev.filter((id) => id !== name)
                  );
                }}
                onClose={closeSidebar}
              />
            )}
            {sidebar === "tools" && (
              <ToolsPanel
                onClose={closeSidebar}
                onToolsChange={(tools) =>
                  setActiveToolCount(tools.length)
                }
              />
            )}
            {sidebar === "mcp" && (
              <McpPanel
                onClose={closeSidebar}
                onServersChange={(enabledCount) => setActiveMcpCount(enabledCount)}
              />
            )}
            {sidebar === "history" && (
              <HistoryPanel
                currentSessionId={sessionId}
                refreshKey={historyRefreshKey}
                disableSessionSwitch={streaming}
                onLoad={(sid, msgs) => {
                  if (streaming) {
                    setError(t("app.sessionSwitchBlocked"), "warning");
                    return;
                  }
                  if (cleanupRef.current) { cleanupRef.current(); cleanupRef.current = null; }
                  setStreaming(false);
                  // A loaded transcript starts pinned to its newest turn.
                  followBottomRef.current = true;
                  setMessages(msgs);
                  setSessionId(sid);

                  const lastMsg = msgs.length > 0 ? msgs[msgs.length - 1] : null;
                  if (lastMsg?.role === "user") {
                    setPendingRetryMessageId(lastMsg.id);
                  } else {
                    setPendingRetryMessageId(null);
                  }
                }}
                onClose={closeSidebar}
              />
            )}
            {sidebar === "agents" && (
              <AgentsPanel
                onClose={closeSidebar}
                onAgentsChange={(count) => setActiveAgentCount(count)}
                useAgentsEnabled={useAgentsEnabled}
                onToggleUseAgents={setUseAgentsEnabled}
                agentStatuses={agentStatuses}
              />
            )}
          </div>
        </aside>
      )}

      {/* Main chat area */}
      <div className="chat-area" onClick={handleChatAreaClick}>
        {/* Toolbar */}
        <header className="toolbar" ref={toolbarRef}>
          <span className="app-title">
            <button
              className="toolbar-btn"
              onClick={clearChat}
              title={t("app.newChatTitle")}
            >
              <FontAwesomeIcon icon={faRotateRight} />
              {t("app.newChat")}
            </button>
          </span>

          <div className="toolbar-actions" ref={toolbarActionsRef}>
            {toolbarIndicator && (
              <span
                className={`toolbar-selection${sidebar ? " visible" : ""}`}
                aria-hidden="true"
                style={{
                  transform: `translateX(${toolbarIndicator.left}px)`,
                  width: toolbarIndicator.width,
                }}
              />
            )}
            <button
              className={`toolbar-btn ${sidebar === "agents" ? "active" : ""}`}
              onClick={() => toggleSidebar("agents")}
              title={useAgentsEnabled ? t("app.toolbarAgentsTitleEnabled") : t("app.toolbarAgentsTitle")}
            >
              <FontAwesomeIcon icon={faRobot} />
              {t("app.toolbarAgents")}
              {activeAgentCount > 0 && useAgentsEnabled && (
                <span className="toolbar-btn-count">{activeAgentCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "skills" ? "active" : ""}`}
              onClick={() => toggleSidebar("skills")}
              title={t("app.toolbarSkills")}
            >
              <FontAwesomeIcon icon={faWandMagicSparkles} />
              {t("app.toolbarSkills")}
              {activeSkillIds.length > 0 && (
                <span className="toolbar-btn-count">{activeSkillIds.length}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "mcp" ? "active" : ""}`}
              onClick={() => toggleSidebar("mcp")}
              title={t("app.toolbarMcpTitle")}
            >
              <FontAwesomeIcon icon={faPlug} />
              {t("app.toolbarMcp")}
              {activeMcpCount > 0 && (
                <span className="toolbar-btn-count">{activeMcpCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "tools" ? "active" : ""}`}
              onClick={() => toggleSidebar("tools")}
              title={t("app.toolbarTools")}
            >
              <FontAwesomeIcon icon={faScrewdriverWrench} />
              {t("app.toolbarTools")}
              {activeToolCount > 0 && (
                <span className="toolbar-btn-count">{activeToolCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "history" ? "active" : ""}`}
              onClick={() => toggleSidebar("history")}
              title={t("app.toolbarHistory")}
            >
              <FontAwesomeIcon icon={faClockRotateLeft} />
              {t("app.toolbarHistory")}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "settings" ? "active" : ""}`}
              onClick={() => toggleSidebar("settings")}
              title={t("app.toolbarSettings")}
            >
              <FontAwesomeIcon icon={faGear} />
              {t("app.toolbarSettings")}
            </button>

          </div>
        </header>

        {profileExporting && (
          <div className="profile-export-progress" role="progressbar" aria-busy="true" aria-live="polite">
            <div className="profile-export-progress-label">
              {profileExportPhase === "preparing"
                ? t("app.profileExportPreparing")
                : profileExportPhase === "status"
                  ? profileExportStatus
                  : t("app.profileExportLabel")}
            </div>
            <div className="profile-export-progress-hint">{t("app.profileExportHint")}</div>
            <div className="profile-export-progress-track">
              <div className="profile-export-progress-fill" />
            </div>
          </div>
        )}

        {updateInfo?.has_update && !updateBannerDismissed && (
          <div className="update-banner" role="status">
            <span className="update-banner-text">
              {t("app.updateAvailable", {
                version: updateInfo.latest_version,
                current: updateInfo.current_version,
              })}
            </span>
            <div className="update-banner-actions">
              <button type="button" className="toolbar-btn" onClick={() => setUpdatePanelOpen(true)}>
                {t("app.viewUpdate")}
              </button>
              <button
                type="button"
                className="toolbar-btn"
                onClick={() => setUpdateBannerDismissed(true)}
              >
                {t("app.dismiss")}
              </button>
            </div>
          </div>
        )}

        {/* A timer that fired while the user was in another chat session: offer
            to move its prompt into the current input box instead of hijacking
            the conversation. */}
        {timerNotice && (
          <div className="timer-notice" role="status">
            <span className="timer-notice-text">
              {t("app.timers.otherSession", {
                label: timerNotice.label || t("app.timers.untitled"),
              })}
            </span>
            <div className="timer-notice-actions">
              <button
                type="button"
                className="toolbar-btn"
                onClick={() => {
                  setInput(timerPromptText(timerNotice));
                  setTimerNotice(null);
                  textareaRef.current?.focus();
                }}
              >
                {t("app.timers.insert")}
              </button>
              <button
                type="button"
                className="toolbar-btn"
                onClick={() => setTimerNotice(null)}
              >
                {t("app.timers.dismiss")}
              </button>
            </div>
          </div>
        )}

        {/* Warnings and errors float here — top-left of the chat window,
            between the last bar and the transcript, outside the scroll
            container. The strip has no height, so a notice never pushes the
            conversation around; the bars above it keep their own space. */}
        {notice && <ChatNotice notice={notice} onDismiss={dismissNotice} />}

        {/* Messages */}
        <div className="messages" ref={messagesRef} onScroll={handleMessagesScroll}>
          {messages.length === 0 && (
            <div className="empty-state">
              <p>{t("app.emptyStart")}</p>
              <p className="empty-hint">
                {t("app.emptyHint", {
                  skills: t("app.toolbarSkills"),
                  settings: t("app.toolbarSettings"),
                })}
              </p>
            </div>
          )}
          {messages.map((m) =>
            m.role === "tool_group" ? (
              <ToolCallGroup key={m.id} message={m} onConfirm={respondToolCallConfirm} onCancelBackground={handleCancelBackgroundTask} />
            ) : (
              <ChatMessage
                key={m.id}
                message={m}
                showRetry={m.role === "user" && m.id === pendingRetryMessageId && !streaming}
                onRetry={retryPendingUserMessage}
                onDelete={handleDeleteMessage}
                onFork={handleForkMessage}
                dbId={m.dbId}
                onConfirm={respondToolCallConfirm}
                onCancelBackground={handleCancelBackgroundTask}
                backgroundTasks={
                  m.role === "assistant" && m.id === messages[messages.length - 1]?.id
                    ? backgroundTasks
                    : undefined
                }
                backgroundWaiting={
                  m.role === "assistant" && m.id === messages[messages.length - 1]?.id
                    ? backgroundWaiting
                    : undefined
                }
                sessionId={sessionId}
                todoActive={m.role === "assistant" && m.id === todoMessageId}
                skillNames={knownSkillNames}
              />
            )
          )}
        </div>

        {/* Input */}
        <div className="input-area" ref={inputAreaRef} style={{ position: "relative", flexDirection: "column", alignItems: "stretch" }}>
          {timers.length > 0 && (
            <div className="timer-bar" role="status" aria-live="polite">
              <span className="timer-bar-title">
                <FontAwesomeIcon icon={faStopwatch} />
                {t("app.timers.title")}
              </span>
              <div className="timer-bar-chips">
                {timers.map((timer) => {
                  const otherSession =
                    Boolean(timer.session_id) && timer.session_id !== sessionId;
                  return (
                    <div
                      key={timer.id}
                      className={`timer-chip${otherSession ? " other-session" : ""}`}
                      title={timer.message}
                    >
                      <span className="timer-chip-label">
                        {timer.label || t("app.timers.untitled")}
                      </span>
                      <span className="timer-chip-countdown">
                        {t("app.timers.firesIn", {
                          time: formatCountdown(timer.fire_at - timerTick),
                        })}
                      </span>
                      <button
                        type="button"
                        className="timer-chip-cancel"
                        title={t("app.timers.cancel")}
                        aria-label={t("app.timers.cancel")}
                        onClick={() => void handleCancelTimer(timer.id)}
                      >
                        <FontAwesomeIcon icon={faXmark} />
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}
          {(usage || streaming) && (
            <div
              className="usage-panel"
              role="progressbar"
              aria-live="polite"
              aria-busy={streaming && !usageReady}
              aria-label={
                streaming && !usageReady
                  ? t("chat.tokenUsage")
                  : usageCacheTooltip
              }
              title={streaming && usageReady ? usageCacheTooltip : undefined}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={streaming && !usageReady ? undefined : usagePercent}
            >
              {streaming && !usageReady ? (
                <div className="usage-panel-sweep" aria-hidden="true" />
              ) : (
                <div
                  className={`usage-panel-fill${usagePercent >= 90 ? " danger" : ""}`}
                  style={{ width: `${usagePercent}%` }}
                />
              )}
            </div>
          )}
          {/* A PDF mention converts before the turn starts: say which page the
              backend is on instead of letting the composer look stuck. Same
              recipe as the injection bar — one muted row, no fill, no border. */}
          {mentionProgress && (
            <div className="mention-progress" role="status" aria-live="polite">
              <span className="mention-progress-text">
                <FontAwesomeIcon icon={faSpinner} spin aria-hidden="true" />
                {t("app.fileMention.pdfConverting", { name: mentionProgress.name })}
              </span>
              {mentionProgress.page > 0 && (
                <span className="mention-progress-detail">
                  {t("app.fileMention.pdfConvertingPage", {
                    page: mentionProgress.page,
                    total: mentionProgress.total,
                  })}
                </span>
              )}
              <span className="mention-progress-track" aria-hidden="true">
                <span
                  className="mention-progress-fill"
                  style={{ width: `${mentionProgress.percent}%` }}
                />
              </span>
            </div>
          )}
          {pendingInjections.length > 0 && (
            <div className="injection-bar" role="status" aria-live="polite">
              <span className="injection-bar-title">
                <FontAwesomeIcon icon={faPenToSquare} />
                {t("app.injections.title")}
              </span>
              {pendingInjections.map((item) => (
                <span key={item.id} className="injection-chip" title={item.text}>
                  <span className="injection-chip-text">{item.text}</span>
                  <span className="injection-chip-state">{t("app.injections.queued")}</span>
                </span>
              ))}
            </div>
          )}
          {renderConfirmDialog()}
          <div className="input-row">
            <SlashMentionPopup
              open={slashState.open}
              items={slashItems}
              activeIndex={slashActiveIndex}
              loading={slashSkillsLoading}
              query={slashState.query}
              onHover={setSlashIndex}
              onSelect={acceptSlash}
            />
            <FileMentionPopup
              open={mentionState.open}
              items={mentionItems}
              activeIndex={mentionActiveIndex}
              loading={workspaceFilesLoading}
              query={mentionState.query}
              onHover={setMentionIndex}
              onSelect={acceptMention}
            />
            <textarea
              ref={textareaRef}
              className="chat-input"
              rows={1}
              value={input}
              onChange={(e) => {
                const value = e.target.value;
                setInput(value);
                syncMentionFromInput(value, e.target.selectionStart ?? value.length);
                syncSlashFromInput(value, e.target.selectionStart ?? value.length);
              }}
              onKeyDown={handleKeyDown}
              // A real blur (clicking elsewhere) dismisses the popup; the popup
              // itself accepts on `mousedown` + `preventDefault`, so choosing a
              // row never blurs the textarea.
              onBlur={() => {
                closeMention();
                closeSlash();
              }}
              placeholder={streaming ? t("app.injections.placeholder") : t("app.inputPlaceholder")}
              // Stays usable while a reply streams: Enter then adds the text to
              // the running turn instead of starting a new one.
              disabled={profileExporting}
            />
            <ModelSelect
              models={availableModels}
              value={selectedModel}
              metadata={modelMetadata}
              disabled={streaming || profileExporting}
              onChange={setSelectedModel}
            />
            {reasoningSupported && (
              // A native `<option>` cannot hold markup, so the brain glyph is a
              // sibling of the select rather than a prefix on every option.
              <div className="reasoning-select-wrap">
                <FontAwesomeIcon
                  icon={faBrain}
                  className="reasoning-select-icon"
                  aria-hidden="true"
                />
                <select
                  className="reasoning-select"
                  title={t("app.reasoningDepthTitle")}
                  aria-label={t("app.reasoningDepth")}
                  value={reasoningEffort}
                  onChange={(e) => void handleReasoningEffortChange(e.target.value)}
                  disabled={streaming || profileExporting}
                >
                  <option value="">{t("app.reasoningDefault")}</option>
                  <option value="low">{t("app.reasoningLow")}</option>
                  <option value="medium">{t("app.reasoningMedium")}</option>
                  <option value="high">{t("app.reasoningHigh")}</option>
                </select>
              </div>
            )}
            <button
              className="send-btn"
              onClick={() => (streaming ? void stopStreaming() : void sendMessage())}
              disabled={profileExporting || (!streaming && !input.trim())}
            >
              {streaming ? t("app.stop") : t("app.send")}
            </button>
          </div>
        </div>
      </div >
    </div >
  );
}
