import { useState, useEffect, useMemo } from "react";
import { loadHistory, deleteHistory } from "../api";
import type { HistoryRecord } from "../api";
import type { Message, ToolCallEntry } from "../types";
import "./HistoryPanel.css";

interface Props {
  currentSessionId: string;
  onLoad: (sessionId: string, messages: Message[]) => void;
  disableSessionSwitch?: boolean;
  onClose: () => void;
}

function formatHistoryTimestamp(timestamp: string): string {
  const parsed = Date.parse(timestamp);
  if (Number.isNaN(parsed)) return timestamp;

  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(parsed);
}

function getSessionCreatedAt(sessionRecords: HistoryRecord[]): string {
  let earliest = Number.POSITIVE_INFINITY;
  let createdAt = sessionRecords[0]?.timestamp ?? "";

  for (const record of sessionRecords) {
    const parsed = Date.parse(record.timestamp);
    if (!Number.isNaN(parsed) && parsed < earliest) {
      earliest = parsed;
      createdAt = record.timestamp;
    }
  }

  return createdAt;
}

export function HistoryPanel({ currentSessionId, onLoad, disableSessionSwitch = false, onClose }: Props) {
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
    if (disableSessionSwitch) return;

    const messages: Message[] = [];
    for (const r of sessionRecords) {
      let toolCalls: ToolCallEntry[] | undefined;
      if (r.tool_calls) {
        try {
          const parsed = (JSON.parse(r.tool_calls) as ToolCallEntry[]).map((e) => ({
            ...e,
            status: "done" as const,
          }));
          if (parsed.length > 0) toolCalls = parsed;
        } catch { /* ignore malformed */ }
      }
      messages.push({
        id: crypto.randomUUID(),
        role: r.role as "user" | "assistant",
        content: r.content,
        tool_calls: toolCalls,
      });
    }

    onLoad(sessionId, messages);
  }

  async function handleDelete(e: React.MouseEvent, sessionId: string) {
    e.stopPropagation();
    if (disableSessionSwitch) return;

    const currentSessions = sessions;
    const deletedIndex = currentSessions.findIndex(([sid]) => sid === sessionId);
    const remainingSessions = currentSessions.filter(([sid]) => sid !== sessionId);

    try {
      await deleteHistory(sessionId);
      setRecords((prev) => prev.filter((r) => r.session_id !== sessionId));

      if (remainingSessions.length > 0) {
        const targetIndex = deletedIndex > 0 ? deletedIndex - 1 : 0;
        const safeIndex = Math.min(targetIndex, remainingSessions.length - 1);
        const [nextSessionId, nextSessionRecords] = remainingSessions[safeIndex];
        handleLoad(nextSessionId, nextSessionRecords);
      }
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
        {disableSessionSwitch && (
          <p className="history-switch-hint">A reply is being generated, switching sessions is temporarily unavailable</p>
        )}
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
            const createdAt = formatHistoryTimestamp(getSessionCreatedAt(recs));
            return (
              <div
                key={sid}
                className={`history-item ${isCurrent ? "active" : ""} ${disableSessionSwitch ? "disabled" : ""}`}
                onClick={() => handleLoad(sid, recs)}
              >
                <div className="history-content">
                  <span className="history-preview">
                    {preview.length > 60 ? preview.slice(0, 60) + "…" : preview}
                  </span>
                  <div className="history-footer">
                    <span className="history-meta">
                      {recs.length} messages{isCurrent ? " · current" : ""}
                    </span>
                    <span className="history-created">Created {createdAt}</span>
                  </div>
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
