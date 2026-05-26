import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { chatCompletion, getConfig, getAgentOrchestration, listMcpServers, listSessionRuntimeStates, listSubAgents, saveConfig, saveHistory, stopChatCompletion, confirmCommand } from "./api";

import { ChatMessage } from "./components/ChatMessage";
import { SettingsPanel } from "./components/SettingsPanel";
import { SkillsPanel } from "./components/SkillsPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { ToolsPanel } from "./components/ToolsPanel";
import { McpPanel } from "./components/McpPanel";
import { AgentsPanel } from "./components/AgentsPanel";
import type { Message, SessionRuntimeState } from "./types";
import "./App.css";

type Sidebar = "settings" | "skills" | "history" | "tools" | "mcp" | "agents" | null;

interface AgentStatus {
  status: "idle" | "running" | "done" | "error";
  description?: string;
  summary?: string;
  error?: string;
  tokens?: number;
}

interface UsageState {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens?: number;
  max_tokens?: number;
  usage_ratio?: number;
}

function derivePendingRetryMessageId(sessionMessages: Message[]): string | null {
  const lastMsg = sessionMessages.length > 0 ? sessionMessages[sessionMessages.length - 1] : null;
  return lastMsg?.role === "user" ? lastMsg.id : null;
}

export default function App() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [attachments, setAttachments] = useState<{ name: string; content: string }[]>([]);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
  const [activeSkillIds, setActiveSkillIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [usage, setUsage] = useState<UsageState | null>(null);
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
    confirm_kind?: "dangerous" | "sudo" | "elevation";
    requires_auth?: "none" | "sudo" | "elevation";
  } | null>(null);
  const [confirmUsername, setConfirmUsername] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [profileExporting, setProfileExporting] = useState(false);
  const [profileExportMessage, setProfileExportMessage] = useState("Exporting and compressing profile...");
  const [pendingRetryMessageId, setPendingRetryMessageId] = useState<string | null>(null);
  const [sessionRuntimes, setSessionRuntimes] = useState<Record<string, SessionRuntimeState>>({});
  const [historyRefreshKey, setHistoryRefreshKey] = useState(0);
  const bottomRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const currentSessionRef = useRef(sessionId);
  const activeStreamingSessionRef = useRef<string | null>(null);
  const messagesRef = useRef<Message[]>([]);
  const sessionMessagesRef = useRef<Record<string, Message[]>>({});
  const sessionRetryRef = useRef<Record<string, string | null>>({});
  const sessionAgentStatusesRef = useRef<Record<string, Record<string, AgentStatus>>>({});
  const sessionUsageRef = useRef<Record<string, UsageState | null>>({});

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  useEffect(() => {
    currentSessionRef.current = sessionId;
  }, [sessionId]);

  useEffect(() => {
    messagesRef.current = messages;
    sessionMessagesRef.current[sessionId] = messages;
  }, [messages, sessionId]);

  useEffect(() => {
    sessionRetryRef.current[sessionId] = pendingRetryMessageId;
  }, [pendingRetryMessageId, sessionId]);

  useEffect(() => {
    sessionAgentStatusesRef.current[sessionId] = agentStatuses;
  }, [agentStatuses, sessionId]);

  useEffect(() => {
    sessionUsageRef.current[sessionId] = usage;
  }, [usage, sessionId]);

  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = "auto";
      textareaRef.current.style.height = textareaRef.current.scrollHeight + "px";
    }
  }, [input]);

  useEffect(() => {
    const unlisten = listen<{
      session_id: string;
      prompt_tokens: number;
      completion_tokens: number;
      total_tokens?: number;
      max_tokens?: number;
      usage_ratio?: number;
    }>("chat-usage", (e) => {
      const { session_id, ...payload } = e.payload;
      const prevUsage = sessionUsageRef.current[session_id];

      const nextUsage = (() => {
        // If this is the first usage data, return it as-is
        if (!prevUsage) return payload;

        // Accumulate token counts within the same session
        // (for session history), but preserve current request's max_tokens and usage_ratio
        const prevTotal = prevUsage.total_tokens ?? prevUsage.prompt_tokens + prevUsage.completion_tokens;
        const currentTotal = payload.total_tokens ?? payload.prompt_tokens + payload.completion_tokens;

        return {
          // Accumulated token counts for session history display
          prompt_tokens: prevUsage.prompt_tokens + payload.prompt_tokens,
          completion_tokens: prevUsage.completion_tokens + payload.completion_tokens,
          total_tokens: prevTotal + currentTotal,
          // Keep the current request's max_tokens for ratio calculation (not accumulated)
          max_tokens: payload.max_tokens ?? prevUsage.max_tokens,
          // Keep the current request's usage_ratio (not accumulated)
          // This correctly reflects the current request's token usage within its max_tokens limit
          usage_ratio: payload.usage_ratio,
        };
      })();

      sessionUsageRef.current[session_id] = nextUsage;
      if (currentSessionRef.current === session_id) {
        setUsage(nextUsage);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const preventContextMenu = (event: MouseEvent) => {
      event.preventDefault();
    };
    document.addEventListener("contextmenu", preventContextMenu);
    return () => {
      document.removeEventListener("contextmenu", preventContextMenu);
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

  // Dangerous-command confirmation dialog
  useEffect(() => {
    const unlisten = listen<{
      reason: string;
      cmd_type: string;
      code: string;
      confirm_kind?: "dangerous" | "sudo" | "elevation";
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

  const persistHistoryRecord = useCallback((targetSessionId: string, role: string, content: string) => {
    saveHistory(targetSessionId, role, content)
      .then(() => setHistoryRefreshKey((prev) => prev + 1))
      .catch(console.error);
  }, []);

  const setSessionRuntime = useCallback((targetSessionId: string, runtime: SessionRuntimeState) => {
    setSessionRuntimes((prev) => ({ ...prev, [targetSessionId]: runtime }));
  }, []);

  const updateSessionMessages = useCallback((
    targetSessionId: string,
    updater: (prev: Message[]) => Message[],
  ) => {
    const previous = sessionMessagesRef.current[targetSessionId]
      ?? (currentSessionRef.current === targetSessionId ? messagesRef.current : []);
    const next = updater(previous);
    sessionMessagesRef.current[targetSessionId] = next;
    if (currentSessionRef.current === targetSessionId) {
      messagesRef.current = next;
      setMessages(next);
    }
  }, []);

  const setSessionPendingRetry = useCallback((targetSessionId: string, nextRetryId: string | null) => {
    sessionRetryRef.current[targetSessionId] = nextRetryId;
    if (currentSessionRef.current === targetSessionId) {
      setPendingRetryMessageId(nextRetryId);
    }
  }, []);

  const updateSessionAgentStatuses = useCallback((
    targetSessionId: string,
    updater: (prev: Record<string, AgentStatus>) => Record<string, AgentStatus>,
  ) => {
    const previous = sessionAgentStatusesRef.current[targetSessionId] ?? {};
    const next = updater(previous);
    sessionAgentStatusesRef.current[targetSessionId] = next;
    if (currentSessionRef.current === targetSessionId) {
      setAgentStatuses(next);
    }
  }, []);

  const isSessionWorking = useCallback((targetSessionId: string, sessionMessages?: Message[]) => {
    const runtimeWorking = sessionRuntimes[targetSessionId]?.status === "working";
    const candidateMessages = sessionMessages
      ?? sessionMessagesRef.current[targetSessionId]
      ?? (currentSessionRef.current === targetSessionId ? messagesRef.current : []);
    return runtimeWorking || candidateMessages.some((message) => message.streaming);
  }, [sessionRuntimes]);

  const activateSessionView = useCallback((targetSessionId: string, fallbackMessages: Message[]) => {
    const nextMessages = sessionMessagesRef.current[targetSessionId] ?? fallbackMessages;
    const nextRetryId = sessionRetryRef.current[targetSessionId] ?? derivePendingRetryMessageId(nextMessages);
    const nextAgentStatuses = sessionAgentStatusesRef.current[targetSessionId] ?? {};
    const nextUsage = sessionUsageRef.current[targetSessionId] ?? null;

    currentSessionRef.current = targetSessionId;
    sessionMessagesRef.current[targetSessionId] = nextMessages;
    sessionRetryRef.current[targetSessionId] = nextRetryId;
    sessionAgentStatusesRef.current[targetSessionId] = nextAgentStatuses;
    sessionUsageRef.current[targetSessionId] = nextUsage;
    messagesRef.current = nextMessages;

    setSessionId(targetSessionId);
    setMessages(nextMessages);
    setPendingRetryMessageId(nextRetryId);
    setAgentStatuses(nextAgentStatuses);
    setUsage(nextUsage);
    setError(null);
  }, [isSessionWorking]);

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
      })
      .catch(console.error);
  }, []);

  useEffect(() => {
    listMcpServers()
      .then((servers) => setActiveMcpCount(servers.filter((s) => s.enabled).length))
      .catch(console.error);
  }, []);

  useEffect(() => {
    let disposed = false;

    const refreshRuntimeStates = async () => {
      try {
        const runtimeRecords = await listSessionRuntimeStates();
        if (disposed) return;
        setSessionRuntimes((prev) => {
          const next = { ...prev };
          for (const record of runtimeRecords) {
            const previous = next[record.session_id];
            next[record.session_id] = {
              status: record.status,
              detail:
                record.status === "working"
                  ? previous?.detail ?? "Worker"
                  : previous?.status === "error"
                    ? previous.detail
                    : "Idle",
            };
          }
          return next;
        });
      } catch (err) {
        console.error(err);
      }
    };

    refreshRuntimeStates();
    const timer = window.setInterval(refreshRuntimeStates, 1000);

    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
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

  const sendMessage = useCallback(async () => {
    if (profileExporting) return;

    let text = input.trim();
    const targetSessionId = sessionId;
    const baseMessages = sessionMessagesRef.current[targetSessionId] ?? messagesRef.current;
    const targetSessionWorking = isSessionWorking(targetSessionId, baseMessages);
    if ((!text && attachments.length === 0) || targetSessionWorking) return;

    for (const file of attachments) {
      const ext = file.name.split('.').pop() || '';
      text += `\n\n<details><summary>Attached File: ${file.name}</summary>\n\n\`\`\`${ext}\n${file.content}\n\`\`\`\n</details>`;
    }
    text = text.trim();

    const userMsg: Message = { id: crypto.randomUUID(), role: "user", content: text };
    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };

    setSessionPendingRetry(targetSessionId, null);
    updateSessionMessages(targetSessionId, (prev) => [...prev, userMsg, assistantMsg]);
    setInput("");
    setAttachments([]);
    activeStreamingSessionRef.current = targetSessionId;
    setError(null);
    setSessionRuntime(targetSessionId, { status: "working", detail: useAgentsEnabled ? "Agents" : "LLM" });

    const history = [...baseMessages, userMsg]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-"))
      .map((m) => ({ role: m.role, content: m.content }));

    persistHistoryRecord(targetSessionId, "user", text);

    let accumulatedContent = "";
    let accumulatedReasoning = "";

    const cleanup = await chatCompletion(history, activeSkillIds, targetSessionId, selectedModel, {
      onToken(token) {
        accumulatedContent += token;
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, content: accumulatedContent } : m
          )
        );
      },
      onReasoningToken(token) {
        accumulatedReasoning += token;
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, reasoning_content: accumulatedReasoning } : m
          )
        );
      },
      onAgentTaskStart(e) {
        updateSessionAgentStatuses(targetSessionId, (prev) => ({
          ...prev,
          [e.agent_id]: { status: "running", description: e.description },
        }));
        updateSessionMessages(targetSessionId, (prev) => [
          ...prev,
          {
            id: `agent-progress-${e.task_id}`,
            role: "system",
            content: `🤖 **[${e.agent_name}]** 正在执行: ${e.description ?? ""}`,
          },
        ]);
        setSessionRuntime(targetSessionId, { status: "working", detail: e.agent_name });
      },
      onAgentTaskDone(e) {
        updateSessionAgentStatuses(targetSessionId, (prev) => ({
          ...prev,
          [e.agent_id]: { status: "done", summary: e.summary ?? "" },
        }));
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === `agent-progress-${e.task_id}`
              ? { ...m, content: `✅ **[${e.agent_name}]** 完成` }
              : m
          )
        );
      },
      onAgentTaskError(e) {
        updateSessionAgentStatuses(targetSessionId, (prev) => ({
          ...prev,
          [e.agent_id]: { status: "error", error: e.error ?? "" },
        }));
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === `agent-progress-${e.task_id}`
              ? { ...m, content: `❌ **[${e.agent_name}]** 失败: ${e.error}` }
              : m
          )
        );
        setSessionRuntime(targetSessionId, { status: "error", detail: e.agent_name });
      },
      onAgentPlanStart(taskCount) {
        if (taskCount > 0) {
          updateSessionMessages(targetSessionId, (prev) => [
            ...prev,
            {
              id: "agent-progress-plan",
              role: "system",
              content: `🗂 **规划完成** — 共 ${taskCount} 个任务`,
            },
          ]);
          setSessionRuntime(targetSessionId, { status: "working", detail: "Planning" });
        }
      },
      onAgentAggregateStart() {
        updateSessionMessages(targetSessionId, (prev) => [
          ...prev,
          {
            id: "agent-progress-aggregate",
            role: "system",
            content: "📝 **正在汇总所有子任务结果...**",
          },
        ]);
        setSessionRuntime(targetSessionId, { status: "working", detail: "Aggregating" });
      },
      onDone() {
        const finalContentToSave = accumulatedReasoning
          ? `<details><summary>Thought Process</summary>\n\n${accumulatedReasoning}\n</details>\n\n${accumulatedContent}`
          : accumulatedContent;

        persistHistoryRecord(targetSessionId, "assistant", finalContentToSave);
        // Remove all agent progress system messages and finalize assistant message
        updateSessionMessages(targetSessionId, (prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => (m.id === assistantId ? { ...m, streaming: false } : m))
        );
        updateSessionAgentStatuses(targetSessionId, () => ({}));
        setSessionPendingRetry(targetSessionId, null);
        setSessionRuntime(targetSessionId, { status: "idle", detail: "Idle" });
        if (activeStreamingSessionRef.current === targetSessionId) {
          activeStreamingSessionRef.current = null;
          cleanupRef.current = null;
        }
      },
      onError(err) {
        if (currentSessionRef.current === targetSessionId) {
          setError(err);
        }
        updateSessionMessages(targetSessionId, (prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) =>
              m.id === assistantId
                ? { ...m, content: m.content || "Error: " + err, streaming: false }
                : m
            )
        );
        updateSessionAgentStatuses(targetSessionId, () => ({}));
        setSessionPendingRetry(targetSessionId, userMsg.id);
        setSessionRuntime(targetSessionId, { status: "error", detail: "Failed" });
        if (activeStreamingSessionRef.current === targetSessionId) {
          activeStreamingSessionRef.current = null;
          cleanupRef.current = null;
        }
      },
    }, useAgentsEnabled);

    cleanupRef.current = cleanup;
  }, [input, profileExporting, activeSkillIds, sessionId, attachments, selectedModel, useAgentsEnabled, persistHistoryRecord, setSessionPendingRetry, updateSessionMessages, setSessionRuntime, updateSessionAgentStatuses, isSessionWorking]);

  const retryPendingUserMessage = useCallback(async () => {
    const targetSessionId = sessionId;
    const targetMessages = sessionMessagesRef.current[targetSessionId] ?? messagesRef.current;
    if (isSessionWorking(targetSessionId, targetMessages) || !pendingRetryMessageId) return;

    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };

    updateSessionMessages(targetSessionId, (prev) => [...prev, assistantMsg]);
    activeStreamingSessionRef.current = targetSessionId;
    setError(null);
    setSessionRuntime(targetSessionId, { status: "working", detail: useAgentsEnabled ? "Agents" : "LLM" });

    const history = [...targetMessages]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-"))
      .map((m) => ({ role: m.role, content: m.content }));

    let accumulatedContent = "";
    let accumulatedReasoning = "";

    const cleanup = await chatCompletion(history, activeSkillIds, targetSessionId, selectedModel, {
      onToken(token) {
        accumulatedContent += token;
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, content: accumulatedContent } : m
          )
        );
      },
      onReasoningToken(token) {
        accumulatedReasoning += token;
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, reasoning_content: accumulatedReasoning } : m
          )
        );
      },
      onAgentTaskStart(e) {
        updateSessionAgentStatuses(targetSessionId, (prev) => ({
          ...prev,
          [e.agent_id]: { status: "running", description: e.description },
        }));
        updateSessionMessages(targetSessionId, (prev) => [
          ...prev,
          {
            id: `agent-progress-${e.task_id}`,
            role: "system",
            content: `🤖 **[${e.agent_name}]** 正在执行: ${e.description ?? ""}`,
          },
        ]);
        setSessionRuntime(targetSessionId, { status: "working", detail: e.agent_name });
      },
      onAgentTaskDone(e) {
        updateSessionAgentStatuses(targetSessionId, (prev) => ({
          ...prev,
          [e.agent_id]: { status: "done", summary: e.summary ?? "" },
        }));
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === `agent-progress-${e.task_id}`
              ? { ...m, content: `✅ **[${e.agent_name}]** 完成` }
              : m
          )
        );
      },
      onAgentTaskError(e) {
        updateSessionAgentStatuses(targetSessionId, (prev) => ({
          ...prev,
          [e.agent_id]: { status: "error", error: e.error ?? "" },
        }));
        updateSessionMessages(targetSessionId, (prev) =>
          prev.map((m) =>
            m.id === `agent-progress-${e.task_id}`
              ? { ...m, content: `❌ **[${e.agent_name}]** 失败: ${e.error}` }
              : m
          )
        );
        setSessionRuntime(targetSessionId, { status: "error", detail: e.agent_name });
      },
      onAgentPlanStart(taskCount) {
        if (taskCount > 0) {
          updateSessionMessages(targetSessionId, (prev) => [
            ...prev,
            {
              id: "agent-progress-plan",
              role: "system",
              content: `🗂 **规划完成** — 共 ${taskCount} 个任务`,
            },
          ]);
          setSessionRuntime(targetSessionId, { status: "working", detail: "Planning" });
        }
      },
      onAgentAggregateStart() {
        updateSessionMessages(targetSessionId, (prev) => [
          ...prev,
          {
            id: "agent-progress-aggregate",
            role: "system",
            content: "📝 **正在汇总所有子任务结果...**",
          },
        ]);
        setSessionRuntime(targetSessionId, { status: "working", detail: "Aggregating" });
      },
      onDone() {
        const finalContentToSave = accumulatedReasoning
          ? `<details><summary>Thought Process</summary>\n\n${accumulatedReasoning}\n</details>\n\n${accumulatedContent}`
          : accumulatedContent;

        persistHistoryRecord(targetSessionId, "assistant", finalContentToSave);
        updateSessionMessages(targetSessionId, (prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => (m.id === assistantId ? { ...m, streaming: false } : m))
        );
        setSessionPendingRetry(targetSessionId, null);
        updateSessionAgentStatuses(targetSessionId, () => ({}));
        setSessionRuntime(targetSessionId, { status: "idle", detail: "Idle" });
        if (activeStreamingSessionRef.current === targetSessionId) {
          activeStreamingSessionRef.current = null;
          cleanupRef.current = null;
        }
      },
      onError(err) {
        if (currentSessionRef.current === targetSessionId) {
          setError(err);
        }
        updateSessionMessages(targetSessionId, (prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) =>
              m.id === assistantId
                ? { ...m, content: m.content || "Error: " + err, streaming: false }
                : m
            )
        );
        updateSessionAgentStatuses(targetSessionId, () => ({}));
        setSessionRuntime(targetSessionId, { status: "error", detail: "Failed" });
        if (activeStreamingSessionRef.current === targetSessionId) {
          activeStreamingSessionRef.current = null;
          cleanupRef.current = null;
        }
      },
    }, useAgentsEnabled);

    cleanupRef.current = cleanup;
  }, [pendingRetryMessageId, activeSkillIds, sessionId, selectedModel, useAgentsEnabled, updateSessionMessages, setSessionRuntime, updateSessionAgentStatuses, setSessionPendingRetry, persistHistoryRecord, isSessionWorking]);

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
    setSidebar((s) => (s === panel ? null : panel));
  }

  async function clearChat() {
    if (isSessionWorking(sessionId, messagesRef.current)) {
      await stopStreaming();
    }
    const nextSessionId = crypto.randomUUID();
    currentSessionRef.current = nextSessionId;
    sessionMessagesRef.current[nextSessionId] = [];
    sessionRetryRef.current[nextSessionId] = null;
    sessionAgentStatusesRef.current[nextSessionId] = {};
    sessionUsageRef.current[nextSessionId] = null;
    setMessages([]);
    setError(null);
    setUsage(null);
    setAgentStatuses({});
    setPendingRetryMessageId(null);
    setSessionId(nextSessionId);
  }

  const renderConfirmDialog = () => {
    if (!confirmDialog) {
      return null;
    }
    const requiresSudo = confirmDialog.requires_auth === "sudo";
    const requiresElevation = confirmDialog.requires_auth === "elevation";
    const title = requiresSudo
      ? "⚠️ Privileged operation (sudo)"
      : requiresElevation
        ? "⚠️ Privileged operation (administrator)"
        : "⚠️ Dangerous command detected";
    const badge = requiresSudo
      ? "SUDO"
      : requiresElevation
        ? "ADMIN"
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
    const targetSessionId = activeStreamingSessionRef.current;
    if (!targetSessionId) {
      return;
    }
    try {
      await stopChatCompletion(targetSessionId);
    } catch (err) {
      setError(String(err));
    }

    if (cleanupRef.current) {
      cleanupRef.current();
      cleanupRef.current = null;
    }

    if (targetSessionId) {
      updateSessionMessages(targetSessionId, (prev) =>
        prev.map((m) => {
          if (!m.streaming) return m;
          const stopSuffix = "\n\n[已停止生成]";
          const content = m.content.includes("[已停止生成]")
            ? m.content
            : (m.content || "") + stopSuffix;
          return { ...m, content, streaming: false };
        })
      );
      updateSessionAgentStatuses(targetSessionId, () => ({}));
      setSessionRuntime(targetSessionId, { status: "idle", detail: "Stopped" });
    }
    activeStreamingSessionRef.current = null;
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
  const currentSessionWorking = isSessionWorking(sessionId, messages);

  return (
    <div className="app-layout">
      {renderConfirmDialog()}
      {/* Sidebar */}
      {sidebar && (
        <aside className="sidebar">
          {sidebar === "settings" && (
            <SettingsPanel
              sessionId={sessionId}
              onClose={() => setSidebar(null)}
              onConfigSaved={(cfg) => {
                const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
                setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
                setSelectedModel(cfg.model || "gpt-4o-mini");
                setMaxTokens(cfg.model_settings?.max_tokens ?? null);
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
              onClose={() => setSidebar(null)}
            />
          )}
          {sidebar === "tools" && (
            <ToolsPanel
              onClose={() => setSidebar(null)}
              onToolsChange={(tools) =>
                setActiveToolCount(tools.length)
              }
            />
          )}
          {sidebar === "mcp" && (
            <McpPanel
              onClose={() => setSidebar(null)}
              onServersChange={(enabledCount) => setActiveMcpCount(enabledCount)}
            />
          )}
          {sidebar === "history" && (
            <HistoryPanel
              currentSessionId={sessionId}
              runtimeStates={sessionRuntimes}
              refreshKey={historyRefreshKey}
              onLoad={(sid, msgs) => {
                activateSessionView(sid, msgs);
              }}
              onClose={() => setSidebar(null)}
            />
          )}
          {sidebar === "agents" && (
            <AgentsPanel
              onClose={() => setSidebar(null)}
              onAgentsChange={(count) => setActiveAgentCount(count)}
              useAgentsEnabled={useAgentsEnabled}
              onToggleUseAgents={setUseAgentsEnabled}
              agentStatuses={agentStatuses}
            />
          )}
        </aside>
      )}

      {/* Main chat area */}
      <div className="chat-area">
        {/* Toolbar */}
        <header className="toolbar">
          <span className="app-title">Chat</span>
          <div className="toolbar-actions">
            <button
              className={`toolbar-btn ${sidebar === "agents" ? "active" : ""}`}
              onClick={() => toggleSidebar("agents")}
              title={`Sub Agents${useAgentsEnabled ? " (启用中)" : ""}`}
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
          {messages.map((m) => (
            <ChatMessage
              key={m.id}
              message={m}
              showRetry={m.role === "user" && m.id === pendingRetryMessageId && !currentSessionWorking}
              onRetry={retryPendingUserMessage}
            />
          ))}
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
              disabled={currentSessionWorking || profileExporting}
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
              disabled={currentSessionWorking || profileExporting}
            />
            <select
              className="model-select"
              title="Select model"
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
              disabled={currentSessionWorking || profileExporting}
            >
              {availableModels.map((model) => (
                <option key={model} value={model}>{model}</option>
              ))}
            </select>
            <button
              className="send-btn"
              onClick={currentSessionWorking ? stopStreaming : sendMessage}
              disabled={profileExporting || (!currentSessionWorking && !input.trim() && attachments.length === 0)}
            >
              {currentSessionWorking ? "Stop" : "Send"}
            </button>
          </div>
        </div>
      </div >
    </div >
  );
}
