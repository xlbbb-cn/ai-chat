import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig, ConfirmKind } from "../types";
import "./ToolsPanel.css";

interface Props {
    onClose: () => void;
    onToolsChange: (tools: string[]) => void;
}

const AVAILABLE_TOOLS = [
    {
        id: "file_actions",
        name: "File Actions",
        description: [
            "Read, write, list, search, patch, rename, move, create, and delete files only inside the current workspace root.",
            "Use `./...` paths such as `./src/App.tsx`, `./skills/demo/skill.md`, or `.` for the workspace root.",
            "Paths such as `workspace/...` and `../...` are rejected. Absolute paths outside the workspace require an explicit approval dialog before access is allowed.",
        ].join("\n"),
    },
    {
        id: "run_cmd",
        name: "Run Command",
        description: "Run an executable program directly (without a shell). The process starts in the workspace root. Preferred for simple commands like curl, git, or wget. Privileged operations and dangerous commands require explicit confirmation.",
    },
    {
        id: "run_shell",
        name: "Run Shell",
        description: "Execute a script in a shell (PowerShell or Bash). The shell starts in the workspace root, and directory changes must stay inside that workspace. Supports pipes, loops, variables, and other shell features. Privileged operations and dangerous commands require explicit confirmation.",
    },
    {
        id: "knowledge_graph",
        name: "Knowledge Graph",
        description: "Connect to a knowledge graph and perform queries",
    }
];

const AVAILABLE_TOOL_IDS = new Set(AVAILABLE_TOOLS.map((t) => t.id));

const KG_ENGINES = [
    { value: "neo4j", label: "Neo4j" }
];

const AUTO_ACCEPT_OPTIONS: Array<{
    kind: ConfirmKind;
    label: string;
    description: string;
}> = [
        {
            kind: "dangerous",
            label: "Dangerous commands",
            description: "Skip the approval prompt for dangerous run_cmd or run_shell requests.",
        },
        {
            kind: "sudo",
            label: "sudo requests",
            description: "Auto-confirms the approval step for sudo. It does not provide a password automatically.",
        },
        {
            kind: "elevation",
            label: "Administrator elevation",
            description: "Skip the approval prompt before PowerShell elevation (UAC) requests.",
        },
        {
            kind: "external_path",
            label: "External absolute paths",
            description: "Allow file_actions to access absolute paths outside the workspace without prompting.",
        },
    ];

const AUTO_ACCEPT_KIND_SET = new Set<ConfirmKind>(AUTO_ACCEPT_OPTIONS.map((option) => option.kind));

export function ToolsPanel({ onClose, onToolsChange }: Props) {
    const [config, setConfig] = useState<AppConfig | null>(null);
    const [expandedTool, setExpandedTool] = useState<string | null>(null);

    useEffect(() => {
        getConfig()
            .then((cfg) => {
                const selected = (cfg.selected_tools ?? []).filter((id) => AVAILABLE_TOOL_IDS.has(id));
                const sanitized = { ...cfg, selected_tools: selected };
                setConfig(sanitized);
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
        return <div className="tools-panel">Loading...</div>;
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
                <h2>Tools</h2>
                <button className="close-btn" onClick={onClose}>✕</button>
            </div>
            <div className="tools-list">
                {AVAILABLE_TOOLS.map(tool => (
                    <div key={tool.id} className="tool-item-container">
                        <div
                            className={`tool-item ${selectedTools.includes(tool.id) ? "active" : ""}`}
                            onClick={() => toggleTool(tool.id)}
                        >
                            <label className="toggle-switch">
                                <input
                                    type="checkbox"
                                    checked={selectedTools.includes(tool.id)}
                                    onChange={() => { }}
                                />
                                <span className="toggle-switch-slider" />
                            </label>
                            <div className="tool-info">
                                <span className="tool-name">{tool.name}</span>
                            </div>
                            <button
                                className="toggle-btn"
                                onClick={(e) => {
                                    e.stopPropagation();
                                    setExpandedTool(expandedTool === tool.id ? null : tool.id);
                                }}
                            >
                                {expandedTool === tool.id ? "▲" : "▼"}
                            </button>
                        </div>
                        {expandedTool === tool.id && (
                            <div className="tool-desc-expanded">
                                {tool.description}
                            </div>
                        )}
                    </div>
                ))}
            </div>

            <div className="search-engine-section">
                <label className="search-engine-label" htmlFor="kg-engine-select">
                    Knowledge Graph Engine
                </label>
                <select
                    id="kg-engine-select"
                    className="search-engine-select"
                    value={kgEngine}
                    onChange={(e) => void updateKgEngine(e.target.value)}
                    disabled={!kgEnabled}
                    title={kgEnabled ? "Default engine for knowledge graph" : "Enable Knowledge Graph tool first"}
                >
                    {KG_ENGINES.map(engine => (
                        <option key={engine.value} value={engine.value}>
                            {engine.label}
                        </option>
                    ))}
                </select>
            </div>

            <div className="tools-footer-section">
                <div className="tools-footer-title">AutoAccept Mode</div>
                <div className="tools-footer-hint">
                    Auto-allow selected confirmation types. Use this only when you trust the active tools and prompts.
                </div>
                <div className="auto-accept-list">
                    {AUTO_ACCEPT_OPTIONS.map((option) => {
                        const checked = autoAcceptKinds.includes(option.kind);
                        return (
                            <label key={option.kind} className="auto-accept-item">
                                <div className="auto-accept-copy">
                                    <span className="auto-accept-name">{option.label}</span>
                                    <span className="auto-accept-desc">{option.description}</span>
                                </div>
                                <div className="auto-accept-control">
                                    <span className="auto-accept-kind">{option.kind}</span>
                                    <span className="toggle-switch" onClick={(event) => event.stopPropagation()}>
                                        <input
                                            type="checkbox"
                                            checked={checked}
                                            onChange={() => void toggleAutoAccept(option.kind)}
                                        />
                                        <span className="toggle-switch-slider" />
                                    </span>
                                </div>
                            </label>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}
