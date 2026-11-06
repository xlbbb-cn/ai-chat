import { useState, useEffect } from "react";
import {
  listSubAgents, saveSubAgent, deleteSubAgent,
  getAgentOrchestration, saveAgentOrchestration,
  probeAgentCli, probeAgentCloud, openAgentsDir, refreshSubAgentIndex,
  getConfig, setModelContextLength,
} from "../api";
import type {
  AgentType,
  CliAgentSettings,
  CloudAgentSettings,
  KeyValue,
  SubAgent,
  AgentOrchestration,
} from "../types";
import { useI18n, type MessageKey } from "../i18n";
import {
  FontAwesomeIcon,
  faCheck,
  faFolderOpen,
  faGear,
  faPen,
  faRotateRight,
  faTriangleExclamation,
  faXmark,
} from "../icons";
import { MarkdownPreview } from "./MarkdownPreview";
import { Portal } from "./Portal";
import "./AgentsPanel.css";

interface ToolOption {
  id: string;
  /** Tool id as sent to the model — technical, never translated. */
  label: string;
  hintKey: MessageKey;
}

/**
 * Tools an `llm` sub-agent may be granted. Sub-agents are execution workers,
 * so the selection is limited to the local execution tools; memory, todos and
 * the knowledge graph stay with the main agent (the backend still understands
 * those ids for backwards compatibility, but they are not offered here).
 */
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
];

const AGENT_TYPES: AgentType[] = ["llm", "cli", "cloud"];

const TYPE_LABEL_KEYS: Record<AgentType, MessageKey> = {
  llm: "agents.typeLlm",
  cli: "agents.typeCli",
  cloud: "agents.typeCloud",
};

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

/** Default timeouts kept in sync with the Rust `serde` defaults. */
const DEFAULT_CLI_TIMEOUT_SECONDS = 600;
const DEFAULT_CLOUD_TIMEOUT_SECONDS = 1800;

function emptyCliSettings(): CliAgentSettings {
  return {
    command: "",
    args: [],
    prompt_via: "arg",
    model_args: [],
    env: [],
    default_timeout_seconds: DEFAULT_CLI_TIMEOUT_SECONDS,
  };
}

function emptyCloudSettings(): CloudAgentSettings {
  return {
    endpoint: "",
    token: "",
    headers: [],
    body_template: '{"messages":[{"role":"user","content":"{prompt}"}]}',
    result_path: "choices.0.message.content",
    timeout_seconds: DEFAULT_CLOUD_TIMEOUT_SECONDS,
    poll_url: "",
    poll_id_path: "id",
    poll_status_path: "status",
    poll_done_values: ["completed", "succeeded", "success", "done", "finished"],
    poll_failed_values: ["failed", "error", "cancelled", "canceled"],
    poll_interval_seconds: 5,
  };
}

/** Tools a newly created `llm` agent starts with (matches discovered agents). */
const DEFAULT_LLM_TOOLS = ["file_actions"];

const emptyAgent = (agentType: AgentType = "llm"): SubAgent => ({
  id: "",
  name: "",
  description: "",
  agent_type: agentType,
  system_prompt: "",
  emoji: undefined,
  color: undefined,
  file: undefined,
  model: undefined,
  max_complete_tokens: undefined,
  temperature: undefined,
  allowed_tools: agentType === "llm" ? [...DEFAULT_LLM_TOOLS] : [],
  allowed_skills: [],
  max_iterations: 0,
  enable_mcp: false,
  cli: emptyCliSettings(),
  cloud: emptyCloudSettings(),
  require_approval: true,
  enabled: true,
});

/** Draft wrapper: array fields are edited as one entry per line. */
interface AgentDraft {
  agent: SubAgent;
  /** Type switching is fixed for an existing file-backed `llm` agent. */
  typeLocked: boolean;
  argsText: string;
  modelArgsText: string;
  envText: string;
  headersText: string;
  doneValuesText: string;
  failedValuesText: string;
}

const valuesToLines = (values: string[]): string => values.join("\n");

const linesToValues = (text: string): string[] =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);

const pairsToText = (pairs: KeyValue[]): string =>
  pairs.map((pair) => `${pair.key}=${pair.value}`).join("\n");

/**
 * Timeout input: `0` means no timeout; positive values are capped at 7200s.
 * An empty (or not-yet-valid) field falls back to `fallback` instead of `0` —
 * clearing the box must not silently turn the run into "no timeout".
 */
function parseTimeoutSeconds(raw: string, fallback: number): number {
  const trimmed = raw.trim();
  if (trimmed === "") return fallback;
  const value = Number(trimmed);
  if (!Number.isFinite(value)) return fallback;
  return Math.max(0, Math.min(7200, Math.trunc(value)));
}

function textToPairs(text: string): KeyValue[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => {
      const index = line.indexOf("=");
      if (index === -1) return { key: line, value: "" };
      return { key: line.slice(0, index).trim(), value: line.slice(index + 1) };
    });
}

function toDraft(agent: SubAgent): AgentDraft {
  return {
    agent,
    typeLocked: agent.agent_type === "llm" && agent.id.trim().length > 0,
    argsText: valuesToLines(agent.cli.args),
    modelArgsText: valuesToLines(agent.cli.model_args),
    envText: pairsToText(agent.cli.env),
    headersText: pairsToText(agent.cloud.headers),
    doneValuesText: valuesToLines(agent.cloud.poll_done_values),
    failedValuesText: valuesToLines(agent.cloud.poll_failed_values),
  };
}

/** Applies the draft's text fields back onto the agent before saving. */
function draftToAgent(draft: AgentDraft): SubAgent {
  return {
    ...draft.agent,
    cli: {
      ...draft.agent.cli,
      args: linesToValues(draft.argsText),
      model_args: linesToValues(draft.modelArgsText),
      env: textToPairs(draft.envText),
    },
    cloud: {
      ...draft.agent.cloud,
      headers: textToPairs(draft.headersText),
      poll_done_values: linesToValues(draft.doneValuesText),
      poll_failed_values: linesToValues(draft.failedValuesText),
    },
  };
}

/** Normalize an agent's definition path (md files always use forward slashes). */
function agentFile(agent: SubAgent): string {
  return (agent.file ?? "").replace(/\\/g, "/");
}

/**
 * Sidebar grouping: file-backed `llm` agents are grouped by the first folder
 * under `agents/` (`custom/…` for panel-created definitions, `engineering/…`
 * and other imported collections after it). Agents with no definition file —
 * `cli` / `cloud` agents and legacy inline `llm` entries, all created by the
 * user — belong to the `custom` group so they stay above the library.
 * A top-level markdown file has no category and sorts last.
 */
function agentCategory(agent: SubAgent): string {
  const file = agentFile(agent);
  if (!file) return "custom";
  const parts = file.split("/");
  return parts.length > 1 ? parts[0] : "";
}

/**
 * Only `agents/custom/**` files may be deleted from the panel. Library agents
 * (`engineering/…` and other imported collections) are read-only: disabling
 * them removes them from the roster, the original file stays untouched.
 */
function canDeleteAgent(agent: SubAgent): boolean {
  if (agent.agent_type !== "llm") return true;
  const file = agentFile(agent);
  return file === "" || file.startsWith("custom/");
}

interface ProbeState {
  status: "running" | "ok" | "error";
  text: string;
}

export function AgentsPanel({ onClose, onAgentsChange, useAgentsEnabled, onToggleUseAgents, agentStatuses }: Props) {
  const { t } = useI18n();
  const [agents, setAgents] = useState<SubAgent[]>([]);
  const [orchestration, setOrchestration] = useState<AgentOrchestration>({
    use_agents: false,
    auto_configure: true,
    max_concurrent: 3,
    mode: "parallel",
    mbus_enabled: true,
    mbus_pair_limit: 4,
  });
  const [draft, setDraft] = useState<AgentDraft | null>(null);
  const [probe, setProbe] = useState<ProbeState | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [isPromptEditorOpen, setIsPromptEditorOpen] = useState(false);
  const [promptDraft, setPromptDraft] = useState("");
  const [search, setSearch] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  /** Per-model context windows — the same map the backend budgets from. */
  const [modelContextLengths, setModelContextLengths] = useState<Record<string, number>>({});
  /** Model inherited when an agent leaves its model field empty. */
  const [fallbackModel, setFallbackModel] = useState("");
  /** In-progress edit, scoped to the model id it was typed for. */
  const [contextEdit, setContextEdit] = useState<{ model: string; value: string } | null>(null);

  const refreshAgents = async () => {
    const updated = await listSubAgents();
    setAgents(updated);
    onAgentsChange(updated.filter((a) => a.enabled).length);
  };

  useEffect(() => {
    refreshAgents().catch(console.error);
    getAgentOrchestration().then(setOrchestration).catch(console.error);
    // Context windows are keyed by model id, so one read covers every agent.
    getConfig()
      .then((cfg) => {
        setModelContextLengths(cfg.model_context_lengths ?? {});
        setFallbackModel(cfg.model);
      })
      .catch(console.error);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function handleRefresh() {
    setRefreshing(true);
    try {
      // Refresh is the only discovery trigger: it rescans agents/ and replaces
      // agents/index.yaml, then returns the merged roster.
      const refreshed = await refreshSubAgentIndex();
      setAgents(refreshed);
      onAgentsChange(refreshed.filter((a) => a.enabled).length);
    } catch (e) {
      console.error(e);
    } finally {
      setRefreshing(false);
    }
  }

  async function handleToggleAgent(agent: SubAgent) {
    // A broken definition has no prompt to run; repairing (or deleting) it is
    // the only meaningful action until the file parses again.
    if (agent.parse_error) return;
    try {
      const saved = await saveSubAgent({ ...agent, enabled: !agent.enabled });
      const newList = agents.map((a) => (a.id === agent.id ? saved : a));
      setAgents(newList);
      onAgentsChange(newList.filter((a) => a.enabled).length);
    } catch (e) {
      console.error(e);
    }
  }

  async function handleSaveAgent() {
    if (!draft || !draft.agent.name.trim()) return;
    setSaving(true);
    setSaveError(null);
    try {
      await saveSubAgent(draftToAgent(draft));
      await refreshAgents();
      setDraft(null);
      setContextEdit(null);
      setProbe(null);
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  }

  /**
   * Persist the context-window edit for the model currently selected in the
   * draft. It is stored per model id (shared by every agent using it) and read
   * by the backend on each request, so it must be saved outside the agent draft.
   */
  async function commitContextWindow() {
    if (!draft || !contextEdit) return;
    const model = activeModelFor(draft.agent);
    if (!model || contextEdit.model !== model) return;

    const raw = contextEdit.value.trim();
    const parsed = raw === "" ? null : Math.floor(Number(raw));
    if (parsed !== null && (!Number.isFinite(parsed) || parsed <= 0)) {
      // Drop an invalid edit instead of leaving the field stuck on "abc".
      setContextEdit(null);
      return;
    }

    try {
      await setModelContextLength(model, parsed);
      setModelContextLengths((prev) => {
        const next = { ...prev };
        if (parsed === null) delete next[model];
        else next[model] = parsed;
        return next;
      });
      setContextEdit(null);
    } catch (e) {
      setSaveError(String(e));
    }
  }

  async function handleDeleteAgent(agent: SubAgent) {
    if (!canDeleteAgent(agent)) return;
    const file = agentFile(agent);
    if (file.startsWith("custom/") && !window.confirm(t("agents.deleteCustomConfirm", { name: agent.name }))) {
      return;
    }
    try {
      await deleteSubAgent(agent.id);
      const updated = agents.filter((a) => a.id !== agent.id);
      setAgents(updated);
      onAgentsChange(updated.filter((a) => a.enabled).length);
    } catch (e) {
      window.alert(String(e));
    }
  }

  async function handleSaveOrchestration(updated: AgentOrchestration) {
    setOrchestration(updated);
    await saveAgentOrchestration(updated).catch(console.error);
  }

  async function handleProbe() {
    if (!draft) return;
    const agent = draftToAgent(draft);
    setProbe({ status: "running", text: t("agents.probing") });
    try {
      if (agent.agent_type === "cli") {
        const result = await probeAgentCli(agent.cli.command, ["--version"]);
        const exit =
          result.exit_code === null || result.exit_code === undefined
            ? ""
            : ` (exit code ${result.exit_code})`;
        setProbe({
          status: "ok",
          text: `${result.resolved_path}${exit}\n${result.output.trim() || t("common.empty")}`,
        });
      } else if (agent.agent_type === "cloud") {
        const result = await probeAgentCloud(
          agent.cloud.endpoint,
          agent.cloud.token,
          agent.cloud.headers,
        );
        const statusLine = result.status ? `HTTP ${result.status}` : t("agents.cloudUnreachable");
        setProbe({
          status: result.reachable ? "ok" : "error",
          text: `${result.endpoint}\n${statusLine}\n${result.detail}`.trim(),
        });
      }
    } catch (e) {
      setProbe({ status: "error", text: String(e) });
    }
  }

  /** Model the draft will actually run on (empty field inherits the main one). */
  const activeModelFor = (agent: SubAgent): string =>
    (agent.model ?? "").trim() || fallbackModel.trim();

  const updateDraft = (patch: Partial<AgentDraft>) => {
    setDraft((prev) => (prev ? { ...prev, ...patch } : prev));
  };

  const updateAgent = (patch: Partial<SubAgent>) => {
    setDraft((prev) => (prev ? { ...prev, agent: { ...prev.agent, ...patch } } : prev));
  };

  const updateCli = (patch: Partial<CliAgentSettings>) => {
    setDraft((prev) =>
      prev ? { ...prev, agent: { ...prev.agent, cli: { ...prev.agent.cli, ...patch } } } : prev
    );
  };

  const updateCloud = (patch: Partial<CloudAgentSettings>) => {
    setDraft((prev) =>
      prev ? { ...prev, agent: { ...prev.agent, cloud: { ...prev.agent.cloud, ...patch } } } : prev
    );
  };

  function toggleAllowedTool(tool: string) {
    if (!draft) return;
    const has = draft.agent.allowed_tools.includes(tool);
    updateAgent({
      allowed_tools: has
        ? draft.agent.allowed_tools.filter((entry) => entry !== tool)
        : [...draft.agent.allowed_tools, tool],
    });
  }

  function openPromptEditor() {
    if (!draft) return;
    setPromptDraft(draft.agent.system_prompt ?? "");
    setIsPromptEditorOpen(true);
  }

  function applyPromptEditor() {
    updateAgent({ system_prompt: promptDraft });
    setIsPromptEditorOpen(false);
  }

  const statusIcon = (agentId: string) => {
    const s = agentStatuses[agentId];
    if (!s || s.status === "idle") return null;
    if (s.status === "running") return (
      <span className="agent-status-badge running">
        <FontAwesomeIcon icon={faGear} spin />
        {t("agents.statusRunning")}
      </span>
    );
    if (s.status === "done") return (
      <span className="agent-status-badge done">
        <FontAwesomeIcon icon={faCheck} />
        {t("agents.statusDone")}
      </span>
    );
    if (s.status === "error") return (
      <span className="agent-status-badge error">
        <FontAwesomeIcon icon={faXmark} />
        {t("agents.statusError")}
      </span>
    );
    return null;
  };

  /** Compact one-line summary of what the agent runs. */
  const agentTarget = (agent: SubAgent): string => {
    if (agent.agent_type === "cli") {
      const model = agent.model?.trim();
      // Mirror the backend: `{model}` templates only render once a model is
      // configured, and a literal `-m <id>` applies either way.
      const needsModel = agent.cli.model_args.some((arg) => arg.includes("{model}"));
      const modelArgs =
        needsModel && !model
          ? []
          : agent.cli.model_args.map((arg) =>
              model ? arg.replace(/\{model\}/g, model) : arg
            );
      return (
        [agent.cli.command, ...agent.cli.args, ...modelArgs].join(" ").trim() ||
        t("agents.noCommand")
      );
    }
    if (agent.agent_type === "cloud") {
      return agent.cloud.endpoint.trim() || t("agents.noEndpoint");
    }
    return agent.model?.trim() || t("agents.inheritModel");
  };

  const renderTypeSpecificForms = () => {
    if (!draft) return null;
    const { agent } = draft;

    if (agent.agent_type === "cli") {
      return (
        <>
          <div className="agent-editor-section-title">{t("agents.cliSection")}</div>
          <label>
            {t("agents.cliCommand")}
            <input
              value={agent.cli.command}
              onChange={(e) => updateCli({ command: e.target.value })}
              placeholder={t("agents.cliCommandPlaceholder")}
            />
            <small className="agent-field-hint">{t("agents.cliCommandHint")}</small>
          </label>
          <label>
            {t("agents.cliArgs")}
            <textarea
              rows={4}
              value={draft.argsText}
              onChange={(e) => updateDraft({ argsText: e.target.value })}
              placeholder={"run\n{prompt}"}
            />
            <small className="agent-field-hint">{t("agents.cliArgsHint")}</small>
          </label>
          <label>
            {t("agents.cliPromptVia")}
            <select
              value={agent.cli.prompt_via}
              onChange={(e) => updateCli({ prompt_via: e.target.value })}
            >
              <option value="arg">{t("agents.cliPromptViaArg")}</option>
              <option value="stdin">{t("agents.cliPromptViaStdin")}</option>
            </select>
            <small className="agent-field-hint">{t("agents.cliPromptViaHint")}</small>
          </label>
          <label>
            {t("agents.cliModelArgs")}
            <textarea
              rows={2}
              value={draft.modelArgsText}
              onChange={(e) => updateDraft({ modelArgsText: e.target.value })}
              placeholder={"-m\nopencode-go/deepseek-v4.1-flash"}
            />
            <small className="agent-field-hint">{t("agents.cliModelArgsHint")}</small>
          </label>
          <label>
            {t("agents.cliEnv")}
            <textarea
              rows={2}
              value={draft.envText}
              onChange={(e) => updateDraft({ envText: e.target.value })}
              placeholder={"KEY=VALUE"}
            />
            <small className="agent-field-hint">{t("agents.cliEnvHint")}</small>
          </label>
          <label>
            {t("agents.timeout")}
            <input
              type="number"
              min={0}
              max={7200}
              value={agent.cli.default_timeout_seconds}
              onChange={(e) =>
                updateCli({
                  default_timeout_seconds: parseTimeoutSeconds(
                    e.target.value,
                    DEFAULT_CLI_TIMEOUT_SECONDS,
                  ),
                })
              }
            />
            <small className="agent-field-hint">{t("agents.timeoutHint")}</small>
          </label>
        </>
      );
    }

    if (agent.agent_type === "cloud") {
      return (
        <>
          <div className="agent-editor-section-title">{t("agents.cloudSection")}</div>
          <label>
            {t("agents.cloudEndpoint")}
            <input
              value={agent.cloud.endpoint}
              onChange={(e) => updateCloud({ endpoint: e.target.value })}
              placeholder={t("agents.cloudEndpointPlaceholder")}
            />
          </label>
          <label>
            {t("agents.cloudToken")}
            <input
              type="password"
              value={agent.cloud.token}
              onChange={(e) => updateCloud({ token: e.target.value })}
              placeholder={t("agents.cloudTokenPlaceholder")}
            />
            <small className="agent-field-hint">{t("agents.cloudTokenHint")}</small>
          </label>
          <label>
            {t("agents.cloudHeaders")}
            <textarea
              rows={2}
              value={draft.headersText}
              onChange={(e) => updateDraft({ headersText: e.target.value })}
              placeholder={"X-Api-Version=2026-01-01"}
            />
          </label>
          <label>
            {t("agents.cloudBodyTemplate")}
            <textarea
              rows={4}
              value={agent.cloud.body_template}
              onChange={(e) => updateCloud({ body_template: e.target.value })}
              placeholder={'{"messages":[{"role":"user","content":"{prompt}"}]}'}
            />
            <small className="agent-field-hint">{t("agents.cloudBodyTemplateHint")}</small>
          </label>
          <label>
            {t("agents.cloudResultPath")}
            <input
              value={agent.cloud.result_path}
              onChange={(e) => updateCloud({ result_path: e.target.value })}
              placeholder="choices.0.message.content"
            />
            <small className="agent-field-hint">{t("agents.cloudResultPathHint")}</small>
          </label>
          <label>
            {t("agents.cloudPollUrl")}
            <input
              value={agent.cloud.poll_url}
              onChange={(e) => updateCloud({ poll_url: e.target.value })}
              placeholder="https://api.example.com/jobs/{id}"
            />
            <small className="agent-field-hint">{t("agents.cloudPollUrlHint")}</small>
          </label>
          {agent.cloud.poll_url.trim() && (
            <>
              <div className="agent-editor-row">
                <label style={{ flex: 1 }}>
                  {t("agents.cloudPollIdPath")}
                  <input
                    value={agent.cloud.poll_id_path}
                    onChange={(e) => updateCloud({ poll_id_path: e.target.value })}
                  />
                </label>
                <label style={{ flex: 1 }}>
                  {t("agents.cloudPollStatusPath")}
                  <input
                    value={agent.cloud.poll_status_path}
                    onChange={(e) => updateCloud({ poll_status_path: e.target.value })}
                  />
                </label>
                <label style={{ flex: 1 }}>
                  {t("agents.cloudPollInterval")}
                  <input
                    type="number"
                    min={1}
                    max={3600}
                    value={agent.cloud.poll_interval_seconds}
                    onChange={(e) =>
                      updateCloud({
                        poll_interval_seconds: Math.max(1, Number(e.target.value) || 5),
                      })
                    }
                  />
                </label>
              </div>
              <label>
                {t("agents.cloudPollDoneValues")}
                <textarea
                  rows={2}
                  value={draft.doneValuesText}
                  onChange={(e) => updateDraft({ doneValuesText: e.target.value })}
                  placeholder={"completed\nsucceeded"}
                />
              </label>
              <label>
                {t("agents.cloudPollFailedValues")}
                <textarea
                  rows={2}
                  value={draft.failedValuesText}
                  onChange={(e) => updateDraft({ failedValuesText: e.target.value })}
                  placeholder={"failed\nerror"}
                />
              </label>
            </>
          )}
          <label>
            {t("agents.timeout")}
            <input
              type="number"
              min={0}
              max={7200}
              value={agent.cloud.timeout_seconds}
              onChange={(e) =>
                updateCloud({
                  timeout_seconds: parseTimeoutSeconds(
                    e.target.value,
                    DEFAULT_CLOUD_TIMEOUT_SECONDS,
                  ),
                })
              }
            />
            <small className="agent-field-hint">{t("agents.cloudTimeoutHint")}</small>
          </label>
        </>
      );
    }

    // ── llm ──
    const activeModel = activeModelFor(agent);
    const storedContext = activeModel ? modelContextLengths[activeModel] : undefined;
    const contextText =
      contextEdit && contextEdit.model === activeModel
        ? contextEdit.value
        : storedContext !== undefined
          ? String(storedContext)
          : "";
    return (
      <>
        <label>
          <div className="field-title-row">
            <span>{t("agents.systemPrompt")}</span>
            <button type="button" className="inline-edit-btn" onClick={openPromptEditor}>
              {t("common.edit")}
            </button>
          </div>
          <textarea
            rows={6}
            value={agent.system_prompt}
            onChange={(e) => updateAgent({ system_prompt: e.target.value })}
            placeholder={t("agents.systemPromptPlaceholder")}
          />
        </label>
        <div className="agent-editor-section-title">{t("agents.modelLimits")}</div>
        <label>
          {t("agents.model")}
          <input
            value={agent.model ?? ""}
            onChange={(e) => {
              // The window belongs to the model id, not to the draft: never
              // carry a pending value over to a different model.
              setContextEdit(null);
              updateAgent({ model: e.target.value || undefined });
            }}
            placeholder={t("agents.modelPlaceholder")}
          />
          <small className="agent-field-hint">{t("agents.modelHint")}</small>
        </label>
        <label>
          {t("agents.contextWindow")}
          <input
            type="number"
            min={1}
            step={1024}
            value={contextText}
            disabled={!activeModel}
            onChange={(e) =>
              setContextEdit({ model: activeModel, value: e.target.value })
            }
            onBlur={() => void commitContextWindow()}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void commitContextWindow();
              }
            }}
            placeholder={t("agents.contextWindowPlaceholder")}
          />
          <small className="agent-field-hint">{t("agents.contextWindowHint")}</small>
        </label>
        <div className="agent-editor-row">
          <label style={{ flex: 1 }}>
            {t("agents.maxTokens")}
            <input
              type="number"
              min={512}
              max={128000}
              value={agent.max_complete_tokens ?? ""}
              onChange={(e) =>
                updateAgent({
                  max_complete_tokens: e.target.value ? Number(e.target.value) : undefined,
                })
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
              value={agent.temperature ?? ""}
              onChange={(e) =>
                updateAgent({ temperature: e.target.value ? Number(e.target.value) : undefined })
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
              value={agent.max_iterations}
              onChange={(e) =>
                updateAgent({
                  max_iterations:
                    e.target.value === ""
                      ? 0
                      : Math.max(0, Math.min(500, Number(e.target.value) || 0)),
                })
              }
            />
            <small className="agent-field-hint">{t("agents.maxIterationsHint")}</small>
          </label>
        </div>
        <div className="agent-editor-section-title">{t("agents.capabilities")}</div>
        <div className="orch-row">
          <span>{t("agents.enableMcp")}</span>
          <label className="toggle-switch">
            <input
              type="checkbox"
              checked={agent.enable_mcp ?? false}
              onChange={(e) => updateAgent({ enable_mcp: e.target.checked })}
            />
            <span className="toggle-switch-slider" />
          </label>
        </div>
        <small className="agent-field-hint">{t("agents.enableMcpHint")}</small>
        <label>
          <div className="field-title-row">
            <span>{t("agents.allowedTools")}</span>
            <span className="agent-tool-quick">
              <button
                type="button"
                className="inline-edit-btn"
                onClick={() =>
                  updateAgent({
                    allowed_tools: Array.from(
                      new Set([...agent.allowed_tools, ...KNOWN_TOOLS.map((tool) => tool.id)])
                    ),
                  })
                }
              >
                {t("common.all")}
              </button>
              <button
                type="button"
                className="inline-edit-btn"
                onClick={() => updateAgent({ allowed_tools: [] })}
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
                  checked={agent.allowed_tools.includes(tool.id)}
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
      </>
    );
  };

  const query = search.trim().toLowerCase();
  const visibleAgents = query
    ? agents.filter((agent) =>
        [agent.name, agent.description, agent.id, agentFile(agent), agentCategory(agent)].some(
          (value) => value.toLowerCase().includes(query),
        ),
      )
    : agents;

  const groups: Array<{ category: string; agents: SubAgent[] }> = [];
  for (const agent of visibleAgents) {
    const category = agentCategory(agent);
    let group = groups.find((entry) => entry.category === category);
    if (!group) {
      group = { category, agents: [] };
      groups.push(group);
    }
    group.agents.push(agent);
  }
  groups.sort((left, right) => {
    const rank = (category: string) => (category === "custom" ? 0 : category === "" ? 2 : 1);
    return rank(left.category) - rank(right.category) || left.category.localeCompare(right.category);
  });
  for (const group of groups) {
    group.agents.sort(
      (left, right) =>
        Number(right.enabled) - Number(left.enabled) || left.name.localeCompare(right.name),
    );
  }
  const enabledCount = agents.filter((agent) => agent.enabled).length;

  const groupLabel = (category: string): string => {
    if (category === "custom") return t("agents.groupCustom");
    if (category === "") return t("agents.groupUncategorized");
    return category;
  };

  return (
    <div className="agents-panel">
      <div className="agents-header">
        <h2>{t("agents.title")}</h2>
        <div className="agents-header-actions">
          <button className="close-btn" onClick={onClose} aria-label={t("common.close")}>
            <FontAwesomeIcon icon={faXmark} />
          </button>
        </div>
      </div>

      {draft ? (
        /* ── Agent Editor ── */
        <div className="agent-editor">
          <label>
            {t("agents.typeLabel")}
            <div className="agent-type-switch">
              {AGENT_TYPES.map((type) => (
                <button
                  key={type}
                  type="button"
                  className={`agent-type-option${draft.agent.agent_type === type ? " active" : ""}`}
                  onClick={() =>
                    updateAgent({
                      agent_type: type,
                      ...(type === "llm" && draft.agent.allowed_tools.length === 0
                        ? { allowed_tools: [...DEFAULT_LLM_TOOLS] }
                        : {}),
                    })
                  }
                  disabled={draft.typeLocked}
                >
                  {t(TYPE_LABEL_KEYS[type])}
                </button>
              ))}
            </div>
            <small className="agent-field-hint">
              {draft.typeLocked ? t("agents.typeLockedHint") : t("agents.typeHint")}
            </small>
          </label>

          <label>
            {t("agents.name")}
            <input
              value={draft.agent.name}
              onChange={(e) => updateAgent({ name: e.target.value })}
              placeholder={t("agents.namePlaceholder")}
            />
          </label>
          <label>
            {t("agents.description")}
            <input
              value={draft.agent.description}
              onChange={(e) => updateAgent({ description: e.target.value })}
              placeholder={t("agents.descriptionPlaceholder")}
            />
          </label>

          {draft.agent.agent_type === "llm" && (
            <div className="agent-editor-row">
              <label className="agent-emoji-field">
                {t("agents.emoji")}
                <input
                  value={draft.agent.emoji ?? ""}
                  maxLength={16}
                  onChange={(e) => updateAgent({ emoji: e.target.value || undefined })}
                  placeholder="🤖"
                />
                <small className="agent-field-hint">{t("agents.emojiHint")}</small>
              </label>
              <label style={{ flex: 1 }}>
                {t("agents.color")}
                <input
                  value={draft.agent.color ?? ""}
                  onChange={(e) => updateAgent({ color: e.target.value || undefined })}
                  placeholder="purple"
                />
                <small className="agent-field-hint">{t("agents.colorHint")}</small>
              </label>
            </div>
          )}

          {draft.agent.agent_type === "llm" && (
            <small className="agent-field-hint agent-source-file">
              {draft.agent.file
                ? t("agents.sourceFile", { file: draft.agent.file })
                : t("agents.definitionStorageHint")}
            </small>
          )}

          {renderTypeSpecificForms()}

          {draft.agent.agent_type !== "llm" && (
            <div className="orch-row">
              <span>{t("agents.requireApproval")}</span>
              <label className="toggle-switch">
                <input
                  type="checkbox"
                  checked={draft.agent.require_approval}
                  onChange={(e) => updateAgent({ require_approval: e.target.checked })}
                />
                <span className="toggle-switch-slider" />
              </label>
            </div>
          )}
          {draft.agent.agent_type !== "llm" && (
            <small className="agent-field-hint">{t("agents.requireApprovalHint")}</small>
          )}

          {draft.agent.agent_type !== "llm" && (
            <div className="cli-agent-probe-row">
              <button type="button" className="btn-secondary" onClick={handleProbe}>
                {t("agents.probe")}
              </button>
              <span className="agent-field-hint">
                {draft.agent.agent_type === "cli" ? t("agents.probeCliHint") : t("agents.probeCloudHint")}
              </span>
            </div>
          )}
          {probe && <pre className={`cli-agent-probe-result ${probe.status}`}>{probe.text}</pre>}

          {saveError && <div className="agent-save-error">{saveError}</div>}

          <div className="editor-actions">
            <button
              className="btn-primary"
              onClick={handleSaveAgent}
              disabled={!draft.agent.name.trim() || saving}
            >
              {saving ? t("common.saving") : t("common.save")}
            </button>
            <button
              className="btn-secondary"
              onClick={() => {
                setDraft(null);
                setContextEdit(null);
                setProbe(null);
                setSaveError(null);
              }}
            >
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
                      <button type="button" className="btn-secondary" onClick={() => setIsPromptEditorOpen(false)}>
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
              <span>{t("agents.mbus")}</span>
              <label className="toggle-switch">
                <input
                  type="checkbox"
                  checked={orchestration.mbus_enabled}
                  onChange={(e) =>
                    handleSaveOrchestration({ ...orchestration, mbus_enabled: e.target.checked })
                  }
                />
                <span className="toggle-switch-slider" />
              </label>
            </div>
            <div className="agent-field-hint">{t("agents.mbusHint")}</div>

            <div className="orch-row">
              <span>{t("agents.mbusPairLimit")}</span>
              <input
                className="orch-number"
                type="number"
                min={0}
                max={20}
                value={orchestration.mbus_pair_limit}
                disabled={!orchestration.mbus_enabled}
                onChange={(e) => {
                  // 0 is meaningful (cap off), so an empty field must fall back
                  // to the default rather than silently disabling the cap.
                  const raw = e.target.value.trim();
                  const parsed = raw === "" ? 4 : Number(raw);
                  const value = Number.isFinite(parsed)
                    ? Math.max(0, Math.min(20, Math.trunc(parsed)))
                    : 4;
                  handleSaveOrchestration({ ...orchestration, mbus_pair_limit: value });
                }}
              />
            </div>
            <div className="agent-field-hint">{t("agents.mbusPairLimitHint")}</div>

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

          {/* ── Filter ── */}
          <div className="agents-filter">
            <input
              className="panel-search-input"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={t("agents.searchPlaceholder")}
              aria-label={t("agents.searchPlaceholder")}
            />
            <div className="agents-filter-row">
              <span className="agents-count">
                {t("agents.enabledCount", { enabled: enabledCount, total: agents.length })}
              </span>
              <div className="agents-filter-actions">
                <button
                  type="button"
                  className="mcp-action-btn"
                  title={t("agents.rescan")}
                  onClick={handleRefresh}
                  disabled={refreshing}
                >
                  <FontAwesomeIcon icon={faRotateRight} spin={refreshing} />
                </button>
                <button
                  type="button"
                  className="mcp-action-btn"
                  title={t("agents.openFolder")}
                  onClick={() => openAgentsDir().catch(console.error)}
                >
                  <FontAwesomeIcon icon={faFolderOpen} />
                </button>
              </div>
            </div>
          </div>

          {/* ── Agent List ── */}
          <div className="agents-list">
            {agents.length === 0 && (
              <div className="agents-empty">{t("agents.empty")}</div>
            )}
            {agents.length > 0 && visibleAgents.length === 0 && (
              <div className="agents-empty">{t("agents.noResults")}</div>
            )}
            {groups.map((group) => (
              <div key={group.category || "__uncategorized"} className="agent-group">
                <div className="agent-group-title">
                  <span>{groupLabel(group.category)}</span>
                  <span className="agent-group-count">{group.agents.length}</span>
                </div>
                {group.agents.map((agent) => {
                  const st = agentStatuses[agent.id];
                  return (
                    <div
                      key={agent.id}
                      className={`agent-item ${agent.enabled ? "active" : ""}`}
                      onClick={() => {
                        if (!agent.parse_error) handleToggleAgent(agent);
                      }}
                    >
                      <div className="agent-content">
                        <div className="agent-title-row">
                          <label className="toggle-switch" onClick={(e) => e.stopPropagation()}>
                            <input
                              type="checkbox"
                              checked={agent.enabled}
                              disabled={Boolean(agent.parse_error)}
                              onChange={() => handleToggleAgent(agent)}
                            />
                            <span className="toggle-switch-slider" />
                          </label>
                          {agent.emoji && (
                            <span className="agent-emoji" aria-hidden="true">
                              {agent.emoji}
                            </span>
                          )}
                          <span className="agent-name">{agent.name}</span>
                          <span className={`agent-type-badge ${agent.agent_type}`}>
                            {t(TYPE_LABEL_KEYS[agent.agent_type] ?? "agents.typeLlm")}
                          </span>
                          {agent.parse_error && (
                            <span className="agent-status-badge error">
                              <FontAwesomeIcon icon={faTriangleExclamation} />
                              {t("agents.statusError")}
                            </span>
                          )}
                          {statusIcon(agent.id)}
                          <div className="skill-actions" onClick={(e) => e.stopPropagation()}>
                            <button
                              className="mcp-action-btn"
                              title={t("common.edit")}
                              onClick={() => {
                                setProbe(null);
                                setSaveError(null);
                                setContextEdit(null);
                                setDraft(toDraft({ ...agent }));
                              }}
                            >
                              <FontAwesomeIcon icon={faPen} />
                            </button>
                            {canDeleteAgent(agent) && (
                              <button
                                className="mcp-action-btn danger"
                                title={t("common.delete")}
                                onClick={() => handleDeleteAgent(agent)}
                              >
                                <FontAwesomeIcon icon={faXmark} />
                              </button>
                            )}
                          </div>
                        </div>
                        <span className="agent-desc">{agent.description}</span>
                        {agent.parse_error && (
                          <span className="agent-summary error">
                            {t("agents.parseError", { error: agent.parse_error })}
                          </span>
                        )}
                        <span className="agent-target">{agentTarget(agent)}</span>
                        {st && st.status === "done" && st.summary && (
                          <span className="agent-summary">
                            <FontAwesomeIcon icon={faCheck} />
                            {st.summary}
                          </span>
                        )}
                        {st && st.status === "error" && st.error && (
                          <span className="agent-summary error">
                            <FontAwesomeIcon icon={faXmark} />
                            {st.error}
                          </span>
                        )}
                        {st && st.tokens && (
                          <span className="agent-tokens">{st.tokens} tokens</span>
                        )}
                        {agent.agent_type === "llm" && (
                          <div className="agent-tools-row">
                            {agent.allowed_tools.map((tool) => (
                              <span key={tool} className="agent-tool-tag">{tool}</span>
                            ))}
                            {agent.enable_mcp && (
                              <span className="agent-tool-tag mcp">MCP</span>
                            )}
                            {agent.model && (
                              <span className="agent-tool-tag model">{agent.model}</span>
                            )}
                            {agent.file && (
                              <span className="agent-tool-tag file">{agent.file}</span>
                            )}
                          </div>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            ))}
          </div>

          <div className="agents-footer">
            <button
              className="btn-primary"
              onClick={() => {
                setContextEdit(null);
                setDraft(toDraft(emptyAgent()));
              }}
            >
              {t("agents.newAgent")}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
