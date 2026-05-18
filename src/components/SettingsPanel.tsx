import { useState, useEffect } from "react";
import { fetchModels, getConfig, getWorkspaceDir, saveConfig } from "../api";
import type { AppConfig, ModelSettings } from "../types";
import "./SettingsPanel.css";

interface Props {
  onClose: () => void;
  onConfigSaved?: (config: AppConfig) => void;
}

const defaultConfig: AppConfig = {
  api_base_url: "https://api.openai.com/v1",
  api_key: "",
  model: "gpt-4o-mini",
  model_catalog: ["gpt-4o-mini"],
  model_settings: {},
  system_message: "",
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

export function SettingsPanel({ onClose, onConfigSaved }: Props) {
  const [config, setConfig] = useState<AppConfig>(defaultConfig);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [manualModel, setManualModel] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(true);
  const [workspaceDirActual, setWorkspaceDirActual] = useState("");

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

  return (
    <div className="settings-panel">
      <div className="settings-header">
        <h2>Settings</h2>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      <div className="settings-body">
        <div className="settings-section-title">API</div>

        <label>
          Base URL
          <input
            type="text"
            value={config.api_base_url}
            onChange={(e) => setConfig({ ...config, api_base_url: e.target.value })}
            placeholder="https://api.openai.com/v1"
          />
        </label>

        <label>
          API Key
          <input
            type="password"
            value={config.api_key}
            onChange={(e) => setConfig({ ...config, api_key: e.target.value })}
            placeholder="sk-..."
          />
        </label>

        <label>
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

        <div className="settings-inline-row">
          <button className="btn-secondary" onClick={handleFetchModels} disabled={loadingModels}>
            {loadingModels ? "Loading models…" : "Fetch Models From API"}
          </button>
        </div>
        {modelsError && <div className="settings-error">{modelsError}</div>}

        <label>
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

        <label>
          System Message
          <textarea
            rows={4}
            value={config.system_message ?? ""}
            onChange={(e) => setConfig({ ...config, system_message: e.target.value })}
            placeholder="You are a helpful assistant…"
          />
        </label>

        <div className="settings-section-title">Workspace</div>

        <label>
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

        <div
          className="settings-section-title settings-section-toggle"
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
          <>
            <label>
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

            <label>
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

            <label>
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

            <label>
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
          </>
        )}
      </div>

      <div className="settings-footer">
        <button className="btn-primary" onClick={handleSave} disabled={saving}>
          {saving ? "Saving…" : saved ? "Saved ✓" : "Save"}
        </button>
      </div>
    </div>
  );
}
