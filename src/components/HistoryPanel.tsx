import { useState, useEffect, useMemo } from "react";
import { loadHistory } from "../api";
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

  function handleLoad(sessionId: string, sessionRecords: HistoryRecord[]) {
    if (sessionId === currentSessionId) return;
    const messages: Message[] = sessionRecords.map((r) => ({
      id: crypto.randomUUID(),
      role: r.role as "user" | "assistant",
      content: r.content,
    }));
    onLoad(sessionId, messages);
    onClose();
  }

  return (
    <div className="history-panel">
      <div className="history-header">
        <h2>History</h2>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      <div className="history-list">
        {sessions.length === 0 ? (
          <p className="history-empty">No history yet.</p>
        ) : (
          sessions.map(([sid, recs]) => {
            const preview = recs.find((r) => r.role === "user")?.content ?? "(empty)";
            const isCurrent = sid === currentSessionId;
            return (
              <div
                key={sid}
                className={`history-item ${isCurrent ? "active" : ""}`}
                onClick={() => handleLoad(sid, recs)}
              >
                <span className="history-preview">
                  {preview.length > 60 ? preview.slice(0, 60) + "…" : preview}
                </span>
                <span className="history-meta">
                  {recs.length} messages{isCurrent ? " · current" : ""}
                </span>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
