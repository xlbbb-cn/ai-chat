import { useState, useEffect } from "react";
import {
  listSubAgents, saveSubAgent, deleteSubAgent,
  getAgentOrchestration, saveAgentOrchestration,
} from "../api";
import type { SubAgent, AgentOrchestration } from "../types";
import "./AgentsPanel.css";

const KNOWN_TOOLS = ["file_actions", "run_cmd", "run_shell", "knowledge_graph"];

interface AgentStatus {
  status: "idle" | "running" | "done" | "error";
  description?: string;
  summary?: string;
  error?: string;
  tokens?: number;
}

interface Props {
  onClose: () => void;
  onAgentsChange: (enabledCount: number) => void;
  useAgentsEnabled: boolean;
  onToggleUseAgents: (enabled: boolean) => void;
  agentStatuses: Record<string, AgentStatus>;
}

const emptyAgent = (): SubAgent => ({
  id: "",
  name: "",
  description: "",
  system_prompt: "",
  model: undefined,
  max_tokens: undefined,
  temperature: undefined,
  allowed_tools: [],
  allowed_skills: [],
  max_iterations: 10,
  enabled: true,
});

export function AgentsPanel({ onClose, onAgentsChange, useAgentsEnabled, onToggleUseAgents, agentStatuses }: Props) {
  const [agents, setAgents] = useState<SubAgent[]>([]);
  const [orchestration, setOrchestration] = useState<AgentOrchestration>({
    use_agents: false,
    auto_configure: false,
    max_concurrent: 3,
    mode: "parallel",
  });
  const [editing, setEditing] = useState<SubAgent | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    listSubAgents().then(setAgents).catch(console.error);
    getAgentOrchestration().then(setOrchestration).catch(console.error);
  }, []);

  async function handleToggleAgent(agent: SubAgent) {
    const updated = { ...agent, enabled: !agent.enabled };
    await saveSubAgent(updated).catch(console.error);
    const newList = agents.map((a) => (a.id === agent.id ? updated : a));
    setAgents(newList);
    onAgentsChange(newList.filter((a) => a.enabled).length);
  }

  async function handleSaveAgent() {
    if (!editing || !editing.name.trim()) return;
    setSaving(true);
    try {
      await saveSubAgent(editing);
      const updated = await listSubAgents();
      setAgents(updated);
      onAgentsChange(updated.filter((a) => a.enabled).length);
      setEditing(null);
    } catch (e) {
      console.error(e);
    } finally {
      setSaving(false);
    }
  }

  async function handleDeleteAgent(id: string) {
    await deleteSubAgent(id).catch(console.error);
    const updated = agents.filter((a) => a.id !== id);
    setAgents(updated);
    onAgentsChange(updated.filter((a) => a.enabled).length);
  }

  async function handleSaveOrchestration(updated: AgentOrchestration) {
    setOrchestration(updated);
    await saveAgentOrchestration(updated).catch(console.error);
  }

  function toggleAllowedTool(tool: string) {
    if (!editing) return;
    const has = editing.allowed_tools.includes(tool);
    setEditing({
      ...editing,
      allowed_tools: has
        ? editing.allowed_tools.filter((t) => t !== tool)
        : [...editing.allowed_tools, tool],
    });
  }

  const statusIcon = (agentId: string) => {
    const s = agentStatuses[agentId];
    if (!s || s.status === "idle") return null;
    if (s.status === "running") return <span className="agent-status-badge running">⚙ Running</span>;
    if (s.status === "done") return <span className="agent-status-badge done">✓ Done</span>;
    if (s.status === "error") return <span className="agent-status-badge error">✕ Error</span>;
    return null;
  };

  return (
    <div className="agents-panel">
      <div className="agents-header">
        <h2>Sub Agents</h2>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      {editing ? (
        /* ── Agent Editor ── */
        <div className="agent-editor">
          <label>
            Name
            <input
              value={editing.name}
              onChange={(e) => setEditing({ ...editing, name: e.target.value })}
              placeholder="e.g. code-analyzer"
            />
          </label>
          <label>
            Description
            <input
              value={editing.description}
              onChange={(e) => setEditing({ ...editing, description: e.target.value })}
              placeholder="Briefly describe this agent's role"
            />
          </label>
          <label>
            System prompt
            <textarea
              rows={6}
              value={editing.system_prompt}
              onChange={(e) => setEditing({ ...editing, system_prompt: e.target.value })}
              placeholder="You are an expert in... focused on..."
            />
          </label>
          <label>
            Model (optional, overrides main config)
            <input
              value={editing.model ?? ""}
              onChange={(e) => setEditing({ ...editing, model: e.target.value || undefined })}
              placeholder="Leave empty to inherit the main model"
            />
          </label>
          <div className="agent-editor-row">
            <label style={{ flex: 1 }}>
              Max tokens
              <input
                type="number"
                min={512}
                max={128000}
                value={editing.max_tokens ?? ""}
                onChange={(e) =>
                  setEditing({ ...editing, max_tokens: e.target.value ? Number(e.target.value) : undefined })
                }
                placeholder="Default 8192"
              />
            </label>
            <label style={{ flex: 1 }}>
              Temperature
              <input
                type="number"
                min={0}
                max={2}
                step={0.1}
                value={editing.temperature ?? ""}
                onChange={(e) =>
                  setEditing({ ...editing, temperature: e.target.value ? Number(e.target.value) : undefined })
                }
                placeholder="Inherit default"
              />
            </label>
            <label style={{ flex: 1 }}>
              Max iter
              <input
                type="number"
                min={1}
                max={50}
                value={editing.max_iterations}
                onChange={(e) =>
                  setEditing({ ...editing, max_iterations: Number(e.target.value) || 10 })
                }
              />
            </label>
          </div>
          <label>
            Allowed tools
            <div className="agent-tool-checkboxes">
              {KNOWN_TOOLS.map((tool) => (
                <label key={tool} className="agent-tool-checkbox">
                  <input
                    type="checkbox"
                    checked={editing.allowed_tools.includes(tool)}
                    onChange={() => toggleAllowedTool(tool)}
                  />
                  {tool}
                </label>
              ))}
            </div>
          </label>
          <div className="editor-actions">
            <button
              className="btn-primary"
              onClick={handleSaveAgent}
              disabled={!editing.name.trim() || saving}
            >
              {saving ? "Saving..." : "Save"}
            </button>
            <button className="btn-secondary" onClick={() => setEditing(null)}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <>
          {/* ── Orchestration Settings ── */}
          <div className="orchestration-settings">
            <div className="orch-title">Orchestration settings</div>

            <div className="orch-row">
              <span>Enable sub-agent mode</span>
              <label className="toggle-switch">
                <input
                  type="checkbox"
                  checked={useAgentsEnabled}
                  onChange={(e) => {
                    const checked = e.target.checked;
                    onToggleUseAgents(checked);
                    handleSaveOrchestration({ ...orchestration, use_agents: checked });
                  }}
                />
                <span className="toggle-switch-slider" />
              </label>
            </div>

            <div className="orch-row">
              <span>Auto-configure mode</span>
              <label className="toggle-switch">
                <input
                  type="checkbox"
                  checked={orchestration.auto_configure}
                  onChange={(e) =>
                    handleSaveOrchestration({ ...orchestration, auto_configure: e.target.checked })
                  }
                />
                <span className="toggle-switch-slider" />
              </label>
            </div>

            <div className="orch-row">
              <span>Execution mode</span>
              <select
                className="orch-select"
                value={orchestration.mode}
                onChange={(e) =>
                  handleSaveOrchestration({ ...orchestration, mode: e.target.value as "parallel" | "sequential" })
                }
              >
                <option value="parallel">Parallel</option>
                <option value="sequential">Sequential</option>
              </select>
            </div>

            <div className="orch-row">
              <span>Max concurrency</span>
              <input
                className="orch-number"
                type="number"
                min={1}
                max={10}
                value={orchestration.max_concurrent}
                onChange={(e) =>
                  handleSaveOrchestration({ ...orchestration, max_concurrent: Number(e.target.value) || 3 })
                }
              />
            </div>
          </div>

          {/* ── Agent List ── */}
          <div className="agents-list">
            {agents.length === 0 && (
              <div className="agents-empty">No sub-agents yet. Click the button below to create one.</div>
            )}
            {agents.map((agent) => {
              const st = agentStatuses[agent.id];
              return (
                <div
                  key={agent.id}
                  className={`agent-item ${agent.enabled ? "active" : ""}`}
                  onClick={() => handleToggleAgent(agent)}
                >
                  <div className="agent-content">
                    <div className="agent-title-row">
                      <label className="toggle-switch" onClick={(e) => e.stopPropagation()}>
                        <input
                          type="checkbox"
                          checked={agent.enabled}
                          onChange={() => handleToggleAgent(agent)}
                        />
                        <span className="toggle-switch-slider" />
                      </label>
                      <span className="agent-name">{agent.name}</span>
                      {statusIcon(agent.id)}
                      <div className="skill-actions" onClick={(e) => e.stopPropagation()}>
                        <button
                          className="mcp-action-btn"
                          title="Edit"
                          onClick={() => setEditing({ ...agent })}
                        >
                          ✎
                        </button>
                        <button
                          className="mcp-action-btn danger"
                          title="Delete"
                          onClick={() => handleDeleteAgent(agent.id)}
                        >
                          ✕
                        </button>
                      </div>
                    </div>
                    <span className="agent-desc">{agent.description}</span>
                    {st && st.status === "done" && st.summary && (
                      <span className="agent-summary">✓ {st.summary}</span>
                    )}
                    {st && st.status === "error" && st.error && (
                      <span className="agent-summary error">✕ {st.error}</span>
                    )}
                    {st && st.tokens && (
                      <span className="agent-tokens">{st.tokens} tokens</span>
                    )}
                    <div className="agent-tools-row">
                      {agent.allowed_tools.map((t) => (
                        <span key={t} className="agent-tool-tag">{t}</span>
                      ))}
                      {agent.model && (
                        <span className="agent-tool-tag model">{agent.model}</span>
                      )}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>

          <div className="agents-footer">
            <button
              className="btn-primary"
              onClick={() => setEditing(emptyAgent())}
            >
              + New Agent
            </button>
          </div>
        </>
      )}
    </div>
  );
}
