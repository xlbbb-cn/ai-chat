import { useState, useEffect } from "react";
import { listInteractions, getInteraction, clearLogs, compactDatabase } from "../api";
import type { InteractionLogRecord, InteractionLogDetail } from "../api";
import { useI18n } from "../i18n";
import { FontAwesomeIcon, faSpinner, faXmark } from "../icons";
import "./MonitorPanel.css";

interface Props {
    sessionId: string;
    onClose: () => void;
}

function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function MonitorPanel({ sessionId, onClose }: Props) {
    const { t, locale } = useI18n();
    const [interactions, setInteractions] = useState<InteractionLogRecord[]>([]);
    const [selectedInteraction, setSelectedInteraction] = useState<InteractionLogDetail | null>(null);
    const [loading, setLoading] = useState(false);
    const [autoRefresh, setAutoRefresh] = useState(true);
    const [maintenance, setMaintenance] = useState<"idle" | "clearing" | "compacting">("idle");
    const [status, setStatus] = useState("");

    // Load interactions
    useEffect(() => {
        const loadInteractions = async () => {
            try {
                setLoading(true);
                const data = await listInteractions(sessionId);
                setInteractions(data);
            } catch (err) {
                console.error("Failed to load interactions:", err);
            } finally {
                setLoading(false);
            }
        };

        loadInteractions();

        if (autoRefresh) {
            const interval = setInterval(() => {
                loadInteractions();
            }, 2000); // Refresh every 2 seconds

            return () => clearInterval(interval);
        }
    }, [sessionId, autoRefresh]);

    // Load selected interaction detail
    const handleSelectInteraction = async (record: InteractionLogRecord) => {
        try {
            const detail = await getInteraction(record.id);
            setSelectedInteraction(detail);
        } catch (err) {
            console.error("Failed to load interaction detail:", err);
        }
    };

    const getTypeColor = (type: string): string => {
        switch (type) {
            case "llm_api":
                return "#4CAF50";
            case "llm_error":
                return "#F44336";
            case "tool_input":
                return "#2196F3";
            case "tool_output":
                return "#FF9800";
            case "mcp_call":
                return "#9C27B0";
            case "mcp_response":
                return "#673AB7";
            default:
                return "#757575";
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

    return (
        <div className="monitor-overlay" role="dialog" aria-modal="true" aria-label={t("monitor.aria")}>
            <div className="monitor-shell">
                <div className="monitor-header">
                    <h2>{t("monitor.title")}</h2>
                    <div className="monitor-controls">
                        {status && <span className="monitor-status">{status}</span>}
                        <label className="monitor-checkbox">
                            <input
                                type="checkbox"
                                checked={autoRefresh}
                                onChange={(e) => setAutoRefresh(e.target.checked)}
                            />
                            <span>{t("monitor.autoRefresh")}</span>
                        </label>
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
                    {/* Left Panel: Interaction List */}
                    <div className="monitor-list-panel">
                        <div className="monitor-list-header">
                            <h3>{t("monitor.interactions", { count: interactions.length })}</h3>
                            {loading && (
                                <span className="loading-spinner">
                                    <FontAwesomeIcon icon={faSpinner} spin />
                                </span>
                            )}
                        </div>
                        <div className="monitor-list">
                            {interactions.length === 0 ? (
                                <div className="monitor-empty">{t("monitor.noInteractions")}</div>
                            ) : (
                                interactions.map((interaction) => (
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
                                                className="monitor-type-badge"
                                                style={{ backgroundColor: getTypeColor(interaction.interaction_type) }}
                                            >
                                                {interaction.interaction_type}
                                            </span>
                                            <span className="monitor-actor">{interaction.actor}</span>
                                        </div>
                                        <div className="monitor-item-title">{interaction.action_name}</div>
                                        <div className="monitor-item-time">{formatDate(interaction.timestamp)}</div>
                                        {interaction.error_message && (
                                            <div className="monitor-item-error">{interaction.error_message}</div>
                                        )}
                                        <div className="monitor-item-preview">{interaction.output_preview}</div>
                                    </div>
                                ))
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
                                        <div className="detail-value" style={{ color: getTypeColor(selectedInteraction.interaction_type) }}>
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
            </div>
        </div>
    );
}
