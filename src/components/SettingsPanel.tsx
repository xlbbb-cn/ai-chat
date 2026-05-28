import { useState, useEffect } from "react";
import { fetchModels, getConfig, getWorkspaceDir, saveConfig } from "../api";
import type { AppConfig, ModelSettings } from "../types";
import { MarkdownPreview } from "./MarkdownPreview";
import { MonitorPanel } from "./MonitorPanel";
import "./SettingsPanel.css";

interface Props {
  onClose: () => void;
  onConfigSaved?: (config: AppConfig) => void;
  sessionId?: string;
}

const defaultConfig: AppConfig = {
  api_base_url: "https://api.openai.com/v1",
  api_key: "",
  model: "gpt-4o-mini",
  model_catalog: ["gpt-4o-mini"],
  model_settings: {},
  system_message: "",
  logger_output: "file",
  self_evolution_mode: false,
};

function mergeModels(current: string[] | undefined, incoming: string[]): string[] {
  const merged = [...(current ?? []), ...incoming]
    .map((m) => m.trim())
    .filter(Boolean);
  return Array.from(new Set(merged));
}

function updateModelSettings(
  settings: ModelSettings | undefined,
  patch: Partial<ModelSettings>
): ModelSettings {
  return { ...(settings ?? {}), ...patch };
}

export function SettingsPanel({ onClose, onConfigSaved, sessionId }: Props) {
  const [config, setConfig] = useState<AppConfig>(defaultConfig);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [manualModel, setManualModel] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(true);
  const [workspaceDirActual, setWorkspaceDirActual] = useState("");
  const [isMessageEditorOpen, setIsMessageEditorOpen] = useState(false);
  const [messageDraft, setMessageDraft] = useState("");
  const [messageSaving, setMessageSaving] = useState(false);
  const [showMonitor, setShowMonitor] = useState(false);

  useEffect(() => {
    getWorkspaceDir().then(setWorkspaceDirActual).catch(console.error);
  }, []);

  useEffect(() => {
    getConfig()
      .then((cfg) => {
        const modelCatalog = mergeModels(cfg.model_catalog, [cfg.model]);
        setConfig({ ...cfg, model_catalog: modelCatalog, model_settings: cfg.model_settings ?? {} });
      })
      .catch(console.error);
  }, []);

  const modelCatalog = mergeModels(config.model_catalog, [config.model]);

  async function handleFetchModels() {
    setLoadingModels(true);
    setModelsError(null);
    try {
      const remoteModels = await fetchModels();
      const merged = mergeModels(config.model_catalog, remoteModels);
      setConfig((prev) => ({
        ...prev,
        model_catalog: merged,
        model: merged.includes(prev.model) ? prev.model : (merged[0] ?? prev.model),
      }));
    } catch (err) {
      setModelsError(String(err));
    } finally {
      setLoadingModels(false);
    }
  }

  function handleAddManualModel() {
    const next = manualModel.trim();
    if (!next) return;
    const merged = mergeModels(config.model_catalog, [next]);
    setConfig((prev) => ({ ...prev, model_catalog: merged, model: next }));
    setManualModel("");
  }

  async function handleSave() {
    setSaving(true);
    try {
      const normalized: AppConfig = {
        ...config,
        model_catalog: modelCatalog,
      };
      await saveConfig(normalized);
      setSaved(true);
      onConfigSaved?.(normalized);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      console.error(e);
    } finally {
      setSaving(false);
    }
  }

  function openMessageEditor() {
    setMessageDraft(config.system_message ?? "");
    setIsMessageEditorOpen(true);
  }

  function closeMessageEditor() {
    setIsMessageEditorOpen(false);
  }

  async function applyMessageEditor() {
    const normalized: AppConfig = {
      ...config,
      system_message: messageDraft,
      model_catalog: modelCatalog,
    };

    setMessageSaving(true);
    try {
      await saveConfig(normalized);
      setConfig(normalized);
      setSaved(true);
      onConfigSaved?.(normalized);
      setTimeout(() => setSaved(false), 2000);
      setIsMessageEditorOpen(false);
    } catch (e) {
      console.error(e);
    } finally {
      setMessageSaving(false);
    }
  }

  return (
    <div className="settings-panel">
      <div className="settings-header">
        <h2>Settings</h2>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      <div className="settings-body">
        <section className="settings-group">
          <div className="settings-group-title">Appearance</div>
          <label className="settings-field">
            Color Mode
          </label>
          <div className="theme-segmented">
            {(["auto", "light", "dark"] as const).map((t) => (
              <button
                key={t}
                type="button"
                className={`theme-seg-btn${(config.theme ?? "auto") === t ? " active" : ""}`}
                onClick={() => setConfig((prev) => ({ ...prev, theme: t }))}
              >
                {t === "auto" ? "Auto" : t === "light" ? "Light" : "Dark"}
              </button>
            ))}
          </div>
        </section>

        <section className="settings-group">
          <div className="settings-group-title">Workspace</div>
          <label className="settings-field">
            Workspace Directory
            <input
              type="text"
              value={config.workspace_dir ?? ""}
              onChange={(e) =>
                setConfig({ ...config, workspace_dir: e.target.value || undefined })
              }
              placeholder={workspaceDirActual || "Default workspace directory"}
            />
          </label>

          <label className="settings-checkbox">
            <input
              type="checkbox"
              checked={config.self_evolution_mode ?? false}
              onChange={(e) =>
                setConfig((prev) => ({
                  ...prev,
                  self_evolution_mode: e.target.checked,
                }))
              }
            />
            <div className="settings-checkbox-copy">
              <span>Enable Self-Evolution Mode</span>
              <small>
                Allow the main agent and sub-agents to inspect and update skill directories and
                sub-agent config so they can iteratively optimize reusable skills and sub-agents.
              </small>
            </div>
          </label>
        </section>

        <section className="settings-group">
          <div className="settings-group-title">API</div>
          <label className="settings-field">
            Base URL
            <input
              type="text"
              value={config.api_base_url}
              onChange={(e) => setConfig({ ...config, api_base_url: e.target.value })}
              placeholder="https://api.openai.com/v1"
            />
          </label>

          <label className="settings-field">
            API Key
            <input
              type="password"
              value={config.api_key}
              onChange={(e) => setConfig({ ...config, api_key: e.target.value })}
              placeholder="sk-..."
            />
          </label>

          <label className="settings-field">
            Model
            <select
              value={config.model}
              onChange={(e) => setConfig({ ...config, model: e.target.value })}
            >
              {modelCatalog.map((name) => (
                <option key={name} value={name}>{name}</option>
              ))}
            </select>
          </label>

          <div className="settings-toolbar">
            <button className="btn-secondary" onClick={handleFetchModels} disabled={loadingModels}>
              {loadingModels ? "Loading models…" : "Fetch Models From API"}
            </button>
            {modelsError && <div className="settings-error">{modelsError}</div>}
          </div>

          <label className="settings-field">
            Add model manually
            <div className="settings-inline-row">
              <input
                type="text"
                value={manualModel}
                onChange={(e) => setManualModel(e.target.value)}
                placeholder="gpt-4.1-mini"
              />
              <button className="btn-secondary" onClick={handleAddManualModel} type="button">
                Add
              </button>
            </div>
          </label>
        </section>

        <section className="settings-group">
          <div className="settings-group-title">System Message</div>
          <label className="settings-field">
            <div className="field-title-row">
              <span>Content</span>
              <button type="button" className="inline-edit-btn" onClick={openMessageEditor}>
                Edit
              </button>
            </div>
            <textarea
              rows={4}
              value={config.system_message ?? ""}
              onChange={(e) => setConfig({ ...config, system_message: e.target.value })}
              placeholder="You are a helpful assistant…"
            />
          </label>
        </section>



        <section className="settings-group">
          <div
            className="settings-group-title settings-section-toggle"
            onClick={() => setAdvancedOpen((prev) => !prev)}
            role="button"
            tabIndex={0}
          >
            <span>Model Advanced Settings</span>
            <span className={`settings-toggle-icon ${advancedOpen ? "open" : ""}`}>
              ▼
            </span>
          </div>

          {advancedOpen && (
            <div className="settings-advanced-grid">
              <label className="settings-field">
                Temperature
                <input
                  type="number"
                  min={0}
                  max={2}
                  step={0.05}
                  value={config.model_settings?.temperature ?? ""}
                  onChange={(e) => {
                    const v = e.target.value;
                    setConfig((prev) => ({
                      ...prev,
                      model_settings: updateModelSettings(prev.model_settings, {
                        temperature: v === "" ? undefined : parseFloat(v),
                      }),
                    }));
                  }}
                  placeholder="default (leave empty)"
                />
              </label>

              <label className="settings-field">
                Top P
                <input
                  type="number"
                  min={0}
                  max={1}
                  step={0.05}
                  value={config.model_settings?.top_p ?? ""}
                  onChange={(e) => {
                    const v = e.target.value;
                    setConfig((prev) => ({
                      ...prev,
                      model_settings: updateModelSettings(prev.model_settings, {
                        top_p: v === "" ? undefined : parseFloat(v),
                      }),
                    }));
                  }}
                  placeholder="default (leave empty)"
                />
              </label>

              <label className="settings-field">
                Max Tokens
                <input
                  type="number"
                  min={1}
                  step={1}
                  value={config.model_settings?.max_tokens ?? ""}
                  onChange={(e) => {
                    const v = e.target.value;
                    setConfig((prev) => ({
                      ...prev,
                      model_settings: updateModelSettings(prev.model_settings, {
                        max_tokens: v === "" ? undefined : Math.max(1, Math.floor(Number(v))),
                      }),
                    }));
                  }}
                  placeholder="default (leave empty)"
                />
              </label>

              <label className="settings-field">
                Reasoning effort
                <select
                  value={config.model_settings?.reasoning_effort ?? ""}
                  onChange={(e) =>
                    setConfig((prev) => ({
                      ...prev,
                      model_settings: updateModelSettings(prev.model_settings, {
                        reasoning_effort: e.target.value,
                      }),
                    }))
                  }
                >
                  <option value="">— not set —</option>
                  <option value="low">low</option>
                  <option value="medium">medium</option>
                  <option value="high">high</option>
                </select>
              </label>
            </div>
          )}
        </section>
        <section className="settings-group">
          <div className="settings-group-title">Runtime & DEBUG</div>
          <label className="settings-field">
            Logger Output (debug build only)
            <select
              value={config.logger_output ?? "file"}
              onChange={(e) =>
                setConfig((prev) => ({
                  ...prev,
                  logger_output: e.target.value as "file" | "println",
                }))
              }
            >
              <option value="file">Write to app.log</option>
              <option value="println">Print to terminal (println)</option>
            </select>
            {sessionId && (
              <button className="btn-secondary" onClick={() => setShowMonitor(true)}>
                🔍 Launch Monitor
              </button>
            )}
          </label>
        </section>
      </div>

      {isMessageEditorOpen && (
        <div className="prompt-editor-overlay" role="dialog" aria-modal="true" aria-label="Edit system message">
          <div className="prompt-editor-shell">
            <div className="prompt-editor-header">
              <h3>System Message Editor</h3>
              <div className="prompt-editor-actions">
                <button type="button" className="btn-secondary" onClick={closeMessageEditor} disabled={messageSaving}>
                  Cancel
                </button>
                <button type="button" className="btn-primary" onClick={applyMessageEditor} disabled={messageSaving}>
                  {messageSaving ? "Saving…" : "Done"}
                </button>
              </div>
            </div>

            <div className="prompt-editor-body">
              <div className="prompt-column">
                <span>Markdown</span>
                <textarea
                  className="prompt-editor-textarea"
                  value={messageDraft}
                  onChange={(e) => setMessageDraft(e.target.value)}
                  placeholder="Write your system message in Markdown..."
                />
              </div>

              <div className="prompt-column">
                <span>Preview</span>
                <div className="prompt-preview">
                  {messageDraft.trim() ? (
                    <MarkdownPreview content={messageDraft} />
                  ) : (
                    <p className="prompt-preview-empty">Markdown preview will appear here.</p>
                  )}
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

      <div className="settings-footer">
        <button className="btn-primary" onClick={handleSave} disabled={saving}>
          {saving ? "Saving…" : saved ? "Saved ✓" : "Save"}
        </button>

      </div>

      {showMonitor && (
        <MonitorPanel sessionId={sessionId!} onClose={() => setShowMonitor(false)} />
      )}
    </div>
  );
}