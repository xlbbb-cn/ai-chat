import { useState, useEffect } from "react";
import {
  listSubAgents, saveSubAgent, deleteSubAgent,
  getAgentOrchestration, saveAgentOrchestration,
} from "../api";
import type { SubAgent, AgentOrchestration } from "../types";
import { useI18n, type MessageKey } from "../i18n";
import { MarkdownPreview } from "./MarkdownPreview";
import { Portal } from "./Portal";
import "./AgentsPanel.css";

interface ToolOption {
  id: string;
  /** Tool id as sent to the model — technical, never translated. */
  label: string;
  hintKey: MessageKey;
}

const KNOWN_TOOLS: ToolOption[] = [
  {
    id: "file_actions",
    label: "file_actions",
    hintKey: "agents.toolHints.fileActions",
  },
  {
    id: "run_cmd",
    label: "run_cmd",
    hintKey: "agents.toolHints.runCmd",
  },
  {
    id: "run_shell",
    label: "run_shell",
    hintKey: "agents.toolHints.runShell",
  },
  {
    id: "knowledge_graph",
    label: "knowledge_graph",
    hintKey: "agents.toolHints.knowledgeGraph",
  },
  {
    id: "todo_list",
    label: "todo_*",
    hintKey: "agents.toolHints.todos",
  },
  {
    id: "memory",
    label: "memory",
    hintKey: "agents.toolHints.memory",
  },
];

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
  const { t } = useI18n();
  const [agents, setAgents] = useState<SubAgent[]>([]);
  const [orchestration, setOrchestration] = useState<AgentOrchestration>({
    use_agents: false,
    auto_configure: false,
    max_concurrent: 3,
    mode: "parallel",
  });
  const [editing, setEditing] = useState<SubAgent | null>(null);
  const [saving, setSaving] = useState(false);
  const [isPromptEditorOpen, setIsPromptEditorOpen] = useState(false);
  const [promptDraft, setPromptDraft] = useState("");

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

  function openPromptEditor() {
    if (!editing) return;
    setPromptDraft(editing.system_prompt ?? "");
    setIsPromptEditorOpen(true);
  }

  function closePromptEditor() {
    setIsPromptEditorOpen(false);
  }

  function applyPromptEditor() {
    if (!editing) return;
    setEditing({ ...editing, system_prompt: promptDraft });
    setIsPromptEditorOpen(false);
  }

  const statusIcon = (agentId: string) => {
    const s = agentStatuses[agentId];
    if (!s || s.status === "idle") return null;
    if (s.status === "running") return <span className="agent-status-badge running">{t("agents.statusRunning")}</span>;
    if (s.status === "done") return <span className="agent-status-badge done">{t("agents.statusDone")}</span>;
    if (s.status === "error") return <span className="agent-status-badge error">{t("agents.statusError")}</span>;
    return null;
  };

  return (
    <div className="agents-panel">
      <div className="agents-header">
        <h2>{t("agents.title")}</h2>
        <div className="agents-header-actions">
          <button className="close-btn" onClick={onClose}>✕</button>
        </div>
      </div>

      {editing ? (
        /* ── Agent Editor ── */
        <div className="agent-editor">
          <label>
            {t("agents.name")}
            <input
              value={editing.name}
              onChange={(e) => setEditing({ ...editing, name: e.target.value })}
              placeholder={t("agents.namePlaceholder")}
            />
          </label>
          <label>
            {t("agents.description")}
            <input
              value={editing.description}
              onChange={(e) => setEditing({ ...editing, description: e.target.value })}
              placeholder={t("agents.descriptionPlaceholder")}
            />
          </label>
          <label>
            <div className="field-title-row">
              <span>{t("agents.systemPrompt")}</span>
              <button type="button" className="inline-edit-btn" onClick={openPromptEditor}>
                {t("common.edit")}
              </button>
            </div>
            <textarea
              rows={6}
              value={editing.system_prompt}
              onChange={(e) => setEditing({ ...editing, system_prompt: e.target.value })}
              placeholder={t("agents.systemPromptPlaceholder")}
            />
          </label>
          <div className="agent-editor-section-title">{t("agents.modelLimits")}</div>
          <label>
            {t("agents.model")}
            <input
              value={editing.model ?? ""}
              onChange={(e) => setEditing({ ...editing, model: e.target.value || undefined })}
              placeholder={t("agents.modelPlaceholder")}
            />
          </label>
          <div className="agent-editor-row">
            <label style={{ flex: 1 }}>
              {t("agents.maxTokens")}
              <input
                type="number"
                min={512}
                max={128000}
                value={editing.max_tokens ?? ""}
                onChange={(e) =>
                  setEditing({ ...editing, max_tokens: e.target.value ? Number(e.target.value) : undefined })
                }
                placeholder={t("agents.maxTokensPlaceholder")}
              />
            </label>
            <label style={{ flex: 1 }}>
              {t("agents.temperature")}
              <input
                type="number"
                min={0}
                max={2}
                step={0.1}
                value={editing.temperature ?? ""}
                onChange={(e) =>
                  setEditing({ ...editing, temperature: e.target.value ? Number(e.target.value) : undefined })
                }
                placeholder={t("agents.temperaturePlaceholder")}
              />
            </label>
            <label style={{ flex: 1 }}>
              {t("agents.maxIterations")}
              <input
                type="number"
                min={0}
                max={500}
                value={editing.max_iterations}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    max_iterations: e.target.value === "" ? 10 : Math.max(0, Number(e.target.value) || 0),
                  })
                }
              />
              <small>{t("agents.maxIterationsHint")}</small>
            </label>
          </div>
          <div className="agent-editor-section-title">{t("agents.capabilities")}</div>
          <label>
            <div className="field-title-row">
              <span>{t("agents.allowedTools")}</span>
              <span className="agent-tool-quick">
                <button
                  type="button"
                  className="inline-edit-btn"
                  onClick={() =>
                    setEditing((prev) =>
                      prev
                        ? {
                          ...prev,
                          allowed_tools: Array.from(
                            new Set([...prev.allowed_tools, ...KNOWN_TOOLS.map((t) => t.id)])
                          ),
                        }
                        : prev
                    )
                  }
                >
                  {t("common.all")}
                </button>
                <button
                  type="button"
                  className="inline-edit-btn"
                  onClick={() =>
                    setEditing((prev) => (prev ? { ...prev, allowed_tools: [] } : prev))
                  }
                >
                  {t("common.none")}
                </button>
              </span>
            </div>
            <div className="agent-tool-checkboxes">
              {KNOWN_TOOLS.map((tool) => (
                <label key={tool.id} className="agent-tool-checkbox">
                  <input
                    type="checkbox"
                    checked={editing.allowed_tools.includes(tool.id)}
                    onChange={() => toggleAllowedTool(tool.id)}
                  />
                  <span className="agent-tool-text">
                    <span className="agent-tool-name">{tool.label}</span>
                    <span className="agent-tool-hint">{t(tool.hintKey)}</span>
                  </span>
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
              {saving ? t("common.saving") : t("common.save")}
            </button>
            <button className="btn-secondary" onClick={() => setEditing(null)}>
              {t("common.cancel")}
            </button>
          </div>

          {isPromptEditorOpen && (
            <Portal>
              <div className="prompt-editor-overlay" role="dialog" aria-modal="true" aria-label={t("agents.promptEditor.aria")}>
                <div className="prompt-editor-shell">
                  <div className="prompt-editor-header">
                    <h3>{t("agents.promptEditor.title")}</h3>
                    <div className="prompt-editor-actions">
                      <button type="button" className="btn-secondary" onClick={closePromptEditor}>
                        {t("common.cancel")}
                      </button>
                      <button type="button" className="btn-primary" onClick={applyPromptEditor}>
                        {t("common.done")}
                      </button>
                    </div>
                  </div>

                  <div className="prompt-editor-body">
                    <div className="prompt-column">
                      <span>{t("app.markdownColumn")}</span>
                      <textarea
                        className="prompt-editor-textarea"
                        value={promptDraft}
                        onChange={(e) => setPromptDraft(e.target.value)}
                        placeholder={t("agents.promptEditor.placeholder")}
                      />
                    </div>

                    <div className="prompt-column">
                      <span>{t("app.previewColumn")}</span>
                      <div className="prompt-preview">
                        {promptDraft.trim() ? (
                          <MarkdownPreview content={promptDraft} />
                        ) : (
                          <p className="prompt-preview-empty">{t("app.markdownPreviewEmpty")}</p>
                        )}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </Portal>
          )}
        </div>
      ) : (
        <>
          {/* ── Orchestration Settings ── */}
          <div className="orchestration-settings">
            <div className="orch-title">{t("agents.orchestration")}</div>

            <div className="orch-row">
              <span>{t("agents.enableMode")}</span>
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
              <span>{t("agents.autoConfigure")}</span>
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
              <span>{t("agents.executionMode")}</span>
              <select
                className="orch-select"
                value={orchestration.mode}
                onChange={(e) =>
                  handleSaveOrchestration({ ...orchestration, mode: e.target.value as "parallel" | "sequential" })
                }
              >
                <option value="parallel">{t("agents.modeParallel")}</option>
                <option value="sequential">{t("agents.modeSequential")}</option>
              </select>
            </div>

            <div className="orch-row">
              <span>{t("agents.maxConcurrency")}</span>
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
              <div className="agents-empty">{t("agents.empty")}</div>
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
                          title={t("common.edit")}
                          onClick={() => setEditing({ ...agent })}
                        >
                          ✎
                        </button>
                        <button
                          className="mcp-action-btn danger"
                          title={t("common.delete")}
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
              {t("agents.newAgent")}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
