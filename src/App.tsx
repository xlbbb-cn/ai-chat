import { useState, useRef, useEffect, useLayoutEffect, useCallback, useMemo } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { chatCompletion, checkUpdate, getConfig, getSkillsStatus, getAgentOrchestration, injectUserMessage, listMcpServers, listSubAgents, listAgentMentions, saveSkillsStatus, saveConfig, saveHistory, stopChatCompletion, confirmCommand, saveMarkdownFile, deleteMessage, forkSession, filterExistingSkills, listTimers, cancelTimer, getSleepLockState, listBackgroundTasks, cancelBackgroundTask, listWorkspaceFiles, listSkills, startEvolutionMaintenance, stopEvolutionMaintenance, getEvolutionMaintenanceState, onEvolutionMaintenanceState, setWindowVibrancy, savePastedFile, importWorkspacePaths } from "./api";

import { ChatMessage, ChatMessageBoundary } from "./components/ChatMessage";
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
import { AgentMentionPopup } from "./components/AgentMentionPopup";
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
  GlassEffect,
  SkillsStatus,
  SleepLockState,
  ThemeMode,
  TimerEntry,
  BackgroundTaskState,
  EvolutionMaintenancePhase,
  UpdateInfo,
  WorkspaceFileEntry,
} from "./types";
import { extensionOf, supportsMentionName } from "./attachments";
import { activeMentionQuery, resolveFileMentions, scoreWorkspaceFile } from "./fileMention";
import type { MentionProgress, MentionResolution } from "./fileMention";
import {
  canonicalTmpMentions,
  insertMentionTokens,
  mentionDisplayName,
  mentionTokenForPath,
  partitionByModelSupport,
  preparePastedFiles,
} from "./composerFiles";
import { activeSlashQuery, scoreSkillOption, toSkillOptions } from "./slashMention";
import type { SkillMentionOption } from "./slashMention";
import { activeAgentQuery, hasAgentToken, scoreAgentMention } from "./agentMention";
import type { AgentMentionOption } from "./agentMention";
import { isGenerationStopped, isLocale, stripGenerationStopped, useI18n } from "./i18n";
import { formatCountdown } from "./utils/time";
import {
  FontAwesomeIcon,
  faBrain,
  faClockRotateLeft,
  faGear,
  faPaperclip,
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
 * Glyphs shown in the napping face's speech bubble. Kept as plain Unicode so
 * the ticker needs no image asset and inherits the current text colour. A new
 * random sequence is generated every time the maintenance pass starts, so the
 * "dream" never looks like the same loop twice.
 */
const DREAM_GLYPHS = "·✦✧⋆﹡※∞∑≈◇○●□■△▽☆★☾☽⌁⌘⊹⋰".split("");

function makeDreamGlyphs(count = 18): string[] {
  return Array.from(
    { length: count },
    () => DREAM_GLYPHS[Math.floor(Math.random() * DREAM_GLYPHS.length)],
  );
}

/**
 * The round face (no mouth, no other features) that hops back and forth. The
 * eyes are tall ellipses (token cutouts) inside a gaze group — the group
 * glances toward the travel direction while each eye blinks on its own
 * transform. Shared by the interactive button and the plain decorative div so
 * both states stay pixel-identical.
 */
const DREAM_FACE = (
  <svg viewBox="0 0 64 64" role="presentation" focusable="false">
    <ellipse className="rabbit-hop-head" cx="32" cy="32" rx="17" ry="16" />
    <g className="rabbit-hop-eyes">
      <ellipse className="rabbit-hop-eye" cx="25.5" cy="30.5" rx="2.4" ry="4.6" />
      <ellipse className="rabbit-hop-eye" cx="38.5" cy="30.5" rx="2.4" ry="4.6" />
    </g>
  </svg>
);

/** Status-line label for each maintenance phase (see `EvolutionMaintenancePhase`). */
const DREAM_PHASE_KEYS = {
  inventory: "app.dream.phase.inventory",
  files: "app.dream.phase.files",
  skills: "app.dream.phase.skills",
  memory: "app.dream.phase.memory",
  wrap_up: "app.dream.phase.wrap_up",
} as const;

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
  // Mica's light / dark variant is a native flag, not a CSS token, so a theme
  // change re-applies the backdrop the vibrancy switch already asked for.
  if (document.documentElement.hasAttribute("data-vibrancy")) {
    applyWindowVibrancy(currentGlassEffect(), resolved === "dark");
  }
}

/** The glass level currently painted on `<html>`. */
function currentGlassEffect(): GlassEffect {
  const level = document.documentElement.getAttribute("data-glass");
  return level === "off" || level === "subtle" || level === "strong" ? level : "standard";
}

/**
 * Ask the backend for the window-level backdrop behind a glass level and record
 * what it managed to apply.
 *
 * `data-vibrancy` is written only when the native call succeeded, so a failure
 * (macOS, Linux, an unsupported Windows build) never leaves a transparent
 * window over a raw desktop. That case gets `data-glass-fallback` instead: the
 * same glass, with the backdrop the app paints itself — a webview can blur only
 * its own pixels, so the compositor is not the only thing that can supply
 * something behind the chrome. Fire and forget: `data-glass` is already set,
 * and the blur level must not wait for the IPC.
 */
function applyWindowVibrancy(level: GlassEffect, dark: boolean) {
  const root = document.documentElement;
  const settle = (kind: string | null) => {
    if (kind) {
      root.setAttribute("data-vibrancy", kind);
      root.removeAttribute("data-glass-fallback");
    } else {
      root.removeAttribute("data-vibrancy");
      // `[data-glass="off"]` gates the fallback out on the CSS side: with no
      // blur there is no glass to sit on a backdrop.
      root.setAttribute("data-glass-fallback", "");
    }
  };
  setWindowVibrancy(level, dark)
    .then((kind) => settle(kind))
    .catch(() => settle(null));
}

/**
 * Paint the frosted-glass level on `<html>`.
 *
 * `data-glass` selects the `--glass-*` token block in `src/App.css`. The level
 * is independent of the light / dark and palette dimensions (and of the OS
 * scheme), so it is applied on its own rather than through `applyTheme`.
 */
function applyGlassEffect(glass: GlassEffect | undefined) {
  const level = glass ?? "standard";
  document.documentElement.setAttribute("data-glass", level);
  // The desktop seen *through* the window is blurred by the compositor, not by
  // `backdrop-filter`, so the level also drives a native Mica / Acrylic call.
  applyWindowVibrancy(level, document.documentElement.getAttribute("data-theme") === "dark");
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
        // The summary says the bytes are already here: a path the model can
        // open is an invitation to read the file a second time, which is one
        // wasted tool round per attachment.
        text: `\n<details><summary>Attached File: ${mentionDisplayName(att.name)} — already provided below</summary>\n\n\`\`\`${fenceLanguage(att)}\n${att.text_content}\n\`\`\`\n</details>`,
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
/** How long the `+agent` popup trusts its cached sub-agent roster. */
const AGENT_MENTIONS_TTL_MS = 15_000;

interface MarkdownEditPayload {
  path: string;
  content: string;
}

/**
 * Sidebar entrance/exit spring — the one animation in the app that is not
 * `--t-dur` + `--t-ease`, because a 400px frosted panel has to settle with mass
 * instead of a fixed curve (Design System v1.0 §三). The travel is 20px, not the
 * panel's width: resizing a frosted panel re-samples its backdrop and reflows
 * every row on each frame, so the width stays fixed and the surface glides.
 * `prefers-reduced-motion` drops the whole thing to an instant swap.
 */
const SIDEBAR_SPRING = { type: "spring" as const, stiffness: 320, damping: 32 };
const SIDEBAR_SHOWN = { opacity: 1, x: 0, pointerEvents: "auto" as const };
const SIDEBAR_HIDDEN = { opacity: 0, x: -20, pointerEvents: "none" as const };

/**
 * The goal-loop end marker (`[goal:DONE]`) is a backend control signal: the
 * main loop reads it from the streamed answer to decide whether the turn may
 * end, but it must never reach the transcript bubble or the persisted row.
 * Keep in sync with `GOAL_DONE_MARKER` in `src-tauri/src/llm_complete.rs`.
 */
const GOAL_DONE_MARKER = "[goal:DONE]";

/** Remove every goal-loop marker and the blank tail it leaves behind. */
function stripGoalDoneMarker(text: string): string {
  if (!text.includes(GOAL_DONE_MARKER)) return text;
  return text.split(GOAL_DONE_MARKER).join("").trimEnd();
}

export default function App() {
  const { t, locale, setLocale } = useI18n();
  /** Spring physics must be off when the OS asks for reduced motion. */
  const reducedMotion = useReducedMotion();
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
  /**
   * Composer `+agent` popup: embeds a sub-agent's system prompt into a **new**
   * conversation. `embeddedAgents` holds the agents whose `+token` was
   * accepted; their ids travel with the first message only, because the
   * backend freezes a snapshot from then on.
   */
  const [agentMentionState, setAgentMentionState] = useState<{
    open: boolean;
    query: string;
    start: number;
  }>({ open: false, query: "", start: 0 });
  const [agentMentionIndex, setAgentMentionIndex] = useState(0);
  /** Enabled `llm` agents for the popup; `null` until the first load. */
  const [agentMentionOptions, setAgentMentionOptions] = useState<AgentMentionOption[] | null>(null);
  const [agentMentionLoading, setAgentMentionLoading] = useState(false);
  /** Timestamped cache, so the roster is read at most every 15s. */
  const agentMentionRef = useRef<{ at: number; options: AgentMentionOption[] } | null>(null);
  const agentMentionPendingRef = useRef(false);
  const [embeddedAgents, setEmbeddedAgents] = useState<AgentMentionOption[]>([]);
  /**
   * Drop the pending `+agent` selections. Called whenever the session changes
   * (new chat, history load, fork): a role belongs to the conversation it was
   * chosen for, never to the next one.
   */
  const resetEmbeddedAgents = useCallback(() => {
    setEmbeddedAgents([]);
    setAgentMentionState({ open: false, query: "", start: 0 });
  }, []);
  const [streaming, setStreaming] = useState(false);
  /**
   * The empty-state smiley's silent maintenance pass. `running` is driven by
   * `evolution-maintenance-state` (and restored by `get_..._state` after a
   * webview reload); it never touches `messages` or `streaming`, so the empty
   * state stays on screen while the face naps.
   */
  const [selfEvolutionMode, setSelfEvolutionMode] = useState(false);
  const [dreamState, setDreamState] = useState<"idle" | "running">("idle");
  const [dreamGlyphs, setDreamGlyphs] = useState<string[]>(() => makeDreamGlyphs());
  /** Live phase/round/tool count shown instead of the idle greeting. */
  const [dreamProgress, setDreamProgress] = useState<{
    phase: EvolutionMaintenancePhase;
    round: number;
    toolCalls: number;
  } | null>(null);
  /**
   * Final report of the last completed pass. It is shown in a clickable bubble
   * under the face until dismissed, so a finish is readable instead of being
   * swallowed by the empty state.
   */
  const [dreamResult, setDreamResult] = useState<string | null>(null);
  /** A terminal event already arrived; a late `get_state` must not revive it. */
  const dreamEventSeenRef = useRef(false);
  /**
   * Timestamp of the previous click while napping. Two clicks inside the
   * double-click window stop the pass; the first click after an idle start is
   * deliberately ignored, so double-clicking the hopping face starts the pass
   * instead of starting and immediately cancelling it.
   */
  const lastDreamClickRef = useRef(0);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
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
  /**
   * The draft as it exists *now*. A paste or a drop resolves its insertion
   * point from an event callback, which may run before React has committed the
   * last keystroke — reading `input` there would splice into a stale string.
   */
  const composerDraftRef = useRef(input);
  composerDraftRef.current = input;
  /** True while a file is dragged over the window (native drag-drop). */
  const [dropActive, setDropActive] = useState(false);
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
    setSelfEvolutionMode(cfg.self_evolution_mode ?? false);
    // A result bubble belongs to the workspace/config it was produced in; a
    // reload, profile import or workspace switch must not carry it over.
    setDreamResult(null);
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

  // The silent maintenance turn (the empty-state nap face) has its own event
  // stream — never `chat-*` — so a maintenance finish cannot settle a real chat
  // reply. `get_..._state` restores the napping face after a webview reload;
  // `dreamEventSeenRef` keeps a late query from reviving a run that already
  // reported its terminal state.
  useEffect(() => {
    const unlisten = onEvolutionMaintenanceState((state) => {
      dreamEventSeenRef.current = true;
      if (state.running) {
        setDreamState("running");
        setDreamResult(null);
        setDreamProgress((previous) => ({
          phase: state.phase ?? previous?.phase ?? "inventory",
          round: state.round ?? previous?.round ?? 1,
          toolCalls: state.toolCalls ?? previous?.toolCalls ?? 0,
        }));
        return;
      }
      setDreamState("idle");
      setDreamProgress(null);
      setDreamResult(
        state.outcome === "done" && state.summary?.trim()
          ? state.summary.trim()
          : null,
      );
      if (state.outcome === "error" && state.error) {
        setError(t("app.dream.failed", { error: state.error }), "warning");
      }
    });
    getEvolutionMaintenanceState()
      .then((running) => {
        if (running && !dreamEventSeenRef.current) {
          setDreamState("running");
          setDreamProgress({ phase: "inventory", round: 1, toolCalls: 0 });
        }
      })
      .catch(console.error);
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [setError, t]);

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
        applyGlassEffect(cfg.glass_effect);
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
      .catch(console.error)
      // Preload the `+agent` roster after the panel roster: both read the same
      // definition store (and a legacy migration may rewrite it), so the two
      // calls are serialised instead of racing. The roster feeds both the popup
      // and the transcript token highlighting, which must work before the
      // popup is ever opened.
      .finally(() => {
        listAgentMentions()
          .then((options) => {
            agentMentionRef.current = { at: Date.now(), options };
            setAgentMentionOptions(options);
          })
          .catch(console.error);
      });
    getAgentOrchestration()
      .then((orch) => setUseAgentsEnabled(orch.use_agents))
      .catch(console.error);
  }, []);

  useEffect(() => {
    const unlisten = listen("profile-restored", async () => {
      try {
        // A restored profile can reference skills that are absent from the
        // current workspace, so re-validate before activating them. The
        // `+agent` roster is refreshed too: a profile swap can enable or
        // disable sub-agents.
        const [, servers, mentions] = await Promise.all([
          applyConfigToUi(),
          listMcpServers(),
          listAgentMentions(),
        ]);
        setActiveMcpCount(servers.filter((s) => s.enabled).length);
        agentMentionRef.current = { at: Date.now(), options: mentions };
        setAgentMentionOptions(mentions);
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

  // Safety net for the turn boundary. Tasks are turn-scoped, so once the turn
  // is over nothing can still be `running`: the backend tore every runner down
  // and dropped its entry (`end_turn_cleanup`), and its own report is discarded
  // by that teardown. Rows left here are therefore stale by definition — drop
  // them and zero the waiting chip, so a missed or late event can never leave
  // the panel spinning on work that is already gone.
  useEffect(() => {
    if (streaming) return;
    setBackgroundTasks((prev) =>
      prev.some((task) => task.status === "running")
        ? prev.filter((task) => task.status !== "running")
        : prev
    );
    setBackgroundWaiting(0);
  }, [streaming]);

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

    // The conversation is starting, so a maintenance report bubble has done
    // its job; it must not reappear over a later empty chat.
    setDreamResult(null);

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

    // The draft already carries the short `@@<name>` form, so the message that
    // reaches the model holds no `.data/tmp` path that reads as "open this
    // file"; a hand-typed long form is collapsed to the same shape here.
    const messageText = canonicalTmpMentions(rawText);

    // `+agent` selections only travel with a conversation that has no reply
    // yet; after that the frozen snapshot in the database speaks for itself —
    // the same rule the backend repeats. Matching the backend's "no assistant
    // row" test (rather than `messages.length === 0`) also covers retrying a
    // first message that failed before the snapshot was written. A timer
    // resume is not a user send, and a token the user deleted from the draft is
    // no longer a selection.
    const embeddedAgentIds =
      !autoResume && !messages.some((m) => m.role === "assistant")
        ? embeddedAgents
            .filter((agent) => hasAgentToken(messageText, agent.token))
            .map((agent) => agent.id)
        : undefined;

    const { content: apiContent } = buildMessageContent(messageText, messageAttachments);
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
      // Sending clears the draft, so a composer popup it opened must close
      // too: Enter can send while a popup is visible (an empty result list
      // leaves the key unclaimed), and a stale popup would hang over the turn.
      setMentionState({ open: false, query: "", start: 0 });
      setSlashState({ open: false, query: "", start: 0 });
      setAgentMentionState({ open: false, query: "", start: 0 });
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
        // Never surface the goal-loop marker: it is the last thing the model
        // writes, so trimming it the moment it completes keeps the live
        // bubble identical to the settled one.
        if (accumulatedContent.endsWith(GOAL_DONE_MARKER)) {
          accumulatedContent = accumulatedContent.slice(0, -GOAL_DONE_MARKER.length);
        }
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
      onRetry({ content, reasoning }) {
        // The failed attempt's tokens were appended verbatim, so dropping its
        // exact tail restores the accumulator to the pre-request state; the
        // retry then re-streams the whole response. A tail that does not
        // match (should not happen) is left in place rather than guessed at.
        // The backend's attempt text still carries the goal-loop marker the
        // live accumulator already dropped — compare against the same form.
        const tail = stripGoalDoneMarker(content);
        if (tail && accumulatedContent.endsWith(tail)) {
          accumulatedContent = accumulatedContent.slice(
            0,
            accumulatedContent.length - tail.length
          );
        }
        if (reasoning && accumulatedReasoning.endsWith(reasoning)) {
          accumulatedReasoning = accumulatedReasoning.slice(
            0,
            accumulatedReasoning.length - reasoning.length
          );
        }
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? { ...m, content: accumulatedContent, reasoning_content: accumulatedReasoning }
              : m
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
        // The goal-loop marker is a control signal; it never reaches the
        // transcript or the persisted row (belt and braces — `onToken` already
        // drops the trailing form).
        accumulatedContent = stripGoalDoneMarker(accumulatedContent);
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
              if (m.id === assistantId) return { ...m, content: accumulatedContent, streaming: false, ...(finalToolCalls?.length ? { tool_calls: finalToolCalls } : {}) };
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
        // The goal-loop marker is a control signal; never persist it or seed
        // the "continue" action with it.
        accumulatedContent = stripGoalDoneMarker(accumulatedContent);
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
                    content: accumulatedContent,
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
    }, useAgentsEnabled, embeddedAgentIds);

    cleanupRef.current = cleanup;
  }, [input, messages, streaming, profileExporting, activeSkillIds, sessionId, selectedModel, selectedMetadata, useAgentsEnabled, embeddedAgents, t, flushUndeliveredInjections]);

  /**
   * Start the silent evolution-maintenance pass from the empty-state face.
   * The command returns as soon as the backend claimed the run; the terminal
   * `evolution-maintenance-state` event clears `dreamState`.
   */
  const startDream = useCallback(async () => {
    if (!selfEvolutionMode || dreamState === "running" || streaming || profileExporting) {
      return;
    }
    setDreamGlyphs(makeDreamGlyphs());
    lastDreamClickRef.current = 0;
    setDreamState("running");
    setDreamProgress({ phase: "inventory", round: 1, toolCalls: 0 });
    setDreamResult(null);
    try {
      await startEvolutionMaintenance(selectedModel);
    } catch (err) {
      // A concurrent run (another window, or a state race) is already napping:
      // restore that state instead of pretending the start failed.
      try {
        if (await getEvolutionMaintenanceState()) return;
      } catch {
        /* fall through to the warning */
      }
      setDreamState("idle");
      setDreamProgress(null);
      setError(t("app.dream.failed", { error: String(err) }), "warning");
    }
  }, [
    selfEvolutionMode,
    dreamState,
    streaming,
    profileExporting,
    selectedModel,
    setError,
    t,
  ]);

  /** Double-click on the napping face: stop the running pass. */
  const stopDream = useCallback(async () => {
    if (dreamState !== "running") return;
    try {
      await stopEvolutionMaintenance();
    } catch (err) {
      console.error("Failed to stop evolution maintenance", err);
    }
  }, [dreamState]);

  /** Click on the result bubble: drop it and restore the greeting. */
  const dismissDreamResult = useCallback(() => {
    setDreamResult(null);
  }, []);

  const handleDreamClick = useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      if (dreamState === "running") {
        // Keyboard activation has detail === 0 and is the accessible stop.
        if (event.detail === 0) {
          void stopDream();
          return;
        }
        // Two clicks inside the double-click window stop the pass. The first
        // click only arms the timer, so a double-click on the *hopping* face
        // (whose first click just started the pass) never cancels it.
        const now = Date.now();
        if (now - lastDreamClickRef.current <= 320) {
          lastDreamClickRef.current = 0;
          void stopDream();
        } else {
          lastDreamClickRef.current = now;
        }
        return;
      }
      void startDream();
    },
    [dreamState, startDream, stopDream],
  );

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
        // Never surface the goal-loop marker: it is the last thing the model
        // writes, so trimming it the moment it completes keeps the live
        // bubble identical to the settled one.
        if (accumulatedContent.endsWith(GOAL_DONE_MARKER)) {
          accumulatedContent = accumulatedContent.slice(0, -GOAL_DONE_MARKER.length);
        }
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
      onRetry({ content, reasoning }) {
        // The failed attempt's tokens were appended verbatim, so dropping its
        // exact tail restores the accumulator to the pre-request state; the
        // retry then re-streams the whole response. A tail that does not
        // match (should not happen) is left in place rather than guessed at.
        // The backend's attempt text still carries the goal-loop marker the
        // live accumulator already dropped — compare against the same form.
        const tail = stripGoalDoneMarker(content);
        if (tail && accumulatedContent.endsWith(tail)) {
          accumulatedContent = accumulatedContent.slice(
            0,
            accumulatedContent.length - tail.length
          );
        }
        if (reasoning && accumulatedReasoning.endsWith(reasoning)) {
          accumulatedReasoning = accumulatedReasoning.slice(
            0,
            accumulatedReasoning.length - reasoning.length
          );
        }
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? { ...m, content: accumulatedContent, reasoning_content: accumulatedReasoning }
              : m
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
        // The goal-loop marker is a control signal; it never reaches the
        // transcript or the persisted row (belt and braces — `onToken` already
        // drops the trailing form).
        accumulatedContent = stripGoalDoneMarker(accumulatedContent);
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
              if (m.id === assistantId) return { ...m, content: accumulatedContent, streaming: false, ...(finalToolCalls?.length ? { tool_calls: finalToolCalls } : {}) };
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
        // The goal-loop marker is a control signal; never persist it or seed
        // the "continue" action with it.
        accumulatedContent = stripGoalDoneMarker(accumulatedContent);
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
                    content: accumulatedContent,
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
      // The fork inherits the role snapshot from the database, but the
      // composer's pending selection belongs to the old conversation.
      resetEmbeddedAgents();
      setPendingRetryMessageId(null);
      setError(null);
    } catch (err) {
      setError(t("app.forkFailed", { error: String(err) }));
    }
  }, [messages, sessionId, resetEmbeddedAgents, t]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (profileExporting) {
      e.preventDefault();
      return;
    }

    // An IME composition (pinyin, kana, …) owns Enter/Tab and the arrows until
    // it commits. Acting on those keys would accept a mention row or send the
    // message mid-composition — and embedded role tokens are usually CJK, so
    // the popup is often open while an IME is active.
    if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;

    // An open mention popup (`@file`, `/skill` or `+agent`) owns the arrow
    // keys, Enter/Tab and Escape while it is visible. Their token grammars are
    // mutually exclusive, so at most one is open and it decides which list,
    // index and accept handler the keys act on.
    if (mentionState.open || slashState.open || agentMentionState.open) {
      const rowCount = mentionState.open
        ? mentionItems.length
        : slashState.open
          ? slashItems.length
          : agentMentionItems.length;
      if (e.key === "Escape") {
        e.preventDefault();
        closeMention();
        closeSlash();
        closeAgentMention();
        return;
      }
      if (rowCount > 0) {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          const delta = e.key === "ArrowDown" ? 1 : -1;
          if (mentionState.open) {
            setMentionIndex((prev) => (prev + delta + rowCount) % rowCount);
          } else if (slashState.open) {
            setSlashIndex((prev) => (prev + delta + rowCount) % rowCount);
          } else {
            setAgentMentionIndex((prev) => (prev + delta + rowCount) % rowCount);
          }
          return;
        }
        if (e.key === "Enter" || e.key === "Tab") {
          e.preventDefault();
          if (mentionState.open) {
            const item = mentionItems[mentionActiveIndex];
            if (item) acceptMention(item);
          } else if (slashState.open) {
            const item = slashItems[slashActiveIndex];
            if (item) acceptSlash(item);
          } else {
            const item = agentMentionItems[agentMentionActiveIndex];
            if (item) acceptAgentMention(item);
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
    // Clicking the open panel's button closes it.
    if (sidebar === panel) {
      closeSidebar();
      return;
    }

    // Switching to another panel while the sidebar is already visible swaps the
    // shell in place: replaying the entrance on every tab click would flash a
    // 400px frosted surface.
    setSidebar(panel);
  }

  function closeSidebar() {
    // `AnimatePresence` owns the exit spring and unmounts the aside when it
    // settles; the state drops to null at once, so the rest of the app (title,
    // Escape, chat clicks) sees a closed sidebar immediately.
    if (!sidebar) return;
    setSidebar(null);
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
    setDreamResult(null);
    resetEmbeddedAgents();
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

  // ─── Composer `+agent` mentions ───────────────────────────────────────────

  /**
   * Agents the `+agent` popup offers, ranked by relevance.
   *
   * The backend already filters to enabled `llm` agents with a real system
   * prompt; nothing else can contribute one. The popup itself is only
   * reachable while the transcript is empty (see `syncAgentMentionFromInput`).
   */
  const agentMentionItems = useMemo(() => {
    if (!agentMentionState.open || !agentMentionOptions) return [];
    const scored: { item: AgentMentionOption; score: number }[] = [];
    for (const option of agentMentionOptions) {
      const score = scoreAgentMention(option, agentMentionState.query);
      if (score === null) continue;
      scored.push({ item: option, score });
    }
    scored.sort(
      (a, b) => a.score - b.score || a.item.name.localeCompare(b.item.name),
    );
    return scored.slice(0, MENTION_MAX_ITEMS).map((entry) => entry.item);
  }, [agentMentionState.open, agentMentionState.query, agentMentionOptions]);

  /** Index stays valid while the filtered list shrinks under the cursor. */
  const agentMentionActiveIndex = Math.min(
    agentMentionIndex,
    Math.max(0, agentMentionItems.length - 1),
  );

  const closeAgentMention = useCallback(() => {
    setAgentMentionState((prev) =>
      prev.open ? { open: false, query: "", start: 0 } : prev,
    );
  }, []);

  /**
   * Load the sub-agent roster for the popup (15 s cache). The roster is
   * app-global, so a workspace switch does not invalidate it; the TTL covers a
   * panel edit made in another window.
   */
  const ensureAgentMentions = useCallback(async () => {
    if (agentMentionPendingRef.current) return;
    const cached = agentMentionRef.current;
    if (cached && Date.now() - cached.at < AGENT_MENTIONS_TTL_MS) return;

    agentMentionPendingRef.current = true;
    setAgentMentionLoading(true);
    try {
      const options = await listAgentMentions();
      agentMentionRef.current = { at: Date.now(), options };
      setAgentMentionOptions(options);
    } catch (err) {
      setError(t("app.agentMention.listFailed", { error: String(err) }));
    } finally {
      agentMentionPendingRef.current = false;
      setAgentMentionLoading(false);
    }
  }, [t]);

  /**
   * Open/close the `+agent` popup for the token under the caret.
   *
   * Only a real user edit reaches this — accepting inserts text
   * programmatically, which must not immediately reopen the popup.
   */
  const syncAgentMentionFromInput = useCallback(
    (value: string, caret: number) => {
      // Embedded roles shape a NEW conversation. Once a turn exists the role
      // snapshot is frozen in the backend, so the popup would lie about being
      // able to change it. Injection mode and profile export disable it too.
      if (streaming || profileExporting || messages.length > 0) {
        closeAgentMention();
        return;
      }
      const active = activeAgentQuery(value, caret);
      if (!active) {
        closeAgentMention();
        return;
      }
      setAgentMentionState((prev) =>
        prev.open && prev.start === active.start && prev.query === active.query
          ? prev
          : { open: true, query: active.query, start: active.start },
      );
      setAgentMentionIndex(0);
      void ensureAgentMentions();
    },
    [closeAgentMention, ensureAgentMentions, messages.length, profileExporting, streaming],
  );

  /**
   * Insert `+<token>` for `item`, replacing the `+` + query that opened the
   * popup, and leave the caret right behind it. A trailing space keeps the
   * line in the `+token +token` grammar, so the next `+` opens the popup again.
   */
  const acceptAgentMention = useCallback(
    (item: AgentMentionOption) => {
      const el = textareaRef.current;
      const token = `+${item.token}`;
      // The whole `+query` run is replaced, not just the part before the
      // caret: the caret can sit inside the token (a click or an arrow key),
      // and replacing only its prefix would glue the remaining letters onto
      // the accepted token.
      const start = agentMentionState.start;
      let end = start + 1;
      while (end < input.length && !/[\s+@/<>#]/.test(input[end])) end += 1;
      const before = input.slice(0, start);
      const after = input.slice(end);
      const suffix = /^\s/.test(after) ? "" : " ";
      setInput(`${before}${token}${suffix}${after}`);
      setEmbeddedAgents((prev) =>
        prev.some((agent) => agent.id === item.id) ? prev : [...prev, item],
      );
      closeAgentMention();
      requestAnimationFrame(() => {
        const position = before.length + token.length + suffix.length;
        el?.focus();
        el?.setSelectionRange(position, position);
      });
    },
    [agentMentionState.start, closeAgentMention, input],
  );

  /**
   * Agent tokens known in this roster. A sent `+token` is only highlighted when
   * it names a real agent, so arithmetic such as `2 + 2` stays plain text.
   */
  const knownAgentTokens = useMemo(
    () => (agentMentionOptions ?? []).map((option) => option.token).sort(),
    [agentMentionOptions],
  );

  // ─── Files pasted / dropped onto the composer ─────────────────────────────

  /**
   * Insert `@.data/tmp/…` mentions for files the backend just wrote.
   *
   * A pasted or dropped file has no attachment channel of its own: it becomes a
   * workspace file in `.data/tmp/` and is then referenced like any other
   * mention, so the model sees both the token and the content. Files the
   * selected model cannot consume are reported instead of being silently
   * attached.
   */
  const insertMentionPaths = useCallback(
    (paths: string[]) => {
      if (paths.length === 0) return;
      const { supported, unsupported } = partitionByModelSupport(paths, selectedMetadata);
      if (unsupported.length > 0) {
        setError(
          t("app.attachmentUnsupported", {
            names: unsupported.join(", "),
            model: selectedModel,
          }),
          "warning",
        );
      }
      if (supported.length === 0) return;

      const el = textareaRef.current;
      const draft = composerDraftRef.current;
      const caret = el?.selectionStart ?? draft.length;
      const { value, caret: next } = insertMentionTokens(
        draft,
        caret,
        supported.map(mentionTokenForPath),
      );
      // Mirror the new draft immediately: a second paste can land before React
      // commits this one, and it must splice into the text it just produced.
      composerDraftRef.current = value;
      setInput(value);
      closeMention();
      closeSlash();
      requestAnimationFrame(() => {
        el?.focus();
        el?.setSelectionRange(next, next);
      });
    },
    [closeMention, closeSlash, selectedMetadata, selectedModel, setError, t],
  );

  /**
   * The `@` popup walks the workspace on a 15 s cache; a paste changes what is
   * on disk, so the cache is dropped here.
   */
  const invalidateWorkspaceFileCache = useCallback(() => {
    workspaceFilesRef.current = null;
    setWorkspaceFiles(null);
  }, []);

  /**
   * Copy files dropped from the OS into `<workspace>/.data/tmp/` and mention
   * them.
   *
   * Tauri hands the drop over as real paths (the webview never sees the bytes),
   * so this is one IPC round trip per drop.
   */
  const importDroppedPaths = useCallback(
    async (paths: string[]) => {
      if (profileExporting) return;
      if (streaming) {
        setError(t("app.composer.busy"), "warning");
        return;
      }
      try {
        const result = await importWorkspacePaths(paths);
        // A drop can be partly refused (a folder, an oversized file): the files
        // that arrived are attached, the rest is reported.
        if (result.problems.length > 0) {
          setError(result.problems.join("\n"), "warning");
        }
        insertMentionPaths(result.imported);
        if (result.imported.length > 0) invalidateWorkspaceFileCache();
      } catch (err) {
        setError(t("app.composer.dropFailed", { error: String(err) }), "warning");
      }
    },
    [profileExporting, streaming, insertMentionPaths, invalidateWorkspaceFileCache, setError, t],
  );

  /**
   * `Ctrl+V` with files in the clipboard.
   *
   * Only a paste that actually carries files is intercepted — text pasting keeps
   * the browser's own undo history and caret behaviour. Each file is written to
   * `.data/tmp/` and then shows up as an `@.data/tmp/…` mention in the draft.
   */
  const handleComposerPaste = useCallback(
    (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
      const data = event.clipboardData;
      if (!data) return;
      const files = Array.from(data.items)
        .filter((item) => item.kind === "file")
        .map((item) => item.getAsFile())
        .filter((file): file is File => file !== null);
      // A plain text paste is none of our business.
      if (files.length === 0) return;
      event.preventDefault();

      if (profileExporting) return;
      if (streaming) {
        // While a turn runs the composer is an injection channel, and injected
        // text carries no attachments: the file would be saved but never sent.
        setError(t("app.composer.busy"), "warning");
        return;
      }

      void (async () => {
        const { ready, problems } = await preparePastedFiles(files, (kind, name, error) =>
          kind === "tooLarge"
            ? t("app.composer.tooLarge", { name })
            : kind === "empty"
              ? t("app.composer.empty", { name })
              : t("app.composer.unreadable", { name, error: error ?? "" }),
        );
        if (problems.length > 0) setError(problems.join("\n"), "warning");

        const saved: string[] = [];
        const failures: string[] = [];
        for (const item of ready) {
          try {
            saved.push(await savePastedFile(item.name, item.dataUrl));
          } catch (err) {
            failures.push(t("app.composer.saveFailed", { name: item.name, error: String(err) }));
          }
        }
        if (failures.length > 0) setError(failures.join("\n"), "warning");
        insertMentionPaths(saved);
        if (saved.length > 0) invalidateWorkspaceFileCache();
      })();
    },
    [
      profileExporting,
      streaming,
      insertMentionPaths,
      invalidateWorkspaceFileCache,
      setError,
      t,
    ],
  );

  // Native drag-drop. Tauri intercepts HTML5 drops (`dragDropEnabled`), so the
  // only way to learn about a file dragged in from Explorer is this event; it
  // also means the overlay must be driven from here rather than from `dragover`.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setDropActive(true);
          return;
        }
        setDropActive(false);
        if (payload.type === "leave") return;
        if (payload.paths.length > 0) void importDroppedPaths(payload.paths);
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((err) => console.error("Failed to watch drag-drop", err));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [importDroppedPaths]);

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
              onGlassPreview={(glass) => {
                applyGlassEffect(glass);
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
                setSelfEvolutionMode(cfg.self_evolution_mode ?? false);
                themeRef.current = cfg.theme ?? "auto";
                paletteRef.current = cfg.palette ?? "default";
                applyTheme(cfg.theme, cfg.palette);
                applyGlassEffect(cfg.glass_effect);
                if (isLocale(cfg.language)) setLocale(cfg.language);
              }}
            />
          </div>
        </Portal>
      )}
      {/* Sidebar */}
      <AnimatePresence>
      {sidebar && (
        <motion.aside
          key="sidebar"
          className="sidebar"
          initial={reducedMotion ? false : SIDEBAR_HIDDEN}
          animate={SIDEBAR_SHOWN}
          exit={SIDEBAR_HIDDEN}
          transition={reducedMotion ? { duration: 0 } : SIDEBAR_SPRING}
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
                  resetEmbeddedAgents();

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
        </motion.aside>
      )}
      </AnimatePresence>

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
              <div
                className={`rabbit-hop${selfEvolutionMode && dreamState === "running" ? " rabbit-hop--sleeping" : ""}`}
                aria-hidden={selfEvolutionMode ? undefined : true}
              >
                {/* Visual affordance only: the button's aria-label carries the
                    same sentence for assistive technology. Both the hint and
                    the button disappear when self-evolution mode is off — the
                    face is then a plain decorative div with no hover/click. */}
                {selfEvolutionMode && (
                  <span className="rabbit-hop-hint" aria-hidden="true">
                    {dreamState === "running"
                      ? `${t("app.dream.sleeping")} · ${t("app.dream.cancelHint")}`
                      : t("app.dream.hint")}
                  </span>
                )}
                {selfEvolutionMode && dreamState === "running" && (
                  <>
                    <div className="rabbit-hop-bubble" aria-hidden="true">
                      <div className="rabbit-hop-bubble-clip">
                        <div className="rabbit-hop-dream-track">
                          {[...dreamGlyphs, ...dreamGlyphs].map((glyph, index) => (
                            <span key={index}>{glyph}</span>
                          ))}
                        </div>
                      </div>
                    </div>
                    <div className="rabbit-hop-zzz" aria-hidden="true">
                      <span>z</span>
                      <span>z</span>
                      <span>z</span>
                    </div>
                  </>
                )}
                <div className="rabbit-hop-runner">
                  {selfEvolutionMode ? (
                    <button
                      type="button"
                      className="rabbit-hop-bunny"
                      aria-label={
                        dreamState === "running"
                          ? t("app.dream.ariaRunning")
                          : t("app.dream.ariaStart")
                      }
                      aria-busy={dreamState === "running"}
                      onClick={handleDreamClick}
                    >
                      {DREAM_FACE}
                    </button>
                  ) : (
                    <div className="rabbit-hop-bunny">{DREAM_FACE}</div>
                  )}
                </div>
              </div>
              {dreamState === "running" ? (
                <div className="empty-dream-status" role="status" aria-live="polite">
                  <p className="empty-dream-title">{t("app.dream.statusTitle")}</p>
                  <p
                    className="empty-hint"
                    title={t(DREAM_PHASE_KEYS[dreamProgress?.phase ?? "inventory"])}
                  >
                    {t("app.dream.statusDetail", {
                      round: dreamProgress?.round ?? 1,
                      tools: dreamProgress?.toolCalls ?? 0,
                    })}
                  </p>
                </div>
              ) : dreamResult && selfEvolutionMode ? (
                <div className="empty-dream-result">
                  <button
                    type="button"
                    className="empty-dream-result-bubble"
                    onClick={dismissDreamResult}
                  >
                    <span className="empty-dream-result-title">
                      {t("app.dream.resultTitle")}
                    </span>
                    <span className="empty-dream-result-body">{dreamResult}</span>
                    <span className="empty-dream-result-dismiss">
                      {t("app.dream.resultDismiss")}
                    </span>
                  </button>
                </div>
              ) : (
                <>
                  <p>{t("app.emptyStart")}</p>
                  <p className="empty-hint">
                    {t("app.emptyHint", {
                      skills: t("app.toolbarSkills"),
                      settings: t("app.toolbarSettings"),
                    })}
                  </p>
                </>
              )}
            </div>
          )}
          {messages.map((m) =>
            m.role === "tool_group" ? (
              <ToolCallGroup key={m.id} message={m} onConfirm={respondToolCallConfirm} onCancelBackground={handleCancelBackgroundTask} />
            ) : (
              // One row failing to render must not blank the transcript, so
              // every message gets its own boundary.
              <ChatMessageBoundary key={m.id}>
                <ChatMessage
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
                  agentTokens={knownAgentTokens}
                />
              </ChatMessageBoundary>
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
          {/* Dragging a file in from the OS: say what is about to happen. The
              drop itself is handled by the native event (see the effect above),
              so this layer never needs pointer events. */}
          {dropActive && (
            <div className="composer-drop" role="status" aria-live="polite">
              <FontAwesomeIcon icon={faPaperclip} aria-hidden="true" />
              <span>{t("app.composer.dropHint")}</span>
            </div>
          )}
          <div className="input-row">
            <AgentMentionPopup
              open={agentMentionState.open}
              items={agentMentionItems}
              activeIndex={agentMentionActiveIndex}
              loading={agentMentionLoading}
              query={agentMentionState.query}
              onHover={setAgentMentionIndex}
              onSelect={acceptAgentMention}
            />
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
                const caret = e.target.selectionStart ?? value.length;
                syncMentionFromInput(value, caret);
                syncSlashFromInput(value, caret);
                syncAgentMentionFromInput(value, caret);
              }}
              onKeyDown={handleKeyDown}
              onPaste={handleComposerPaste}
              // A real blur (clicking elsewhere) dismisses the popup; the popup
              // itself accepts on `mousedown` + `preventDefault`, so choosing a
              // row never blurs the textarea.
              onBlur={() => {
                closeMention();
                closeSlash();
                closeAgentMention();
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
