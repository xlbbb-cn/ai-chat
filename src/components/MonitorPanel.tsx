import { useState, useEffect, useRef } from "react";
import { listInteractions, getInteraction, clearLogs, compactDatabase, getSessionStats } from "../api";
import type { InteractionLogRecord, InteractionLogDetail, SessionStats } from "../api";
import { useI18n } from "../i18n";
import { formatBytes } from "../utils/format";
import { FontAwesomeIcon, faSpinner, faXmark } from "../icons";
import "./MonitorPanel.css";

interface Props {
    sessionId: string;
    /**
     * A turn is streaming. The log rows are written by the streaming thread, so
     * the panel shows a "wait for the turn" notice instead of reading them.
     */
    busy?: boolean;
    onClose: () => void;
}

/** Compact token count: `1.2K`, `34.5K`, `1.2M`. */
function formatTokens(tokens: number): string {
    if (tokens < 1000) return tokens.toLocaleString();
    if (tokens < 1_000_000) return `${(tokens / 1000).toFixed(tokens < 10_000 ? 1 : 0)}K`;
    return `${(tokens / 1_000_000).toFixed(1)}M`;
}

/** Wall clock for a request/tool call: `820ms`, `4.2s`, `1m 12s`. */
function formatDuration(ms: number): string {
    if (ms <= 0) return "—";
    if (ms < 1000) return `${Math.round(ms)}ms`;
    if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
    const minutes = Math.floor(ms / 60_000);
    return `${minutes}m ${Math.round((ms % 60_000) / 1000)}s`;
}

function averageDuration(totalMs: number, count: number): string {
    return count > 0 ? formatDuration(totalMs / count) : "—";
}

/** `73%`, and `0%` instead of `NaN` when the divisor is empty. */
function percent(part: number, total: number): string {
    if (total <= 0) return "0%";
    return `${Math.round((part / total) * 100)}%`;
}

/** Cache hit rate over the session's prompt tokens. */
function hitRate(stats: SessionStats): number {
    if (stats.prompt_tokens <= 0) return 0;
    return Math.min(100, Math.round((stats.hit_tokens / stats.prompt_tokens) * 100));
}

/**
 * Prompt-cache metrics carried in an `llm_api` row's metadata JSON.
 */
interface CacheInfo {
    cached: number;
    prompt: number;
    /** 0–100, computed from the weighted aggregate of the row itself. */
    percent: number;
}

/**
 * Parse the cache-hit metrics out of a row's metadata JSON. Rows with no
 * reported caching (older rows, non-chat tools, endpoints without a cache
 * dialect) return `null` instead of a fake `0%`.
 */
function cacheInfo(metadata: string | undefined): CacheInfo | null {
    if (!metadata || metadata === "{}") return null;
    try {
        const parsed = JSON.parse(metadata) as {
            cached_tokens?: unknown;
            prompt_tokens?: unknown;
            hit_ratio?: unknown;
        };
        const cached = typeof parsed.cached_tokens === "number" ? parsed.cached_tokens : null;
        const prompt = typeof parsed.prompt_tokens === "number" ? parsed.prompt_tokens : null;
        if (cached === null || prompt === null || prompt <= 0) return null;
        return { cached, prompt, percent: Math.round((cached / prompt) * 100) };
    } catch {
        return null;
    }
}

/** Weighted aggregate across the loaded rows that reported cache data. */
function aggregateCacheInfo(interactions: { metadata: string; interaction_type: string }[]): CacheInfo | null {
    let cached = 0;
    let prompt = 0;
    for (const interaction of interactions) {
        if (interaction.interaction_type !== "llm_api") continue;
        const info = cacheInfo(interaction.metadata);
        if (!info) continue;
        cached += info.cached;
        prompt += info.prompt;
    }
    if (prompt <= 0) return null;
    return { cached, prompt, percent: Math.round((cached / prompt) * 100) };
}

/**
 * Thinking phase of one `llm_api` row, from its metadata JSON. `null` when the
 * model streamed no reasoning (or the row predates the metric) — never a fake
 * `0 ms`.
 */
function rowReasoningMs(metadata: string | undefined): number | null {
    if (!metadata || metadata === "{}") return null;
    try {
        const parsed = JSON.parse(metadata) as { reasoning_ms?: unknown };
        return typeof parsed.reasoning_ms === "number" ? parsed.reasoning_ms : null;
    } catch {
        return null;
    }
}

/**
 * Error rows of the dispatch path. `mcp_call` is the *failure* row of an MCP
 * call (`mcp_response` is the success one) — see `log_interaction` in
 * `src-tauri/src/llm_complete.rs`.
 */
function isErrorRow(type: string): boolean {
    return type === "llm_error" || type === "tool_error" || type === "mcp_call";
}

/** One metric of the session report. */
function ReportTile({
    label,
    value,
    detail,
    meter,
    tone,
}: {
    label: string;
    value: string;
    detail?: string;
    /** 0–100: renders a 2px progress track under the value. */
    meter?: number;
    tone?: "accent" | "danger";
}) {
    return (
        <div className="report-tile">
            <span className="report-tile-label">{label}</span>
            <span className="report-tile-value">{value}</span>
            {meter !== undefined && (
                <span
                    className={`report-tile-meter${tone === "danger" ? " is-danger" : ""}`}
                    role="img"
                    aria-label={`${label} ${meter}%`}
                >
                    <span style={{ width: `${Math.max(0, Math.min(100, meter))}%` }} />
                </span>
            )}
            {detail && <span className="report-tile-detail">{detail}</span>}
        </div>
    );
}

export function MonitorPanel({ sessionId, busy = false, onClose }: Props) {
    const { t, locale } = useI18n();
    const [interactions, setInteractions] = useState<InteractionLogRecord[]>([]);
    const [selectedInteraction, setSelectedInteraction] = useState<InteractionLogDetail | null>(null);
    const [stats, setStats] = useState<SessionStats | null>(null);
    const [loading, setLoading] = useState(false);
    const [loadError, setLoadError] = useState<string | null>(null);
    /** Bumped by the refresh button to re-run the load effect. */
    const [reloadToken, setReloadToken] = useState(0);
    const [maintenance, setMaintenance] = useState<"idle" | "clearing" | "compacting">("idle");
    const [status, setStatus] = useState("");
    /** Set while a turn runs, so the load that follows it can wait a beat. */
    const afterTurn = useRef(false);

    // Read the log on open, after a turn ends and on demand — never while a
    // turn is running: that is when the rows are being written, and a
    // half-written session only costs the writer its lock. The two commands
    // are independent, so a failing report never blanks the list.
    useEffect(() => {
        if (busy) {
            afterTurn.current = true;
            setLoading(false);
            return;
        }
        let cancelled = false;
        // A finished turn writes its last rows just before `busy` flips, so a
        // load that follows one gives the writer a beat to land them.
        const delay = afterTurn.current ? 500 : 0;
        afterTurn.current = false;

        const load = async () => {
            setLoading(true);
            const [rowsResult, statsResult] = await Promise.allSettled([
                listInteractions(sessionId),
                getSessionStats(sessionId),
            ]);
            if (cancelled) return;
            if (rowsResult.status === "fulfilled") {
                setInteractions(rowsResult.value);
                setLoadError(null);
            } else {
                console.error("Failed to load interactions:", rowsResult.reason);
                setLoadError(String(rowsResult.reason));
            }
            if (statsResult.status === "fulfilled") {
                setStats(statsResult.value);
            } else {
                console.error("Failed to load the session report:", statsResult.reason);
            }
            setLoading(false);
        };

        const timer = setTimeout(load, delay);
        return () => {
            cancelled = true;
            clearTimeout(timer);
        };
    }, [sessionId, busy, reloadToken]);

    // Load selected interaction detail
    const handleSelectInteraction = async (record: InteractionLogRecord) => {
        try {
            const detail = await getInteraction(record.id);
            setSelectedInteraction(detail);
        } catch (err) {
            console.error("Failed to load interaction detail:", err);
        }
    };

    const formatDate = (dateStr: string): string => {
        try {
            const date = new Date(dateStr);
            return date.toLocaleTimeString(locale);
        } catch {
            return dateStr;
        }
    };

    const tryParseJson = (str: string): string => {
        if (!str) return "";
        try {
            const obj = JSON.parse(str);
            return JSON.stringify(obj, null, 2);
        } catch {
            return str;
        }
    };

    /** Delete all log rows (requests + interactions). History is untouched. */
    const handleClearLogs = async () => {
        if (!window.confirm(t("monitor.clearConfirm"))) return;
        setMaintenance("clearing");
        setStatus("");
        try {
            const removed = await clearLogs();
            setSelectedInteraction(null);
            setInteractions([]);
            setLoadError(null);
            setStats(await getSessionStats(sessionId));
            setStatus(t("monitor.removed", { count: removed }));
        } catch (err) {
            console.error("Failed to clear logs:", err);
            setStatus(t("monitor.clearFailed", { error: String(err) }));
        } finally {
            setMaintenance("idle");
        }
    };

    /** Apply the retention window, then VACUUM to return space to the disk. */
    const handleCompact = async () => {
        if (!window.confirm(t("monitor.compactConfirm"))) return;
        setMaintenance("compacting");
        setStatus(t("monitor.compacting2"));
        try {
            const result = await compactDatabase();
            const freed = result.bytes_before - result.bytes_after;
            const window = result.retention_days > 0
                ? t("monitor.window", { days: result.retention_days })
                : t("monitor.retentionDisabled");
            setStatus(
                t("monitor.compacted", {
                    before: formatBytes(result.bytes_before),
                    after: formatBytes(result.bytes_after),
                }) +
                (freed > 0 ? t("monitor.freed", { bytes: formatBytes(freed) }) : "") +
                (result.rows_pruned > 0 ? t("monitor.pruned", { count: result.rows_pruned.toLocaleString() }) : "") +
                window
            );
        } catch (err) {
            console.error("Failed to compact database:", err);
            setStatus(t("monitor.compactFailed", { error: String(err) }));
        } finally {
            setMaintenance("idle");
        }
    };

    /** Weighted prompt-cache hit rate over the loaded `llm_api` rows. */
    const cacheAggregate = aggregateCacheInfo(interactions);

    const cacheRate = stats ? hitRate(stats) : 0;
    const contextPercent =
        stats && stats.context_window > 0
            ? Math.round((stats.context_tokens / stats.context_window) * 100)
            : null;
    const reportEmpty = !stats || (stats.requests === 0 && stats.interactions === 0);

    return (
        <div className="monitor-overlay" role="dialog" aria-modal="true" aria-label={t("monitor.aria")}>
            <div className="monitor-shell">
                <div className="monitor-header">
                    <h2>{t("monitor.title")}</h2>
                    <div className="monitor-controls">
                        {status && <span className="monitor-status">{status}</span>}
                        <button
                            type="button"
                            className="monitor-action-btn"
                            onClick={() => setReloadToken((token) => token + 1)}
                            disabled={busy || loading || maintenance !== "idle"}
                            title={t("monitor.refresh")}
                        >
                            {t("monitor.refresh")}
                        </button>
                        <button
                            type="button"
                            className="monitor-action-btn"
                            onClick={handleClearLogs}
                            disabled={maintenance !== "idle"}
                            title={t("monitor.clearTitle")}
                        >
                            {maintenance === "clearing" ? t("monitor.clearing") : t("monitor.clearLogs")}
                        </button>
                        <button
                            type="button"
                            className="monitor-action-btn"
                            onClick={handleCompact}
                            disabled={maintenance !== "idle"}
                            title={t("monitor.compactTitle")}
                        >
                            {maintenance === "compacting" ? t("monitor.compacting") : t("monitor.compact")}
                        </button>
                        <button type="button" className="close-btn" onClick={onClose} aria-label={t("common.close")}>
                            <FontAwesomeIcon icon={faXmark} />
                        </button>
                    </div>
                </div>

                <div className="monitor-body">
                    <div className="monitor-panes">
                        {/* Left Panel: Interaction List */}
                        <div className="monitor-list-panel">
                            <div className="monitor-list-header">
                                <h3>{t("monitor.interactions", { count: interactions.length })}</h3>
                                {cacheAggregate && (
                                    <span
                                        className="monitor-cache-avg"
                                        title={t("monitor.cacheAvg", { percent: cacheAggregate.percent })}
                                    >
                                        {t("monitor.cacheAvg", { percent: cacheAggregate.percent })}
                                    </span>
                                )}
                                {loading && (
                                    <span className="loading-spinner">
                                        <FontAwesomeIcon icon={faSpinner} spin />
                                    </span>
                                )}
                            </div>
                            <div className="monitor-list">
                                {busy ? (
                                    <div className="monitor-empty">{t("monitor.waitForTurn")}</div>
                                ) : interactions.length === 0 ? (
                                    <div className="monitor-empty">
                                        {loadError ? t("monitor.loadFailed") : t("monitor.noInteractions")}
                                    </div>
                                ) : (
                                    interactions.map((interaction) => {
                                        const rowCache =
                                            interaction.interaction_type === "llm_api"
                                                ? cacheInfo(interaction.metadata)
                                                : null;
                                        const rowThinking =
                                            interaction.interaction_type === "llm_api"
                                                ? rowReasoningMs(interaction.metadata)
                                                : null;
                                        return (
                                            <div
                                                key={interaction.id}
                                                className={`monitor-item ${selectedInteraction?.id === interaction.id ? "selected" : ""
                                                    }`}
                                                onClick={() => handleSelectInteraction(interaction)}
                                                role="button"
                                                tabIndex={0}
                                            >
                                                <div className="monitor-item-header">
                                                    <span
                                                        className={`monitor-type-badge${isErrorRow(interaction.interaction_type) ? " is-error" : ""
                                                            }`}
                                                    >
                                                        {interaction.interaction_type}
                                                    </span>
                                                    <span className="monitor-actor">{interaction.actor}</span>
                                                    {rowCache && (
                                                        <span
                                                            className="monitor-cache-chip"
                                                            title={`${rowCache.cached} / ${rowCache.prompt}`}
                                                        >
                                                            {t("monitor.cacheChip", { percent: rowCache.percent })}
                                                        </span>
                                                    )}
                                                </div>
                                                <div className="monitor-item-title">{interaction.action_name}</div>
                                                <div className="monitor-item-time">
                                                    {formatDate(interaction.timestamp)}
                                                    {rowThinking !== null &&
                                                        ` · ${t("monitor.thinkingChip", {
                                                            duration: formatDuration(rowThinking),
                                                        })}`}
                                                </div>
                                                {interaction.error_message && (
                                                    <div className="monitor-item-error">{interaction.error_message}</div>
                                                )}
                                                <div className="monitor-item-preview">{interaction.output_preview}</div>
                                            </div>
                                        );
                                    })
                                )}
                            </div>
                        </div>

                        {/* Right Panel: Details */}
                        <div className="monitor-detail-panel">
                            {selectedInteraction ? (
                                <>
                                    <div className="monitor-detail-header">
                                        <h3>{t("monitor.details")}</h3>
                                        <span className="monitor-detail-id">#{selectedInteraction.id}</span>
                                    </div>

                                    <div className="monitor-detail-content">
                                        <div className="detail-section">
                                            <label className="detail-label">{t("monitor.type")}</label>
                                            <div className="detail-value">
                                                {selectedInteraction.interaction_type}
                                            </div>
                                        </div>

                                        <div className="detail-section">
                                            <label className="detail-label">{t("monitor.actor")}</label>
                                            <div className="detail-value">{selectedInteraction.actor}</div>
                                        </div>

                                        <div className="detail-section">
                                            <label className="detail-label">{t("monitor.action")}</label>
                                            <div className="detail-value">{selectedInteraction.action_name}</div>
                                        </div>

                                        <div className="detail-section">
                                            <label className="detail-label">{t("monitor.timestamp")}</label>
                                            <div className="detail-value">{selectedInteraction.timestamp}</div>
                                        </div>

                                        <div className="detail-section">
                                            <label className="detail-label">{t("monitor.duration")}</label>
                                            <div className="detail-value">{selectedInteraction.duration_ms}ms</div>
                                        </div>

                                        {selectedInteraction.error_message && (
                                            <div className="detail-section error-section">
                                                <label className="detail-label">{t("monitor.error")}</label>
                                                <div className="detail-value error-text">{selectedInteraction.error_message}</div>
                                            </div>
                                        )}

                                        <div className="detail-section">
                                            <label className="detail-label">{t("monitor.input")}</label>
                                            <pre className="detail-code">{tryParseJson(selectedInteraction.input_data)}</pre>
                                        </div>

                                        <div className="detail-section">
                                            <label className="detail-label">{t("monitor.output")}</label>
                                            <pre className="detail-code">{tryParseJson(selectedInteraction.output_data)}</pre>
                                        </div>

                                        {selectedInteraction.metadata && selectedInteraction.metadata !== "{}" && (
                                            <div className="detail-section">
                                                <label className="detail-label">{t("monitor.metadata")}</label>
                                                <pre className="detail-code">{tryParseJson(selectedInteraction.metadata)}</pre>
                                            </div>
                                        )}
                                    </div>
                                </>
                            ) : (
                                <div className="monitor-empty">{t("monitor.selectPrompt")}</div>
                            )}
                        </div>
                    </div>

                    {/* Bottom third: analysis report of this session (absent while a turn runs) */}
                    {!busy && (
                        <section className="monitor-report" aria-label={t("monitor.report.aria")}>
                            <div className="monitor-report-header">
                                <h3>{t("monitor.report.title")}</h3>
                                {!reportEmpty && <span className="monitor-report-hint">{t("monitor.report.hint")}</span>}
                            </div>

                            {reportEmpty || !stats ? (
                                <div className="monitor-report-empty">{t("monitor.report.empty")}</div>
                            ) : (
                                <div className="monitor-report-body">
                                    <div className="monitor-report-tiles">
                                        <ReportTile
                                            label={t("monitor.report.totalTokens")}
                                            value={formatTokens(stats.prompt_tokens + stats.completion_tokens)}
                                            detail={t("monitor.report.tokenSplit", {
                                                prompt: formatTokens(stats.prompt_tokens),
                                                completion: formatTokens(stats.completion_tokens),
                                            })}
                                        />
                                        <ReportTile
                                            label={t("monitor.report.cacheHit")}
                                            value={formatTokens(stats.hit_tokens)}
                                            meter={cacheRate}
                                            detail={
                                                stats.cache_reported_requests > 0
                                                    ? t("monitor.report.cacheReported", {
                                                        hits: stats.cache_hit_requests,
                                                        reported: stats.cache_reported_requests,
                                                    })
                                                    : t("monitor.report.noCacheData")
                                            }
                                        />
                                        <ReportTile
                                            label={t("monitor.report.cacheMiss")}
                                            value={formatTokens(stats.miss_tokens)}
                                            meter={100 - cacheRate}
                                            detail={percent(stats.miss_tokens, stats.prompt_tokens)}
                                        />
                                        <ReportTile
                                            label={t("monitor.report.context")}
                                            value={contextPercent === null ? "—" : `${contextPercent}%`}
                                            meter={contextPercent ?? 0}
                                            tone={contextPercent !== null && contextPercent >= 85 ? "danger" : "accent"}
                                            detail={
                                                contextPercent === null
                                                    ? t("monitor.report.contextUnknown")
                                                    : t("monitor.report.contextDetail", {
                                                        used: formatTokens(stats.context_tokens),
                                                        window: formatTokens(stats.context_window),
                                                    })
                                            }
                                        />
                                        <ReportTile
                                            label={t("monitor.report.requests")}
                                            value={String(stats.requests)}
                                            detail={t("monitor.report.requestDetail", {
                                                failed: stats.failed_requests + stats.llm_errors,
                                                avg: averageDuration(stats.total_request_ms, stats.requests),
                                            })}
                                        />
                                        <ReportTile
                                            label={t("monitor.report.reasoning")}
                                            value={
                                                stats.reasoning_requests > 0
                                                    ? averageDuration(stats.reasoning_ms_total, stats.reasoning_requests)
                                                    : "—"
                                            }
                                            detail={
                                                stats.reasoning_requests > 0
                                                    ? t("monitor.report.reasoningDetail", {
                                                        count: stats.reasoning_requests,
                                                        total: formatDuration(stats.reasoning_ms_total),
                                                        ttft: averageDuration(
                                                            stats.first_token_ms_total,
                                                            stats.first_token_requests
                                                        ),
                                                    })
                                                    : t("monitor.report.reasoningNone")
                                            }
                                        />
                                        <ReportTile
                                            label={t("monitor.report.toolCalls")}
                                            value={String(stats.tool_calls)}
                                            detail={t("monitor.report.toolDetail", {
                                                failed: stats.tool_failures,
                                                avg: averageDuration(stats.total_tool_ms, stats.tool_calls),
                                            })}
                                        />
                                    </div>

                                    <div className="monitor-report-tools">
                                        <h4>{t("monitor.report.toolRanking")}</h4>
                                        {stats.tools.length === 0 ? (
                                            <p className="monitor-report-none">{t("monitor.report.noTools")}</p>
                                        ) : (
                                            <ul className="report-tool-list">
                                                {stats.tools.map((tool) => (
                                                    <li className="report-tool-row" key={tool.name}>
                                                        <span className="report-tool-name">{tool.name}</span>
                                                        <span className="report-tool-calls">
                                                            {t("monitor.report.toolRow", { count: tool.calls })}
                                                        </span>
                                                        {tool.failures > 0 && (
                                                            <span className="report-tool-failures">
                                                                {t("monitor.report.toolFailures", { count: tool.failures })}
                                                            </span>
                                                        )}
                                                    </li>
                                                ))}
                                            </ul>
                                        )}
                                    </div>
                                </div>
                            )}
                        </section>
                    )}
                </div>
            </div>
        </div>
    );
}
