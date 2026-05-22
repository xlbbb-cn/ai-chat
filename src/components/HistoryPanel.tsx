import { useState, useEffect, useMemo } from "react";
import { loadHistory, deleteHistory } from "../api";
import type { HistoryRecord } from "../api";
import type { Message } from "../types";
import "./HistoryPanel.css";

interface Props {
  currentSessionId: string;
  onLoad: (sessionId: string, messages: Message[]) => void;
  onClose: () => void;
}

export function HistoryPanel({ currentSessionId, onLoad, onClose }: Props) {
  const [records, setRecords] = useState<HistoryRecord[]>([]);
  const [searchKeyword, setSearchKeyword] = useState("");

  useEffect(() => {
    loadHistory().then(setRecords).catch(console.error);
  }, []);

  const sessions = useMemo(() => {
    const map = new Map<string, HistoryRecord[]>();
    for (const rec of records) {
      if (!map.has(rec.session_id)) map.set(rec.session_id, []);
      map.get(rec.session_id)!.push(rec);
    }
    return Array.from(map.entries()).reverse();
  }, [records]);

  const filteredSessions = useMemo(() => {
    const keyword = searchKeyword.trim().toLowerCase();
    if (!keyword) return sessions;

    return sessions.filter(([sid, recs]) => {
      if (sid.toLowerCase().includes(keyword)) return true;
      return recs.some((rec) => rec.content.toLowerCase().includes(keyword));
    });
  }, [searchKeyword, sessions]);

  function handleLoad(sessionId: string, sessionRecords: HistoryRecord[]) {
    const messages: Message[] = sessionRecords.map((r) => ({
      id: crypto.randomUUID(),
      role: r.role as "user" | "assistant",
      content: r.content,
    }));

    onLoad(sessionId, messages);
  }

  async function handleDelete(e: React.MouseEvent, sessionId: string) {
    e.stopPropagation();
    try {
      await deleteHistory(sessionId);
      setRecords((prev) => prev.filter((r) => r.session_id !== sessionId));
    } catch (err) {
      console.error("Failed to delete history:", err);
    }
  }

  return (
    <div className="history-panel">
      <div className="history-header">
        <h2>History</h2>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      <div className="history-search-wrap">
        <input
          className="history-search-input"
          type="text"
          value={searchKeyword}
          onChange={(e) => setSearchKeyword(e.target.value)}
          placeholder="Search sessions by keyword"
          aria-label="Search history sessions"
        />
      </div>

      <div className="history-list">
        {sessions.length === 0 ? (
          <p className="history-empty">No history yet.</p>
        ) : filteredSessions.length === 0 ? (
          <p className="history-empty">No sessions matched "{searchKeyword}".</p>
        ) : (
          filteredSessions.map(([sid, recs]) => {
            const preview = recs.find((r) => r.role === "user")?.content ?? "(empty)";
            const isCurrent = sid === currentSessionId;
            return (
              <div
                key={sid}
                className={`history-item ${isCurrent ? "active" : ""}`}
                onClick={() => handleLoad(sid, recs)}
              >
                <div className="history-content">
                  <span className="history-preview">
                    {preview.length > 60 ? preview.slice(0, 60) + "…" : preview}
                  </span>
                  <span className="history-meta">
                    {recs.length} messages{isCurrent ? " · current" : ""}
                  </span>
                </div>
                <button
                  className="history-delete-btn"
                  onClick={(e) => handleDelete(e, sid)}
                  title="Delete session"
                >
                  🗑️
                </button>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
