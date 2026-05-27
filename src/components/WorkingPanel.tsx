import { useEffect, useMemo, useState } from "react";
import { dispatchWorkingTask, listWorkingClients, setWorkingMode } from "../api";
import type { WorkingClientRecord, WorkingRuntime } from "../types";
import "./WorkingPanel.css";

interface Props {
    runtime: WorkingRuntime | null;
    onClose: () => void;
    onRuntimeChange: (runtime: WorkingRuntime) => void;
}

function formatUpdatedAt(updatedAtMs: number): string {
    if (!updatedAtMs) return "Unknown";
    return new Date(updatedAtMs).toLocaleString();
}

export function WorkingPanel({ runtime, onClose, onRuntimeChange }: Props) {
    const [clients, setClients] = useState<WorkingClientRecord[]>([]);
    const [loading, setLoading] = useState(false);
    const [toggling, setToggling] = useState(false);
    const [selectedClientUid, setSelectedClientUid] = useState("");
    const [dispatchContent, setDispatchContent] = useState("");
    const [dispatching, setDispatching] = useState(false);
    const [dispatchMessage, setDispatchMessage] = useState<string | null>(null);
    const [dispatchError, setDispatchError] = useState<string | null>(null);

    const dispatchTargets = useMemo(
        () => clients.filter((client) => !client.is_current && client.status === "idle"),
        [clients]
    );

    async function loadClients() {
        try {
            setLoading(true);
            const nextClients = await listWorkingClients();
            setClients(nextClients);
        } catch (err) {
            console.error("Failed to load working clients", err);
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        let cancelled = false;

        const refreshClients = async () => {
            try {
                setLoading(true);
                const nextClients = await listWorkingClients();
                if (!cancelled) {
                    setClients(nextClients);
                }
            } catch (err) {
                console.error("Failed to load working clients", err);
            } finally {
                if (!cancelled) {
                    setLoading(false);
                }
            }
        };

        void refreshClients();
        const interval = window.setInterval(refreshClients, 3000);

        return () => {
            cancelled = true;
            window.clearInterval(interval);
        };
    }, [runtime?.enabled, runtime?.uid]);

    useEffect(() => {
        if (dispatchTargets.length === 0) {
            setSelectedClientUid("");
            return;
        }

        const stillExists = dispatchTargets.some((client) => client.uid === selectedClientUid);
        if (!stillExists) {
            setSelectedClientUid(dispatchTargets[0].uid);
        }
    }, [dispatchTargets, selectedClientUid]);

    async function handleToggleWorking() {
        if (!runtime) return;

        setToggling(true);
        try {
            const nextRuntime = await setWorkingMode(!runtime.enabled);
            onRuntimeChange(nextRuntime);
            await loadClients();
        } catch (err) {
            console.error("Failed to toggle working mode", err);
        } finally {
            setToggling(false);
        }
    }

    async function handleDispatchTask() {
        if (!selectedClientUid || !dispatchContent.trim()) {
            return;
        }

        setDispatching(true);
        setDispatchMessage(null);
        setDispatchError(null);
        try {
            await dispatchWorkingTask(selectedClientUid, dispatchContent.trim());
            setDispatchMessage(`Task dispatched to ${selectedClientUid}.`);
            setDispatchContent("");
            await loadClients();
        } catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            setDispatchError(message);
        } finally {
            setDispatching(false);
        }
    }

    return (
        <div className="working-panel">
            <div className="working-panel-header">
                <div>
                    <h2>Working Mode</h2>
                    <p>Polls the app data directory for todo files assigned to this client.</p>
                </div>
                <button className="close-btn" onClick={onClose}>✕</button>
            </div>

            <div className="working-panel-card">
                <div className="working-panel-card-row">
                    <span className="working-label">This client</span>
                    <span className={`working-status-badge ${runtime?.status === "busy" ? "busy" : "idle"}`}>
                        {runtime?.enabled ? runtime?.status ?? "idle" : "disabled"}
                    </span>
                </div>
                <div className="working-panel-meta">UID: {runtime?.uid ?? "loading..."}</div>
                <div className="working-panel-meta">
                    Todo file: {runtime?.uid ? `todo-${runtime.uid}.md` : "loading..."}
                </div>
                <div className="working-panel-meta">
                    Lock file: {runtime?.uid ? `app-${runtime.uid}.lck` : "loading..."}
                </div>
                {(runtime?.active_task_file || runtime?.status_detail) && (
                    <div className="working-panel-detail">
                        {runtime?.active_task_file ?? runtime?.status_detail}
                    </div>
                )}
                <button className="working-toggle-btn" onClick={handleToggleWorking} disabled={!runtime || toggling}>
                    {runtime?.enabled ? "Disable Working Mode" : "Enable Working Mode"}
                </button>
            </div>

            <div className="working-panel-card">
                <div className="working-panel-section-header">
                    <h3>Dispatch Task</h3>
                    <span className="working-loading">{dispatchTargets.length} idle client{dispatchTargets.length === 1 ? "" : "s"}</span>
                </div>
                <div className="working-panel-meta">
                    Write a task and assign it directly to an idle working client. The receiver will process it from its own chat window.
                </div>
                <label className="working-field">
                    <span className="working-field-label">Target client</span>
                    <select
                        className="working-select"
                        value={selectedClientUid}
                        onChange={(e) => setSelectedClientUid(e.target.value)}
                        disabled={dispatching || dispatchTargets.length === 0}
                    >
                        {dispatchTargets.length === 0 ? (
                            <option value="">No idle clients available</option>
                        ) : (
                            dispatchTargets.map((client) => (
                                <option key={client.uid} value={client.uid}>
                                    {client.uid}
                                </option>
                            ))
                        )}
                    </select>
                </label>
                <label className="working-field">
                    <span className="working-field-label">Task content</span>
                    <textarea
                        className="working-textarea"
                        value={dispatchContent}
                        onChange={(e) => setDispatchContent(e.target.value)}
                        placeholder="Describe the task or paste a markdown todo list..."
                        rows={6}
                        disabled={dispatching}
                    />
                </label>
                {dispatchMessage && <div className="working-feedback success">{dispatchMessage}</div>}
                {dispatchError && <div className="working-feedback error">{dispatchError}</div>}
                <button
                    className="working-toggle-btn"
                    onClick={handleDispatchTask}
                    disabled={dispatching || !selectedClientUid || !dispatchContent.trim()}
                >
                    {dispatching ? "Dispatching..." : "Dispatch Task"}
                </button>
            </div>

            <div className="working-panel-clients">
                <div className="working-panel-section-header">
                    <h3>Active Working Clients</h3>
                    {loading && <span className="working-loading">Refreshing...</span>}
                </div>

                {clients.length === 0 ? (
                    <div className="working-panel-empty">No working clients are advertising a lock file.</div>
                ) : (
                    <div className="working-client-list">
                        {clients.map((client) => (
                            <div key={client.uid} className={`working-client-card ${client.is_current ? "is-current" : ""}`}>
                                <div className="working-client-row">
                                    <div className="working-client-uid">{client.uid}</div>
                                    <span className={`working-status-badge ${client.status === "busy" ? "busy" : "idle"}`}>
                                        {client.status}
                                    </span>
                                </div>
                                <div className="working-client-meta">
                                    Updated: {formatUpdatedAt(client.updated_at_ms)}
                                </div>
                                {client.active_task_file && (
                                    <div className="working-client-meta">Task: {client.active_task_file}</div>
                                )}
                                {client.status_detail && client.status_detail !== client.active_task_file && (
                                    <div className="working-client-meta">Detail: {client.status_detail}</div>
                                )}
                                {client.is_current && <div className="working-client-self">Current window</div>}
                            </div>
                        ))}
                    </div>
                )}
            </div>
        </div>
    );
}