import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { chatCompletion, checkUpdate, getConfig, getAgentOrchestration, listMcpServers, listSubAgents, saveConfig, saveHistory, stopChatCompletion, confirmCommand, saveMarkdownFile, deleteMessage, forkSession, filterExistingSkills, listTimers, cancelTimer } from "./api";

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
import { UpdatePanel } from "./components/UpdatePanel";
import type {
  AppConfig,
  Message,
  AgentTaskEvent,
  ToolCallEntry,
  Attachment,
  ContentPart,
  MessageContent,
  ModelMetadata,
  TimerEntry,
  UpdateInfo,
} from "./types";
import { isGenerationStopped, isLocale, useI18n } from "./i18n";
import "./App.css";

type Sidebar = "settings" | "skills" | "history" | "tools" | "mcp" | "agents" | null;

function applyTheme(theme: "auto" | "light" | "dark" | undefined) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const resolved = theme === "dark" || (theme !== "light" && prefersDark) ? "dark" : "light";
  document.documentElement.setAttribute("data-theme", resolved);
}

// ─── Delay timers ────────────────────────────────────────────────────────────

/**
 * Countdown label for the timer bar: `mm:ss`, or `h:mm:ss` past an hour.
 * Clamped at zero; the row disappears as soon as the timer fires.
 */
function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const seconds = total % 60;
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  const pad = (value: number) => String(value).padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${pad(minutes)}:${pad(seconds)}`;
}

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

/**
 * Which extensions the picker offers depends on the selected model's
 * capabilities (see `acceptedFileTypes`):
 * - text is always accepted — it is inlined into the prompt, not sent as a
 *   content part, so any model can consume it;
 * - images become `image_url` parts → need `supports_vision`;
 * - documents become `file` parts → need `supports_files`;
 * - audio becomes an `input_audio` part → needs `supports_audio`, and only the
 *   two formats the Chat Completions schema allows (`wav`, `mp3`).
 */
const TEXT_EXTENSIONS = [
  "txt", "md", "markdown", "csv", "tsv", "json", "jsonl",
  "yaml", "yml", "xml", "html", "htm", "css", "js", "ts", "jsx", "tsx",
  "py", "rs", "go", "java", "c", "cpp", "h", "hpp", "cs", "rb", "php",
  "sh", "bat", "ps1", "sql", "log", "ini", "cfg", "toml", "env",
  "diff", "patch", "tex", "rst", "adoc", "org", "r", "m", "scala",
  "swift", "kt", "dart", "lua", "pl", "ex", "exs", "clj", "hs", "ml",
  "fs", "erl", "vim", "conf", "v",
];
const IMAGE_EXTENSIONS = [
  "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "heic", "heif", "ico", "tif", "tiff",
];
const FILE_EXTENSIONS = ["pdf"];
const AUDIO_EXTENSIONS = ["mp3", "wav"];

const IMAGE_MIME_PREFIXES = ["image/"];
const AUDIO_MIME_PREFIXES = ["audio/"];
const TEXT_MIME_PREFIXES = ["text/"];

const FILE_TYPE_EXT_OVERRIDES: Record<string, string> = {
  pdf: "application/pdf",
};

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
}

function classifyFile(file: File): Attachment["kind"] {
  const mime = file.type.toLowerCase();
  if (mime && IMAGE_MIME_PREFIXES.some((p) => mime.startsWith(p))) return "image";
  if (mime && AUDIO_MIME_PREFIXES.some((p) => mime.startsWith(p))) return "audio";
  if (mime && TEXT_MIME_PREFIXES.some((p) => mime.startsWith(p))) return "text";
  if (mime && mime !== "application/octet-stream") return "file";
  const ext = extensionOf(file.name);
  if (!ext) return "file";
  if (IMAGE_EXTENSIONS.includes(ext)) return "image";
  if (AUDIO_EXTENSIONS.includes(ext)) return "audio";
  if (TEXT_EXTENSIONS.includes(ext)) return "text";
  return "file";
}

/**
 * Can the model behind `metadata` consume this kind of attachment?
 *
 * Unknown metadata (the catalogue was never fetched, or the id was not matched)
 * stays permissive: blocking files a model might well accept is worse than
 * letting the provider answer with its own error.
 */
function supportsAttachmentKind(metadata: ModelMetadata | undefined, kind: Attachment["kind"]): boolean {
  if (!metadata) return true;
  switch (kind) {
    case "text":
      return true;
    case "image":
      return metadata.supports_vision === true;
    case "audio":
      return metadata.supports_audio === true;
    case "file":
      return metadata.supports_files === true;
  }
}

/** `accept` attribute for the file picker, narrowed to the model's abilities. */
function acceptedFileTypes(metadata: ModelMetadata | undefined): string {
  const extensions = new Set(TEXT_EXTENSIONS);
  const allow = (kind: Attachment["kind"], list: string[]) => {
    if (supportsAttachmentKind(metadata, kind)) list.forEach((ext) => extensions.add(ext));
  };
  allow("image", IMAGE_EXTENSIONS);
  allow("file", FILE_EXTENSIONS);
  allow("audio", AUDIO_EXTENSIONS);
  return Array.from(extensions, (ext) => `.${ext}`).join(",");
}

function mimeFor(file: File): string {
  const mime = file.type.toLowerCase();
  if (mime && mime !== "application/octet-stream") return mime;
  const ext = extensionOf(file.name);
  if (!ext) return "application/octet-stream";
  if (ext === "jpg" || ext === "jpeg") return "image/jpeg";
  if (FILE_TYPE_EXT_OVERRIDES[ext]) return FILE_TYPE_EXT_OVERRIDES[ext];
  return `application/${ext}`;
}

/** Strip the `data:<mime>;base64,` prefix — `input_audio` wants the payload. */
function dataUrlBase64(dataUrl: string): string {
  const comma = dataUrl.indexOf(",");
  return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("Failed to read file"));
    reader.onload = () => {
      const result = reader.result;
      if (typeof result === "string") resolve(result);
      else reject(new Error("Unexpected reader result type"));
    };
    reader.readAsDataURL(file);
  });
}

async function readAttachment(file: File): Promise<Attachment> {
  const kind = classifyFile(file);
  const mime = mimeFor(file);
  if (kind === "text") {
    const text_content = await file.text();
    return { name: file.name, kind, mime, text_content };
  }
  const data_url = await readFileAsDataUrl(file);
  return { name: file.name, kind, mime, data_url };
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
      const ext = extensionOf(att.name);
      const lang = ext || "";
      parts.push({
        type: "text",
        text: `\n<details><summary>Attached File: ${att.name}</summary>\n\n\`\`\`${lang}\n${att.text_content}\n\`\`\`\n</details>`,
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

interface AgentStatus {
  status: "idle" | "running" | "done" | "error";
  description?: string;
  summary?: string;
  error?: string;
  tokens?: number;
}

interface MarkdownEditPayload {
  path: string;
  content: string;
}

export default function App() {
  const { t, setLocale } = useI18n();
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
  const [sidebarMotion, setSidebarMotion] = useState<"opening" | "closing" | null>(null);
  const [activeSkillIds, setActiveSkillIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
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
  } | null>(null);
  const [maxTokens, setMaxTokens] = useState<number | null>(null);
  const [availableModels, setAvailableModels] = useState<string[]>(["gpt-4o-mini"]);
  const [selectedModel, setSelectedModel] = useState("gpt-4o-mini");
  /** Capabilities per model, learned from the llm-metadata catalogue. */
  const [modelMetadata, setModelMetadata] = useState<Record<string, ModelMetadata>>({});
  /** Thinking depth per model (`reasoning_effort`); absent = provider default. */
  const [reasoningEfforts, setReasoningEfforts] = useState<Record<string, string>>({});
  const [activeToolCount, setActiveToolCount] = useState(0);
  const [activeMcpCount, setActiveMcpCount] = useState(0);
  const [activeAgentCount, setActiveAgentCount] = useState(0);
  const [useAgentsEnabled, setUseAgentsEnabled] = useState(false);
  const [agentStatuses, setAgentStatuses] = useState<Record<string, AgentStatus>>({});
  const [skillsLoadedFromConfig, setSkillsLoadedFromConfig] = useState(false);
  const [confirmDialog, setConfirmDialog] = useState<{
    request_id: string;
    reason: string;
    cmd_type: string;
    code: string;
    confirm_kind?: "dangerous" | "sudo" | "elevation" | "external_path" | "system_config" | "user_software" | "user_data" | "sensitive_read" | "general_query";
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
  /** 1 s heartbeat that drives the countdowns in the timer bar. */
  const [timerTick, setTimerTick] = useState(() => Date.now());
  /** Timer that fired for a session the user is not currently in. */
  const [timerNotice, setTimerNotice] = useState<TimerEntry | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const sidebarMotionTimerRef = useRef<number | null>(null);
  const themeRef = useRef<"auto" | "light" | "dark">("auto");
  const currentAssistantMessageIdRef = useRef<string | null>(null);
  const hasRunningToolCallRef = useRef(false);
  const currentToolCallsRef = useRef<ToolCallEntry[]>([]);
  /** Resume prompts from fired timers, waiting for the chat to become idle. */
  const [pendingTimerPrompts, setPendingTimerPrompts] = useState<string[]>([]);
  /** Latest active session for the timer listener (registered once). */
  const sessionIdRef = useRef(sessionId);

  /**
   * Drop `selected_skills` entries that no longer resolve to a real skill.
   *
   * `selected_skills` is persisted in the config, so it goes stale whenever the
   * workspace directory changes or a different profile is applied — the skill a
   * name referred to may live in the previous workspace and no longer exist.
   */
  const reconcileActiveSkills = useCallback(async (ids: string[]): Promise<string[]> => {
    if (ids.length === 0) return ids;
    try {
      const existing = await filterExistingSkills(ids);
      if (existing.length === ids.length) return ids;
      const existingSet = new Set(existing);
      // Preserve the original selection order.
      return ids.filter((id) => existingSet.has(id));
    } catch (err) {
      // If validation itself fails (IPC error), keep the selection rather than
      // silently discarding the user's active skills.
      console.error("Failed to validate active skills", err);
      return ids;
    }
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
    setActiveSkillIds(await reconcileActiveSkills(cfg.selected_skills ?? []));
    setActiveToolCount((cfg.selected_tools ?? []).length);
    // The config is the source of truth for the UI language (localStorage is
    // only a first-paint cache), so a profile import can switch it too.
    if (isLocale(cfg.language)) setLocale(cfg.language);
    return cfg;
  }, [reconcileActiveSkills, setLocale]);

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

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  useEffect(() => {
    return () => {
      if (sidebarMotionTimerRef.current !== null) {
        window.clearTimeout(sidebarMotionTimerRef.current);
      }
    };
  }, []);

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
    }>("chat-usage", (e) => {
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

  // Switching workspaces changes which skills resolve. Reload the config and
  // re-validate `selected_skills` so stale names (e.g. skills that only existed
  // in the previous workspace) are dropped instead of being sent to the model.
  useEffect(() => {
    const unlisten = listen<string>("workspace-changed", async () => {
      try {
        await applyConfigToUi();
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
      confirm_kind?: "dangerous" | "sudo" | "elevation" | "external_path" | "system_config" | "user_software" | "user_data" | "sensitive_read" | "general_query";
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
  }, []);

  const respondToConfirm = useCallback((confirmed: boolean) => {
    const requiresSudo = confirmDialog?.requires_auth === "sudo";
    confirmCommand(confirmDialog?.request_id ?? "", confirmed, requiresSudo ? { username: confirmUsername, password: confirmPassword } : undefined).catch(console.error);
    setConfirmDialog(null);
    setConfirmUsername("");
    setConfirmPassword("");
    setRiskAssessment(null);
  }, [confirmDialog, confirmUsername, confirmPassword]);

  useEffect(() => {
    applyConfigToUi()
      .then((cfg) => {
        // Only start persisting `selected_skills` once the restored list has
        // been validated against the skills that actually exist.
        setSkillsLoadedFromConfig(true);
        themeRef.current = cfg.theme ?? "auto";
        applyTheme(cfg.theme);
      })
      .catch(console.error);
  }, [applyConfigToUi]);

  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => { if (themeRef.current === "auto") applyTheme("auto"); };
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

  useEffect(() => {
    if (!skillsLoadedFromConfig) return;

    getConfig()
      .then((cfg) => saveConfig({ ...cfg, selected_skills: activeSkillIds }))
      .catch(console.error);
  }, [activeSkillIds, skillsLoadedFromConfig]);

  // Agent task progress tracking
  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      listen<AgentTaskEvent>("agent-task-start", (e) => {
        const { agent_id, agent_name, description, task_id } = e.payload;
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "running", description },
        }));
        const entry: ToolCallEntry = {
          task_id,
          agent_name,
          description: description ?? "",
          status: "running",
        };
        currentToolCallsRef.current = [...currentToolCallsRef.current, entry];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen<AgentTaskEvent>("agent-task-done", (e) => {
        const { agent_id, summary, task_id } = e.payload;
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "done", summary: summary ?? "" },
        }));
        currentToolCallsRef.current = currentToolCallsRef.current.map((entry) =>
          entry.task_id === task_id
            ? { ...entry, status: "done" as const, summary: summary ?? undefined }
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
            ? { ...entry, status: "error" as const, error: error ?? undefined }
            : entry
        );
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
      }),
      listen("agent-plan-start", (e: { payload: { task_count: number } }) => {
        if (e.payload.task_count > 0) {
          const entry: ToolCallEntry = {
            task_id: `plan-${Date.now()}`,
            agent_name: t("app.agents.planner"),
            description: t("app.agents.planDone", { count: e.payload.task_count }),
            status: "done",
          };
          currentToolCallsRef.current = [...currentToolCallsRef.current, entry];
          updateActiveAssistantToolCalls(currentToolCallsRef.current);
        }
      }),
      listen("agent-aggregate-start", () => {
        const entry: ToolCallEntry = {
          task_id: `aggregate-${Date.now()}`,
          agent_name: t("app.agents.aggregator"),
          description: t("app.agents.aggregating"),
          status: "running",
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
          description: label,
          status: "running",
          summary: detail,
        };

        currentToolCallsRef.current = [
          ...currentToolCallsRef.current.map((tc) =>
            tc.status === "running" ? { ...tc, status: "done" as const } : tc
          ),
          entry,
        ];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
        hasRunningToolCallRef.current = true;
      }),
      listen<string>("skill-loaded", (e) => {
        // Structured skill-load event. The entry carries `skill_name`, which
        // is what later turns send back as `loaded_skills`.
        const skillName = e.payload;
        if (!skillName) return;
        const entry: ToolCallEntry = {
          task_id: crypto.randomUUID(),
          agent_name: t("app.agents.tool"),
          description: t("app.agents.loadingSkill", { name: skillName }),
          status: "running",
          skill_name: skillName,
        };
        currentToolCallsRef.current = [
          ...currentToolCallsRef.current.map((tc) =>
            tc.status === "running" ? { ...tc, status: "done" as const } : tc
          ),
          entry,
        ];
        updateActiveAssistantToolCalls(currentToolCallsRef.current);
        hasRunningToolCallRef.current = true;
      }),
    );
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, [updateActiveAssistantToolCalls, t]);

  // The timer listener is registered once, so it reads the active session from
  // a ref instead of re-subscribing whenever the user switches chats.
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  // Delay timers: initial list, live updates, and the resume hook.
  useEffect(() => {
    listTimers()
      .then(setTimers)
      .catch((err) => console.error("Failed to load timers", err));

    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      listen<TimerEntry[]>("timer-state", (e) => setTimers(e.payload ?? [])),
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
        setPendingTimerPrompts((prev) => [...prev, timerPromptText(timer)]);
      }),
    );
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  // Tick the countdowns — only while something is actually pending.
  useEffect(() => {
    if (timers.length === 0) return;
    const id = window.setInterval(() => setTimerTick(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [timers.length]);

  const handleCancelTimer = useCallback(async (id: string) => {
    try {
      await cancelTimer(id);
      setTimers((prev) => prev.filter((timer) => timer.id !== id));
    } catch (err) {
      setError(String(err));
    }
  }, []);

  const sendMessage = useCallback(async (overrideText?: string) => {
    if (profileExporting) return;

    // `overrideText` is the timer resume path: it bypasses the input box and
    // never picks up the attachments the user is still composing.
    const autoResume = overrideText !== undefined;
    const activeAttachments = autoResume ? [] : attachments;
    const rawText = (overrideText ?? input).trim();
    if ((!rawText && activeAttachments.length === 0) || streaming) return;

    const { content: apiContent } = buildMessageContent(rawText, activeAttachments);
    const userAttachments = activeAttachments.length > 0 ? activeAttachments : undefined;
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
    hasRunningToolCallRef.current = false;

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    // An auto-resume must not discard a draft or attachments the user is
    // still composing in the input box.
    if (!autoResume) {
      setInput("");
      setAttachments([]);
    }
    setStreaming(true);
    setError(null);

    const history = [...messages, userMsg]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-") && m.role !== "tool_group")
      .map((m) => ({
        role: m.role,
        content: m.content,
        reasoning_content: m.reasoning_content,
        loaded_skills: loadedSkillsOfMessage(m),
      }));

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
        setMessages((prev) =>
          prev.map((m) => {
            if (m.id !== assistantId) return m;

            const nextToolCalls = shouldMarkDone
              ? (m.tool_calls ?? []).map((e) =>
                e.status === "running" ? { ...e, status: "done" as const } : e
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
        saveHistory(sessionId, "assistant", accumulatedContent, currentToolCallsRef.current.length > 0 ? JSON.stringify(currentToolCallsRef.current) : undefined, accumulatedReasoning || undefined)
          .then((dbId) => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, dbId } : m))
            );
          })
          .catch(console.error);
        setMessages((prev) => {
          const finalToolCalls = currentToolCallsRef.current.map((e) =>
            e.status === "running" ? { ...e, status: "done" as const } : e
          );
          return prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => {
              if (m.id === assistantId) return { ...m, streaming: false, ...(finalToolCalls?.length ? { tool_calls: finalToolCalls } : {}) };
              return m;
            });
        });
        setAgentStatuses({});
        hasRunningToolCallRef.current = false;
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        setStreaming(false);
        cleanupRef.current = null;
      },
      onError(err) {
        setError(err);
        setMessages((prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) =>
              m.id === assistantId
                ? { ...m, content: m.content || t("app.errorPrefix") + err, streaming: false }
                : m
            )
        );
        setPendingRetryMessageId(userMsg.id);
        setAgentStatuses({});
        hasRunningToolCallRef.current = false;
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        setStreaming(false);
        cleanupRef.current = null;
      },
    }, useAgentsEnabled);

    cleanupRef.current = cleanup;
  }, [input, messages, streaming, profileExporting, activeSkillIds, sessionId, attachments, selectedModel, useAgentsEnabled, t]);

  // Continue the conversation from a fired timer once the chat is idle. The
  // prompt goes out as a new user turn, so the assistant resumes the task with
  // the full history. Sending one prompt at a time keeps the turn order intact
  // when several timers fire together.
  useEffect(() => {
    if (streaming || profileExporting || pendingTimerPrompts.length === 0) return;
    const [next, ...rest] = pendingTimerPrompts;
    setPendingTimerPrompts(rest);
    void sendMessage(next);
  }, [pendingTimerPrompts, streaming, profileExporting, sendMessage]);

  const retryPendingUserMessage = useCallback(async () => {
    if (streaming || !pendingRetryMessageId) return;

    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };
    currentAssistantMessageIdRef.current = assistantId;
    currentToolCallsRef.current = [];
    hasRunningToolCallRef.current = false;

    setMessages((prev) => [...prev, assistantMsg]);
    setStreaming(true);
    setError(null);

    const history = [...messages]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-") && m.role !== "tool_group")
      .map((m) => ({
        role: m.role,
        content: m.content,
        reasoning_content: m.reasoning_content,
        loaded_skills: loadedSkillsOfMessage(m),
      }));

    let accumulatedContent = "";
    let accumulatedReasoning = "";

    const cleanup = await chatCompletion(history, activeSkillIds, sessionId, selectedModel, {
      onToken(token) {
        accumulatedContent += token;
        const shouldMarkDone = hasRunningToolCallRef.current;
        if (shouldMarkDone) hasRunningToolCallRef.current = false;
        setMessages((prev) =>
          prev.map((m) => {
            if (m.id !== assistantId) return m;

            const nextToolCalls = shouldMarkDone
              ? (m.tool_calls ?? []).map((e) =>
                e.status === "running" ? { ...e, status: "done" as const } : e
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
        saveHistory(sessionId, "assistant", accumulatedContent, currentToolCallsRef.current.length > 0 ? JSON.stringify(currentToolCallsRef.current) : undefined, accumulatedReasoning || undefined)
          .then((dbId) => {
            setMessages((prev) =>
              prev.map((m) => (m.id === assistantId ? { ...m, dbId } : m))
            );
          })
          .catch(console.error);
        setMessages((prev) => {
          const finalToolCalls = currentToolCallsRef.current.map((e) =>
            e.status === "running" ? { ...e, status: "done" as const } : e
          );
          return prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => {
              if (m.id === assistantId) return { ...m, streaming: false, ...(finalToolCalls?.length ? { tool_calls: finalToolCalls } : {}) };
              return m;
            });
        });
        setPendingRetryMessageId(null);
        setAgentStatuses({});
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        setStreaming(false);
        cleanupRef.current = null;
      },
      onError(err) {
        setError(err);
        setMessages((prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) =>
              m.id === assistantId
                ? { ...m, content: m.content || t("app.errorPrefix") + err, streaming: false }
                : m
            )
        );
        setAgentStatuses({});
        hasRunningToolCallRef.current = false;
        currentAssistantMessageIdRef.current = null;
        currentToolCallsRef.current = [];
        setStreaming(false);
        cleanupRef.current = null;
      },
    }, useAgentsEnabled);

    cleanupRef.current = cleanup;
  }, [messages, streaming, pendingRetryMessageId, activeSkillIds, sessionId, selectedModel, useAgentsEnabled, t]);

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

    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
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
    if (sidebar === panel) {
      closeSidebar();
      return;
    }

    if (sidebarMotionTimerRef.current !== null) {
      window.clearTimeout(sidebarMotionTimerRef.current);
      sidebarMotionTimerRef.current = null;
    }

    setSidebar(panel);
    setSidebarMotion("opening");
    sidebarMotionTimerRef.current = window.setTimeout(() => {
      setSidebarMotion(null);
      sidebarMotionTimerRef.current = null;
    }, 180);
  }

  function closeSidebar() {
    if (!sidebar) return;

    if (sidebarMotionTimerRef.current !== null) {
      window.clearTimeout(sidebarMotionTimerRef.current);
    }

    setSidebarMotion("closing");
    sidebarMotionTimerRef.current = window.setTimeout(() => {
      setSidebar(null);
      setSidebarMotion(null);
      sidebarMotionTimerRef.current = null;
    }, 180);
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
    setPendingRetryMessageId(null);
    setSessionId(crypto.randomUUID());
  }

  const renderConfirmDialog = () => {
    if (!confirmDialog) {
      return null;
    }
    const requiresSudo = confirmDialog.requires_auth === "sudo";
    const requiresElevation = confirmDialog.requires_auth === "elevation";
    const isExternalPath = confirmDialog.confirm_kind === "external_path";
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
        : isExternalPath
          ? t("app.confirm.titleExternalPath")
          : hasRisk
            ? t("app.confirm.titleRisk", { level: riskLevel ?? "" })
            : t("app.confirm.titleDangerous");
    const badge = requiresSudo
      ? t("app.confirm.badgeSudo")
      : requiresElevation
        ? t("app.confirm.badgeAdmin")
        : isExternalPath
          ? t("app.confirm.badgePath")
          : hasRisk
            ? riskLevel ?? t("app.confirm.badgeRisk")
            : t("app.confirm.badgeDangerous");
    const preview =
      confirmDialog.code.length > 400
        ? confirmDialog.code.slice(0, 400) + "…"
        : confirmDialog.code;

    return (
      <div className="confirm-overlay">
        <div className="confirm-dialog">
          <h2>
            {title} <span className="confirm-dialog-badge">{badge}</span>
          </h2>
          <p>
            <strong>{t("app.confirm.reason")}</strong> {confirmDialog.reason}
          </p>
          <p>
            <strong>{t("app.confirm.type")}</strong> {confirmDialog.cmd_type}
          </p>
          {hasRisk && (
            <div className="confirm-dialog-risk">
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
          {requiresSudo && (
            <div className="confirm-dialog-credentials">
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
              <p className="confirm-dialog-hint">
                {t("app.confirm.sudoHint")}
              </p>
            </div>
          )}
          {requiresElevation && (
            <p className="confirm-dialog-hint">
              {t("app.confirm.elevationHint")}
            </p>
          )}
          {isExternalPath && (
            <p className="confirm-dialog-hint">
              {t("app.confirm.externalPathHint")}
            </p>
          )}
          <div className="confirm-dialog-preview">
            <pre>{preview}</pre>
          </div>
          <div className="confirm-dialog-buttons">
            <button
              className="confirm-dialog-button cancel"
              onClick={() => respondToConfirm(false)}
            >
              {t("app.confirm.deny")}
            </button>
            <button
              className="confirm-dialog-button confirm"
              onClick={() => respondToConfirm(true)}
              disabled={requiresSudo && confirmPassword.trim().length === 0}
            >
              {t("app.confirm.allow")}
            </button>
          </div>
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

    setMessages((prev) =>
      prev.map((m) => {
        if (!m.streaming) return m;
        const stopSuffix = "\n\n" + t("app.generationStopped");
        // `content` may be a string OR a multimodal array — only mutate
        // string content (assistant streamed text). Leave other shapes
        // untouched.
        if (typeof m.content !== "string") {
          return { ...m, streaming: false };
        }
        const content = isGenerationStopped(m.content)
          ? m.content
          : (m.content || "") + stopSuffix;
        return { ...m, content, streaming: false };
      })
    );
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

  /** Capabilities of the model currently selected in the toolbar. */
  const selectedMetadata = modelMetadata[selectedModel];
  /** The thinking-depth control only makes sense for reasoning-capable models. */
  const reasoningEffort = reasoningEfforts[selectedModel] ?? "";
  // Shown for catalogue-matched reasoning models, and kept for any model that
  // already has a depth configured (e.g. an unmatched model set by hand).
  const reasoningSupported = selectedMetadata?.supports_reasoning === true || reasoningEffort !== "";

  // Switching to a model that cannot consume an attached file type would only
  // fail upstream, so drop those attachments and say which ones went away.
  useEffect(() => {
    const dropped = attachments.filter((a) => !supportsAttachmentKind(selectedMetadata, a.kind));
    if (dropped.length === 0) return;

    setAttachments((prev) => prev.filter((a) => supportsAttachmentKind(selectedMetadata, a.kind)));
    setError(t("app.attachmentUnsupported", {
      names: dropped.map((a) => a.name).join(", "),
      model: selectedModel,
    }));
  }, [attachments, selectedMetadata, selectedModel, t]);

  const usageTotal = usage ? (usage.total_tokens ?? usage.prompt_tokens + usage.completion_tokens) : 0;
  const fallbackMaxTokens = 131072; // 128k tokens as a hard upper bound for usage ratio calculations when no explicit max is provided
  const usageMax = usage?.max_tokens ?? maxTokens ?? fallbackMaxTokens;
  const usageRatio = usage
    ? (usage.usage_ratio ?? (usageMax > 0 ? usageTotal / usageMax : 0))
    : 0;
  const usagePercent = Math.max(0, Math.min(100, Math.round(usageRatio * 100)));

  return (
    <div className="app-layout">
      {renderConfirmDialog()}
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
                applyTheme(theme);
              }}
              onConfigSaved={(cfg) => {
                const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
                setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
                setSelectedModel(cfg.model || "gpt-4o-mini");
                setMaxTokens(cfg.model_context_lengths?.[cfg.model] ?? cfg.model_settings?.max_tokens ?? null);
                // A "Fetch Models From API" in Settings refreshes the capability
                // metadata (and with it the badges in the input row).
                setModelMetadata(cfg.model_metadata ?? {});
                setReasoningEfforts(cfg.model_reasoning_effort ?? {});
                themeRef.current = cfg.theme ?? "auto";
                applyTheme(cfg.theme);
                if (isLocale(cfg.language)) setLocale(cfg.language);
              }}
            />
          </div>
        </Portal>
      )}
      {/* Sidebar */}
      {sidebar && (
        <aside className={`sidebar ${sidebarMotion === "opening" ? "sidebar-opening" : ""} ${sidebarMotion === "closing" ? "sidebar-closing" : ""}`}>
          <div className={`sidebar-shell ${sidebar === "settings" ? "panel-settings" : ""} ${sidebar === "skills" ? "panel-skills" : ""} ${sidebar === "history" ? "panel-history" : ""} ${sidebar === "tools" ? "panel-tools" : ""} ${sidebar === "mcp" ? "panel-mcp" : ""} ${sidebar === "agents" ? "panel-agents" : ""}`} key={sidebar}>
            {sidebar === "skills" && (
              <SkillsPanel
                activeSkillIds={activeSkillIds}
                onToggle={(name, active) => {
                  setActiveSkillIds((prev) =>
                    active ? [...prev, name] : prev.filter((id) => id !== name)
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
                disableSessionSwitch={streaming}
                onLoad={(sid, msgs) => {
                  if (streaming) {
                    setError(t("app.sessionSwitchBlocked"));
                    return;
                  }
                  if (cleanupRef.current) { cleanupRef.current(); cleanupRef.current = null; }
                  setStreaming(false);
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
        <header className="toolbar">
          <span className="app-title">                      <button
            className="toolbar-btn"
            onClick={clearChat}
            title={t("app.newChatTitle")}
          >
            ↻ {t("app.newChat")}
          </button></span>

          <div className="toolbar-actions">
            <button
              className={`toolbar-btn ${sidebar === "agents" ? "active" : ""}`}
              onClick={() => toggleSidebar("agents")}
              title={useAgentsEnabled ? t("app.toolbarAgentsTitleEnabled") : t("app.toolbarAgentsTitle")}
            >
              🤖 {t("app.toolbarAgents")}
              {activeAgentCount > 0 && useAgentsEnabled && (
                <span className="toolbar-btn-count">{activeAgentCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "skills" ? "active" : ""}`}
              onClick={() => toggleSidebar("skills")}
              title={t("app.toolbarSkills")}
            >
              ✦ {t("app.toolbarSkills")}
              {activeSkillIds.length > 0 && (
                <span className="toolbar-btn-count">{activeSkillIds.length}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "mcp" ? "active" : ""}`}
              onClick={() => toggleSidebar("mcp")}
              title={t("app.toolbarMcpTitle")}
            >
              ⬡ {t("app.toolbarMcp")}
              {activeMcpCount > 0 && (
                <span className="toolbar-btn-count">{activeMcpCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "tools" ? "active" : ""}`}
              onClick={() => toggleSidebar("tools")}
              title={t("app.toolbarTools")}
            >
              🛠 {t("app.toolbarTools")}
              {activeToolCount > 0 && (
                <span className="toolbar-btn-count">{activeToolCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "history" ? "active" : ""}`}
              onClick={() => toggleSidebar("history")}
              title={t("app.toolbarHistory")}
            >
              🕒 {t("app.toolbarHistory")}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "settings" ? "active" : ""}`}
              onClick={() => toggleSidebar("settings")}
              title={t("app.toolbarSettings")}
            >
              ⚙ {t("app.toolbarSettings")}
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

        {/* Messages */}
        <div className="messages">
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
              <ToolCallGroup key={m.id} message={m} />
            ) : (
              <ChatMessage
                key={m.id}
                message={m}
                showRetry={m.role === "user" && m.id === pendingRetryMessageId && !streaming}
                onRetry={retryPendingUserMessage}
                onDelete={handleDeleteMessage}
                onFork={handleForkMessage}
                dbId={m.dbId}
              />
            )
          )}
          {error && <div className="error-banner">{error}</div>}
          <div ref={bottomRef} />
        </div>

        {/* Input */}
        <div className="input-area" style={{ position: "relative", flexDirection: "column", alignItems: "stretch" }}>
          {timers.length > 0 && (
            <div className="timer-bar" role="status" aria-live="polite">
              <span className="timer-bar-title">⏱️ {t("app.timers.title")}</span>
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
                        ×
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}
          {usage && (
            <div
              className="usage-panel"
              role="status"
              aria-live="polite"
              aria-label={t("app.contextUsage", { percent: usagePercent })}
            >
              <div
                className={`usage-panel-fill${usagePercent >= 90 ? " danger" : ""}`}
                style={{ width: `${usagePercent}%` }}
              />
            </div>
          )}
          {attachments.length > 0 && (
            <div className="attachments-bar">
              {attachments.map((file, i) => (
                <div key={i} className={`attachment-pill attachment-kind-${file.kind}`}>
                  {file.kind === "image" && file.data_url && (
                    <img
                      className="attachment-thumb"
                      src={file.data_url}
                      alt={file.name}
                      title={file.name}
                    />
                  )}
                  {file.kind === "file" && (
                    <span className="attachment-icon" aria-hidden="true">📄</span>
                  )}
                  {file.kind === "audio" && (
                    <span className="attachment-icon" aria-hidden="true">🔊</span>
                  )}
                  {file.kind === "text" && (
                    <span className="attachment-icon" aria-hidden="true">📝</span>
                  )}
                  <span className="attachment-name" title={file.name}>{file.name}</span>
                  <button className="attachment-remove" onClick={() => {
                    setAttachments(prev => prev.filter((_, idx) => idx !== i));
                  }}>×</button>
                </div>
              ))}
            </div>
          )}
          <div className="input-row">
            <button
              className="attach-btn"
              title={t("app.attachFiles")}
              onClick={() => fileInputRef.current?.click()}
              disabled={streaming || profileExporting}
            >
              📎
            </button>
            <input
              type="file"
              multiple
              // Only offer what the selected model can actually consume.
              accept={acceptedFileTypes(selectedMetadata)}
              ref={fileInputRef}
              style={{ display: 'none' }}
              onChange={async (e) => {
                const files = e.target.files;
                if (files && files.length > 0) {
                  const newAttachments: Attachment[] = [];
                  const skipped: string[] = [];
                  for (const f of Array.from(files)) {
                    // The picker is filtered already, but the OS dialog can be
                    // talked into other files — never send one the model
                    // cannot read.
                    if (!supportsAttachmentKind(selectedMetadata, classifyFile(f))) {
                      skipped.push(f.name);
                      continue;
                    }
                    try {
                      newAttachments.push(await readAttachment(f));
                    } catch (err) {
                      console.error("Failed to read file", f.name, err);
                    }
                  }
                  if (newAttachments.length > 0) {
                    setAttachments(prev => [...prev, ...newAttachments]);
                  }
                  if (skipped.length > 0) {
                    setError(t("app.attachmentUnsupported", {
                      names: skipped.join(", "),
                      model: selectedModel,
                    }));
                  }
                }
                e.target.value = '';
              }}
            />
            <textarea
              ref={textareaRef}
              className="chat-input"
              rows={1}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder={t("app.inputPlaceholder")}
              disabled={streaming || profileExporting}
            />
            <ModelSelect
              models={availableModels}
              value={selectedModel}
              metadata={modelMetadata}
              disabled={streaming || profileExporting}
              onChange={setSelectedModel}
            />
            {reasoningSupported && (
              <select
                className="reasoning-select"
                title={t("app.reasoningDepthTitle")}
                aria-label={t("app.reasoningDepth")}
                value={reasoningEffort}
                onChange={(e) => void handleReasoningEffortChange(e.target.value)}
                disabled={streaming || profileExporting}
              >
                <option value="">{`🧠 ${t("app.reasoningDefault")}`}</option>
                <option value="low">{`🧠 ${t("app.reasoningLow")}`}</option>
                <option value="medium">{`🧠 ${t("app.reasoningMedium")}`}</option>
                <option value="high">{`🧠 ${t("app.reasoningHigh")}`}</option>
              </select>
            )}
            <button
              className="send-btn"
              onClick={() => (streaming ? void stopStreaming() : void sendMessage())}
              disabled={profileExporting || (!streaming && !input.trim() && attachments.length === 0)}
            >
              {streaming ? t("app.stop") : t("app.send")}
            </button>
          </div>
        </div>
      </div >
    </div >
  );
}
