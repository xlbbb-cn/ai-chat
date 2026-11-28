import { useState, useEffect, useLayoutEffect, useMemo, useRef } from "react";
import { listHistorySessions, loadSessionMessages, deleteHistory, listSessionMeta, updateSessionMeta } from "../api";
import type { HistorySessionSummary, SessionMeta } from "../api";
import { recordsToMessages, parseStoredContent } from "../historyRows";
import type { Message } from "../types";
import { useI18n, type Locale, type MessageKey } from "../i18n";
import {
  FontAwesomeIcon,
  faAnglesLeft,
  faBoxArchive,
  faBoxOpen,
  faListCheck,
  faMagnifyingGlass,
  faPenToSquare,
  faStar,
  faStarOutline,
  faTrash,
  faXmark,
  type IconDefinition,
} from "../icons";
import { formatUptime } from "../utils/time";
import "./HistoryPanel.css";

interface Props {
  currentSessionId: string;
  onLoad: (sessionId: string, messages: Message[]) => void;
  /**
   * Bumped by the parent when the active workspace changed: the history
   * database lives inside the workspace (`<workspace>/.data/chat.db`), so the
   * sidebar must re-query instead of showing the previous workspace's sessions.
   */
  refreshKey?: number;
  /**
   * Sessions whose turn is currently running (foreground or background), with
   * the turn's start time so the row can show this turn's elapsed duration and
   * the model it runs with (the backend publishes it with the turn: a session
   * keeps its own model even when another conversation picked a different one).
   * Switching is always allowed — the app hands a running foreground turn to
   * its background driver on the way out.
   */
  runningSessions?: Record<
    string,
    { started_at_ms: number; model?: string; effort?: string }
  >;
  onClose: () => void;
}

type HistoryTab = "all" | "favorites" | "archived";

/**
 * Depth labels reused from the composer's thinking-depth control: the badge and
 * that control must never word the same state differently.
 */
const DEPTH_LABELS = {
  low: "app.reasoningLow",
  medium: "app.reasoningMedium",
  high: "app.reasoningHigh",
} as const satisfies Record<string, MessageKey>;

const DAY_MS = 86_400_000;

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/**
 * Short, list-scale timestamp: the time of day for today, "yesterday", the
 * weekday inside the last week, then a date. The full date stays available
 * as the row's tooltip.
 */
function formatHistoryTimestamp(timestamp: string, locale: Locale): string {
  const parsed = Date.parse(timestamp);
  if (Number.isNaN(parsed)) return timestamp;

  const then = new Date(parsed);
  const now = new Date();
  const dayDiff = Math.round((startOfDay(now) - startOfDay(then)) / DAY_MS);

  if (dayDiff === 0) {
    return new Intl.DateTimeFormat(locale, {
      hour: "2-digit",
      minute: "2-digit",
    }).format(then);
  }
  if (dayDiff === 1) {
    return new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(-1, "day");
  }
  if (dayDiff > 1 && dayDiff < 7) {
    return new Intl.DateTimeFormat(locale, { weekday: "short" }).format(then);
  }

  return new Intl.DateTimeFormat(locale, {
    year: then.getFullYear() === now.getFullYear() ? undefined : "numeric",
    month: "short",
    day: "numeric",
  }).format(then);
}

function formatHistoryTimestampFull(timestamp: string, locale: Locale): string {
  const parsed = Date.parse(timestamp);
  if (Number.isNaN(parsed)) return timestamp;
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(parsed);
}

/**
 * Convert the DB rows of one session into renderable chat messages.
 * Shared with the reload re-attach path (`src/historyRows.ts`).
 */

export function HistoryPanel({ currentSessionId, onLoad, refreshKey = 0, runningSessions, onClose }: Props) {
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
  /** Wall clock behind the running badge's live duration (see the tick below). */
  const [nowMs, setNowMs] = useState(() => Date.now());
  /** Measured geometry of the sliding tab fill (see the layout effect below). */
  const [tabIndicator, setTabIndicator] = useState<{ left: number; width: number } | null>(null);
  /** Measured geometry of the session-list highlight, in list content space. */
  const [selectionFrame, setSelectionFrame] = useState<{
    top: number;
    left: number;
    width: number;
    height: number;
  } | null>(null);
  const tabsRef = useRef<HTMLDivElement>(null);
  const tabRefs = useRef<Map<HistoryTab, HTMLButtonElement>>(new Map());
  const listRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<Map<string, HTMLDivElement>>(new Map());

  useEffect(() => {
    listHistorySessions().then(setSummaries).catch(console.error);
    listSessionMeta()
      .then((metas) => setMetaMap(new Map(metas.map((m) => [m.session_id, m]))))
      .catch(console.error);
  }, [refreshKey]);

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

  const anyRunning = !!runningSessions && Object.keys(runningSessions).length > 0;

  // The running badge counts this turn's duration, so the panel ticks once a
  // second — and only while a turn is actually running. The start time itself
  // comes from the backend turn registry, so a reload mid-turn keeps counting
  // from the real start instead of restarting at zero.
  useEffect(() => {
    if (!anyRunning) return;
    setNowMs(Date.now());
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [anyRunning]);

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

  /**
   * Sliding tab selection.
   *
   * The fill is one absolutely positioned layer rather than a background on
   * `.active`, so changing tabs makes it travel instead of blinking from one
   * button to the other. Geometry is measured from the buttons because their
   * widths depend on the locale.
   */
  useLayoutEffect(() => {
    const tabs = tabsRef.current;
    const active = tabRefs.current.get(activeTab);
    if (!tabs || !active) {
      setTabIndicator(null);
      return;
    }

    const measure = () => {
      // `offsetLeft` and the absolutely positioned indicator share the same
      // origin (the container's padding edge), so the cell's offset is used
      // as-is; adding the border width here would shift the fill left.
      setTabIndicator({ left: active.offsetLeft, width: active.offsetWidth });
    };

    measure();
    const observer = new ResizeObserver(measure);
    if (tabsRef.current) observer.observe(tabsRef.current);
    tabRefs.current.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  }, [activeTab]);

  /**
   * Sliding session selection.
   *
   * The highlight is a single layer *behind* the rows, positioned in the
   * list's scrollable content coordinates: it scrolls with the list and
   * animates between rows when the current session changes. Measuring also
   * keeps it correct when filtering, renaming or a metadata edit changes the
   * layout.
   */
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;

    const measure = () => {
      const row = itemRefs.current.get(currentSessionId);
      if (!row) {
        setSelectionFrame(null);
        return;
      }
      const listRect = list.getBoundingClientRect();
      const rowRect = row.getBoundingClientRect();
      setSelectionFrame({
        top: rowRect.top - listRect.top + list.scrollTop,
        left: rowRect.left - listRect.left + list.scrollLeft,
        width: rowRect.width,
        height: rowRect.height,
      });
    };

    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(list);
    const activeRow = itemRefs.current.get(currentSessionId);
    if (activeRow) observer.observe(activeRow);
    return () => observer.disconnect();
  }, [currentSessionId, filteredSessions, renamingSessionId, metaMap]);

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
    if (loadingSessionId) return;

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

  /**
   * Keyboard traversal of the session list. Arrow keys move focus only —
   * loading a session is a deliberate action (Enter / Space), because each
   * load reads a full transcript from SQLite and replaces the chat.
   */
  function focusSessionAt(index: number) {
    const target = filteredSessions[index];
    if (!target) return;
    const row = itemRefs.current.get(target.session_id);
    if (!row) return;
    row.focus({ preventScroll: true });
    row.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  function handleSessionKeyDown(
    event: React.KeyboardEvent<HTMLDivElement>,
    index: number,
    sessionId: string,
  ) {
    // A row contains its own buttons and the rename input; never steal their
    // keys (Enter on the trash can must delete, not reload the chat).
    if ((event.target as HTMLElement).closest("button, input, textarea, select, a")) {
      return;
    }

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        focusSessionAt(index + 1);
        break;
      case "ArrowUp":
        event.preventDefault();
        focusSessionAt(index - 1);
        break;
      case "Home":
        event.preventDefault();
        focusSessionAt(0);
        break;
      case "End":
        event.preventDefault();
        focusSessionAt(filteredSessions.length - 1);
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        void handleLoad(sessionId);
        break;
    }
  }

  return (
    <div className="history-panel">
      <div className="panel-header">
        <h2>{t("history.title")}</h2>
        <button className="close-btn" onClick={onClose} aria-label={t("common.collapse")}>
          <FontAwesomeIcon icon={faAnglesLeft} />
        </button>
      </div>

      <div className="history-search-wrap">
        <div className="history-search-field">
          <FontAwesomeIcon
            className="history-search-icon"
            icon={faMagnifyingGlass}
            aria-hidden="true"
          />
          <input
            className="history-search-input"
            type="text"
            value={searchKeyword}
            onChange={(e) => setSearchKeyword(e.target.value)}
            placeholder={t("history.searchPlaceholder")}
            aria-label={t("history.searchAria")}
          />
          {searchKeyword && (
            <button
              type="button"
              className="history-search-clear"
              onClick={() => setSearchKeyword("")}
              onMouseDown={(e) => e.preventDefault()}
              aria-label={t("history.clearSearch")}
              title={t("history.clearSearch")}
            >
              <FontAwesomeIcon icon={faXmark} />
            </button>
          )}
        </div>
        <div className="history-tabs" role="tablist" ref={tabsRef}>
          {tabIndicator && (
            <span
              className="history-tabs-indicator"
              aria-hidden="true"
              style={{
                transform: `translateX(${tabIndicator.left}px)`,
                width: tabIndicator.width,
              }}
            />
          )}
          {([
            ["all", t("history.tabAll"), faListCheck],
            ["favorites", t("history.tabFavorites"), faStar],
            ["archived", t("history.tabArchived"), faBoxArchive],
          ] as [HistoryTab, string, IconDefinition][]).map(([tab, label, icon]) => (
            <button
              key={tab}
              ref={(el) => {
                if (el) tabRefs.current.set(tab, el);
                else tabRefs.current.delete(tab);
              }}
              role="tab"
              aria-selected={activeTab === tab}
              className={`history-tab ${activeTab === tab ? "active" : ""}`}
              onClick={() => setActiveTab(tab)}
            >
              <FontAwesomeIcon icon={icon} />
              {label}
            </button>
          ))}
        </div>
      </div>

      <div className="history-list" ref={listRef}>
        {selectionFrame && (
          <span
            className="history-selection"
            aria-hidden="true"
            style={{
              transform: `translate(${selectionFrame.left}px, ${selectionFrame.top}px)`,
              width: selectionFrame.width,
              height: selectionFrame.height,
            }}
          />
        )}
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
          filteredSessions.map((summary, index) => {
            const sid = summary.session_id;
            const meta = metaMap.get(sid);
            const isFavorite = !!meta?.favorite;
            const isArchived = !!meta?.archived;
            const isCurrent = sid === currentSessionId;
            const isLoading = loadingSessionId === sid;
            const createdAt = formatHistoryTimestamp(summary.created_at, locale);
            const isRenaming = renamingSessionId === sid;
            const title = getSessionTitle(summary);
            const runningEntry = runningSessions?.[sid];
            const runningStartedAt = runningEntry?.started_at_ms;
            const isRunning = runningStartedAt !== undefined;
            const runningSeconds = runningStartedAt === undefined
              ? 0
              : Math.max(0, Math.floor((nowMs - runningStartedAt) / 1000));
            const runningModel = runningEntry?.model;
            // `minimal` is accepted by the backend but not offered by the
            // composer's control: show it verbatim rather than mislabelling it.
            const depthKey = runningEntry?.effort
              ? DEPTH_LABELS[runningEntry.effort as keyof typeof DEPTH_LABELS]
              : undefined;
            const runningDepth = depthKey
              ? t(depthKey)
              : runningEntry?.effort || t("app.reasoningDefault");
            return (
              <div
                key={sid}
                ref={(el) => {
                  if (el) itemRefs.current.set(sid, el);
                  else itemRefs.current.delete(sid);
                }}
                className={`history-item ${isCurrent ? "active" : ""} ${isArchived ? "archived" : ""} ${isLoading ? "loading" : ""}`}
                tabIndex={0}
                aria-current={isCurrent ? "true" : undefined}
                onClick={() => void handleLoad(sid)}
                onKeyDown={(event) => handleSessionKeyDown(event, index, sid)}
              >
                <div className="history-head">
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
                      {title.length > 60 ? title.slice(0, 60) + "…" : title}
                    </span>
                  )}
                  <div className="history-actions">
                    <button
                      className={`history-action-btn ${isFavorite ? "fav" : ""}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        void patchMeta(sid, { favorite: !isFavorite });
                      }}
                      title={isFavorite ? t("history.removeFavorite") : t("history.addFavorite")}
                    >
                      <FontAwesomeIcon icon={isFavorite ? faStar : faStarOutline} />
                    </button>
                    <button
                      className="history-action-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        if (!isRenaming) startRename(summary);
                      }}
                      title={t("history.rename")}
                    >
                      <FontAwesomeIcon icon={faPenToSquare} />
                    </button>
                    <button
                      className={`history-action-btn ${isArchived ? "on" : ""}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        void patchMeta(sid, { archived: !isArchived });
                      }}
                      title={isArchived ? t("history.unarchive") : t("history.archive")}
                    >
                      <FontAwesomeIcon icon={isArchived ? faBoxOpen : faBoxArchive} />
                    </button>
                    <button
                      className="history-action-btn delete"
                      onClick={(e) => handleDelete(e, sid)}
                      title={t("history.delete")}
                    >
                      <FontAwesomeIcon icon={faTrash} />
                    </button>
                  </div>
                </div>
                <div className="history-footer">
                  <span className="history-meta">
                    {isLoading ? t("history.loading") : t("history.messageCount", { count: summary.message_count })}
                    {isArchived ? t("history.archived") : ""}
                  </span>
                  {isRunning && (
                    <span
                      className="history-running-badge"
                      title={
                        runningModel
                          ? t("history.runningTitleWithModel", {
                              model: runningModel,
                              depth: runningDepth,
                            })
                          : t("history.runningTitle")
                      }
                    >
                      {/* The model keeps its own conversation visible here: a
                          turn running in the background is not using whatever
                          model this window has selected. */}
                      {runningModel && (
                        <span className="history-running-model">{runningModel}</span>
                      )}
                      {t("history.running")}
                      <span className="history-running-time">{formatUptime(runningSeconds)}</span>
                    </span>
                  )}
                  <span
                    className="history-created"
                    title={formatHistoryTimestampFull(summary.created_at, locale)}
                  >
                    {createdAt}
                  </span>
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
