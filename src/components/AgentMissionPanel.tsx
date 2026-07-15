import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { listAgentMissions } from "../api";
import type { AgentMissionSnapshot } from "../types";
import "./AgentMissionPanel.css";

interface Props {
    sessionId: string;
    onClose: () => void;
}

function formatTimestamp(value: string): string {
    if (!value) return "-";
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) {
        return value;
    }
    return date.toLocaleString();
}

function statusLabel(mission: AgentMissionSnapshot): string {
    if (mission.mission_accomplished) return "completed";
    return mission.status || "running";
}

export function AgentMissionPanel({ sessionId, onClose }: Props) {
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
                    <h2>Mission Monitor</h2>
                    <p>Track external mission state, active tasks, and episodic summaries for this chat session.</p>
                </div>
                <div className="mission-monitor-controls">
                    <label className="mission-monitor-checkbox">
                        <input
                            type="checkbox"
                            checked={autoRefresh}
                            onChange={(event) => setAutoRefresh(event.target.checked)}
                        />
                        <span>Auto refresh</span>
                    </label>
                    <button type="button" className="mission-monitor-refresh" onClick={() => void loadMissions()}>
                        Refresh
                    </button>
                    <button type="button" className="close-btn" onClick={onClose}>
                        ✕
                    </button>
                </div>
            </div>

            <div className="mission-monitor-body">
                <div className="mission-monitor-list-panel">
                    <div className="mission-monitor-list-header">
                        <h3>Missions ({missions.length})</h3>
                        {loading && <span className="mission-monitor-loading">⟳</span>}
                    </div>
                    <div className="mission-monitor-list">
                        {missions.length === 0 ? (
                            <div className="mission-monitor-empty">No mission state recorded for this session yet.</div>
                        ) : (
                            missions.map((mission) => (
                                <button
                                    type="button"
                                    key={mission.mission_id}
                                    className={`mission-monitor-item ${selectedMissionId === mission.mission_id ? "selected" : ""}`}
                                    onClick={() => setSelectedMissionId(mission.mission_id)}
                                >
                                    <div className="mission-monitor-item-header">
                                        <span className={`mission-monitor-status mission-${statusLabel(mission)}`}>
                                            {statusLabel(mission)}
                                        </span>
                                        <span className="mission-monitor-agent">{mission.agent_name}</span>
                                    </div>
                                    <div className="mission-monitor-title">{mission.root_task_description}</div>
                                    <div className="mission-monitor-meta">
                                        <span>{mission.active_task_count} active tasks</span>
                                        <span>{formatTimestamp(mission.updated_at)}</span>
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
                                <label>Mission</label>
                                <div className="mission-monitor-value">{selectedMission.root_task_description}</div>
                            </div>

                            <div className="mission-monitor-section two-column">
                                <div>
                                    <label>Agent</label>
                                    <div className="mission-monitor-value">{selectedMission.agent_name}</div>
                                </div>
                                <div>
                                    <label>Status</label>
                                    <div className="mission-monitor-value">{statusLabel(selectedMission)}</div>
                                </div>
                            </div>

                            <div className="mission-monitor-section two-column">
                                <div>
                                    <label>Created</label>
                                    <div className="mission-monitor-value">{formatTimestamp(selectedMission.created_at)}</div>
                                </div>
                                <div>
                                    <label>Updated</label>
                                    <div className="mission-monitor-value">{formatTimestamp(selectedMission.updated_at)}</div>
                                </div>
                            </div>

                            <div className="mission-monitor-section">
                                <label>Context</label>
                                <pre className="mission-monitor-code">{selectedMission.root_task_context || "(empty)"}</pre>
                            </div>

                            <div className="mission-monitor-section">
                                <label>Active Tasks</label>
                                {selectedMission.active_tasks.length === 0 ? (
                                    <div className="mission-monitor-value">No active tasks.</div>
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
                                <label>Episodic Summary</label>
                                <pre className="mission-monitor-code">{selectedMission.episodic_summary || "(empty)"}</pre>
                            </div>

                            <div className="mission-monitor-section">
                                <label>Final Report</label>
                                <pre className="mission-monitor-code">{selectedMission.final_report || "(empty)"}</pre>
                            </div>
                        </div>
                    ) : (
                        <div className="mission-monitor-empty detail">Select a mission to inspect its external state.</div>
                    )}
                </div>
            </div>
        </div>
    );
}