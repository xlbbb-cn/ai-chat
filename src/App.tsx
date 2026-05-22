import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { chatCompletion, getConfig, getAgentOrchestration, listMcpServers, listSubAgents, saveConfig, saveHistory, stopChatCompletion, confirmCommand } from "./api";

import { ChatMessage } from "./components/ChatMessage";
import { SettingsPanel } from "./components/SettingsPanel";
import { SkillsPanel } from "./components/SkillsPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { ToolsPanel } from "./components/ToolsPanel";
import { McpPanel } from "./components/McpPanel";
import { AgentsPanel } from "./components/AgentsPanel";
import type { Message, AgentTaskEvent } from "./types";
import "./App.css";

type Sidebar = "settings" | "skills" | "history" | "tools" | "mcp" | "agents" | null;

interface AgentStatus {
  status: "idle" | "running" | "done" | "error";
  description?: string;
  summary?: string;
  error?: string;
  tokens?: number;
}

export default function App() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [attachments, setAttachments] = useState<{ name: string; content: string }[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
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
  } | null>(null);
  const [profileExporting, setProfileExporting] = useState(false);
  const [profileExportMessage, setProfileExportMessage] = useState("Exporting and compressing profile...");
  const [pendingRetryMessageId, setPendingRetryMessageId] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const cleanupRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

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
      setUsage(e.payload);
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
    const unlisten = listen<{ reason: string; cmd_type: string; code: string }>(
      "confirm-required",
      (e) => {
        const { reason, cmd_type, code } = e.payload;
        setConfirmDialog((current) =>
          current ?? { reason, cmd_type, code }
        );
      }
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const respondToConfirm = useCallback((confirmed: boolean) => {
    confirmCommand(confirmed).catch(console.error);
    setConfirmDialog(null);
  }, []);

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
        setMessages((prev) => [
          ...prev,
          {
            id: `agent-progress-${task_id}`,
            role: "system" as const,
            content: `🤖 **[${agent_name}]** 正在执行: ${description ?? ""}`,
          },
        ]);
      }),
      listen<AgentTaskEvent>("agent-task-done", (e) => {
        const { agent_id, agent_name, summary, task_id } = e.payload;
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "done", summary: summary ?? "" },
        }));
        setMessages((prev) =>
          prev.map((m) =>
            m.id === `agent-progress-${task_id}`
              ? { ...m, content: `✅ **[${agent_name}]** 完成` }
              : m
          )
        );
      }),
      listen<AgentTaskEvent>("agent-task-error", (e) => {
        const { agent_id, agent_name, error, task_id } = e.payload;
        setAgentStatuses((prev) => ({
          ...prev,
          [agent_id]: { status: "error", error: error ?? "" },
        }));
        setMessages((prev) =>
          prev.map((m) =>
            m.id === `agent-progress-${task_id}`
              ? { ...m, content: `❌ **[${agent_name}]** 失败: ${error}` }
              : m
          )
        );
      }),
      listen("agent-plan-start", (e: { payload: { task_count: number } }) => {
        if (e.payload.task_count > 0) {
          setMessages((prev) => [
            ...prev,
            {
              id: "agent-progress-plan",
              role: "system" as const,
              content: `🗂 **规划完成** — 共 ${e.payload.task_count} 个任务`,
            },
          ]);
        }
      }),
      listen("agent-aggregate-start", () => {
        setMessages((prev) => [
          ...prev,
          {
            id: "agent-progress-aggregate",
            role: "system" as const,
            content: `📝 **正在汇总所有子任务结果...**`,
          },
        ]);
      }),
    );
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

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

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setInput("");
    setAttachments([]);
    setStreaming(true);
    setError(null);

    const history = [...messages, userMsg]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-"))
      .map((m) => ({ role: m.role, content: m.content }));

    saveHistory(sessionId, "user", text);

    let accumulatedContent = "";
    let accumulatedReasoning = "";

    const cleanup = await chatCompletion(history, activeSkillIds, sessionId, selectedModel, {
      onToken(token) {
        accumulatedContent += token;
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, content: accumulatedContent } : m
          )
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
        const finalContentToSave = accumulatedReasoning
          ? `<details><summary>Thought Process</summary>\n\n${accumulatedReasoning}\n</details>\n\n${accumulatedContent}`
          : accumulatedContent;

        saveHistory(sessionId, "assistant", finalContentToSave);
        // Remove all agent progress system messages and finalize assistant message
        setMessages((prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => (m.id === assistantId ? { ...m, streaming: false } : m))
        );
        setAgentStatuses({});
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

    setMessages((prev) => [...prev, assistantMsg]);
    setStreaming(true);
    setError(null);

    const history = [...messages]
      .filter((m) => !m.streaming && !m.id.startsWith("agent-progress-"))
      .map((m) => ({ role: m.role, content: m.content }));

    let accumulatedContent = "";
    let accumulatedReasoning = "";

    const cleanup = await chatCompletion(history, activeSkillIds, sessionId, selectedModel, {
      onToken(token) {
        accumulatedContent += token;
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, content: accumulatedContent } : m
          )
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
        const finalContentToSave = accumulatedReasoning
          ? `<details><summary>Thought Process</summary>\n\n${accumulatedReasoning}\n</details>\n\n${accumulatedContent}`
          : accumulatedContent;

        saveHistory(sessionId, "assistant", finalContentToSave);
        setMessages((prev) =>
          prev
            .filter((m) => !m.id.startsWith("agent-progress-"))
            .map((m) => (m.id === assistantId ? { ...m, streaming: false } : m))
        );
        setPendingRetryMessageId(null);
        setAgentStatuses({});
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
        setStreaming(false);
        cleanupRef.current = null;
      },
    }, useAgentsEnabled);

    cleanupRef.current = cleanup;
  }, [messages, streaming, pendingRetryMessageId, activeSkillIds, sessionId, selectedModel, useAgentsEnabled]);

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
    const preview =
      confirmDialog.code.length > 400
        ? confirmDialog.code.slice(0, 400) + "…"
        : confirmDialog.code;

    return (
      <div className="confirm-overlay">
        <div className="confirm-dialog">
          <h2>⚠️ Dangerous command detected</h2>
          <p>
            <strong>Reason:</strong> {confirmDialog.reason}
          </p>
          <p>
            <strong>Type:</strong> {confirmDialog.cmd_type}
          </p>
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
        const stopSuffix = "\n\n[已停止生成]";
        const content = m.content.includes("[已停止生成]")
          ? m.content
          : (m.content || "") + stopSuffix;
        return { ...m, content, streaming: false };
      })
    );
    setStreaming(false);
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
              onLoad={(sid, msgs) => {
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
              showRetry={m.role === "user" && m.id === pendingRetryMessageId && !streaming}
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
