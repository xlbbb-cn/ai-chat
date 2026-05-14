import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig } from "../types";
import "./SettingsPanel.css";

interface Props {
  onClose: () => void;
}

const defaultConfig: AppConfig = {
  api_base_url: "https://api.openai.com/v1",
  api_key: "",
  model: "gpt-4o-mini",
  temperature: undefined,
  enable_thinking: false,
  reasoning_effort: "",
  system_message: "",
};

export function SettingsPanel({ onClose }: Props) {
  const [config, setConfig] = useState<AppConfig>(defaultConfig);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    getConfig().then(setConfig).catch(console.error);
  }, []);

  async function handleSave() {
    setSaving(true);
    try {
      await saveConfig(config);
      setSaved(true);
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
          <input
            type="text"
            value={config.model}
            onChange={(e) => setConfig({ ...config, model: e.target.value })}
            placeholder="gpt-4o-mini"
          />
        </label>

        <div className="settings-section-title">Generation</div>

        <label>
          System Message
          <textarea
            rows={4}
            value={config.system_message ?? ""}
            onChange={(e) => setConfig({ ...config, system_message: e.target.value })}
            placeholder="You are a helpful assistant…"
          />
        </label>

        <label>
          Temperature
          <input
            type="number"
            min={0}
            max={2}
            step={0.05}
            value={config.temperature ?? ""}
            onChange={(e) => {
              const v = e.target.value;
              setConfig({ ...config, temperature: v === "" ? undefined : parseFloat(v) });
            }}
            placeholder="default (leave empty)"
          />
        </label>

        <div className="settings-section-title">Reasoning</div>

        <label className="settings-row">
          <input
            type="checkbox"
            checked={config.enable_thinking ?? false}
            onChange={(e) => setConfig({ ...config, enable_thinking: e.target.checked })}
          />
          Enable thinking
          <span className="settings-hint">adds <code>thinking: &#123;"type":"enabled"&#125;</code></span>
        </label>

        <label>
          Reasoning effort
          <select
            value={config.reasoning_effort ?? ""}
            onChange={(e) => setConfig({ ...config, reasoning_effort: e.target.value })}
          >
            <option value="">— not set —</option>
            <option value="low">low</option>
            <option value="medium">medium</option>
            <option value="high">high</option>
          </select>
        </label>
      </div>

      <div className="settings-footer">
        <button className="btn-primary" onClick={handleSave} disabled={saving}>
          {saving ? "Saving…" : saved ? "Saved ✓" : "Save"}
        </button>
      </div>
    </div>
  );
}
