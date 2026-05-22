import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig } from "../types";
import "./ToolsPanel.css";

interface Props {
    onClose: () => void;
    onToolsChange: (tools: string[]) => void;
}

const AVAILABLE_TOOLS = [


    {
        id: "file_actions",
        name: "File Actions",
        description: "he file action to perform: read file content, write/overwrite a file, list directory entries, edit by replacing a string, or apply a unified diff patch.",
    },
    {
        id: "run_cmd",
        name: "Run Command",
        description: "Run an executable program directly (without a shell). Preferred for simple commands like curl, git, wget, etc. Privileged operations (sudo) and dangerous commands require explicit confirmation.",
    },
    {
        id: "run_shell",
        name: "Run Shell",
        description: "Execute a script in a shell (PowerShell or Bash). Supports pipes, loops, variables, and other shell features. Privileged operations (sudo / admin elevation) and dangerous commands require explicit confirmation.",
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

    if (!config) {
        return <div className="tools-panel">Loading...</div>;
    }

    const selectedTools = config.selected_tools ?? [];
    const kgEngine = config.kg_engine ?? "neo4j";
    const kgEnabled = selectedTools.includes("knowledge_graph");

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
        </div>
    );
}
