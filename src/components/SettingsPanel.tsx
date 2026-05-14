import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig } from "../types";
import "./SettingsPanel.css";

interface Props {
  onClose: () => void;
}

export function SettingsPanel({ onClose }: Props) {
  const [config, setConfig] = useState<AppConfig>({
    api_base_url: "https://api.openai.com/v1",
    api_key: "",
    model: "gpt-4o-mini",
  });
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
        <label>
          API Base URL
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
      </div>

      <div className="settings-footer">
        <button className="btn-primary" onClick={handleSave} disabled={saving}>
          {saving ? "Saving…" : saved ? "Saved ✓" : "Save"}
        </button>
      </div>
    </div>
  );
}
