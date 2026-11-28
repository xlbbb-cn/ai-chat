/**
 * The plugin manager: the plugin list, live instance state, bus traffic, logs
 * and one settings page per plugin.
 *
 * Runs in its own webview (`plugin-manager.html`). Everything it shows comes
 * from the four commands in `src-tauri/src/ipc/plugins_config.rs`; nothing here
 * talks to the bus directly — the window is a viewer over `PluginHost`.
 *
 * Settings pages come in two shapes, both served as `PluginConfigView.mode`:
 * `schema` renders the generic form built from `config.schema.yaml`, `page`
 * means the plugin ships its own HTML (hosted later; the values it edits are
 * the same `config.yaml`). `none` means the plugin declares no settings.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
    exportPluginPackage,
    getPluginBusRecords,
    getPluginConfig,
    getPluginLogs,
    getPluginManagerState,
    importPluginPackage,
    reloadPlugins,
    resetPluginConfig,
    savePluginConfig,
    startPlugin,
    stopPlugin,
} from "../api";
import type {
    PluginBusRecord,
    PluginConfigField,
    PluginConfigView,
    PluginEntry,
    PluginFieldError,
    PluginInstanceView,
    PluginLogEntry,
    PluginManagerState,
    PluginTransport,
} from "../types";
import { useI18n, type MessageKey } from "../i18n";
import { FontAwesomeIcon, faBookOpen, faBoxOpen, faFileArrowDown } from "../icons";
import { PluginHelpSheet } from "./PluginHelpSheet";
import "./PluginManagerPanel.css";

/** Bus audit ring shown in the footer. */
const BUS_LIMIT = 24;
/** Log lines kept for the selected plugin. */
const LOG_LIMIT = 160;

/** Wire value → message key; keeps `t()` on the literal union. */
const TRANSPORT_LABEL: Record<PluginTransport, MessageKey> = {
    spawn: "plugins.transportSpawn",
    service: "plugins.transportService",
    both: "plugins.transportBoth",
};

const STATE_LABEL: Record<string, MessageKey> = {
    starting: "plugins.stateStarting",
    ready: "plugins.stateReady",
    failed: "plugins.stateFailed",
    stopped: "plugins.stateStopped",
};

function timeOf(ts: number): string {
    const d = new Date(ts);
    const pad = (n: number, w = 2) => String(n).padStart(w, "0");
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}

/** Draft values for one settings page; secrets start empty (kept unless typed). */
function valuesFromForm(view: PluginConfigView | null): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    if (!view?.form) return out;
    for (const section of view.form.sections) {
        for (const field of section.fields) {
            out[field.key] = field.type === "secret" ? null : field.value;
        }
    }
    return out;
}

function reveals(field: PluginConfigField, draft: Record<string, unknown>): boolean {
    const rule = field.reveal_when;
    if (!rule) return true;
    return JSON.stringify(draft[rule.key] ?? null) === JSON.stringify(rule.equals ?? null);
}

/** `null` clears nothing on its own: the backend keeps the stored secret. */
function secretHint(field: PluginConfigField): string | null {
    return field.has_value ? "stored" : "empty";
}

export function PluginManagerPanel() {
    const { t } = useI18n();
    const [state, setState] = useState<PluginManagerState | null>(null);
    const [selected, setSelected] = useState<string | null>(null);
    const [config, setConfig] = useState<PluginConfigView | null>(null);
    const [draft, setDraft] = useState<Record<string, unknown>>({});
    const [errors, setErrors] = useState<PluginFieldError[]>([]);
    const [logs, setLogs] = useState<PluginLogEntry[]>([]);
    const [bus, setBus] = useState<PluginBusRecord[]>([]);
    const [busy, setBusy] = useState<string | null>(null);
    const [helpOpen, setHelpOpen] = useState(false);
    const [notice, setNotice] = useState<string | null>(null);
    const [copied, setCopied] = useState<string | null>(null);
    const [search, setSearch] = useState("");
    const [noticeTone, setNoticeTone] = useState<"info" | "error">("info");
    const logTail = useRef<HTMLDivElement | null>(null);

    /** One notice line, coloured by whether it reports success or failure. */
    function notify(message: string, tone: "info" | "error" = "info") {
        setNotice(message);
        setNoticeTone(tone);
    }

    const refresh = useCallback(async () => {
        try {
            const [next, records] = await Promise.all([
                getPluginManagerState(),
                getPluginBusRecords(BUS_LIMIT),
            ]);
            setState(next);
            // Newest first in the footer strip.
            setBus([...records].reverse());
            setSelected((prev) =>
                prev && next.plugins.some((plugin) => plugin.id === prev)
                    ? prev
                    : next.plugins[0]?.id ?? null
            );
        } catch (err) {
            notify(String(err), "error");
        }
    }, []);

    useEffect(() => {
        void refresh();
    }, [refresh]);

    // Live updates: instance state flips and bus traffic. Cheap to listen to,
    // and the window is only open while the user is looking at it.
    useEffect(() => {
        let disposed = false;
        const stops: Promise<UnlistenFn>[] = [
            listen("plugin-state", () => {
                if (!disposed) void refresh();
            }),
            listen("plugin-bus", () => {
                if (disposed) return;
                getPluginBusRecords(BUS_LIMIT)
                    .then((records) => {
                        if (!disposed) setBus([...records].reverse());
                    })
                    .catch(() => {});
            }),
            // `open_plugin_manager` may name a plugin (Settings → a plugin's
            // own entry point); an unknown id is ignored by the list refresh.
            listen<string>("plugin-manager-select", (event) => {
                if (!disposed && typeof event.payload === "string") {
                    setSelected(event.payload);
                }
            }),
        ];
        return () => {
            disposed = true;
            stops.forEach((p) => p.then((fn) => fn()).catch(() => {}));
        };
    }, [refresh]);

    // Load the selected plugin's settings page and log tail.
    useEffect(() => {
        if (!selected) {
            setConfig(null);
            setLogs([]);
            return;
        }
        let disposed = false;
        Promise.all([getPluginConfig(selected), getPluginLogs(selected, LOG_LIMIT)])
            .then(([view, entries]) => {
                if (disposed) return;
                setConfig(view);
                setDraft(valuesFromForm(view));
                setErrors([]);
                setLogs(entries);
            })
            .catch((err) => {
                if (!disposed) notify(String(err), "error");
            });
        return () => {
            disposed = true;
        };
    }, [selected]);

    useEffect(() => {
        const tail = logTail.current;
        if (tail) tail.scrollTop = tail.scrollHeight;
    }, [logs]);

    const plugin = useMemo(
        () => state?.plugins.find((p) => p.id === selected) ?? null,
        [state, selected]
    );
    const instances = useMemo(
        () => state?.instances.filter((instance) => instance.plugin_id === selected) ?? [],
        [state, selected]
    );
    // The column is short, but a workspace may hold a few dozen plugins once
    // they are importable; filtering by name, id, description or tool name
    // finds one faster than reading the list.
    const filtered = useMemo(() => {
        const list = state?.plugins ?? [];
        const query = search.trim().toLowerCase();
        if (!query) return list;
        return list.filter(
            (entry) =>
                entry.name.toLowerCase().includes(query) ||
                entry.id.toLowerCase().includes(query) ||
                entry.description.toLowerCase().includes(query) ||
                entry.tools.some((tool) => tool.toLowerCase().includes(query))
        );
    }, [state, search]);

    const setValue = useCallback((key: string, value: unknown) => {
        setDraft((prev) => ({ ...prev, [key]: value }));
    }, []);

    async function handleSave() {
        if (!selected) return;
        setBusy("save");
        setNotice(null);
        try {
            const view = await savePluginConfig(selected, draft);
            setConfig(view);
            setErrors(view.errors ?? []);
            setDraft(valuesFromForm(view));
            if (view.errors?.length) setNotice(null);
            else notify(t("plugins.saved"));
            await refresh();
        } catch (err) {
            // A validation failure arrives as `[{ key, message }]`.
            if (Array.isArray(err)) setErrors(err as PluginFieldError[]);
            else notify(String(err), "error");
        } finally {
            setBusy(null);
        }
    }

    async function handleReset() {
        if (!selected) return;
        setBusy("reset");
        setNotice(null);
        try {
            const view = await resetPluginConfig(selected);
            setConfig(view);
            setDraft(valuesFromForm(view));
            setErrors([]);
            notify(t("plugins.saved"));
            await refresh();
        } catch (err) {
            notify(String(err), "error");
        } finally {
            setBusy(null);
        }
    }

    async function handleLifecycle(kind: "start" | "stop" | "reload") {
        setBusy(kind);
        setNotice(null);
        try {
            if (kind === "start" && selected) await startPlugin(selected);
            else if (kind === "stop" && selected) await stopPlugin(selected);
            else await reloadPlugins();
            await refresh();
        } catch (err) {
            notify(String(err), "error");
        } finally {
            setBusy(null);
        }
    }

    /**
     * Import a `.zip` package. The backend stages the folder, validates the
     * manifest, stops the instances of a plugin it replaces and keeps the old
     * copy under `.data/plugin-backups/` — the window only reports it.
     */
    async function handleImport() {
        setBusy("import");
        setNotice(null);
        try {
            const picked = await openDialog({
                multiple: false,
                directory: false,
                title: t("plugins.package.importTitle"),
                filters: [{ name: t("plugins.package.filter"), extensions: ["zip"] }],
            });
            if (!picked || Array.isArray(picked)) return;
            const report = await importPluginPackage(picked);
            await refresh();
            setSelected(report.plugin.id);
            const parts = [
                t("plugins.package.imported", {
                    name: report.plugin.name || report.plugin.id,
                    files: report.files,
                }),
            ];
            if (report.replaced) {
                parts.push(t("plugins.package.importReplace", { dir: report.replaced }));
            }
            notify(parts.join(" · "));
        } catch (err) {
            notify(t("plugins.package.importFailed", { message: String(err) }), "error");
        } finally {
            setBusy(null);
        }
    }

    /** Export the selected plugin to the `.zip` the user names. */
    async function handleExport() {
        if (!plugin) return;
        setBusy("export");
        setNotice(null);
        try {
            const picked = await saveDialog({
                title: t("plugins.package.exportTitle", { name: plugin.name }),
                defaultPath: `${plugin.id}-${plugin.version || "0.0.0"}.zip`,
                filters: [{ name: t("plugins.package.filter"), extensions: ["zip"] }],
            });
            if (!picked || Array.isArray(picked)) return;
            const report = await exportPluginPackage(plugin.id, picked);
            const parts = [
                t("plugins.package.exported", { files: report.files, path: report.path }),
            ];
            if (report.secrets_removed > 0) {
                parts.push(
                    t("plugins.package.exportedSecrets", { count: report.secrets_removed })
                );
            }
            if (!report.settings_exported) {
                parts.push(t("plugins.package.exportedNoSettings"));
            }
            notify(parts.join(" · "));
        } catch (err) {
            notify(t("plugins.package.exportFailed", { message: String(err) }), "error");
        } finally {
            setBusy(null);
        }
    }

    async function copy(text: string, tag: string) {
        try {
            await navigator.clipboard.writeText(text);
            setCopied(tag);
            window.setTimeout(() => setCopied(null), 1200);
        } catch {
            notify(text);
        }
    }

    const globalError = errors.find((error) => !error.key);
    const errorFor = (key: string) => errors.find((error) => error.key === key)?.message;

    return (
        <div className="pm-root">
            <header className="pm-header">
                <div className="pm-heading">
                    <div className="pm-title-row">
                        <h1>{t("plugins.windowTitle")}</h1>
                        <button
                            type="button"
                            className="close-btn"
                            onClick={() => setHelpOpen(true)}
                            aria-label={t("plugins.help.open")}
                            title={t("plugins.help.open")}
                        >
                            <FontAwesomeIcon icon={faBookOpen} />
                        </button>
                    </div>
                    <p>{t("plugins.subtitle")}</p>
                </div>
                <div className="pm-header-actions">
                    <button
                        type="button"
                        className="btn-secondary pm-action"
                        onClick={() => void handleImport()}
                        disabled={busy !== null}
                        title={t("plugins.package.hint")}
                    >
                        <FontAwesomeIcon icon={faBoxOpen} aria-hidden="true" />
                        {busy === "import"
                            ? t("plugins.package.importing")
                            : t("plugins.package.import")}
                    </button>
                    <button
                        type="button"
                        className="btn-secondary pm-action"
                        onClick={() => void refresh()}
                        disabled={busy !== null}
                    >
                        {t("plugins.refresh")}
                    </button>
                    <button
                        type="button"
                        className="btn-secondary pm-action"
                        onClick={() => void handleLifecycle("reload")}
                        disabled={busy !== null}
                    >
                        {t("plugins.reload")}
                    </button>
                </div>
            </header>

            {helpOpen && <PluginHelpSheet onClose={() => setHelpOpen(false)} />}

            {state?.endpoint && (
                <div className="pm-endpoint">
                    <div className="pm-endpoint-row">
                        <span className="pm-endpoint-label">{t("plugins.endpoint")}</span>
                        <code className="pm-endpoint-value">{state.endpoint.address}</code>
                        <button
                            type="button"
                            className="pm-link"
                            onClick={() => void copy(state.endpoint!.address, "address")}
                        >
                            {copied === "address" ? t("plugins.copied") : t("plugins.copy")}
                        </button>
                    </div>
                    <div className="pm-endpoint-row">
                        <span className="pm-endpoint-label">{t("plugins.token")}</span>
                        <code className="pm-endpoint-value pm-endpoint-token">
                            {state.endpoint.token}
                        </code>
                        <button
                            type="button"
                            className="pm-link"
                            onClick={() => void copy(state.endpoint!.token, "token")}
                        >
                            {copied === "token" ? t("plugins.copied") : t("plugins.copy")}
                        </button>
                    </div>
                    <p className="pm-hint">{t("plugins.endpointHint")}</p>
                </div>
            )}

            {state && state.issues.length > 0 && (
                <div className="pm-issues">
                    <strong>{t("plugins.issues")}</strong>
                    <ul>
                        {state.issues.map((issue) => (
                            <li key={issue.dir}>
                                <code>{issue.dir}</code> — {issue.error}
                            </li>
                        ))}
                    </ul>
                </div>
            )}

            {notice && (
                <div
                    className={`pm-notice pm-notice-${noticeTone}`}
                    role={noticeTone === "error" ? "alert" : "status"}
                >
                    {notice}
                </div>
            )}

            <div className="pm-body">
                <aside className="pm-list" aria-label={t("plugins.windowTitle")}>
                    <div className="pm-list-head">
                        <input
                            className="panel-search-input"
                            type="search"
                            value={search}
                            onChange={(event) => setSearch(event.target.value)}
                            placeholder={t("plugins.search")}
                            aria-label={t("plugins.search")}
                        />
                        <span className="pm-list-count">
                            {t("plugins.count", {
                                shown: filtered.length,
                                total: state?.plugins.length ?? 0,
                            })}
                        </span>
                    </div>
                    {state?.plugins.length === 0 && (
                        <div className="pm-empty">
                            <strong>{t("plugins.empty")}</strong>
                            <span>{t("plugins.emptyHint")}</span>
                        </div>
                    )}
                    {state && state.plugins.length > 0 && filtered.length === 0 && (
                        <div className="pm-empty">
                            <span>{t("plugins.searchEmpty", { query: search.trim() })}</span>
                        </div>
                    )}
                    {filtered.map((entry) => (
                        <PluginRow
                            key={entry.id}
                            entry={entry}
                            active={entry.id === selected}
                            onSelect={setSelected}
                        />
                    ))}
                </aside>

                <main className="pm-detail">
                    {!plugin && <div className="pm-empty pm-empty-main">{t("plugins.empty")}</div>}
                    {plugin && (
                        <>
                            <div className="pm-plugin-head">
                                <div className="pm-plugin-title">
                                    <h2>{plugin.name}</h2>
                                    <span className="pm-version">{plugin.version || "0.0.0"}</span>
                                    <span className={`pm-badge pm-badge-${plugin.transport}`}>
                                        {plugin.transport === "service"
                                            ? t("plugins.transportService")
                                            : plugin.transport === "both"
                                                ? t("plugins.transportBoth")
                                                : t("plugins.transportSpawn")}
                                    </span>
                                    <span className={`pm-badge ${plugin.running > 0 ? "pm-badge-on" : ""}`}>
                                        {plugin.running > 0 ? t("plugins.running") : t("plugins.stopped")}
                                    </span>
                                </div>
                                <p className="pm-plugin-desc">
                                    {plugin.description || <code>{plugin.id}</code>}
                                </p>
                                <div className="pm-plugin-actions">
                                    {/* `start` retires the pool first (`PluginHost::start`),
                                        so a live plugin says what the click actually does. */}
                                    <button
                                        type="button"
                                        className="btn-primary pm-action"
                                        onClick={() => void handleLifecycle("start")}
                                        disabled={busy !== null || !plugin.enabled}
                                    >
                                        {plugin.running > 0
                                            ? t("plugins.restart")
                                            : t("plugins.start")}
                                    </button>
                                    <button
                                        type="button"
                                        className="btn-secondary pm-action"
                                        onClick={() => void handleLifecycle("stop")}
                                        disabled={busy !== null}
                                    >
                                        {t("plugins.stop")}
                                    </button>
                                    <button
                                        type="button"
                                        className="btn-secondary pm-action"
                                        onClick={() => void handleExport()}
                                        disabled={busy !== null}
                                        title={t("plugins.package.hint")}
                                    >
                                        <FontAwesomeIcon icon={faFileArrowDown} aria-hidden="true" />
                                        {busy === "export"
                                            ? t("plugins.package.exporting")
                                            : t("plugins.package.export")}
                                    </button>
                                </div>
                            </div>

                            <section className="pm-section">
                                <h3>{t("plugins.instances")}</h3>
                                {instances.length === 0 ? (
                                    <p className="pm-hint">{t("plugins.noInstances")}</p>
                                ) : (
                                    <table className="pm-instances">
                                        <thead>
                                            <tr>
                                                <th>{t("plugins.instances")}</th>
                                                <th>{t("plugins.state")}</th>
                                                <th>pid</th>
                                                <th>{t("plugins.inflight")}</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {instances.map((instance) => (
                                                <InstanceRow key={instance.label} instance={instance} />
                                            ))}
                                        </tbody>
                                    </table>
                                )}
                            </section>

                            <section className="pm-section">
                                <h3>{t("plugins.settings")}</h3>
                                {globalError && <div className="pm-notice">{globalError.message}</div>}
                                {config?.schema_error && (
                                    <div className="pm-notice">{config.schema_error}</div>
                                )}
                                {config?.mode === "page" && (
                                    <p className="pm-hint">
                                        {t("plugins.pageHint", { page: config.page ?? "" })}{" "}
                                        {t("plugins.pagePending")}
                                    </p>
                                )}
                                {config?.mode === "none" && (
                                    <p className="pm-hint">{t("plugins.schemaMissing")}</p>
                                )}
                                {config?.form && (
                                    <>
                                        {config.form.sections.map((section) => {
                                            const visible = section.fields.filter((field) =>
                                                reveals(field, draft)
                                            );
                                            const normal = visible.filter((field) => !field.advanced);
                                            const advanced = visible.filter((field) => field.advanced);
                                            if (visible.length === 0) return null;
                                            return (
                                                <div className="pm-form-section" key={section.key}>
                                                    <h4>{section.title || section.key}</h4>
                                                    {section.description && (
                                                        <p className="pm-hint">{section.description}</p>
                                                    )}
                                                    {normal.map((field) => (
                                                        <Field
                                                            key={field.key}
                                                            field={field}
                                                            draft={draft}
                                                            error={errorFor(field.key)}
                                                            onChange={setValue}
                                                        />
                                                    ))}
                                                    {advanced.length > 0 && (
                                                        <details className="pm-advanced">
                                                            <summary>{t("plugins.advanced")}</summary>
                                                            {advanced.map((field) => (
                                                                <Field
                                                                    key={field.key}
                                                                    field={field}
                                                                    draft={draft}
                                                                    error={errorFor(field.key)}
                                                                    onChange={setValue}
                                                                />
                                                            ))}
                                                        </details>
                                                    )}
                                                </div>
                                            );
                                        })}
                                        <div className="pm-form-actions">
                                            <button
                                                type="button"
                                                className="btn-primary pm-action"
                                                onClick={() => void handleSave()}
                                                disabled={busy !== null}
                                            >
                                                {busy === "save" ? t("plugins.saving") : t("plugins.save")}
                                            </button>
                                            <button
                                                type="button"
                                                className="btn-secondary pm-action"
                                                onClick={() => void handleReset()}
                                                disabled={busy !== null}
                                            >
                                                {t("plugins.reset")}
                                            </button>
                                        </div>
                                    </>
                                )}
                            </section>

                            <section className="pm-section">
                                <h3>{t("plugins.logs")}</h3>
                                {logs.length === 0 ? (
                                    <p className="pm-hint">{t("plugins.noLogs")}</p>
                                ) : (
                                    <div className="pm-logs" ref={logTail}>
                                        {logs.map((entry, index) => (
                                            <div className={`pm-log pm-log-${entry.level}`} key={index}>
                                                <span className="pm-log-ts">{timeOf(entry.ts_ms)}</span>
                                                <span className="pm-log-level">{entry.level}</span>
                                                <span className="pm-log-msg">{entry.message}</span>
                                            </div>
                                        ))}
                                    </div>
                                )}
                            </section>
                        </>
                    )}
                </main>
            </div>

            <footer className="pm-bus">
                <h3>{t("plugins.bus")}</h3>
                {bus.length === 0 ? (
                    <p className="pm-hint">{t("plugins.noBus")}</p>
                ) : (
                    <div className="pm-bus-rows">
                        {bus.map((record) => (
                            <div className={`pm-bus-row pm-bus-${record.kind}`} key={record.seq}>
                                <span className="pm-log-ts">{timeOf(record.ts_ms)}</span>
                                <span className={`pm-kind pm-kind-${record.kind}`}>{record.kind}</span>
                                <span className="pm-bus-plugin">{record.plugin}</span>
                                <span className="pm-bus-topic">{record.topic}</span>
                                {record.tool && <span className="pm-bus-tool">{record.tool}</span>}
                                {record.ok !== null && record.ok !== undefined && (
                                    <span className={record.ok ? "pm-ok" : "pm-fail"}>
                                        {record.ok ? "ok" : "fail"}
                                    </span>
                                )}
                                {record.duration_ms !== null && record.duration_ms !== undefined && (
                                    <span className="pm-bus-dur">{record.duration_ms} ms</span>
                                )}
                                {record.detail && (
                                    <span className="pm-bus-detail">{record.detail}</span>
                                )}
                            </div>
                        ))}
                    </div>
                )}
            </footer>
        </div>
    );
}

function PluginRow({
    entry,
    active,
    onSelect,
}: {
    entry: PluginEntry;
    active: boolean;
    onSelect: (id: string) => void;
}) {
    const { t } = useI18n();
    return (
        <button
            type="button"
            className={`pm-row ${active ? "pm-row-active" : ""} ${entry.enabled ? "" : "pm-row-off"}`}
            onClick={() => onSelect(entry.id)}
        >
            <span className="pm-row-name">
                <span
                    className={`pm-dot ${entry.running > 0 ? "pm-dot-on" : ""}`}
                    aria-hidden="true"
                />
                {entry.name}
            </span>
            <span className="pm-row-meta">
                <code>{entry.id}</code>
                <span>
                    {entry.running}/{entry.instances}
                </span>
            </span>
            <span className="pm-row-modes">
                <span className="pm-chip">{t(TRANSPORT_LABEL[entry.transport])}</span>
                {entry.config_mode !== "none" && (
                    <span className="pm-chip">
                        {entry.config_mode === "page"
                            ? t("plugins.configModePage")
                            : t("plugins.configModeSchema")}
                    </span>
                )}
            </span>
        </button>
    );
}

function InstanceRow({ instance }: { instance: PluginInstanceView }) {
    const { t } = useI18n();
    return (
        <tr>
            <td>
                <code>{instance.label}</code>
            </td>
            <td>
                <span className={`pm-state pm-state-${instance.state}`}>
                    {t(STATE_LABEL[instance.state] ?? "plugins.stateStopped")}
                </span>
                {instance.last_error && <span className="pm-fail"> · {instance.last_error}</span>}
            </td>
            <td>{instance.pid ?? "—"}</td>
            <td>
                {t("plugins.instanceInflight", { count: instance.inflight })}
                {instance.restarts > 0 && ` · ${t("plugins.instanceRestarts", { count: instance.restarts })}`}
            </td>
        </tr>
    );
}

function Field({
    field,
    draft,
    error,
    onChange,
}: {
    field: PluginConfigField;
    draft: Record<string, unknown>;
    error?: string;
    onChange: (key: string, value: unknown) => void;
}) {
    const { t } = useI18n();
    const value = draft[field.key];
    const label = field.title || field.key;

    return (
        <label className={`pm-field ${error ? "pm-field-error" : ""}`}>
            <span className="pm-field-label">
                {label}
                {field.required && <span className="pm-required">*</span>}
                {field.unit && <span className="pm-unit">{field.unit}</span>}
            </span>
            {field.description && <span className="pm-hint">{field.description}</span>}
            {field.type === "boolean" ? (
                <input
                    type="checkbox"
                    checked={Boolean(value)}
                    onChange={(event) => onChange(field.key, event.target.checked)}
                />
            ) : field.type === "enum" ? (
                <select
                    value={typeof value === "string" ? value : ""}
                    onChange={(event) => onChange(field.key, event.target.value)}
                >
                    {field.options.map((option) => (
                        <option key={option.value} value={option.value}>
                            {option.label || option.value}
                        </option>
                    ))}
                </select>
            ) : field.type === "multiline" || field.type === "json" ? (
                <textarea
                    rows={field.type === "json" ? 6 : 4}
                    value={typeof value === "string" ? value : value == null ? "" : JSON.stringify(value, null, 2)}
                    placeholder={field.placeholder}
                    onChange={(event) => onChange(field.key, event.target.value)}
                />
            ) : field.type === "number" || field.type === "integer" ? (
                <input
                    type="number"
                    value={typeof value === "number" ? value : ""}
                    min={field.min ?? undefined}
                    max={field.max ?? undefined}
                    step={field.step ?? undefined}
                    placeholder={field.placeholder}
                    onChange={(event) =>
                        onChange(
                            field.key,
                            event.target.value === "" ? null : Number(event.target.value)
                        )
                    }
                />
            ) : field.type === "secret" ? (
                <input
                    type="password"
                    autoComplete="off"
                    value={typeof value === "string" ? value : ""}
                    placeholder={
                        secretHint(field) === "stored"
                            ? t("plugins.secretStored")
                            : t("plugins.secretEmpty")
                    }
                    maxLength={field.max_length ?? undefined}
                    onChange={(event) => onChange(field.key, event.target.value)}
                />
            ) : (
                <input
                    type="text"
                    value={typeof value === "string" ? value : ""}
                    placeholder={field.placeholder}
                    maxLength={field.max_length ?? undefined}
                    onChange={(event) => onChange(field.key, event.target.value)}
                />
            )}
            {error && <span className="pm-field-error-text">{error}</span>}
        </label>
    );
}
