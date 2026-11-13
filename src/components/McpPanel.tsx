import { useEffect, useRef, useState } from "react";
import {
    listMcpServers,
    saveMcpServer,
    deleteMcpServer,
    testMcpServer,
    startStdioMcp,
    stopStdioMcp,
    getMcpLogs,
    clearMcpLogs,
    onMcpServersChanged,
} from "../api";
import type { McpServer, McpTransport, McpLogEntry } from "../types";
import { useI18n } from "../i18n";
import {
    FontAwesomeIcon,
    faAnglesLeft,
    faBolt,
    faCircleCheck,
    faCircleXmark,
    faPen,
    faScroll,
    faXmark,
} from "../icons";
import { Dropdown } from "./Dropdown";
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

function transportLabel(s: McpServer): string {
    if (s.transport === "stdio") {
        return `stdio: ${s.command}${s.args.length ? " " + s.args.join(" ") : ""}`;
    }
    return `${s.transport}: ${s.url}`;
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
    const { t } = useI18n();
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

    // The assistant can build and enable servers itself (`mcp_self`, evolution
    // mode). Refresh the list when the registry changes so the panel never
    // shows a stale roster; a deleted entry also closes its log modal.
    useEffect(() => {
        let disposed = false;
        let unlisten: (() => void) | undefined;
        onMcpServersChanged(() => {
            if (disposed) return;
            listMcpServers()
                .then((list) => {
                    if (disposed) return;
                    setServers(list);
                    if (logServer && !list.some((s) => s.id === logServer.id)) {
                        closeLogs();
                    }
                })
                .catch(console.error);
        })
            .then((stop) => {
                if (disposed) stop();
                else unlisten = stop;
            })
            .catch(console.error);
        return () => {
            disposed = true;
            unlisten?.();
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [logServer]);

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
        const enabling = !s.enabled;
        const updated = { ...s, enabled: enabling };
        await saveMcpServer(updated);
        setServers((prev) => prev.map((x) => (x.id === updated.id ? updated : x)));

        if (enabling) {
            // Start the persistent stdio process
            try {
                await startStdioMcp(updated.id);
            } catch (err) {
                console.error("Failed to start stdio process:", err);
            }
        } else {
            // Stop the persistent stdio process
            try {
                await stopStdioMcp(updated.id);
            } catch (err) {
                console.error("Failed to stop stdio process:", err);
            }
        }
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
        // Auto-open the log modal so the user can see real-time stderr output
        // during package installation or startup.
        openLogs(s);
        try {
            const msg = await testMcpServer(s);
            setTestStatus((prev) => ({ ...prev, [s.id]: { ok: true, msg } }));
        } catch (err) {
            setTestStatus((prev) => ({ ...prev, [s.id]: { ok: false, msg: String(err) } }));
        } finally {
            setTesting(null);
        }
        // Refresh logs after test completes
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
                <div className="panel-header">
                    <h2>{servers.some((s) => s.id === editing.id) ? t("mcp.editTitle") : t("mcp.addTitle")}</h2>
                    <button className="close-btn" onClick={() => setEditing(null)} aria-label={t("common.close")}>
                        <FontAwesomeIcon icon={faXmark} />
                    </button>
                </div>

                <div className="mcp-form">
                    <label>{t("mcp.name")}</label>
                    <input
                        className="mcp-input"
                        value={editing.name}
                        onChange={(e) => setEditing({ ...editing, name: e.target.value })}
                        placeholder={t("mcp.namePlaceholder")}
                    />

                    <label>{t("mcp.transport")}</label>
                    <Dropdown
                        label={t("mcp.transport")}
                        value={editing.transport}
                        onChange={(transport) => setEditing({ ...editing, transport: transport as McpTransport })}
                        options={[
                            { value: "stdio", label: t("mcp.transportStdio") },
                            { value: "sse", label: t("mcp.transportSse") },
                            { value: "http", label: t("mcp.transportHttp") },
                            { value: "stream-http", label: t("mcp.transportStreamHttp") },
                        ]}
                    />

                    {editing.transport === "stdio" ? (
                        <>
                            <label>{t("mcp.command")}</label>
                            <input
                                className="mcp-input"
                                value={editing.command}
                                onChange={(e) => setEditing({ ...editing, command: e.target.value })}
                                placeholder={t("mcp.commandPlaceholder")}
                            />

                            <label>{t("mcp.arguments")} <span className="mcp-hint">{t("mcp.argumentsHint")}</span></label>
                            <input
                                className="mcp-input"
                                value={argsInput}
                                onChange={(e) => setArgsInput(e.target.value)}
                                placeholder={t("mcp.argumentsPlaceholder")}
                            />

                            <label>{t("mcp.envVars")} <span className="mcp-hint">{t("mcp.envVarsHint")}</span></label>
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
                            <label>{t("mcp.url")}</label>
                            <input
                                className="mcp-input"
                                value={editing.url}
                                onChange={(e) => setEditing({ ...editing, url: e.target.value })}
                                placeholder={
                                    editing.transport === "sse"
                                        ? "http://localhost:8000/sse"
                                        : "http://localhost:8000/mcp"
                                }
                            />

                            <label>{t("mcp.authToken")} <span className="mcp-hint">{t("mcp.authTokenHint")}</span></label>
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
                    <button className="btn-secondary" onClick={() => setEditing(null)}>{t("common.cancel")}</button>
                    <button
                        className="btn-primary"
                        onClick={() => void saveEditing()}
                        disabled={!editing.name.trim()}
                    >
                        {t("common.save")}
                    </button>
                </div>
            </div>
        );
    }

    return (
        <div className="mcp-panel">
            <div className="panel-header">
                <h2>{t("mcp.title")}</h2>
                <button className="close-btn" onClick={onClose} aria-label={t("common.collapse")}>
                    <FontAwesomeIcon icon={faAnglesLeft} />
                </button>
            </div>

            <div className="mcp-desc">
                {t("mcp.description")}
            </div>

            <div className="mcp-list">
                {servers.length === 0 && (
                    <div className="mcp-empty">{t("mcp.empty")}</div>
                )}
                {servers.map((s) => (
                    <div key={s.id} className={`mcp-server-item ${s.enabled ? "enabled" : "disabled"}`}>
                        <div className="mcp-server-row">
                            <label className="toggle-switch" title={s.enabled ? t("common.disable") : t("common.enable")}>
                                <input
                                    type="checkbox"
                                    checked={s.enabled}
                                    onChange={() => void toggleEnabled(s)}
                                />
                                <span className="toggle-switch-slider" />
                            </label>
                            <div className="mcp-server-info">
                                <span
                                    className="mcp-server-name"
                                    title={t("mcp.toggleTitle", {
                                        action: s.enabled ? t("common.disable") : t("common.enable"),
                                        name: s.name,
                                    })}
                                    onClick={() => void toggleEnabled(s)}
                                >
                                    {s.name || t("common.unnamed")}
                                    {s.source === "self-evolution" && (
                                        <span
                                            className="mcp-self-badge"
                                            title={t("mcp.selfBuiltHint", {
                                                path: s.definition_path || t("common.unnamed"),
                                            })}
                                        >
                                            {t("mcp.selfBuilt")}
                                        </span>
                                    )}
                                    {testing === s.id && (
                                        <span className="mcp-testing-badge" title={t("mcp.testingTitle")}>
                                            <span className="mcp-testing-spinner" />
                                            {t("mcp.testing")}
                                        </span>
                                    )}
                                </span>
                                <span className="mcp-server-transport">
                                    {transportLabel(s)}
                                </span>
                            </div>
                            <div className="mcp-server-actions">
                                <button
                                    className="mcp-action-btn"
                                    title={t("mcp.testConnection")}
                                    onClick={() => void runTest(s)}
                                    disabled={testing === s.id || !s.enabled}
                                >
                                    <FontAwesomeIcon icon={faBolt} />
                                </button>
                                <button
                                    className="mcp-action-btn"
                                    title={t("mcp.viewLogs")}
                                    onClick={() => openLogs(s)}
                                >
                                    <FontAwesomeIcon icon={faScroll} />
                                </button>
                                <button
                                    className="mcp-action-btn"
                                    title={t("common.edit")}
                                    onClick={() => startEdit(s)}
                                >
                                    <FontAwesomeIcon icon={faPen} />
                                </button>
                                <button
                                    className="mcp-action-btn danger"
                                    title={t("common.delete")}
                                    onClick={() => void remove(s.id)}
                                >
                                    <FontAwesomeIcon icon={faXmark} />
                                </button>
                            </div>
                        </div>
                        {testStatus[s.id] && (
                            <div className={`mcp-test-result ${testStatus[s.id].ok ? "ok" : "fail"}`}>
                                <FontAwesomeIcon icon={testStatus[s.id].ok ? faCircleCheck : faCircleXmark} />
                                {testStatus[s.id].msg}
                            </div>
                        )}
                    </div>
                ))}
            </div>

            <div className="mcp-footer">
                <button className="btn-primary" onClick={startAdd}>{t("mcp.addServer")}</button>
            </div>

            {logServer && (
                <div className="mcp-log-modal-backdrop" onClick={closeLogs}>
                    <div className="mcp-log-modal" onClick={(e) => e.stopPropagation()}>
                        <div className="mcp-log-modal-header">
                            <div className="mcp-log-modal-title">
                                <span className="mcp-log-modal-title-main">
                                    {t("mcp.logsTitle", { name: logServer.name || t("common.unnamed") })}
                                </span>
                                <span className="mcp-log-modal-title-sub">
                                    {transportLabel(logServer)}
                                </span>
                            </div>
                            <div className="mcp-log-modal-actions">
                                <button
                                    className="mcp-log-btn"
                                    onClick={() => void refreshLogs()}
                                    disabled={logLoading}
                                >
                                    {t("common.refresh")}
                                </button>
                                <button
                                    className="mcp-log-btn"
                                    onClick={() => void clearLogs()}
                                >
                                    {t("common.delete")}
                                </button>
                                <button className="mcp-log-btn" onClick={closeLogs} aria-label={t("common.close")}>
                                    <FontAwesomeIcon icon={faXmark} />
                                </button>
                            </div>
                        </div>
                        <div className="mcp-log-modal-meta">
                            {testing === logServer.id ? (
                                <span className="mcp-log-modal-testing">
                                    <span className="mcp-testing-spinner" /> {t("mcp.testInProgress")}
                                </span>
                            ) : logEntries.length === 0 ? (
                                t("mcp.noLogs")
                            ) : (
                                t("mcp.logCount", { count: logEntries.length })
                            )}
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
                                    {t("mcp.logPlaceholder")}
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
