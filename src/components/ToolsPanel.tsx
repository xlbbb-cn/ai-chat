import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig } from "../types";
import "./ToolsPanel.css";

interface Props {
    onClose: () => void;
    onToolsChange: (tools: string[]) => void;
}

const SEARCH_ENGINES = [
    { value: "duckduckgo", label: "DuckDuckGo" },
    { value: "google", label: "Google" },
    { value: "google_hk", label: "Google HK" },
    { value: "bing", label: "Bing INT" },
    { value: "bing_cn", label: "Bing CN" },
    { value: "baidu", label: "Baidu" },
    { value: "360", label: "360" },
    { value: "sogou", label: "Sogou" },
    { value: "wechat", label: "WeChat Search" },
    { value: "shenma", label: "Shenma" },
    { value: "yahoo", label: "Yahoo" },
    { value: "startpage", label: "Startpage" },
    { value: "brave", label: "Brave" },
    { value: "ecosia", label: "Ecosia" },
    { value: "qwant", label: "Qwant" },
    { value: "wolframalpha", label: "WolframAlpha" },
];

const AVAILABLE_TOOLS = [
    {
        id: "web_search",
        name: "Web Search",
        description: "Search the web for current information",
    },
    {
        id: "execute_command",
        name: "Command Execution",
        description: "Execute generic Bash/Python/Powershell scripts globally",
    },
    {
        id: "file_actions",
        name: "File Actions",
        description: "Read, write, and list items in the workspace",
    },
    {
        id: "fetch_web",
        name: "Fetch Web",
        description: "Fetch true webpage content (bypassing anti-bot & JS rendering)",
    },
    {
        id: "knowledge_graph",
        name: "Knowledge Graph",
        description: "Connect to a knowledge graph and perform queries",
    }
];

const KG_ENGINES = [
    { value: "neo4j", label: "Neo4j" }
];

export function ToolsPanel({ onClose, onToolsChange }: Props) {
    const [config, setConfig] = useState<AppConfig | null>(null);
    const [expandedTool, setExpandedTool] = useState<string | null>(null);

    useEffect(() => {
        getConfig().then(setConfig).catch(console.error);
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

    async function updateSearchEngine(search_engine: string) {
        if (!config) return;
        const newConfig = { ...config, search_engine };
        setConfig(newConfig);
        await saveConfig(newConfig);
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
    const searchEngine = config.search_engine ?? "duckduckgo";
    const webSearchEnabled = selectedTools.includes("web_search");
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
                            <input
                                type="checkbox"
                                checked={selectedTools.includes(tool.id)}
                                onChange={() => { }}
                            />
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
                <label className="search-engine-label" htmlFor="search-engine-select">
                    Web Search Engine
                </label>
                <select
                    id="search-engine-select"
                    className="search-engine-select"
                    value={searchEngine}
                    onChange={(e) => void updateSearchEngine(e.target.value)}
                    disabled={!webSearchEnabled}
                    title={webSearchEnabled ? "Default engine for web_search" : "Enable Web Search tool first"}
                >
                    {SEARCH_ENGINES.map(engine => (
                        <option key={engine.value} value={engine.value}>
                            {engine.label}
                        </option>
                    ))}
                </select>
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
