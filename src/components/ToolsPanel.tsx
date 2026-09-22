import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig, ConfirmKind } from "../types";
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
    }
];

const AVAILABLE_TOOL_IDS = new Set(AVAILABLE_TOOLS.map((t) => t.id));

const KG_ENGINES = [
    { value: "neo4j", label: "Neo4j" }
];

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
            kind: "sudo",
            labelKey: "tools.autoAcceptItems.sudo.label",
            descriptionKey: "tools.autoAcceptItems.sudo.description",
        },
        {
            kind: "elevation",
            labelKey: "tools.autoAcceptItems.elevation.label",
            descriptionKey: "tools.autoAcceptItems.elevation.description",
        },
        {
            kind: "external_path",
            labelKey: "tools.autoAcceptItems.externalPath.label",
            descriptionKey: "tools.autoAcceptItems.externalPath.description",
        },
    ];

const AUTO_ACCEPT_KIND_SET = new Set<ConfirmKind>(AUTO_ACCEPT_OPTIONS.map((option) => option.kind));

export function ToolsPanel({ onClose, onToolsChange }: Props) {
    const { t } = useI18n();
    const [config, setConfig] = useState<AppConfig | null>(null);
    const [expandedTool, setExpandedTool] = useState<string | null>(null);
    const [autoAcceptExpanded, setAutoAcceptExpanded] = useState(true);

    useEffect(() => {
        getConfig()
            .then((cfg) => {
                const selected = (cfg.selected_tools ?? []).filter((id) => AVAILABLE_TOOL_IDS.has(id));
                const selectedAutoAcceptKinds = (cfg.auto_accept_confirm_kinds ?? []).filter((value): value is ConfirmKind =>
                    AUTO_ACCEPT_KIND_SET.has(value as ConfirmKind)
                );
                const sanitized = {
                    ...cfg,
                    selected_tools: selected,
                    auto_accept_confirm_kinds: selectedAutoAcceptKinds,
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

    async function toggleAutoAccept(kind: ConfirmKind) {
        if (!config) return;
        const current = (config.auto_accept_confirm_kinds ?? []).filter((value): value is ConfirmKind =>
            AUTO_ACCEPT_KIND_SET.has(value as ConfirmKind)
        );
        const updated = current.includes(kind)
            ? current.filter((value) => value !== kind)
            : [...current, kind];

        const newConfig = { ...config, auto_accept_confirm_kinds: updated };
        setConfig(newConfig);
        await saveConfig(newConfig);
    }

    if (!config) {
        return <div className="tools-panel">{t("common.loading")}</div>;
    }

    const selectedTools = config.selected_tools ?? [];
    const kgEngine = config.kg_engine ?? "neo4j";
    const kgEnabled = selectedTools.includes("knowledge_graph");
    const autoAcceptKinds = (config.auto_accept_confirm_kinds ?? []).filter((value): value is ConfirmKind =>
        AUTO_ACCEPT_KIND_SET.has(value as ConfirmKind)
    );

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
                        {AUTO_ACCEPT_OPTIONS.map((option) => {
                            const checked = autoAcceptKinds.includes(option.kind);
                            return (
                                <label key={option.kind} className="auto-accept-option">
                                    <input
                                        className="auto-accept-checkbox"
                                        type="checkbox"
                                        checked={checked}
                                        onChange={() => void toggleAutoAccept(option.kind)}
                                    />
                                    <div className="auto-accept-option-body">
                                        <span className="auto-accept-name">{t(option.labelKey)}</span>
                                        <span className="auto-accept-desc">{t(option.descriptionKey)}</span>
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
