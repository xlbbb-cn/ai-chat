import { useState, useEffect } from "react";
import { getConfig, saveConfig, listServeProcesses, stopServeProcess } from "../api";
import type { AppConfig, AutoAcceptKind, ConfirmKind, ServeProcessInfo } from "../types";
import { useI18n, type MessageKey } from "../i18n";
import { FontAwesomeIcon, faChevronDown, faChevronUp, faTrash, faXmark } from "../icons";
import "./ToolsPanel.css";

interface Props {
    onClose: () => void;
    onToolsChange: (tools: string[]) => void;
}

/** Tool ids with the message keys for their label / description. */
const AVAILABLE_TOOLS: { id: string; nameKey: MessageKey; descriptionKey: MessageKey }[] = [
    {
        id: "file_actions",
        nameKey: "tools.items.fileActions.name",
        descriptionKey: "tools.items.fileActions.description",
    },
    {
        id: "run_cmd",
        nameKey: "tools.items.runCmd.name",
        descriptionKey: "tools.items.runCmd.description",
    },
    {
        id: "run_shell",
        nameKey: "tools.items.runShell.name",
        descriptionKey: "tools.items.runShell.description",
    },
    {
        id: "memory",
        nameKey: "tools.items.memory.name",
        descriptionKey: "tools.items.memory.description",
    },
    {
        id: "todo_list",
        nameKey: "tools.items.todoList.name",
        descriptionKey: "tools.items.todoList.description",
    },

    {
        id: "timer",
        nameKey: "tools.items.timer.name",
        descriptionKey: "tools.items.timer.description",
    },
    {
        id: "knowledge_graph",
        nameKey: "tools.items.knowledgeGraph.name",
        descriptionKey: "tools.items.knowledgeGraph.description",
    }
];

const AVAILABLE_TOOL_IDS = new Set(AVAILABLE_TOOLS.map((t) => t.id));

/**
 * Tools that self-evolution mode owns. The assistant needs `file_actions` to
 * edit its own skills and `memory` to record what it learned, so while the
 * mode is on they cannot be switched off. The backend enforces the same
 * invariant (`AppConfig::effective_tools`), so this is only the UI mirror of
 * it — the switches stay visible and checked, just locked.
 */
const SELF_EVOLUTION_LOCKED_TOOLS = ["file_actions", "memory"];

const KG_ENGINES = [
    { value: "neo4j", label: "Neo4j" }
];

/**
 * Confirmation kinds that can actually reach the approval dialog. The `kind`
 * strings are matched against `auto_accept_confirm_kinds` by the Rust side,
 * which sends them from `RiskLevel::confirm_kind()` in `src-tauri/src/tools/risk.rs`
 * (L0-L3 ask for approval, L4-L6 never prompt). Keep in sync.
 */
const AUTO_ACCEPT_OPTIONS: Array<{
    kind: ConfirmKind;
    labelKey: MessageKey;
    descriptionKey: MessageKey;
}> = [
        {
            kind: "dangerous",
            labelKey: "tools.autoAcceptItems.dangerous.label",
            descriptionKey: "tools.autoAcceptItems.dangerous.description",
        },
        {
            kind: "system_config",
            labelKey: "tools.autoAcceptItems.systemConfig.label",
            descriptionKey: "tools.autoAcceptItems.systemConfig.description",
        },
        {
            kind: "user_software",
            labelKey: "tools.autoAcceptItems.userSoftware.label",
            descriptionKey: "tools.autoAcceptItems.userSoftware.description",
        },
        {
            kind: "user_data",
            labelKey: "tools.autoAcceptItems.userData.label",
            descriptionKey: "tools.autoAcceptItems.userData.description",
        },
    ];

/**
 * Kinds that are preserved in `auto_accept_confirm_kinds` but are not offered
 * here: `cli` / `cloud` agents are approved per agent from the Sub Agents
 * panel (`require_approval`), so their kind has no Tools-panel switch.
 */
const HIDDEN_AUTO_ACCEPT_KINDS = ["external_agent"];

/** Every kind the panel can toggle individually. */
const ALL_AUTO_ACCEPT_KINDS: ConfirmKind[] = AUTO_ACCEPT_OPTIONS.map((option) => option.kind);

const AUTO_ACCEPT_KIND_SET = new Set<string>(AUTO_ACCEPT_OPTIONS.map((option) => option.kind));

/**
 * Wildcard stored in `auto_accept_confirm_kinds`: the Rust matcher treats it as
 * "any kind", so the one-click "auto-approve everything" switch also covers
 * kinds added in a future version instead of silently missing them.
 */
const AUTO_ACCEPT_ALL = "*";

/**
 * Drop values the backend would never emit (e.g. the retired `sudo` /
 * `elevation` kinds) so a stale config cannot keep dead entries around, while
 * preserving the `"*"` wildcard and the kinds hidden from this panel.
 */
function sanitizeAutoAcceptKinds(values: readonly string[] | undefined): AutoAcceptKind[] {
    return (values ?? []).filter(
        (value): value is AutoAcceptKind =>
            value === AUTO_ACCEPT_ALL ||
            AUTO_ACCEPT_KIND_SET.has(value) ||
            HIDDEN_AUTO_ACCEPT_KINDS.includes(value),
    );
}

/** Message key for a serve process state reported by the backend. */
const SERVE_STATE_KEYS: Record<string, MessageKey> = {
    running: "tools.serveStateRunning",
    exited: "tools.serveStateExited",
    lost: "tools.serveStateLost",
    stopped: "tools.serveStateStopped",
};

/** `75` → `1m 15s`; keeps the monitor row to one line. */
function formatUptime(totalSeconds: number): string {
    if (totalSeconds >= 3600) {
        return `${Math.floor(totalSeconds / 3600)}h ${Math.floor((totalSeconds % 3600) / 60)}m`;
    }
    if (totalSeconds >= 60) {
        return `${Math.floor(totalSeconds / 60)}m ${totalSeconds % 60}s`;
    }
    return `${totalSeconds}s`;
}

export function ToolsPanel({ onClose, onToolsChange }: Props) {
    const { t } = useI18n();
    const [config, setConfig] = useState<AppConfig | null>(null);
    const [expandedTool, setExpandedTool] = useState<string | null>(null);
    const [autoAcceptExpanded, setAutoAcceptExpanded] = useState(true);
    const [kgExpanded, setKgExpanded] = useState(false);
    const [serveExpanded, setServeExpanded] = useState(false);
    const [serveProcesses, setServeProcesses] = useState<ServeProcessInfo[]>([]);
    const [stoppingServe, setStoppingServe] = useState<string | null>(null);

    useEffect(() => {
        getConfig()
            .then((cfg) => {
                const selected = (cfg.selected_tools ?? []).filter((id) => AVAILABLE_TOOL_IDS.has(id));
                const sanitized = {
                    ...cfg,
                    selected_tools: selected,
                    auto_accept_confirm_kinds: sanitizeAutoAcceptKinds(cfg.auto_accept_confirm_kinds),
                };
                setConfig(sanitized);
                setAutoAcceptExpanded(false);
                onToolsChange(selected);
            })
            .catch(console.error);
    }, []);

    // Serve processes can start or exit at any moment (the model may launch one
    // while this panel is open), so poll the registry while the panel is
    // mounted. The header count also stays correct when the section is closed.
    useEffect(() => {
        let cancelled = false;
        const refresh = async () => {
            try {
                const list = await listServeProcesses();
                if (!cancelled) setServeProcesses(list);
            } catch (err) {
                console.error("listServeProcesses failed", err);
            }
        };
        void refresh();
        const timer = window.setInterval(refresh, 2000);
        return () => {
            cancelled = true;
            window.clearInterval(timer);
        };
    }, []);

    async function toggleTool(toolId: string) {
        if (!config) return;
        // Self-evolution mode owns these two — they stay on.
        if (config.self_evolution_mode && SELF_EVOLUTION_LOCKED_TOOLS.includes(toolId)) return;
        const current = config.selected_tools ?? [];
        const updated = current.includes(toolId)
            ? current.filter(t => t !== toolId)
            : [...current, toolId];

        const newConfig = { ...config, selected_tools: updated };
        setConfig(newConfig);
        await saveConfig(newConfig);
        onToolsChange(updated);
    }

    async function updateKgEngine(kg_engine: string) {
        if (!config) return;
        const newConfig = { ...config, kg_engine };
        setConfig(newConfig);
        await saveConfig(newConfig);
    }

    async function stopServe(id: string) {
        setStoppingServe(id);
        try {
            await stopServeProcess(id);
            setServeProcesses(await listServeProcesses());
        } catch (err) {
            console.error("stopServeProcess failed", err);
        } finally {
            setStoppingServe(null);
        }
    }

    async function persistAutoAcceptKinds(kinds: AutoAcceptKind[]) {
        if (!config) return;
        const newConfig = { ...config, auto_accept_confirm_kinds: kinds };
        setConfig(newConfig);
        await saveConfig(newConfig);
    }

    async function toggleAutoAccept(kind: ConfirmKind) {
        if (!config) return;
        const current = sanitizeAutoAcceptKinds(config.auto_accept_confirm_kinds);
        // The wildcard means "every kind on": expand it to the explicit list
        // first, so unchecking a single kind keeps the remaining ones enabled.
        const base: AutoAcceptKind[] = current.includes(AUTO_ACCEPT_ALL)
            ? [...ALL_AUTO_ACCEPT_KINDS]
            : current;
        const updated = base.includes(kind)
            ? base.filter((value) => value !== kind)
            : [...base, kind];

        await persistAutoAcceptKinds(updated);
    }

    async function toggleAutoAcceptAll() {
        if (!config) return;
        const current = sanitizeAutoAcceptKinds(config.auto_accept_confirm_kinds);
        const allEnabled =
            current.includes(AUTO_ACCEPT_ALL) ||
            ALL_AUTO_ACCEPT_KINDS.every((kind) => current.includes(kind));

        await persistAutoAcceptKinds(allEnabled ? [] : [AUTO_ACCEPT_ALL]);
    }

    if (!config) {
        return <div className="tools-panel">{t("common.loading")}</div>;
    }

    const selectedTools = config.selected_tools ?? [];
    const selfEvolution = config.self_evolution_mode ?? false;
    const isLockedTool = (toolId: string) =>
        selfEvolution && SELF_EVOLUTION_LOCKED_TOOLS.includes(toolId);
    const kgEngine = config.kg_engine ?? "neo4j";
    const kgEnabled = selectedTools.includes("knowledge_graph");
    const kgEngineLabel = KG_ENGINES.find((engine) => engine.value === kgEngine)?.label ?? kgEngine;
    const runningServeCount = serveProcesses.filter((process) => process.state === "running").length;
    const autoAcceptKinds = sanitizeAutoAcceptKinds(config.auto_accept_confirm_kinds);
    const autoAcceptAll = autoAcceptKinds.includes(AUTO_ACCEPT_ALL);
    const allAutoAcceptEnabled =
        autoAcceptAll || ALL_AUTO_ACCEPT_KINDS.every((kind) => autoAcceptKinds.includes(kind));
    const isAutoAcceptEnabled = (kind: ConfirmKind) => autoAcceptAll || autoAcceptKinds.includes(kind);

    return (
        <div className="tools-panel">
            <div className="tools-header">
                <h2>{t("tools.title")}</h2>
                <button className="close-btn" onClick={onClose} aria-label={t("common.close")}>
                    <FontAwesomeIcon icon={faXmark} />
                </button>
            </div>
            <div className="tools-description">
                {t("tools.builtIn")}
            </div>
            <div className="tools-list">
                {AVAILABLE_TOOLS.map(tool => {
                    const locked = isLockedTool(tool.id);
                    const enabled = selectedTools.includes(tool.id);
                    return (
                        <div
                            key={tool.id}
                            className={`tool-item ${!enabled ? "disabled" : ""}`}
                        >
                            <div className="tool-row">
                                <label
                                    className="tool-toggle"
                                    title={
                                        locked
                                            ? t("tools.lockedBySelfEvolution")
                                            : enabled
                                                ? t("common.disable")
                                                : t("common.enable")
                                    }
                                >
                                    <input
                                        type="checkbox"
                                        checked={enabled}
                                        disabled={locked}
                                        onChange={() => toggleTool(tool.id)}
                                    />
                                    <span className="tool-toggle-slider" />
                                </label>
                                <div className="tool-info">
                                    <span
                                        className={`tool-name ${enabled ? "active" : ""}`}
                                        onClick={() => toggleTool(tool.id)}
                                    >
                                        {t(tool.nameKey)}
                                        {locked && (
                                            <span className="tool-locked">{t("tools.lockedBadge")}</span>
                                        )}
                                    </span>
                                    <span className="tool-transport">
                                        {t(tool.descriptionKey).split("\n")[0]}
                                    </span>
                                </div>
                                <div className="tool-actions">
                                    <button
                                        className="tool-action-btn"
                                        onClick={(e) => {
                                            e.stopPropagation();
                                            setExpandedTool(expandedTool === tool.id ? null : tool.id);
                                        }}
                                    >
                                        <FontAwesomeIcon
                                            icon={expandedTool === tool.id ? faChevronUp : faChevronDown}
                                        />
                                    </button>
                                </div>
                            </div>
                            {expandedTool === tool.id && (
                                <div className="tool-desc-expanded">
                                    {t(tool.descriptionKey)}
                                </div>
                            )}
                        </div>
                    );
                })}
            </div>

            <div className="tools-footer-section">
                <button
                    type="button"
                    className="tools-footer-toggle"
                    onClick={() => setKgExpanded((expanded) => !expanded)}
                    aria-expanded={kgExpanded}
                    title={kgEnabled ? t("tools.kgEngineTitle") : t("tools.kgEngineDisabled")}
                >
                    <span className="tools-footer-title">{t("tools.kgEngine")}</span>
                    <span className="tools-footer-aside">
                        <span className="tools-footer-value">{kgEngineLabel}</span>
                        <span className="tools-footer-chevron">
                            <FontAwesomeIcon icon={kgExpanded ? faChevronUp : faChevronDown} />
                        </span>
                    </span>
                </button>

                {kgExpanded && (
                    <select
                        id="kg-engine-select"
                        className="search-engine-select"
                        value={kgEngine}
                        onChange={(e) => void updateKgEngine(e.target.value)}
                        disabled={!kgEnabled}
                        aria-label={t("tools.kgEngine")}
                        title={kgEnabled ? t("tools.kgEngineTitle") : t("tools.kgEngineDisabled")}
                    >
                        {KG_ENGINES.map(engine => (
                            <option key={engine.value} value={engine.value}>
                                {engine.label}
                            </option>
                        ))}
                    </select>
                )}
            </div>

            <div className="tools-footer-section">
                <button
                    type="button"
                    className="tools-footer-toggle"
                    onClick={() => setServeExpanded((expanded) => !expanded)}
                    aria-expanded={serveExpanded}
                >
                    <span className="tools-footer-title">{t("tools.serveProcesses")}</span>
                    <span className="tools-footer-aside">
                        {runningServeCount > 0 && (
                            <span className="serve-count">
                                {t("tools.serveCount", { count: runningServeCount })}
                            </span>
                        )}
                        <span className="tools-footer-chevron">
                            <FontAwesomeIcon icon={serveExpanded ? faChevronUp : faChevronDown} />
                        </span>
                    </span>
                </button>

                {serveExpanded && (
                    <div className="serve-list">
                        {serveProcesses.length === 0 && (
                            <div className="serve-empty">{t("tools.serveEmpty")}</div>
                        )}
                        {serveProcesses.map((process) => {
                            const running = process.state === "running";
                            return (
                                <div
                                    key={process.id}
                                    className={`serve-item ${running ? "" : "inactive"}`}
                                >
                                    <div className="serve-row">
                                        <span
                                            className={`serve-dot ${process.state}`}
                                            aria-hidden="true"
                                        />
                                        <span className="serve-id" title={process.id}>
                                            {process.id}
                                        </span>
                                        <span className="serve-state">
                                            {t(SERVE_STATE_KEYS[process.state] ?? "tools.serveStateUnknown")}
                                        </span>
                                        <span className="serve-meta">
                                            {t("tools.serveMeta", {
                                                pid: process.pid,
                                                uptime: formatUptime(process.uptime_secs),
                                            })}
                                            {typeof process.exit_code === "number"
                                                ? ` · ${t("tools.serveExitCode", { code: process.exit_code })}`
                                                : ""}
                                        </span>
                                        <button
                                            type="button"
                                            className="tool-action-btn danger"
                                            onClick={() => void stopServe(process.id)}
                                            disabled={stoppingServe === process.id}
                                            title={running ? t("tools.serveStop") : t("tools.serveRemove")}
                                            aria-label={running ? t("tools.serveStop") : t("tools.serveRemove")}
                                        >
                                            <FontAwesomeIcon icon={faTrash} />
                                        </button>
                                    </div>
                                    <code className="serve-code" title={process.shell_type}>
                                        {process.shell_type} · {process.code}
                                    </code>
                                    <span className="serve-cwd" title={process.cwd}>
                                        {t("tools.serveCwd", { path: process.cwd })}
                                    </span>
                                    {process.recent_output.trim() !== "" && (
                                        <pre
                                            className="serve-output"
                                            aria-label={t("tools.serveOutput")}
                                            title={t("tools.serveOutput")}
                                        >
                                            {process.recent_output}
                                        </pre>
                                    )}
                                </div>
                            );
                        })}
                    </div>
                )}
            </div>

            <div className="tools-footer-section">
                <button
                    type="button"
                    className="tools-footer-toggle"
                    onClick={() => setAutoAcceptExpanded((expanded) => !expanded)}
                    aria-expanded={autoAcceptExpanded}
                >
                    <span className="tools-footer-title">{t("tools.autoAccept")}</span>
                    <span className="tools-footer-chevron">
                        <FontAwesomeIcon icon={autoAcceptExpanded ? faChevronUp : faChevronDown} />
                    </span>
                </button>

                {autoAcceptExpanded && (
                    <div className="auto-accept-list">
                        <label
                            className="auto-accept-option auto-accept-option-all"
                            title={t("tools.autoAcceptItems.all.description")}
                        >
                            <input
                                className="auto-accept-checkbox"
                                type="checkbox"
                                checked={allAutoAcceptEnabled}
                                onChange={() => void toggleAutoAcceptAll()}
                            />
                            <div className="auto-accept-option-body">
                                <span className="auto-accept-name">{t("tools.autoAcceptItems.all.label")}</span>
                            </div>
                        </label>
                        {AUTO_ACCEPT_OPTIONS.map((option) => {
                            const checked = isAutoAcceptEnabled(option.kind);
                            return (
                                <label
                                    key={option.kind}
                                    className="auto-accept-option"
                                    title={t(option.descriptionKey)}
                                >
                                    <input
                                        className="auto-accept-checkbox"
                                        type="checkbox"
                                        checked={checked}
                                        onChange={() => void toggleAutoAccept(option.kind)}
                                    />
                                    <div className="auto-accept-option-body">
                                        <span className="auto-accept-name">{t(option.labelKey)}</span>
                                    </div>
                                </label>
                            );
                        })}
                    </div>
                )}
            </div>
        </div>
    );
}
