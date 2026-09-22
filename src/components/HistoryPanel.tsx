import { useState, useEffect, useMemo } from "react";
import { listHistorySessions, loadSessionMessages, deleteHistory, listSessionMeta, updateSessionMeta } from "../api";
import type { HistoryRecord, HistorySessionSummary, SessionMeta } from "../api";
import type { Attachment, Message, MessageContent, ToolCallEntry } from "../types";
import { useI18n, type Locale } from "../i18n";
import "./HistoryPanel.css";

interface Props {
  currentSessionId: string;
  onLoad: (sessionId: string, messages: Message[]) => void;
  disableSessionSwitch?: boolean;
  onClose: () => void;
}

type HistoryTab = "all" | "favorites" | "archived";

function formatHistoryTimestamp(timestamp: string, locale: Locale): string {
  const parsed = Date.parse(timestamp);
  if (Number.isNaN(parsed)) return timestamp;

  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(parsed);
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

/**
 * Convert the DB rows of one session into renderable chat messages. All
 * columns (tool calls, reasoning, attachments) are restored so the loaded
 * conversation renders exactly like the live one.
 */
function recordsToMessages(sessionRecords: HistoryRecord[]): Message[] {
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
  return messages;
}

export function HistoryPanel({ currentSessionId, onLoad, disableSessionSwitch = false, onClose }: Props) {
  const { t, locale } = useI18n();
  const [summaries, setSummaries] = useState<HistorySessionSummary[]>([]);
  const [searchKeyword, setSearchKeyword] = useState("");
  // Content-search hits, tagged with the keyword they answer so a slow query
  // can never filter a newer keyword's results.
  const [contentMatches, setContentMatches] = useState<{ keyword: string; ids: Set<string> } | null>(null);
  const [activeTab, setActiveTab] = useState<HistoryTab>("all");
  const [metaMap, setMetaMap] = useState<Map<string, SessionMeta>>(new Map());
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [loadingSessionId, setLoadingSessionId] = useState<string | null>(null);

  useEffect(() => {
    listHistorySessions().then(setSummaries).catch(console.error);
    listSessionMeta()
      .then((metas) => setMetaMap(new Map(metas.map((m) => [m.session_id, m]))))
      .catch(console.error);
  }, []);

  // Message-content search runs in SQLite — the panel only holds session
  // summaries, so the full history never has to fit in the webview. The
  // keyword is debounced to keep typing from firing a query per keystroke.
  useEffect(() => {
    const keyword = searchKeyword.trim();
    if (!keyword) {
      setContentMatches(null);
      return;
    }

    let cancelled = false;
    const timer = window.setTimeout(() => {
      listHistorySessions(keyword)
        .then((matches) => {
          if (!cancelled) {
            setContentMatches({ keyword, ids: new Set(matches.map((m) => m.session_id)) });
          }
        })
        .catch((err) => console.error("History search failed:", err));
    }, 180);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [searchKeyword]);

  const tabFilteredSessions = useMemo(() => {
    if (activeTab === "all") return summaries;
    return summaries.filter((summary) => {
      const meta = metaMap.get(summary.session_id);
      return activeTab === "favorites" ? !!meta?.favorite : !!meta?.archived;
    });
  }, [activeTab, summaries, metaMap]);

  const filteredSessions = useMemo(() => {
    const keyword = searchKeyword.trim().toLowerCase();
    if (!keyword) return tabFilteredSessions;

    const matchIds = contentMatches?.keyword === keyword.trim() ? contentMatches.ids : null;

    return tabFilteredSessions.filter(({ session_id: sid }) => {
      const meta = metaMap.get(sid);
      if (meta?.title?.toLowerCase().includes(keyword)) return true;
      if (sid.toLowerCase().includes(keyword)) return true;
      // Message-content hits arrive from the backend (debounced above).
      return matchIds?.has(sid) ?? false;
    });
  }, [searchKeyword, tabFilteredSessions, metaMap, contentMatches]);

  function getSessionTitle(summary: HistorySessionSummary): string {
    const meta = metaMap.get(summary.session_id);
    if (meta?.title && meta.title.trim()) return meta.title;
    // The backend sends the first user message's text only (extracted from
    // multimodal content and clamped), so the list stays lightweight.
    const parsed = parseStoredContent(summary.first_user_content);
    if (typeof parsed === "string") return parsed.trim() || t("history.titleEmpty");
    const textPart = parsed.find((p) => p.type === "text");
    return textPart?.text ?? t("history.titleAttachment");
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

  function startRename(summary: HistorySessionSummary) {
    setRenamingSessionId(summary.session_id);
    setRenameDraft(getSessionTitle(summary));
  }

  function commitRename(sid: string) {
    const trimmed = renameDraft.trim();
    if (trimmed) void patchMeta(sid, { title: trimmed });
    setRenamingSessionId(null);
    setRenameDraft("");
  }

  /** Fetch one session's complete message list and hand it to the app. */
  async function handleLoad(sessionId: string) {
    if (disableSessionSwitch || loadingSessionId) return;

    setLoadingSessionId(sessionId);
    try {
      const sessionRecords = await loadSessionMessages(sessionId);
      onLoad(sessionId, recordsToMessages(sessionRecords));
    } catch (err) {
      console.error("Failed to load session messages:", err);
    } finally {
      setLoadingSessionId(null);
    }
  }

  async function handleDelete(e: React.MouseEvent, sessionId: string) {
    e.stopPropagation();
    if (disableSessionSwitch) return;

    const currentSessions = filteredSessions;
    const deletedIndex = currentSessions.findIndex((s) => s.session_id === sessionId);
    const remainingSessions = currentSessions.filter((s) => s.session_id !== sessionId);

    try {
      await deleteHistory(sessionId);
      setSummaries((prev) => prev.filter((s) => s.session_id !== sessionId));
      setMetaMap((prev) => {
        const next = new Map(prev);
        next.delete(sessionId);
        return next;
      });

      if (remainingSessions.length > 0) {
        const targetIndex = deletedIndex > 0 ? deletedIndex - 1 : 0;
        const safeIndex = Math.min(targetIndex, remainingSessions.length - 1);
        await handleLoad(remainingSessions[safeIndex].session_id);
      }
    } catch (err) {
      console.error("Failed to delete history:", err);
    }
  }

  return (
    <div className="history-panel">
      <div className="history-header">
        <h2>{t("history.title")}</h2>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      <div className="history-search-wrap">
        <input
          className="history-search-input"
          type="text"
          value={searchKeyword}
          onChange={(e) => setSearchKeyword(e.target.value)}
          placeholder={t("history.searchPlaceholder")}
          aria-label={t("history.searchAria")}
        />
        {disableSessionSwitch && (
          <p className="history-switch-hint">{t("history.switchHint")}</p>
        )}
        <div className="history-tabs" role="tablist">
          {([
            ["all", t("history.tabAll")],
            ["favorites", t("history.tabFavorites")],
            ["archived", t("history.tabArchived")],
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
        {summaries.length === 0 ? (
          <p className="history-empty">{t("history.empty")}</p>
        ) : filteredSessions.length === 0 ? (
          <p className="history-empty">
            {searchKeyword
              ? t("history.noMatch", { keyword: searchKeyword })
              : activeTab === "favorites"
                ? t("history.noFavorites")
                : t("history.noArchived")}
          </p>
        ) : (
          filteredSessions.map((summary) => {
            const sid = summary.session_id;
            const meta = metaMap.get(sid);
            const isFavorite = !!meta?.favorite;
            const isArchived = !!meta?.archived;
            const isCurrent = sid === currentSessionId;
            const isLoading = loadingSessionId === sid;
            const createdAt = formatHistoryTimestamp(summary.created_at, locale);
            const isRenaming = renamingSessionId === sid;
            const title = getSessionTitle(summary);
            return (
              <div
                key={sid}
                className={`history-item ${isCurrent ? "active" : ""} ${disableSessionSwitch ? "disabled" : ""} ${isArchived ? "archived" : ""} ${isLoading ? "loading" : ""}`}
                onClick={() => void handleLoad(sid)}
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
                      aria-label={t("history.renameAria")}
                    />
                  ) : (
                    <span className="history-preview" title={title}>
                      {isFavorite ? "★ " : ""}
                      {title.length > 60 ? title.slice(0, 60) + "…" : title}
                    </span>
                  )}
                  <div className="history-footer">
                    <span className="history-meta">
                      {isLoading ? t("history.loading") : t("history.messageCount", { count: summary.message_count })}
                      {isCurrent ? t("history.current") : ""}{isArchived ? t("history.archived") : ""}
                    </span>
                    <span className="history-created">{t("history.created", { date: createdAt })}</span>
                  </div>
                </div>
                <div className="history-actions">
                  <button
                    className={`history-action-btn ${isFavorite ? "fav" : ""}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void patchMeta(sid, { favorite: !isFavorite });
                    }}
                    title={isFavorite ? t("history.removeFavorite") : t("history.addFavorite")}
                  >
                    {isFavorite ? "★" : "☆"}
                  </button>
                  <button
                    className="history-action-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      if (!isRenaming) startRename(summary);
                    }}
                    title={t("history.rename")}
                  >
                    ✏️
                  </button>
                  <button
                    className={`history-action-btn ${isArchived ? "on" : ""}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void patchMeta(sid, { archived: !isArchived });
                    }}
                    title={isArchived ? t("history.unarchive") : t("history.archive")}
                  >
                    {isArchived ? "📤" : "🗄"}
                  </button>
                  <button
                    className="history-action-btn delete"
                    onClick={(e) => handleDelete(e, sid)}
                    title={t("history.delete")}
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
