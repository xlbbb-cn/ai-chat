import { useState, useEffect } from "react";
import { listMcpServers, saveMcpServer, deleteMcpServer, testMcpServer } from "../api";
import type { McpServer, McpTransport } from "../types";
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

export function McpPanel({ onClose, onServersChange }: Props) {
    const [servers, setServers] = useState<McpServer[]>([]);
    const [editing, setEditing] = useState<McpServer | null>(null);
    const [testStatus, setTestStatus] = useState<Record<string, { ok: boolean; msg: string }>>({});
    const [testing, setTesting] = useState<string | null>(null);
    const [argsInput, setArgsInput] = useState("");
    const [envInput, setEnvInput] = useState("");

    useEffect(() => {
        listMcpServers().then(setServers).catch(console.error);
    }, []);

    useEffect(() => {
        onServersChange?.(servers.filter((s) => s.enabled).length);
    }, [servers, onServersChange]);

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

            <div className="mcp-footer">
                <button className="btn-primary" onClick={startAdd}>+ Add Server</button>
            </div>
        </div>
    );
}
