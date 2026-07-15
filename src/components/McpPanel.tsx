import { useState, useEffect, useRef } from "react";
import {
    listMcpServers,
    saveMcpServer,
    deleteMcpServer,
    testMcpServer,
    startMcpServer,
    stopMcpServer,
    restartMcpServer,
    listMcpStatus,
    onMcpStatus,
    onMcpLog,
} from "../api";
import type {
    McpServer,
    McpTransport,
    McpStatus,
    McpLogEvent,
    McpLogStream,
    McpStatusKind,
} from "../types";
import "./McpPanel.css";

interface Props {
    onClose: () => void;
    onServersChange?: (enabledCount: number) => void;
}

const MAX_LOG_LINES = 200;

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

function formatUptime(startedAtMs: number | null): string {
    if (!startedAtMs) return "—";
    const ms = Date.now() - startedAtMs;
    if (ms < 0) return "—";
    const s = Math.floor(ms / 1000);
    if (s < 60) return `${s}s`;
    const m = Math.floor(s / 60);
    if (m < 60) return `${m}m ${s % 60}s`;
    const h = Math.floor(m / 60);
    return `${h}h ${m % 60}m`;
}

export function McpPanel({ onClose, onServersChange }: Props) {
    const [servers, setServers] = useState<McpServer[]>([]);
    const [editing, setEditing] = useState<McpServer | null>(null);
    const [testStatus, setTestStatus] = useState<Record<string, { ok: boolean; msg: string }>>({});
    const [testing, setTesting] = useState<string | null>(null);
    const [argsInput, setArgsInput] = useState("");
    const [envInput, setEnvInput] = useState("");

    // Monitor state
    const [runtimes, setRuntimes] = useState<Record<string, McpStatus>>({});
    const [logs, setLogs] = useState<Record<string, McpLogEvent[]>>({});
    const [selectedId, setSelectedId] = useState<string | null>(null);
    const [streamFilter, setStreamFilter] = useState<"all" | McpLogStream>("all");
    const [, setTick] = useState(0); // for uptime ticker
    const logScrollRef = useRef<HTMLDivElement>(null);
    const logStuckToBottom = useRef(true);

    useEffect(() => {
        listMcpServers().then(setServers).catch(console.error);
    }, []);

    useEffect(() => {
        onServersChange?.(servers.filter((s) => s.enabled).length);
    }, [servers, onServersChange]);

    // Subscribe to MCP monitor events
    useEffect(() => {
        let unlistenStatus: (() => void) | null = null;
        let unlistenLog: (() => void) | null = null;
        listMcpStatus()
            .then((list) =>
                setRuntimes(
                    list.reduce<Record<string, McpStatus>>((acc, s) => {
                        acc[s.id] = s;
                        return acc;
                    }, {}),
                ),
            )
            .catch(console.error);
        onMcpStatus((s) => {
            setRuntimes((prev) => ({ ...prev, [s.id]: s }));
        }).then((u) => {
            unlistenStatus = u;
        });
        onMcpLog((e) => {
            setLogs((prev) => {
                const list = prev[e.id] ?? [];
                const next = list.length >= MAX_LOG_LINES
                    ? [...list.slice(list.length - MAX_LOG_LINES + 1), e]
                    : [...list, e];
                return { ...prev, [e.id]: next };
            });
        }).then((u) => {
            unlistenLog = u;
        });
        return () => {
            unlistenStatus?.();
            unlistenLog?.();
        };
    }, []);

    // Re-render every second to refresh uptime
    useEffect(() => {
        const t = window.setInterval(() => setTick((x) => x + 1), 1000);
        return () => window.clearInterval(t);
    }, []);

    // Auto-scroll log to bottom on new lines if user is at the bottom
    useEffect(() => {
        if (logStuckToBottom.current && logScrollRef.current) {
            logScrollRef.current.scrollTop = logScrollRef.current.scrollHeight;
        }
    }, [logs, selectedId, streamFilter]);

    // Clear monitor state for a removed server
    useEffect(() => {
        const known = new Set(servers.map((s) => s.id));
        const dangling = Object.keys(runtimes).filter((id) => !known.has(id));
        if (dangling.length > 0) {
            setRuntimes((prev) => {
                const next = { ...prev };
                for (const id of dangling) delete next[id];
                return next;
            });
        }
        const danglingLogs = Object.keys(logs).filter((id) => !known.has(id));
        if (danglingLogs.length > 0) {
            setLogs((prev) => {
                const next = { ...prev };
                for (const id of danglingLogs) delete next[id];
                return next;
            });
        }
        if (selectedId && !known.has(selectedId)) {
            setSelectedId(null);
        }
    }, [servers, runtimes, logs, selectedId]);

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

    const enabledServers = servers.filter((s) => s.enabled);

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

            <div className="mcp-monitor">
                <div className="mcp-monitor-header">
                    <span className="mcp-monitor-title">Monitor</span>
                    {selectedId && (
                        <button
                            className="mcp-monitor-clear"
                            onClick={() => setLogs((prev) => ({ ...prev, [selectedId]: [] }))}
                            title="Clear logs for selected server"
                        >
                            Clear
                        </button>
                    )}
                </div>

                <div className="mcp-monitor-status">
                    {enabledServers.length === 0 ? (
                        <div className="mcp-monitor-empty">No enabled servers.</div>
                    ) : (
                        enabledServers.map((s) => {
                            const rt = runtimes[s.id];
                            const status: McpStatusKind = rt?.status ?? "stopped";
                            return (
                                <div
                                    key={s.id}
                                    className={`mcp-monitor-row ${selectedId === s.id ? "selected" : ""}`}
                                    onClick={() => setSelectedId(s.id)}
                                >
                                    <span className={`mcp-status-dot ${status}`} />
                                    <span
                                        className="mcp-monitor-name"
                                        title={rt?.last_error ?? ""}
                                    >
                                        {s.name || "(unnamed)"}
                                    </span>
                                    <span className={`mcp-status-badge ${status}`}>{status}</span>
                                    {rt?.pid != null && (
                                        <span className="mcp-monitor-pid">pid {rt.pid}</span>
                                    )}
                                    <span className="mcp-monitor-uptime">
                                        {status === "running" || status === "ready"
                                            ? formatUptime(rt?.started_at_ms ?? null)
                                            : "—"}
                                    </span>
                                    <div
                                        className="mcp-monitor-actions"
                                        onClick={(e) => e.stopPropagation()}
                                    >
                                        {status === "running" || status === "starting" ? (
                                            <button
                                                className="mcp-action-btn"
                                                title="Stop"
                                                onClick={() =>
                                                    stopMcpServer(s.id).catch(console.error)
                                                }
                                            >
                                                ⏹
                                            </button>
                                        ) : (
                                            <button
                                                className="mcp-action-btn"
                                                title="Start"
                                                onClick={() =>
                                                    startMcpServer(s.id).catch(console.error)
                                                }
                                            >
                                                ▶
                                            </button>
                                        )}
                                        <button
                                            className="mcp-action-btn"
                                            title="Restart"
                                            onClick={() =>
                                                restartMcpServer(s.id).catch(console.error)
                                            }
                                            disabled={s.transport !== "stdio"}
                                        >
                                            ↻
                                        </button>
                                    </div>
                                </div>
                            );
                        })
                    )}
                </div>

                <div className="mcp-monitor-logs">
                    <div className="mcp-monitor-logs-header">
                        <span className="mcp-monitor-logs-title">
                            {selectedId
                                ? `Logs: ${enabledServers.find((s) => s.id === selectedId)?.name ?? selectedId}`
                                : "Logs"}
                        </span>
                        <select
                            className="mcp-monitor-stream-filter"
                            value={streamFilter}
                            onChange={(e) =>
                                setStreamFilter(e.target.value as "all" | McpLogStream)
                            }
                            disabled={!selectedId}
                        >
                            <option value="all">all</option>
                            <option value="stdout">stdout</option>
                            <option value="stderr">stderr</option>
                        </select>
                    </div>
                    <div
                        className="mcp-monitor-log-list"
                        ref={logScrollRef}
                        onScroll={(e) => {
                            const el = e.currentTarget;
                            const distFromBottom =
                                el.scrollHeight - el.scrollTop - el.clientHeight;
                            logStuckToBottom.current = distFromBottom < 20;
                        }}
                    >
                        {!selectedId && (
                            <div className="mcp-monitor-empty">
                                Select a server above to view its logs.
                            </div>
                        )}
                        {selectedId && (logs[selectedId]?.length ?? 0) === 0 && (
                            <div className="mcp-monitor-empty">No log output yet.</div>
                        )}
                        {selectedId &&
                            (logs[selectedId] ?? [])
                                .filter(
                                    (l) => streamFilter === "all" || l.stream === streamFilter
                                )
                                .map((l, i) => (
                                    <div key={i} className={`mcp-log-line ${l.stream}`}>
                                        <span className="mcp-log-time">
                                            {formatLogTime(l.ts)}
                                        </span>
                                        <span className="mcp-log-stream">{l.stream}</span>
                                        <span className="mcp-log-text">{l.line}</span>
                                    </div>
                                ))}
                    </div>
                </div>
            </div>

            <div className="mcp-footer">
                <button className="btn-primary" onClick={startAdd}>+ Add Server</button>
            </div>
        </div>
    );
}
