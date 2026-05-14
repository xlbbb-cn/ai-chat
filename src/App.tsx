import { useState, useRef, useEffect, useCallback } from "react";
import { chatCompletion, saveHistory } from "./api";

import { ChatMessage } from "./components/ChatMessage";
import { SettingsPanel } from "./components/SettingsPanel";
import { SkillsPanel } from "./components/SkillsPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import type { Message } from "./types";
import "./App.css";

type Sidebar = "settings" | "skills" | "history" | null;

export default function App() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [webSearch, setWebSearch] = useState(false);
  const [sessionId, setSessionId] = useState<string>(() => crypto.randomUUID());
  const [sidebar, setSidebar] = useState<Sidebar>(null);
  const [activeSkillId, setActiveSkillId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const cleanupRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const sendMessage = useCallback(async () => {
    const text = input.trim();
    if (!text || streaming) return;

    const userMsg: Message = { id: crypto.randomUUID(), role: "user", content: text };
    const assistantId = crypto.randomUUID();
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", streaming: true };

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setInput("");
    setStreaming(true);
    setError(null);

    const history = [...messages, userMsg].map((m) => ({ role: m.role, content: m.content }));

    saveHistory(sessionId, "user", text);

    const cleanup = await chatCompletion(history, activeSkillId, webSearch, {
      onToken(token) {
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId ? { ...m, content: m.content + token } : m
          )
        );
      },
      onDone() {
        setMessages((prev) => {
           let finalMsg = prev.find(m => m.id === assistantId);
           if (finalMsg) saveHistory(sessionId, "assistant", finalMsg.content);
           return prev.map((m) => (m.id === assistantId ? { ...m, streaming: false } : m));
        });
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
  }, [input, messages, streaming, activeSkillId]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  function toggleSidebar(panel: Sidebar) {
    setSidebar((s) => (s === panel ? null : panel));
  }

  function clearChat() {
    if (streaming && cleanupRef.current) {
      cleanupRef.current();
      setStreaming(false);
    }
    setMessages([]);
    setError(null);
    setSessionId(crypto.randomUUID());
  }

  return (
    <div className="app-layout">
      {/* Sidebar */}
      {sidebar && (
        <aside className="sidebar">
          {sidebar === "settings" && (
            <SettingsPanel onClose={() => setSidebar(null)} />
          )}
          {sidebar === "skills" && (
            <SkillsPanel
              activeSkillId={activeSkillId}
              onSelect={setActiveSkillId}
              onClose={() => setSidebar(null)}
            />
          )}
          {sidebar === "history" && (
            <HistoryPanel
              currentSessionId={sessionId}
              onLoad={(sid, msgs) => { setMessages(msgs); setSessionId(sid); }}
              onClose={() => setSidebar(null)}
            />
          )}
        </aside>
      )}

      {/* Main chat area */}
      <div className="chat-area">
        {/* Toolbar */}
        <header className="toolbar">
          <span className="app-title">AI Chat</span>
          <div className="toolbar-actions">
            {activeSkillId && (
              <span className="skill-badge">Skill active</span>
            )}
            <button
              className={`toolbar-btn ${sidebar === "history" ? "active" : ""}`}
              onClick={() => toggleSidebar("history")}
              title="History"
            >
              🕒 History
            </button>
            <button
              className={`toolbar-btn ${sidebar === "skills" ? "active" : ""}`}
              onClick={() => toggleSidebar("skills")}
              title="Skills"
            >
              ✦ Skills
            </button>
            <button
              className={`toolbar-btn ${sidebar === "settings" ? "active" : ""}`}
              onClick={() => toggleSidebar("settings")}
              title="Settings"
            >
              ⚙ Settings
            </button>
            <label className="toolbar-btn" style={{display: 'flex', alignItems: 'center', gap: '5px'}}>
              <input type="checkbox" checked={webSearch} onChange={e => setWebSearch(e.target.checked)} />
              Web Search
            </label>
            <button className="toolbar-btn" onClick={clearChat} title="New chat">
              ✕ Clear
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
        <div className="input-area">
          <textarea
            className="chat-input"
            rows={1}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Type a message… (Enter to send, Shift+Enter for newline)"
            disabled={streaming}
          />
          <button
            className="send-btn"
            onClick={sendMessage}
            disabled={!input.trim() || streaming}
          >
            {streaming ? "…" : "Send"}
          </button>
        </div>
      </div>
    </div>
  );
}
