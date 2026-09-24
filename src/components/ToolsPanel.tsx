import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig, AutoAcceptKind, ConfirmKind } from "../types";
import { useI18n, type MessageKey } from "../i18n";
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
        id: "knowledge_graph",
        nameKey: "tools.items.knowledgeGraph.name",
        descriptionKey: "tools.items.knowledgeGraph.description",
    },
    {
        id: "timer",
        nameKey: "tools.items.timer.name",
        descriptionKey: "tools.items.timer.description",
    }
];

const AVAILABLE_TOOL_IDS = new Set(AVAILABLE_TOOLS.map((t) => t.id));

const KG_ENGINES = [
    { value: "neo4j", label: "Neo4j" }
];

/**
 * Confirmation kinds that can actually reach the approval dialog. The `kind`
 * strings are matched against `auto_accept_confirm_kinds` by the Rust side,
 * which sends them from `RiskLevel::confirm_kind()` in `src-tauri/src/tools.rs`
 * (L0-L3 ask for approval, L4-L6 never prompt) plus the literal
 * `external_path` for out-of-workspace `file_actions` paths. Keep in sync.
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
        {
            kind: "external_path",
            labelKey: "tools.autoAcceptItems.externalPath.label",
            descriptionKey: "tools.autoAcceptItems.externalPath.description",
        },
    ];

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
 * preserving the `"*"` wildcard.
 */
function sanitizeAutoAcceptKinds(values: readonly string[] | undefined): AutoAcceptKind[] {
    return (values ?? []).filter(
        (value): value is AutoAcceptKind =>
            value === AUTO_ACCEPT_ALL || AUTO_ACCEPT_KIND_SET.has(value),
    );
}

export function ToolsPanel({ onClose, onToolsChange }: Props) {
    const { t } = useI18n();
    const [config, setConfig] = useState<AppConfig | null>(null);
    const [expandedTool, setExpandedTool] = useState<string | null>(null);
    const [autoAcceptExpanded, setAutoAcceptExpanded] = useState(true);

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

    async function toggleTool(toolId: string) {
        if (!config) return;
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
    const kgEngine = config.kg_engine ?? "neo4j";
    const kgEnabled = selectedTools.includes("knowledge_graph");
    const autoAcceptKinds = sanitizeAutoAcceptKinds(config.auto_accept_confirm_kinds);
    const autoAcceptAll = autoAcceptKinds.includes(AUTO_ACCEPT_ALL);
    const allAutoAcceptEnabled =
        autoAcceptAll || ALL_AUTO_ACCEPT_KINDS.every((kind) => autoAcceptKinds.includes(kind));
    const isAutoAcceptEnabled = (kind: ConfirmKind) => autoAcceptAll || autoAcceptKinds.includes(kind);

    return (
        <div className="tools-panel">
            <div className="tools-header">
                <h2>{t("tools.title")}</h2>
                <button className="close-btn" onClick={onClose}>✕</button>
            </div>
            <div className="tools-description">
                {t("tools.builtIn")}
            </div>
            <div className="tools-list">
                {AVAILABLE_TOOLS.map(tool => (
                    <div
                        key={tool.id}
                        className={`tool-item ${!selectedTools.includes(tool.id) ? "disabled" : ""}`}
                    >
                        <div className="tool-row">
                            <label className="tool-toggle" title={selectedTools.includes(tool.id) ? t("common.disable") : t("common.enable")}>
                                <input
                                    type="checkbox"
                                    checked={selectedTools.includes(tool.id)}
                                    onChange={() => toggleTool(tool.id)}
                                />
                                <span className="tool-toggle-slider" />
                            </label>
                            <div className="tool-info">
                                <span
                                    className={`tool-name ${selectedTools.includes(tool.id) ? "active" : ""}`}
                                    onClick={() => toggleTool(tool.id)}
                                >
                                    {t(tool.nameKey)}
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
                                    {expandedTool === tool.id ? "▲" : "▼"}
                                </button>
                            </div>
                        </div>
                        {expandedTool === tool.id && (
                            <div className="tool-desc-expanded">
                                {t(tool.descriptionKey)}
                            </div>
                        )}
                    </div>
                ))}
            </div>

            <div className="search-engine-section">
                <label className="search-engine-label" htmlFor="kg-engine-select">
                    {t("tools.kgEngine")}
                </label>
                <select
                    id="kg-engine-select"
                    className="search-engine-select"
                    value={kgEngine}
                    onChange={(e) => void updateKgEngine(e.target.value)}
                    disabled={!kgEnabled}
                    title={kgEnabled ? t("tools.kgEngineTitle") : t("tools.kgEngineDisabled")}
                >
                    {KG_ENGINES.map(engine => (
                        <option key={engine.value} value={engine.value}>
                            {engine.label}
                        </option>
                    ))}
                </select>
            </div>

            <div className="tools-footer-section">
                <button
                    type="button"
                    className="tools-footer-toggle"
                    onClick={() => setAutoAcceptExpanded((expanded) => !expanded)}
                    aria-expanded={autoAcceptExpanded}
                >
                    <span className="tools-footer-title">{t("tools.autoAccept")}</span>
                    <span className="auto-accept-chevron">{autoAcceptExpanded ? "▲" : "▼"}</span>
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
