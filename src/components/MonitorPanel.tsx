import { useState, useEffect } from "react";
import { listInteractions, getInteraction } from "../api";
import type { InteractionLogRecord, InteractionLogDetail } from "../api";
import "./MonitorPanel.css";

interface Props {
    sessionId: string;
    onClose: () => void;
}

export function MonitorPanel({ sessionId, onClose }: Props) {
    const [interactions, setInteractions] = useState<InteractionLogRecord[]>([]);
    const [selectedInteraction, setSelectedInteraction] = useState<InteractionLogDetail | null>(null);
    const [loading, setLoading] = useState(false);
    const [autoRefresh, setAutoRefresh] = useState(true);

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
            return date.toLocaleTimeString();
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

    return (
        <div className="monitor-overlay" role="dialog" aria-modal="true" aria-label="Interaction Monitor">
            <div className="monitor-shell">
                <div className="monitor-header">
                    <h2>Interaction Monitor</h2>
                    <div className="monitor-controls">
                        <label className="monitor-checkbox">
                            <input
                                type="checkbox"
                                checked={autoRefresh}
                                onChange={(e) => setAutoRefresh(e.target.checked)}
                            />
                            <span>Auto Refresh</span>
                        </label>
                        <button type="button" className="close-btn" onClick={onClose}>
                            ✕
                        </button>
                    </div>
                </div>

                <div className="monitor-body">
                    {/* Left Panel: Interaction List */}
                    <div className="monitor-list-panel">
                        <div className="monitor-list-header">
                            <h3>Interactions ({interactions.length})</h3>
                            {loading && <span className="loading-spinner">⟳</span>}
                        </div>
                        <div className="monitor-list">
                            {interactions.length === 0 ? (
                                <div className="monitor-empty">No interactions recorded</div>
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
                                    <h3>Details</h3>
                                    <span className="monitor-detail-id">#{selectedInteraction.id}</span>
                                </div>

                                <div className="monitor-detail-content">
                                    <div className="detail-section">
                                        <label className="detail-label">Type</label>
                                        <div className="detail-value" style={{ color: getTypeColor(selectedInteraction.interaction_type) }}>
                                            {selectedInteraction.interaction_type}
                                        </div>
                                    </div>

                                    <div className="detail-section">
                                        <label className="detail-label">Actor</label>
                                        <div className="detail-value">{selectedInteraction.actor}</div>
                                    </div>

                                    <div className="detail-section">
                                        <label className="detail-label">Action</label>
                                        <div className="detail-value">{selectedInteraction.action_name}</div>
                                    </div>

                                    <div className="detail-section">
                                        <label className="detail-label">Timestamp</label>
                                        <div className="detail-value">{selectedInteraction.timestamp}</div>
                                    </div>

                                    <div className="detail-section">
                                        <label className="detail-label">Duration</label>
                                        <div className="detail-value">{selectedInteraction.duration_ms}ms</div>
                                    </div>

                                    {selectedInteraction.error_message && (
                                        <div className="detail-section error-section">
                                            <label className="detail-label">Error</label>
                                            <div className="detail-value error-text">{selectedInteraction.error_message}</div>
                                        </div>
                                    )}

                                    <div className="detail-section">
                                        <label className="detail-label">Input</label>
                                        <pre className="detail-code">{tryParseJson(selectedInteraction.input_data)}</pre>
                                    </div>

                                    <div className="detail-section">
                                        <label className="detail-label">Output</label>
                                        <pre className="detail-code">{tryParseJson(selectedInteraction.output_data)}</pre>
                                    </div>

                                    {selectedInteraction.metadata && selectedInteraction.metadata !== "{}" && (
                                        <div className="detail-section">
                                            <label className="detail-label">Metadata</label>
                                            <pre className="detail-code">{tryParseJson(selectedInteraction.metadata)}</pre>
                                        </div>
                                    )}
                                </div>
                            </>
                        ) : (
                            <div className="monitor-empty">Select an interaction to view details</div>
                        )}
                    </div>
                </div>
            </div>
        </div>
    );
}
