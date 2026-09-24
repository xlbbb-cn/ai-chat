import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { listAgentMissions } from "../api";
import type { AgentMissionSnapshot } from "../types";
import { useI18n, type Locale, type MessageKey, type TranslateVars } from "../i18n";
import { FontAwesomeIcon, faSpinner, faXmark } from "../icons";
import "./AgentMissionPanel.css";

interface Props {
    sessionId: string;
    onClose: () => void;
}

function formatTimestamp(value: string, locale: Locale): string {
    if (!value) return "-";
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) {
        return value;
    }
    return date.toLocaleString(locale);
}

/**
 * Raw status string. Also used as the CSS class suffix, so it must stay
 * language-independent — only `statusLabel` localizes it for display.
 */
function rawStatus(mission: AgentMissionSnapshot): string {
    if (mission.mission_accomplished) return "completed";
    return mission.status || "running";
}

function statusLabel(mission: AgentMissionSnapshot, t: (key: MessageKey, vars?: TranslateVars) => string): string {
    const status = rawStatus(mission);
    if (status === "completed") return t("mission.statusCompleted");
    if (status === "running") return t("mission.statusRunning");
    return status;
}

export function AgentMissionPanel({ sessionId, onClose }: Props) {
    const { t, locale } = useI18n();
    const [missions, setMissions] = useState<AgentMissionSnapshot[]>([]);
    const [selectedMissionId, setSelectedMissionId] = useState<string | null>(null);
    const [loading, setLoading] = useState(false);
    const [autoRefresh, setAutoRefresh] = useState(true);

    const loadMissions = useCallback(async () => {
        try {
            setLoading(true);
            const data = await listAgentMissions(sessionId);
            setMissions(data);
            setSelectedMissionId((current) => {
                if (current && data.some((mission) => mission.mission_id === current)) {
                    return current;
                }
                return data[0]?.mission_id ?? null;
            });
        } catch (err) {
            console.error("Failed to load agent missions:", err);
        } finally {
            setLoading(false);
        }
    }, [sessionId]);

    useEffect(() => {
        loadMissions();
    }, [loadMissions]);

    useEffect(() => {
        if (!autoRefresh) return;

        const interval = window.setInterval(() => {
            loadMissions();
        }, 2000);

        return () => window.clearInterval(interval);
    }, [autoRefresh, loadMissions]);

    useEffect(() => {
        const unlisteners: Promise<() => void>[] = [];
        const refresh = () => {
            void loadMissions();
        };

        unlisteners.push(
            listen("agent-task-state", refresh),
            listen("agent-task-start", refresh),
            listen("agent-task-done", refresh),
            listen("agent-task-error", refresh)
        );

        return () => {
            unlisteners.forEach((promise) => promise.then((fn) => fn()));
        };
    }, [loadMissions]);

    const selectedMission = useMemo(
        () => missions.find((mission) => mission.mission_id === selectedMissionId) ?? null,
        [missions, selectedMissionId]
    );

    return (
        <div className="mission-monitor-shell">
            <div className="mission-monitor-header">
                <div>
                    <h2>{t("mission.title")}</h2>
                    <p>{t("mission.subtitle")}</p>
                </div>
                <div className="mission-monitor-controls">
                    <label className="mission-monitor-checkbox">
                        <input
                            type="checkbox"
                            checked={autoRefresh}
                            onChange={(event) => setAutoRefresh(event.target.checked)}
                        />
                        <span>{t("mission.autoRefresh")}</span>
                    </label>
                    <button type="button" className="mission-monitor-refresh" onClick={() => void loadMissions()}>
                        {t("mission.refresh")}
                    </button>
                    <button type="button" className="close-btn" onClick={onClose} aria-label={t("common.close")}>
                        <FontAwesomeIcon icon={faXmark} />
                    </button>
                </div>
            </div>

            <div className="mission-monitor-body">
                <div className="mission-monitor-list-panel">
                    <div className="mission-monitor-list-header">
                        <h3>{t("mission.missions", { count: missions.length })}</h3>
                        {loading && (
                            <span className="mission-monitor-loading">
                                <FontAwesomeIcon icon={faSpinner} spin />
                            </span>
                        )}
                    </div>
                    <div className="mission-monitor-list">
                        {missions.length === 0 ? (
                            <div className="mission-monitor-empty">{t("mission.empty")}</div>
                        ) : (
                            missions.map((mission) => (
                                <button
                                    type="button"
                                    key={mission.mission_id}
                                    className={`mission-monitor-item ${selectedMissionId === mission.mission_id ? "selected" : ""}`}
                                    onClick={() => setSelectedMissionId(mission.mission_id)}
                                >
                                    <div className="mission-monitor-item-header">
                                        <span className={`mission-monitor-status mission-${rawStatus(mission)}`}>
                                            {statusLabel(mission, t)}
                                        </span>
                                        <span className="mission-monitor-agent">{mission.agent_name}</span>
                                    </div>
                                    <div className="mission-monitor-title">{mission.root_task_description}</div>
                                    <div className="mission-monitor-meta">
                                        <span>{t("mission.activeTasks", { count: mission.active_task_count })}</span>
                                        <span>{formatTimestamp(mission.updated_at, locale)}</span>
                                    </div>
                                </button>
                            ))
                        )}
                    </div>
                </div>

                <div className="mission-monitor-detail-panel">
                    {selectedMission ? (
                        <div className="mission-monitor-detail-content">
                            <div className="mission-monitor-section">
                                <label>{t("mission.mission")}</label>
                                <div className="mission-monitor-value">{selectedMission.root_task_description}</div>
                            </div>

                            <div className="mission-monitor-section two-column">
                                <div>
                                    <label>{t("mission.agent")}</label>
                                    <div className="mission-monitor-value">{selectedMission.agent_name}</div>
                                </div>
                                <div>
                                    <label>{t("mission.status")}</label>
                                    <div className="mission-monitor-value">{statusLabel(selectedMission, t)}</div>
                                </div>
                            </div>

                            <div className="mission-monitor-section two-column">
                                <div>
                                    <label>{t("mission.created")}</label>
                                    <div className="mission-monitor-value">{formatTimestamp(selectedMission.created_at, locale)}</div>
                                </div>
                                <div>
                                    <label>{t("mission.updated")}</label>
                                    <div className="mission-monitor-value">{formatTimestamp(selectedMission.updated_at, locale)}</div>
                                </div>
                            </div>

                            <div className="mission-monitor-section">
                                <label>{t("mission.context")}</label>
                                <pre className="mission-monitor-code">{selectedMission.root_task_context || t("common.empty")}</pre>
                            </div>

                            <div className="mission-monitor-section">
                                <label>{t("mission.activeTasksLabel")}</label>
                                {selectedMission.active_tasks.length === 0 ? (
                                    <div className="mission-monitor-value">{t("mission.noActiveTasks")}</div>
                                ) : (
                                    <div className="mission-task-list">
                                        {selectedMission.active_tasks.map((task) => (
                                            <div key={task.task_id} className="mission-task-item">
                                                <div className="mission-task-header">
                                                    <span className={`mission-monitor-status mission-${task.status}`}>{task.status}</span>
                                                    <span className="mission-task-name">{task.name}</span>
                                                </div>
                                                <div className="mission-task-description">{task.description}</div>
                                                <div className="mission-task-id">{task.task_id}</div>
                                            </div>
                                        ))}
                                    </div>
                                )}
                            </div>

                            <div className="mission-monitor-section">
                                <label>{t("mission.episodicSummary")}</label>
                                <pre className="mission-monitor-code">{selectedMission.episodic_summary || t("common.empty")}</pre>
                            </div>

                            <div className="mission-monitor-section">
                                <label>{t("mission.finalReport")}</label>
                                <pre className="mission-monitor-code">{selectedMission.final_report || t("common.empty")}</pre>
                            </div>
                        </div>
                    ) : (
                        <div className="mission-monitor-empty detail">{t("mission.selectPrompt")}</div>
                    )}
                </div>
            </div>
        </div>
    );
}