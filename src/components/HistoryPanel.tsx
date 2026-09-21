import { useState, useEffect, useMemo } from "react";
import { loadHistory, deleteHistory, listSessionMeta, updateSessionMeta } from "../api";
import type { HistoryRecord, SessionMeta } from "../api";
import type { Attachment, Message, MessageContent, ToolCallEntry } from "../types";
import "./HistoryPanel.css";

interface Props {
  currentSessionId: string;
  onLoad: (sessionId: string, messages: Message[]) => void;
  disableSessionSwitch?: boolean;
  onClose: () => void;
}

type HistoryTab = "all" | "favorites" | "archived";

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

/**
 * Restore a multimodal `MessageContent` from its DB-stored string form.
 * Falls back to the raw string for plain text or malformed rows.
 */
function parseStoredContent(raw: string): MessageContent {
  if (!raw) return "";
  const trimmed = raw.trimStart();
  if (trimmed.startsWith("[")) {
    try {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) return parsed as MessageContent;
    } catch { /* fall through to plain text */ }
  }
  return raw;
}

/**
 * Restore the `Attachment[]` array from its DB-stored JSON form.
 * Returns `undefined` for missing/malformed rows so the message falls
 * back to the legacy `<details>` regex in `ChatMessage`.
 */
function parseStoredAttachments(raw: string | undefined): Attachment[] | undefined {
  if (!raw) return undefined;
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed) && parsed.length > 0) {
      return parsed as Attachment[];
    }
  } catch { /* fall through */ }
  return undefined;
}

export function HistoryPanel({ currentSessionId, onLoad, disableSessionSwitch = false, onClose }: Props) {
  const [records, setRecords] = useState<HistoryRecord[]>([]);
  const [searchKeyword, setSearchKeyword] = useState("");
  const [activeTab, setActiveTab] = useState<HistoryTab>("all");
  const [metaMap, setMetaMap] = useState<Map<string, SessionMeta>>(new Map());
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");

  useEffect(() => {
    loadHistory().then(setRecords).catch(console.error);
    listSessionMeta()
      .then((metas) => setMetaMap(new Map(metas.map((m) => [m.session_id, m]))))
      .catch(console.error);
  }, []);

  const sessions = useMemo(() => {
    const map = new Map<string, HistoryRecord[]>();
    for (const rec of records) {
      if (!map.has(rec.session_id)) map.set(rec.session_id, []);
      map.get(rec.session_id)!.push(rec);
    }
    return Array.from(map.entries()).reverse();
  }, [records]);

  const tabFilteredSessions = useMemo(() => {
    if (activeTab === "all") return sessions;
    return sessions.filter(([sid]) => {
      const meta = metaMap.get(sid);
      return activeTab === "favorites" ? !!meta?.favorite : !!meta?.archived;
    });
  }, [activeTab, sessions, metaMap]);

  const filteredSessions = useMemo(() => {
    const keyword = searchKeyword.trim().toLowerCase();
    if (!keyword) return tabFilteredSessions;

    return tabFilteredSessions.filter(([sid, recs]) => {
      const meta = metaMap.get(sid);
      if (meta?.title?.toLowerCase().includes(keyword)) return true;
      if (sid.toLowerCase().includes(keyword)) return true;
      return recs.some((rec) => {
        const parsed = parseStoredContent(rec.content);
        if (typeof parsed === "string") {
          return parsed.toLowerCase().includes(keyword);
        }
        return parsed.some(
          (p) => p.type === "text" && p.text.toLowerCase().includes(keyword),
        );
      });
    });
  }, [searchKeyword, tabFilteredSessions, metaMap]);

  function getSessionTitle(sid: string, recs: HistoryRecord[]): string {
    const meta = metaMap.get(sid);
    if (meta?.title && meta.title.trim()) return meta.title;
    const userRec = recs.find((r) => r.role === "user");
    if (!userRec) return "(empty)";
    const parsed = parseStoredContent(userRec.content);
    if (typeof parsed === "string") return parsed;
    const textPart = parsed.find((p) => p.type === "text");
    return textPart?.text ?? "(attachment)";
  }

  async function patchMeta(sid: string, fields: { title?: string; favorite?: boolean; archived?: boolean }) {
    try {
      await updateSessionMeta(sid, fields);
      setMetaMap((prev) => {
        const next = new Map(prev);
        const existing = next.get(sid);
        next.set(sid, {
          session_id: sid,
          title: fields.title ?? existing?.title,
          favorite: fields.favorite ?? existing?.favorite ?? false,
          archived: fields.archived ?? existing?.archived ?? false,
        });
        return next;
      });
    } catch (err) {
      console.error("Failed to update session meta:", err);
    }
  }

  function startRename(sid: string, recs: HistoryRecord[]) {
    setRenamingSessionId(sid);
    setRenameDraft(getSessionTitle(sid, recs));
  }

  function commitRename(sid: string) {
    const trimmed = renameDraft.trim();
    if (trimmed) void patchMeta(sid, { title: trimmed });
    setRenamingSessionId(null);
    setRenameDraft("");
  }

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
      const attachments = parseStoredAttachments(r.attachments);
      messages.push({
        id: crypto.randomUUID(),
        role: r.role as "user" | "assistant",
        content: parseStoredContent(r.content),
        ...(attachments ? { attachments } : {}),
        tool_calls: toolCalls,
        ...(r.reasoning_content ? { reasoning_content: r.reasoning_content } : {}),
        dbId: r.id,
      });
    }

    onLoad(sessionId, messages);
  }

  async function handleDelete(e: React.MouseEvent, sessionId: string) {
    e.stopPropagation();
    if (disableSessionSwitch) return;

    const currentSessions = tabFilteredSessions;
    const deletedIndex = currentSessions.findIndex(([sid]) => sid === sessionId);
    const remainingSessions = currentSessions.filter(([sid]) => sid !== sessionId);

    try {
      await deleteHistory(sessionId);
      setRecords((prev) => prev.filter((r) => r.session_id !== sessionId));
      setMetaMap((prev) => {
        const next = new Map(prev);
        next.delete(sessionId);
        return next;
      });

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
        <div className="history-tabs" role="tablist">
          {([
            ["all", "All"],
            ["favorites", "★ Favorites"],
            ["archived", "🗄 Archived"],
          ] as [HistoryTab, string][]).map(([tab, label]) => (
            <button
              key={tab}
              role="tab"
              aria-selected={activeTab === tab}
              className={`history-tab ${activeTab === tab ? "active" : ""}`}
              onClick={() => setActiveTab(tab)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      <div className="history-list">
        {sessions.length === 0 ? (
          <p className="history-empty">No history yet.</p>
        ) : filteredSessions.length === 0 ? (
          <p className="history-empty">
            {searchKeyword
              ? `No sessions matched "${searchKeyword}".`
              : activeTab === "favorites"
                ? "No favorite sessions yet."
                : "No archived sessions."}
          </p>
        ) : (
          filteredSessions.map(([sid, recs]) => {
            const meta = metaMap.get(sid);
            const isFavorite = !!meta?.favorite;
            const isArchived = !!meta?.archived;
            const isCurrent = sid === currentSessionId;
            const createdAt = formatHistoryTimestamp(getSessionCreatedAt(recs));
            const isRenaming = renamingSessionId === sid;
            const title = getSessionTitle(sid, recs);
            return (
              <div
                key={sid}
                className={`history-item ${isCurrent ? "active" : ""} ${disableSessionSwitch ? "disabled" : ""} ${isArchived ? "archived" : ""}`}
                onClick={() => handleLoad(sid, recs)}
              >
                <div className="history-content">
                  {isRenaming ? (
                    <input
                      className="history-rename-input"
                      type="text"
                      value={renameDraft}
                      autoFocus
                      onClick={(e) => e.stopPropagation()}
                      onChange={(e) => setRenameDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitRename(sid);
                        if (e.key === "Escape") setRenamingSessionId(null);
                      }}
                      onBlur={() => commitRename(sid)}
                      aria-label="Rename session"
                    />
                  ) : (
                    <span className="history-preview" title={title}>
                      {isFavorite ? "★ " : ""}
                      {title.length > 60 ? title.slice(0, 60) + "…" : title}
                    </span>
                  )}
                  <div className="history-footer">
                    <span className="history-meta">
                      {recs.length} messages{isCurrent ? " · current" : ""}{isArchived ? " · archived" : ""}
                    </span>
                    <span className="history-created">Created {createdAt}</span>
                  </div>
                </div>
                <div className="history-actions">
                  <button
                    className={`history-action-btn ${isFavorite ? "fav" : ""}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void patchMeta(sid, { favorite: !isFavorite });
                    }}
                    title={isFavorite ? "Remove from favorites" : "Add to favorites"}
                  >
                    {isFavorite ? "★" : "☆"}
                  </button>
                  <button
                    className="history-action-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      if (!isRenaming) startRename(sid, recs);
                    }}
                    title="Rename session"
                  >
                    ✏️
                  </button>
                  <button
                    className={`history-action-btn ${isArchived ? "on" : ""}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void patchMeta(sid, { archived: !isArchived });
                    }}
                    title={isArchived ? "Unarchive session" : "Archive session"}
                  >
                    {isArchived ? "📤" : "🗄"}
                  </button>
                  <button
                    className="history-action-btn delete"
                    onClick={(e) => handleDelete(e, sid)}
                    title="Delete session"
                  >
                    🗑️
                  </button>
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
