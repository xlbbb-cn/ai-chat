import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { chatCompletion, getConfig, getAgentOrchestration, listMcpServers, listSubAgents, saveConfig, saveHistory, stopChatCompletion, confirmCommand, saveMarkdownFile, deleteMessage, forkSession } from "./api";

import { ChatMessage } from "./components/ChatMessage";
import { ToolCallGroup } from "./components/ToolCallGroup";
import { SettingsPanel } from "./components/SettingsPanel";
import { SkillsPanel } from "./components/SkillsPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { ToolsPanel } from "./components/ToolsPanel";
import { McpPanel } from "./components/McpPanel";
import { AgentsPanel } from "./components/AgentsPanel";
import { AgentMissionPanel } from "./components/AgentMissionPanel";
import { MarkdownPreview } from "./components/MarkdownPreview";
import { Portal } from "./components/Portal";
import type { Message, AgentTaskEvent, ToolCallEntry } from "./types";
import "./App.css";

type Sidebar = "settings" | "skills" | "history" | "tools" | "mcp" | "agents" | "monitor" | null;

function applyTheme(theme: "auto" | "light" | "dark" | undefined) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const resolved = theme === "dark" || (theme !== "light" && prefersDark) ? "dark" : "light";
  document.documentElement.setAttribute("data-theme", resolved);
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
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [attachments, setAttachments] = useState<{ name: string; content: string }[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
  const [sidebarMotion, setSidebarMotion] = useState<"opening" | "closing" | null>(null);
  const [activeSkillIds, setActiveSkillIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
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
  const [activeToolCount, setActiveToolCount] = useState(0);
  const [activeMcpCount, setActiveMcpCount] = useState(0);
  const [activeAgentCount, setActiveAgentCount] = useState(0);
  const [useAgentsEnabled, setUseAgentsEnabled] = useState(false);
  const [agentStatuses, setAgentStatuses] = useState<Record<string, AgentStatus>>({});
  const [skillsLoadedFromConfig, setSkillsLoadedFromConfig] = useState(false);
  const [confirmDialog, setConfirmDialog] = useState<{
    reason: string;
    cmd_type: string;
    code: string;
    confirm_kind?: "dangerous" | "sudo" | "elevation" | "external_path";
    requires_auth?: "none" | "sudo" | "elevation";
  } | null>(null);
  const [confirmUsername, setConfirmUsername] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [profileExporting, setProfileExporting] = useState(false);
  const [profileExportMessage, setProfileExportMessage] = useState("Exporting and compressing profile...");
  const [pendingRetryMessageId, setPendingRetryMessageId] = useState<string | null>(null);
  const [markdownEditorOpen, setMarkdownEditorOpen] = useState(false);
  const [markdownPath, setMarkdownPath] = useState("");
  const [markdownDraft, setMarkdownDraft] = useState("");
  const [markdownSaving, setMarkdownSaving] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const sidebarMotionTimerRef = useRef<number | null>(null);
  const themeRef = useRef<"auto" | "light" | "dark">("auto");
  const currentAssistantMessageIdRef = useRef<string | null>(null);
  const hasRunningToolCallRef = useRef(false);
  const currentToolCallsRef = useRef<ToolCallEntry[]>([]);

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
    const unlisten = listen<{
      reason: string;
      cmd_type: string;
      code: string;
      confirm_kind?: "dangerous" | "sudo" | "elevation" | "external_path";
      requires_auth?: "none" | "sudo" | "elevation";
    }>(
      "confirm-required",
      (e) => {
        const { reason, cmd_type, code, confirm_kind, requires_auth } = e.payload;
        setConfirmDialog((current) =>
          current ?? { reason, cmd_type, code, confirm_kind, requires_auth }
        );
        setConfirmUsername("");
        setConfirmPassword("");
      }
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const respondToConfirm = useCallback((confirmed: boolean) => {
    const requiresSudo = confirmDialog?.requires_auth === "sudo";
    confirmCommand(confirmed, requiresSudo ? { username: confirmUsername, password: confirmPassword } : undefined).catch(console.error);
    setConfirmDialog(null);
    setConfirmUsername("");
    setConfirmPassword("");
  }, [confirmDialog, confirmUsername, confirmPassword]);

  useEffect(() => {
    getConfig()
      .then((cfg) => {
        const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
        setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
        setSelectedModel(cfg.model || "gpt-4o-mini");
        setMaxTokens(cfg.model_settings?.max_tokens ?? null);
        setActiveSkillIds(cfg.selected_skills ?? []);
        setActiveToolCount((cfg.selected_tools ?? []).length);
        setSkillsLoadedFromConfig(true);
        themeRef.current = cfg.theme ?? "auto";
        applyTheme(cfg.theme);
      })
      .catch(console.error);
  }, []);

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
        const [cfg, servers] = await Promise.all([getConfig(), listMcpServers()]);
        const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
        setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
        setSelectedModel(cfg.model || "gpt-4o-mini");
        setMaxTokens(cfg.model_settings?.max_tokens ?? null);
        setActiveSkillIds(cfg.selected_skills ?? []);
        setActiveToolCount((cfg.selected_tools ?? []).length);
        setActiveMcpCount(servers.filter((s) => s.enabled).length);
      } catch (err) {
        console.error(err);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      listen("profile-export-start", () => {
        setProfileExporting(true);
        setProfileExportMessage("Preparing profile export...");
      }),
      listen<string>("profile-export-status", (e) => {
        setProfileExportMessage(e.payload);
      }),
      listen("profile-export-done", () => {
        setProfileExporting(false);
        setProfileExportMessage("Exporting and compressing profile...");
      }),
      listen<string>("profile-export-error", (e) => {
        setProfileExporting(false);
        setProfileExportMessage("Exporting and compressing profile...");
        setError(`Profile export failed: ${e.payload}`);
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

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
            agent_name: "Planner",
            description: `Plan completed — ${e.payload.task_count} tasks total`,
            status: "done",
          };
          currentToolCallsRef.current = [...currentToolCallsRef.current, entry];
          updateActiveAssistantToolCalls(currentToolCallsRef.current);
        }
      }),
      listen("agent-aggregate-start", () => {
        const entry: ToolCallEntry = {
          task_id: `aggregate-${Date.now()}`,
          agent_name: "Aggregator",
          description: "Aggregating all subtask results...",
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
          agent_name: "Tool",
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
    );
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, [updateActiveAssistantToolCalls]);

  const sendMessage = useCallback(async () => {
    if (profileExporting) return;

    let text = input.trim();
    if ((!text && attachments.length === 0) || streaming) return;

    for (const file of attachments) {
      const ext = file.name.split('.').pop() || '';
      text += `\n\n<details><summary>Attached File: ${file.name}</summary>\n\n\`\`\`${ext}\n${file.content}\n\`\`\`\n</details>`;
    }
    text = text.trim();
    setPendingRetryMessageId(null);

    const userMsg: Message = { id: crypto.randomUUID(), role: "user", content: text };
    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };
    currentAssistantMessageIdRef.current = assistantId;
    currentToolCallsRef.current = [];
    hasRunningToolCallRef.current = false;

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setInput("");
    setAttachments([]);
    setStreaming(true);
    setError(null);

    const history = [...messages, userMsg]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-") && m.role !== "tool_group")
      .map((m) => ({ role: m.role, content: m.content, reasoning_content: m.reasoning_content }));

    saveHistory(sessionId, "user", text).then((dbId) => {
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
                ? { ...m, content: m.content || "Error: " + err, streaming: false }
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
  }, [input, messages, streaming, profileExporting, activeSkillIds, sessionId, attachments, selectedModel, useAgentsEnabled]);

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
      .map((m) => ({ role: m.role, content: m.content, reasoning_content: m.reasoning_content }));

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
                ? { ...m, content: m.content || "Error: " + err, streaming: false }
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
  }, [messages, streaming, pendingRetryMessageId, activeSkillIds, sessionId, selectedModel, useAgentsEnabled]);

  const handleDeleteMessage = useCallback(async (messageId: string) => {
    const msg = messages.find((m) => m.id === messageId);
    if (!msg || msg.dbId === undefined) return;

    try {
      await deleteMessage(msg.dbId);
      setMessages((prev) => prev.filter((m) => m.id !== messageId));
    } catch (err) {
      setError(`Failed to delete message: ${String(err)}`);
    }
  }, [messages]);

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
      setError(`Failed to fork session: ${String(err)}`);
    }
  }, [messages, sessionId]);

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
    const title = requiresSudo
      ? "⚠️ Privileged operation (sudo)"
      : requiresElevation
        ? "⚠️ Privileged operation (administrator)"
        : isExternalPath
          ? "⚠️ External file access request"
          : "⚠️ Dangerous command detected";
    const badge = requiresSudo
      ? "SUDO"
      : requiresElevation
        ? "ADMIN"
        : isExternalPath
          ? "PATH"
          : "DANGEROUS";
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
            <strong>Reason:</strong> {confirmDialog.reason}
          </p>
          <p>
            <strong>Type:</strong> {confirmDialog.cmd_type}
          </p>
          {requiresSudo && (
            <div className="confirm-dialog-credentials">
              <label>
                Username (optional)
                <input
                  value={confirmUsername}
                  onChange={(e) => setConfirmUsername(e.target.value)}
                  placeholder="leave blank for current user"
                />
              </label>
              <label>
                Password
                <input
                  type="password"
                  value={confirmPassword}
                  onChange={(e) => setConfirmPassword(e.target.value)}
                  placeholder="required"
                  autoFocus
                />
              </label>
              <p className="confirm-dialog-hint">
                This will run with elevated privileges and may modify your system.
              </p>
            </div>
          )}
          {requiresElevation && (
            <p className="confirm-dialog-hint">
              This will request administrator elevation (UAC) and may modify system settings.
            </p>
          )}
          {isExternalPath && (
            <p className="confirm-dialog-hint">
              This file action wants to access an absolute path outside the current workspace root.
              Allow it only if that external location is intentional.
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
              Deny
            </button>
            <button
              className="confirm-dialog-button confirm"
              onClick={() => respondToConfirm(true)}
              disabled={requiresSudo && confirmPassword.trim().length === 0}
            >
              Allow
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
        const stopSuffix = "\n\n[Generation stopped]";
        const content = m.content.includes("[Generation stopped]")
          ? m.content
          : (m.content || "") + stopSuffix;
        return { ...m, content, streaming: false };
      })
    );
    setStreaming(false);
  }

  async function handleSaveMarkdownEditor() {
    if (!markdownPath.trim()) {
      setError("Markdown file path is empty.");
      return;
    }

    setMarkdownSaving(true);
    try {
      await saveMarkdownFile(markdownPath, markdownDraft);
      setMarkdownEditorOpen(false);
    } catch (err) {
      setError(`Failed to save markdown file: ${String(err)}`);
    } finally {
      setMarkdownSaving(false);
    }
  }

  const usageTotal = usage ? (usage.total_tokens ?? usage.prompt_tokens + usage.completion_tokens) : 0;
  const fallbackMaxTokens = 131072; // 128k tokens as a hard upper bound for usage ratio calculations when no explicit max is provided
  const usageMax = usage?.max_tokens ?? maxTokens ?? fallbackMaxTokens;
  const usageRatio = usage
    ? (usage.usage_ratio ?? (usageMax > 0 ? usageTotal / usageMax : 0))
    : 0;
  const usagePercent = Math.max(0, Math.min(100, Math.round(usageRatio * 100)));
  const usageBarWidth = 16;
  const usageBarFilled = Math.max(0, Math.min(usageBarWidth, Math.round((usagePercent / 100) * usageBarWidth)));
  const usageBarText = `[${"x".repeat(usageBarFilled)}${"-".repeat(usageBarWidth - usageBarFilled)} ]`;

  return (
    <div className="app-layout">
      {renderConfirmDialog()}
      {markdownEditorOpen && (
        <Portal>
          <div className="markdown-editor-overlay" role="dialog" aria-modal="true" aria-label="Markdown editor">
            <div className="markdown-editor-shell">
              <div className="markdown-editor-header">
                <h3>Markdown Edit</h3>
                <div className="markdown-editor-file" title={markdownPath}>{markdownPath}</div>
                <div className="markdown-editor-actions">
                  <button
                    type="button"
                    className="toolbar-btn"
                    onClick={() => setMarkdownEditorOpen(false)}
                    disabled={markdownSaving}
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    className="send-btn"
                    onClick={handleSaveMarkdownEditor}
                    disabled={markdownSaving}
                  >
                    {markdownSaving ? "Saving..." : "Save"}
                  </button>
                </div>
              </div>
              <div className="markdown-editor-body">
                <div className="markdown-editor-column">
                  <span>Markdown</span>
                  <textarea
                    className="markdown-editor-textarea"
                    value={markdownDraft}
                    onChange={(e) => setMarkdownDraft(e.target.value)}
                    placeholder="Write markdown here..."
                  />
                </div>
                <div className="markdown-editor-column">
                  <span>Preview</span>
                  <div className="markdown-editor-preview">
                    {markdownDraft.trim()
                      ? <MarkdownPreview content={markdownDraft} />
                      : <p className="markdown-editor-preview-empty">Markdown preview will appear here.</p>}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </Portal>
      )}
      {/* Sidebar */}
      {sidebar && (
        <aside className={`sidebar ${sidebar === "monitor" ? "sidebar-wide" : ""} ${sidebarMotion === "opening" ? "sidebar-opening" : ""} ${sidebarMotion === "closing" ? "sidebar-closing" : ""}`}>
          <div className={`sidebar-shell ${sidebar === "settings" ? "panel-settings" : ""} ${sidebar === "skills" ? "panel-skills" : ""} ${sidebar === "history" ? "panel-history" : ""} ${sidebar === "tools" ? "panel-tools" : ""} ${sidebar === "mcp" ? "panel-mcp" : ""} ${sidebar === "agents" ? "panel-agents" : ""} ${sidebar === "monitor" ? "panel-monitor" : ""}`} key={sidebar}>
            {sidebar === "settings" && (
              <SettingsPanel
                sessionId={sessionId}
                onClose={closeSidebar}
                onConfigSaved={(cfg) => {
                  const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
                  setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
                  setSelectedModel(cfg.model || "gpt-4o-mini");
                  setMaxTokens(cfg.model_settings?.max_tokens ?? null);
                  themeRef.current = cfg.theme ?? "auto";
                  applyTheme(cfg.theme);
                }}
              />
            )}
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
                    setError("A reply is currently being generated. Please stop it before switching history sessions.");
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
                onOpenMonitor={() => toggleSidebar("monitor")}
              />
            )}
            {sidebar === "monitor" && (
              <AgentMissionPanel
                sessionId={sessionId}
                onClose={closeSidebar}
              />
            )}
          </div>
        </aside>
      )}

      {/* Main chat area */}
      <div className="chat-area" onClick={handleChatAreaClick}>
        {/* Toolbar */}
        <header className="toolbar">
          <span className="app-title">Chat</span>
          <div className="toolbar-actions">
            <button
              className={`toolbar-btn ${sidebar === "agents" ? "active" : ""}`}
              onClick={() => toggleSidebar("agents")}
              title={`Sub Agents${useAgentsEnabled ? " (Enabled)" : ""}`}
            >
              🤖 Agents
              {activeAgentCount > 0 && useAgentsEnabled && (
                <span className="toolbar-btn-count">{activeAgentCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "skills" ? "active" : ""}`}
              onClick={() => toggleSidebar("skills")}
              title="Skills"
            >
              ✦ Skills
              {activeSkillIds.length > 0 && (
                <span className="toolbar-btn-count">{activeSkillIds.length}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "mcp" ? "active" : ""}`}
              onClick={() => toggleSidebar("mcp")}
              title="MCP Servers"
            >
              ⬡ MCP
              {activeMcpCount > 0 && (
                <span className="toolbar-btn-count">{activeMcpCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "tools" ? "active" : ""}`}
              onClick={() => toggleSidebar("tools")}
              title="Tools"
            >
              🛠 Tools
              {activeToolCount > 0 && (
                <span className="toolbar-btn-count">{activeToolCount}</span>
              )}
            </button>
            <button
              className={`toolbar-btn ${sidebar === "settings" ? "active" : ""}`}
              onClick={() => toggleSidebar("settings")}
              title="Settings"
            >
              ⚙ Settings
            </button>
            <button
              className={`toolbar-btn ${sidebar === "history" ? "active" : ""}`}
              onClick={() => toggleSidebar("history")}
              title="History"
            >
              🕒 History
            </button>
            <button
              className="toolbar-btn"
              onClick={clearChat}
              title="New chat"
            >
              ↻ New Chat
            </button>
          </div>
        </header>

        {profileExporting && (
          <div className="profile-export-progress" role="progressbar" aria-busy="true" aria-live="polite">
            <div className="profile-export-progress-label">{profileExportMessage}</div>
            <div className="profile-export-progress-hint">Chat is locked during export and sending is disabled.</div>
            <div className="profile-export-progress-track">
              <div className="profile-export-progress-fill" />
            </div>
          </div>
        )}

        {/* Messages */}
        <div className="messages">
          {messages.length === 0 && (
            <div className="empty-state">
              <p>Start a conversation</p>
              <p className="empty-hint">
                Use <strong>Skills</strong> to set a system prompt, or configure the API in <strong>Settings</strong>.
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
          {usage && (
            <div className="usage-panel">
              <div className="usage-line" role="status" aria-live="polite">
                Tokens: {usage.prompt_tokens} prompt / {usage.completion_tokens} completion   {usageBarText} {usageTotal} / {usageMax} ({usagePercent}%)
              </div>
            </div>
          )}
          {attachments.length > 0 && (
            <div className="attachments-bar">
              {attachments.map((file, i) => (
                <div key={i} className="attachment-pill">
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
              title="Attach files"
              onClick={() => fileInputRef.current?.click()}
              disabled={streaming || profileExporting}
            >
              📎
            </button>
            <input
              type="file"
              multiple
              accept=".txt,.md,.markdown,.csv,.tsv,.json,.jsonl,.yaml,.yml,.xml,.html,.htm,.css,.js,.ts,.jsx,.tsx,.py,.rs,.go,.java,.c,.cpp,.h,.hpp,.cs,.rb,.php,.sh,.bat,.ps1,.sql,.log,.ini,.cfg,.toml,.env,.diff,.patch,.tex,.rst,.adoc,.org,.r,.m,.scala,.swift,.kt,.dart,.lua,.pl,.ex,.exs,.clj,.hs,.ml,.fs,.erl,.vim,.conf,.cfg,.v"
              ref={fileInputRef}
              style={{ display: 'none' }}
              onChange={async (e) => {
                const files = e.target.files;
                if (files && files.length > 0) {
                  const newAttachments: { name: string; content: string }[] = [];
                  for (const f of Array.from(files)) {
                    try {
                      const content = await f.text();
                      newAttachments.push({ name: f.name, content });
                    } catch (err) {
                      console.error("Failed to read file", f.name, err);
                    }
                  }
                  setAttachments(prev => [...prev, ...newAttachments]);
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
              placeholder="Type a message… (Enter to send, Shift+Enter for newline)"
              disabled={streaming || profileExporting}
            />
            <select
              className="model-select"
              title="Select model"
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
              disabled={streaming || profileExporting}
            >
              {availableModels.map((model) => (
                <option key={model} value={model}>{model}</option>
              ))}
            </select>
            <button
              className="send-btn"
              onClick={streaming ? stopStreaming : sendMessage}
              disabled={profileExporting || (!streaming && !input.trim() && attachments.length === 0)}
            >
              {streaming ? "Stop" : "Send"}
            </button>
          </div>
        </div>
      </div >
    </div >
  );
}
