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
        id: "read_file",
        name: "Read File",
        description: "Read files from the workspace",
    },
    {
        id: "write_file",
        name: "Write File",
        description: "Write files to the workspace",
    },
    {
        id: "list_dir",
        name: "List Directory",
        description: "List directory contents in the workspace",
    }
];

export function ToolsPanel({ onClose, onToolsChange }: Props) {
    const [config, setConfig] = useState<AppConfig | null>(null);

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

    if (!config) {
        return <div className="tools-panel">Loading...</div>;
    }

    const selectedTools = config.selected_tools ?? [];

    return (
        <div className="tools-panel">
            <div className="tools-header">
                <h2>Tools</h2>
                <button className="close-btn" onClick={onClose}>✕</button>
            </div>
            <div className="tools-list">
                {AVAILABLE_TOOLS.map(tool => (
                    <div
                        key={tool.id}
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
                            <span className="tool-desc">{tool.description}</span>
                        </div>
                    </div>
                ))}
            </div>
        </div>
    );
}
