import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { chatCompletion, getConfig, saveHistory, stopChatCompletion } from "./api";

import { ChatMessage } from "./components/ChatMessage";
import { SettingsPanel } from "./components/SettingsPanel";
import { SkillsPanel } from "./components/SkillsPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { ToolsPanel } from "./components/ToolsPanel";
import { RequestMonitorPanel } from "./components/RequestMonitorPanel";
import { McpPanel } from "./components/McpPanel";
import type { Message } from "./types";
import "./App.css";

type Sidebar = "settings" | "skills" | "history" | "tools" | "mcp" | "monitor" | null;

export default function App() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [attachments, setAttachments] = useState<{ name: string; content: string }[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
  const [activeSkillIds, setActiveSkillIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [usage, setUsage] = useState<{ prompt_tokens: number, completion_tokens: number } | null>(null);
  const [availableModels, setAvailableModels] = useState<string[]>(["gpt-4o-mini"]);
  const [selectedModel, setSelectedModel] = useState("gpt-4o-mini");
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
    const unlisten = listen<{ prompt_tokens: number; completion_tokens: number }>("chat-usage", (e) => {
      setUsage(e.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
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
    getConfig()
      .then((cfg) => {
        const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
        setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
        setSelectedModel(cfg.model || "gpt-4o-mini");
      })
      .catch(console.error);
  }, []);

  const sendMessage = useCallback(async () => {
    let text = input.trim();
    if ((!text && attachments.length === 0) || streaming) return;

    for (const file of attachments) {
      const ext = file.name.split('.').pop() || '';
      text += `\n\n<details><summary>Attached File: ${file.name}</summary>\n\n\`\`\`${ext}\n${file.content}\n\`\`\`\n</details>`;
    }
    text = text.trim();
    setUsage(null);

    const userMsg: Message = { id: crypto.randomUUID(), role: "user", content: text };
    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setInput("");
    setAttachments([]);
    setStreaming(true);
    setError(null);

    const history = [...messages, userMsg]
      .filter((m) => !m.streaming)
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
        // Optionally save reasoning context as well, but for now we'll just save the final content
        // Or append reasoning to content if we want it in history
        const finalContentToSave = accumulatedReasoning
          ? `<details><summary>Thought Process</summary>\n\n${accumulatedReasoning}\n</details>\n\n${accumulatedContent}`
          : accumulatedContent;

        saveHistory(sessionId, "assistant", finalContentToSave);
        setMessages((prev) =>
          prev.map((m) => (m.id === assistantId ? { ...m, streaming: false } : m))
        );
        setStreaming(false);
        cleanupRef.current = null;
      },
      onError(err) {
        setError(err);
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? { ...m, content: m.content || "Error: " + err, streaming: false }
              : m
          )
        );
        setStreaming(false);
        cleanupRef.current = null;
      },
    });

    cleanupRef.current = cleanup;
  }, [input, messages, streaming, activeSkillIds, sessionId, attachments, selectedModel]);

  function handleKeyDown(e: React.KeyboardEvent) {
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
    setSessionId(crypto.randomUUID());
  }

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

  return (
    <div className="app-layout">
      {/* Sidebar */}
      {sidebar && (
        <aside className={`sidebar${sidebar === "monitor" ? " sidebar-wide" : ""}`}>
          {sidebar === "settings" && (
            <SettingsPanel
              onClose={() => setSidebar(null)}
              onConfigSaved={(cfg) => {
                const catalog = Array.from(new Set([...(cfg.model_catalog ?? []), cfg.model].filter(Boolean)));
                setAvailableModels(catalog.length > 0 ? catalog : ["gpt-4o-mini"]);
                setSelectedModel(cfg.model || "gpt-4o-mini");
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
              onToolsChange={() => { }}
            />
          )}
          {sidebar === "mcp" && (
            <McpPanel onClose={() => setSidebar(null)} />
          )}
          {sidebar === "history" && (
            <HistoryPanel
              currentSessionId={sessionId}
              onLoad={(sid, msgs) => {
                if (cleanupRef.current) { cleanupRef.current(); cleanupRef.current = null; }
                setStreaming(false);
                setMessages(msgs);
                setSessionId(sid);
              }}
              onClose={() => setSidebar(null)}
            />
          )}
          {sidebar === "monitor" && (
            <RequestMonitorPanel onClose={() => setSidebar(null)} />
          )}
        </aside>
      )}

      {/* Main chat area */}
      <div className="chat-area">
        {/* Toolbar */}
        <header className="toolbar">
          <span className="app-title">Chat</span>
          <div className="toolbar-actions">
            {activeSkillIds.length > 0 && (
              <span className="skill-badge">{activeSkillIds.length} Skill{activeSkillIds.length > 1 ? "s" : ""} active</span>
            )}

            <button
              className={`toolbar-btn ${sidebar === "skills" ? "active" : ""}`}
              onClick={() => toggleSidebar("skills")}
              title="Skills"
            >
              ✦ Skills
            </button>
            <button
              className={`toolbar-btn ${sidebar === "mcp" ? "active" : ""}`}
              onClick={() => toggleSidebar("mcp")}
              title="MCP Servers"
            >
              ⬡ MCP
            </button>
            <button
              className={`toolbar-btn ${sidebar === "tools" ? "active" : ""}`}
              onClick={() => toggleSidebar("tools")}
              title="Tools"
            >
              🛠 Tools
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
              className={`toolbar-btn ${sidebar === "monitor" ? "active" : ""}`}
              onClick={() => toggleSidebar("monitor")}
              title="Request Monitor"
            >
              📡 Monitor
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
            <ChatMessage key={m.id} message={m} />
          ))}
          {error && <div className="error-banner">{error}</div>}
          <div ref={bottomRef} />
        </div>

        {/* Input */}
        <div className="input-area" style={{ position: "relative", flexDirection: "column", alignItems: "stretch" }}>
          {usage && (
            <div style={{ position: "absolute", top: "-15px", left: "10px", fontSize: "10px", color: "gray" }}>
              Tokens: {usage.prompt_tokens} prompt / {usage.completion_tokens} completion
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
              disabled={streaming}
            >
              📎
            </button>
            <input
              type="file"
              multiple
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
              disabled={streaming}
            />
            <select
              className="model-select"
              title="Select model"
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
              disabled={streaming}
            >
              {availableModels.map((model) => (
                <option key={model} value={model}>{model}</option>
              ))}
            </select>
            <button
              className="send-btn"
              onClick={streaming ? stopStreaming : sendMessage}
              disabled={!streaming && !input.trim() && attachments.length === 0}
            >
              {streaming ? "Stop" : "Send"}
            </button>
          </div>
        </div>
      </div >
    </div >
  );
}
