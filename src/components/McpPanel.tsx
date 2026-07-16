import { useEffect, useRef, useState } from "react";
import {
    listMcpServers,
    saveMcpServer,
    deleteMcpServer,
    testMcpServer,
    getMcpLogs,
    clearMcpLogs,
} from "../api";
import type { McpServer, McpTransport, McpLogEntry } from "../types";
import "./McpPanel.css";

interface Props {
    onClose: () => void;
    onServersChange?: (enabledCount: number) => void;
}

function emptyServer(): McpServer {
    return {
        id: crypto.randomUUID(),
        name: "",
        transport: "stdio",
        command: "",
        args: [],
        env: {},
        url: "",
        auth_token: "",
        enabled: true,
    };
}

function formatLogTime(ts: number): string {
    const d = new Date(ts);
    const hh = String(d.getHours()).padStart(2, "0");
    const mm = String(d.getMinutes()).padStart(2, "0");
    const ss = String(d.getSeconds()).padStart(2, "0");
    const ms = String(d.getMilliseconds()).padStart(3, "0");
    return `${hh}:${mm}:${ss}.${ms}`;
}

export function McpPanel({ onClose, onServersChange }: Props) {
    const [servers, setServers] = useState<McpServer[]>([]);
    const [editing, setEditing] = useState<McpServer | null>(null);
    const [testStatus, setTestStatus] = useState<Record<string, { ok: boolean; msg: string }>>({});
    const [testing, setTesting] = useState<string | null>(null);
    const [argsInput, setArgsInput] = useState("");
    const [envInput, setEnvInput] = useState("");

    // Diagnostic log modal state
    const [logServer, setLogServer] = useState<McpServer | null>(null);
    const [logEntries, setLogEntries] = useState<McpLogEntry[]>([]);
    const [logLoading, setLogLoading] = useState(false);
    const logScrollRef = useRef<HTMLDivElement>(null);
    const logStuckToBottom = useRef(true);

    useEffect(() => {
        listMcpServers().then(setServers).catch(console.error);
    }, []);

    useEffect(() => {
        onServersChange?.(servers.filter((s) => s.enabled).length);
    }, [servers, onServersChange]);

    // Auto-refresh the open log modal every 1.5s so newly captured stderr /
    // tool-call results stream in while the user is watching.
    useEffect(() => {
        if (!logServer) return;
        let cancelled = false;
        const refresh = async () => {
            if (cancelled) return;
            try {
                const entries = await getMcpLogs(logServer.id);
                if (!cancelled) setLogEntries(entries);
            } catch (err) {
                console.error("getMcpLogs failed", err);
            }
        };
        void refresh();
        const t = window.setInterval(refresh, 1500);
        return () => {
            cancelled = true;
            window.clearInterval(t);
        };
    }, [logServer]);

    // Auto-scroll log to bottom if the user hasn't scrolled up.
    useEffect(() => {
        if (logStuckToBottom.current && logScrollRef.current) {
            logScrollRef.current.scrollTop = logScrollRef.current.scrollHeight;
        }
    }, [logEntries]);

    function startAdd() {
        const s = emptyServer();
        setEditing(s);
        setArgsInput("");
        setEnvInput("");
    }

    function startEdit(s: McpServer) {
        setEditing({ ...s });
        setArgsInput(s.args.join(" "));
        setEnvInput(
            Object.entries(s.env)
                .map(([k, v]) => `${k}=${v}`)
                .join("\n")
        );
    }

    async function saveEditing() {
        if (!editing) return;
        const args = argsInput
            .split(/\s+/)
            .map((a) => a.trim())
            .filter(Boolean);
        const env: Record<string, string> = {};
        for (const line of envInput.split("\n")) {
            const eq = line.indexOf("=");
            if (eq > 0) {
                env[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
            }
        }
        const updated = { ...editing, args, env };
        await saveMcpServer(updated);
        const list = await listMcpServers();
        setServers(list);
        setEditing(null);
    }

    async function toggleEnabled(s: McpServer) {
        const updated = { ...s, enabled: !s.enabled };
        await saveMcpServer(updated);
        setServers((prev) => prev.map((x) => (x.id === updated.id ? updated : x)));
    }

    async function remove(id: string) {
        await deleteMcpServer(id);
        setServers((prev) => prev.filter((s) => s.id !== id));
        setTestStatus((prev) => {
            const next = { ...prev };
            delete next[id];
            return next;
        });
        if (logServer?.id === id) closeLogs();
    }

    async function runTest(s: McpServer) {
        setTesting(s.id);
        try {
            const msg = await testMcpServer(s);
            setTestStatus((prev) => ({ ...prev, [s.id]: { ok: true, msg } }));
        } catch (err) {
            setTestStatus((prev) => ({ ...prev, [s.id]: { ok: false, msg: String(err) } }));
        } finally {
            setTesting(null);
        }
        // If the log modal is open for this server, refresh it now so the
        // user can see the test trace without manually clicking refresh.
        if (logServer?.id === s.id) {
            try {
                const entries = await getMcpLogs(s.id);
                setLogEntries(entries);
            } catch (err) {
                console.error(err);
            }
        }
    }

    function openLogs(s: McpServer) {
        logStuckToBottom.current = true;
        setLogServer(s);
    }

    function closeLogs() {
        setLogServer(null);
        setLogEntries([]);
    }

    async function refreshLogs() {
        if (!logServer) return;
        setLogLoading(true);
        try {
            const entries = await getMcpLogs(logServer.id);
            setLogEntries(entries);
        } catch (err) {
            console.error(err);
        } finally {
            setLogLoading(false);
        }
    }

    async function clearLogs() {
        if (!logServer) return;
        try {
            await clearMcpLogs(logServer.id);
            setLogEntries([]);
        } catch (err) {
            console.error(err);
        }
    }

    if (editing) {
        return (
            <div className="mcp-panel">
                <div className="mcp-header">
                    <h2>{servers.some((s) => s.id === editing.id) ? "Edit" : "Add"} MCP Server</h2>
                    <button className="close-btn" onClick={() => setEditing(null)}>✕</button>
                </div>

                <div className="mcp-form">
                    <label>Name</label>
                    <input
                        className="mcp-input"
                        value={editing.name}
                        onChange={(e) => setEditing({ ...editing, name: e.target.value })}
                        placeholder="My MCP Server"
                    />

                    <label>Transport</label>
                    <select
                        className="mcp-select"
                        value={editing.transport}
                        onChange={(e) => setEditing({ ...editing, transport: e.target.value as McpTransport })}
                    >
                        <option value="stdio">stdio (subprocess)</option>
                        <option value="sse">SSE / HTTP</option>
                    </select>

                    {editing.transport === "stdio" ? (
                        <>
                            <label>Command</label>
                            <input
                                className="mcp-input"
                                value={editing.command}
                                onChange={(e) => setEditing({ ...editing, command: e.target.value })}
                                placeholder="npx / python / ./server"
                            />

                            <label>Arguments <span className="mcp-hint">(space-separated)</span></label>
                            <input
                                className="mcp-input"
                                value={argsInput}
                                onChange={(e) => setArgsInput(e.target.value)}
                                placeholder="-m my_mcp_server --port 8080"
                            />

                            <label>Environment Variables <span className="mcp-hint">(KEY=value, one per line)</span></label>
                            <textarea
                                className="mcp-textarea"
                                rows={4}
                                value={envInput}
                                onChange={(e) => setEnvInput(e.target.value)}
                                placeholder={"API_KEY=abc123\nDEBUG=true"}
                            />
                        </>
                    ) : (
                        <>
                            <label>URL</label>
                            <input
                                className="mcp-input"
                                value={editing.url}
                                onChange={(e) => setEditing({ ...editing, url: e.target.value })}
                                placeholder="http://localhost:8080/sse"
                            />

                            <label>Auth Token <span className="mcp-hint">(optional, Bearer)</span></label>
                            <input
                                className="mcp-input"
                                type="password"
                                value={editing.auth_token}
                                onChange={(e) => setEditing({ ...editing, auth_token: e.target.value })}
                                placeholder="sk-..."
                            />
                        </>
                    )}
                </div>

                <div className="mcp-form-actions">
                    <button className="btn-secondary" onClick={() => setEditing(null)}>Cancel</button>
                    <button
                        className="btn-primary"
                        onClick={() => void saveEditing()}
                        disabled={!editing.name.trim()}
                    >
                        Save
                    </button>
                </div>
            </div>
        );
    }

    return (
        <div className="mcp-panel">
            <div className="mcp-header">
                <h2>MCP Servers</h2>
                <button className="close-btn" onClick={onClose}>✕</button>
            </div>

            <div className="mcp-desc">
                Model Context Protocol (MCP) servers extend the AI with external tools and data sources.
            </div>

            <div className="mcp-list">
                {servers.length === 0 && (
                    <div className="mcp-empty">No servers configured yet.</div>
                )}
                {servers.map((s) => (
                    <div key={s.id} className={`mcp-server-item ${s.enabled ? "enabled" : "disabled"}`}>
                        <div className="mcp-server-row">
                            <label className="mcp-toggle" title={s.enabled ? "Disable" : "Enable"}>
                                <input
                                    type="checkbox"
                                    checked={s.enabled}
                                    onChange={() => void toggleEnabled(s)}
                                />
                                <span className="mcp-toggle-slider" />
                            </label>
                            <div className="mcp-server-info">
                                <span
                                    className={`mcp-server-name ${s.enabled ? "active" : ""}`}
                                    title={`Click to ${s.enabled ? "disable" : "enable"} ${s.name}`}
                                    onClick={() => void toggleEnabled(s)}
                                >
                                    {s.name || "(unnamed)"}
                                </span>
                                <span className="mcp-server-transport">
                                    {s.transport === "stdio"
                                        ? `stdio: ${s.command}${s.args.length ? " " + s.args.join(" ") : ""}`
                                        : `sse: ${s.url}`}
                                </span>
                            </div>
                            <div className="mcp-server-actions">
                                <button
                                    className="mcp-action-btn"
                                    title="Test connection"
                                    onClick={() => void runTest(s)}
                                    disabled={testing === s.id}
                                >
                                    {testing === s.id ? "…" : "⚡"}
                                </button>
                                <button
                                    className="mcp-action-btn"
                                    title="View diagnostic logs"
                                    onClick={() => openLogs(s)}
                                >
                                    🗒
                                </button>
                                <button
                                    className="mcp-action-btn"
                                    title="Edit"
                                    onClick={() => startEdit(s)}
                                >
                                    ✎
                                </button>
                                <button
                                    className="mcp-action-btn danger"
                                    title="Delete"
                                    onClick={() => void remove(s.id)}
                                >
                                    ✕
                                </button>
                            </div>
                        </div>
                        {testStatus[s.id] && (
                            <div className={`mcp-test-result ${testStatus[s.id].ok ? "ok" : "fail"}`}>
                                {testStatus[s.id].ok ? "✓" : "✗"} {testStatus[s.id].msg}
                            </div>
                        )}
                    </div>
                ))}
            </div>

            <div className="mcp-footer">
                <button className="btn-primary" onClick={startAdd}>+ Add Server</button>
            </div>

            {logServer && (
                <div className="mcp-log-modal-backdrop" onClick={closeLogs}>
                    <div className="mcp-log-modal" onClick={(e) => e.stopPropagation()}>
                        <div className="mcp-log-modal-header">
                            <div className="mcp-log-modal-title">
                                <span className="mcp-log-modal-title-main">
                                    Logs: {logServer.name || "(unnamed)"}
                                </span>
                                <span className="mcp-log-modal-title-sub">
                                    {logServer.transport === "stdio"
                                        ? `stdio: ${logServer.command}${logServer.args.length ? " " + logServer.args.join(" ") : ""}`
                                        : `sse: ${logServer.url}`}
                                </span>
                            </div>
                            <div className="mcp-log-modal-actions">
                                <button
                                    className="mcp-log-btn"
                                    onClick={() => void refreshLogs()}
                                    disabled={logLoading}
                                >
                                    Refresh
                                </button>
                                <button
                                    className="mcp-log-btn"
                                    onClick={() => void clearLogs()}
                                >
                                    Clear
                                </button>
                                <button className="mcp-log-btn" onClick={closeLogs}>✕</button>
                            </div>
                        </div>
                        <div className="mcp-log-modal-meta">
                            {logEntries.length === 0
                                ? "No log entries yet. Run a test (⚡) to populate."
                                : `${logEntries.length} entries (auto-refresh every 1.5s)`}
                        </div>
                        <div
                            className="mcp-log-modal-list"
                            ref={logScrollRef}
                            onScroll={(e) => {
                                const el = e.currentTarget;
                                const distFromBottom =
                                    el.scrollHeight - el.scrollTop - el.clientHeight;
                                logStuckToBottom.current = distFromBottom < 20;
                            }}
                        >
                            {logEntries.length === 0 ? (
                                <div className="mcp-log-modal-empty">
                                    Diagnostic info will appear here once you run a test.
                                </div>
                            ) : (
                                logEntries.map((entry, i) => (
                                    <div key={i} className={`mcp-log-modal-entry mcp-log-level-${entry.level}`}>
                                        <span className="mcp-log-modal-time">
                                            {formatLogTime(entry.ts)}
                                        </span>
                                        <span className="mcp-log-modal-level">{entry.level}</span>
                                        <span className="mcp-log-modal-message">{entry.message}</span>
                                    </div>
                                ))
                            )}
                        </div>
                    </div>
                </div>
            )}
        </div>
    );
}
