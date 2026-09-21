import { useState, useEffect, useRef } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { fetchModels, getConfig, getWorkspaceDir, saveConfig, listProfiles, saveProfile, deleteProfile, applyProfile, listMcpServers, listSubAgents, getAgentOrchestration } from "../api";
import type { AppConfig, ModelSettings, Profile } from "../types";
import { MarkdownPreview } from "./MarkdownPreview";
import { MonitorPanel } from "./MonitorPanel";
import { Portal } from "./Portal";
import "./SettingsPanel.css";

interface Props {
  onClose: () => void;
  onConfigSaved?: (config: AppConfig) => void;
  onThemePreview?: (theme: "auto" | "light" | "dark" | undefined) => void;
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

const SETTINGS_SECTIONS = [
  { id: "appearance", label: "Appearance" },
  { id: "workspace", label: "Workspace & Profiles" },
  { id: "api", label: "API & Model" },
  { id: "system", label: "System Message" },
  { id: "advanced", label: "Advanced" },
  { id: "runtime", label: "Runtime & Debug" },
] as const;

export function SettingsPanel({ onClose, onConfigSaved, onThemePreview, sessionId }: Props) {
  const [config, setConfig] = useState<AppConfig>(defaultConfig);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [manualModel, setManualModel] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(true);
  const [workspaceDirActual, setWorkspaceDirActual] = useState("");
  const [pickingWorkspace, setPickingWorkspace] = useState(false);
  const [isMessageEditorOpen, setIsMessageEditorOpen] = useState(false);
  const [messageDraft, setMessageDraft] = useState("");
  const [messageSaving, setMessageSaving] = useState(false);
  const [showMonitor, setShowMonitor] = useState(false);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [newProfileName, setNewProfileName] = useState("");
  const [profileSaving, setProfileSaving] = useState(false);
  const [profileApplying, setProfileApplying] = useState<string | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [activeSection, setActiveSection] = useState<string>(SETTINGS_SECTIONS[0].id);

  function handleSectionScroll() {
    const container = contentRef.current;
    if (!container) return;
    const top = container.scrollTop + 140;
    let current: string = SETTINGS_SECTIONS[0].id;
    for (const section of SETTINGS_SECTIONS) {
      const el = document.getElementById(`settings-section-${section.id}`);
      if (el && el.offsetTop <= top) current = section.id;
    }
    setActiveSection(current);
  }

  function scrollToSection(id: string) {
    const container = contentRef.current;
    const el = document.getElementById(`settings-section-${id}`);
    if (container && el) container.scrollTo({ top: el.offsetTop - 28, behavior: "smooth" });
  }

  useEffect(() => {
    getWorkspaceDir().then(setWorkspaceDirActual).catch(console.error);
    listProfiles().then(setProfiles).catch(console.error);
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

  async function handlePickWorkspace() {
    if (pickingWorkspace) return;
    setPickingWorkspace(true);
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "Select workspace directory",
        defaultPath: config.workspace_dir || workspaceDirActual || undefined,
      });
      if (typeof selected === "string" && selected) {
        setConfig((prev) => ({ ...prev, workspace_dir: selected }));
      }
    } catch (err) {
      console.error("Failed to open directory picker:", err);
    } finally {
      setPickingWorkspace(false);
    }
  }

  function handleClearWorkspace() {
    setConfig((prev) => {
      const { workspace_dir: _drop, ...rest } = prev;
      void _drop;
      return rest;
    });
  }

  async function handleFetchModels() {
    setLoadingModels(true);
    setModelsError(null);
    try {
      const remoteModels = await fetchModels();
      //const merged = Array.from(new Set([...(config.model_catalog ?? []), ...remoteModels])));
      const merged = mergeModels([], remoteModels);
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
      // `selected_skills` / `selected_tools` are owned by the skills and tools
      // panels, not by this form. Re-read them before saving so that changing
      // the workspace (or any other setting) doesn't roll back skill/tool
      // toggles made while this panel was open.
      const persisted = await getConfig();
      const normalized: AppConfig = {
        ...config,
        model_catalog: modelCatalog,
        selected_skills: persisted.selected_skills ?? [],
        selected_tools: persisted.selected_tools ?? [],
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

  async function handleSaveProfile() {
    if (!newProfileName.trim()) return;
    setProfileSaving(true);
    try {
      const [mcpServers, agents, orchestration] = await Promise.all([
        listMcpServers(),
        listSubAgents(),
        getAgentOrchestration(),
      ]);
      const profile: Profile = {
        name: newProfileName.trim(),
        selected_skills: config.selected_skills || [],
        selected_tools: config.selected_tools || [],
        agents,
        orchestration,
        mcp_servers: mcpServers,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };
      await saveProfile(profile);
      setNewProfileName("");
      const updated = await listProfiles();
      setProfiles(updated);
    } catch (e) {
      console.error(e);
    } finally {
      setProfileSaving(false);
    }
  }

  async function handleApplyProfile(name: string) {
    setProfileApplying(name);
    try {
      await applyProfile(name);
      const cfg = await getConfig();
      const modelCatalog = mergeModels(cfg.model_catalog, [cfg.model]);
      setConfig({ ...cfg, model_catalog: modelCatalog, model_settings: cfg.model_settings ?? {} });
      onThemePreview?.(cfg.theme);
      onConfigSaved?.(cfg);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      console.error(e);
    } finally {
      setProfileApplying(null);
    }
  }

  async function handleDeleteProfile(name: string) {
    try {
      await deleteProfile(name);
      const updated = await listProfiles();
      setProfiles(updated);
    } catch (e) {
      console.error(e);
    }
  }

  return (
    <div className="settings-page" role="dialog" aria-modal="true" aria-label="Settings">
      <header className="settings-page-header">
        <div className="settings-page-heading">
          <h2>Settings</h2>
          <p>Manage appearance, workspace, API and runtime options.</p>
        </div>
        <div className="settings-page-actions">
          <button type="button" className="settings-btn" onClick={onClose}>
            Close
          </button>
          <button type="button" className="settings-btn settings-btn-primary" onClick={handleSave} disabled={saving}>
            {saving ? "Saving…" : saved ? "Saved ✓" : "Save Changes"}
          </button>
        </div>
      </header>

      <div className="settings-layout">
        <nav className="settings-nav" aria-label="Settings sections">
          {SETTINGS_SECTIONS.map((section) => (
            <button
              key={section.id}
              type="button"
              className={`settings-nav-item${activeSection === section.id ? " active" : ""}`}
              onClick={() => scrollToSection(section.id)}
            >
              {section.label}
            </button>
          ))}
        </nav>

        <div className="settings-content" ref={contentRef} onScroll={handleSectionScroll}>
          <section id="settings-section-appearance" className="settings-section">
            <div className="settings-section-head">
              <h3>Appearance</h3>
              <p>Pick how the interface follows your system theme.</p>
            </div>
            <div className="settings-section-body">
              <div className="settings-field-row">
                <span className="settings-field-label">Color mode</span>
                <div className="theme-segmented">
                  {(["auto", "light", "dark"] as const).map((t) => (
                    <button
                      key={t}
                      type="button"
                      className={`theme-seg-btn${(config.theme ?? "auto") === t ? " active" : ""}`}
                      onClick={() => {
                        setConfig((prev) => ({ ...prev, theme: t }));
                        // Apply immediately so the switch is visible in real time.
                        onThemePreview?.(t);
                      }}
                    >
                      {t === "auto" ? "Auto" : t === "light" ? "Light" : "Dark"}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </section>

          <section id="settings-section-workspace" className="settings-section">
            <div className="settings-section-head">
              <h3>Workspace & Profiles</h3>
              <p>Directory for skills, tools and files, plus saved configuration profiles.</p>
            </div>
            <div className="settings-section-body">
              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">Workspace directory</span>
                <div className="workspace-dir-row">
                  <input
                    type="text"
                    readOnly
                    value={config.workspace_dir ?? ""}
                    placeholder={workspaceDirActual || "Default workspace directory"}
                    title={config.workspace_dir || workspaceDirActual || "Default workspace directory"}
                    onClick={handlePickWorkspace}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        handlePickWorkspace();
                      }
                    }}
                  />
                  <button
                    type="button"
                    className="settings-btn"
                    onClick={handlePickWorkspace}
                    disabled={pickingWorkspace}
                    title="Browse for a folder"
                  >
                    {pickingWorkspace ? "Picking…" : "Browse…"}
                  </button>
                  {config.workspace_dir && (
                    <button
                      type="button"
                      className="settings-btn workspace-dir-clear"
                      onClick={handleClearWorkspace}
                      title="Use the default workspace directory"
                      aria-label="Reset workspace directory to default"
                    >
                      Reset
                    </button>
                  )}
                </div>
              </div>

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
                  <small>Let the assistant evolve its own skills and tools inside the workspace.</small>
                </div>
              </label>

              <div className="settings-divider" />

              <div className="profile-section">
                <div className="profile-section-title">Configuration Profiles</div>
                <div className="profile-create-row">
                  <input
                    type="text"
                    value={newProfileName}
                    onChange={(e) => setNewProfileName(e.target.value)}
                    placeholder="Profile name"
                    onKeyDown={(e) => e.key === "Enter" && handleSaveProfile()}
                  />
                  <button
                    type="button"
                    className="settings-btn"
                    onClick={handleSaveProfile}
                    disabled={!newProfileName.trim() || profileSaving}
                  >
                    {profileSaving ? "Saving..." : "Save Current"}
                  </button>
                </div>
                {profiles.length > 0 && (
                  <div className="profile-list">
                    {profiles.map((profile) => (
                      <div key={profile.name} className="profile-item">
                        <div className="profile-item-info">
                          <span className="profile-item-name">{profile.name}</span>
                          <span className="profile-item-meta">
                            {profile.selected_skills.length} skills · {profile.selected_tools.length} tools · {profile.agents.length} agents
                          </span>
                        </div>
                        <div className="profile-item-actions">
                          <button
                            type="button"
                            className="settings-btn"
                            onClick={() => handleApplyProfile(profile.name)}
                            disabled={profileApplying !== null}
                          >
                            {profileApplying === profile.name ? "Applying..." : "Apply"}
                          </button>
                          <button
                            type="button"
                            className="settings-btn settings-btn-danger"
                            onClick={() => handleDeleteProfile(profile.name)}
                            aria-label={`Delete profile ${profile.name}`}
                          >
                            ✕
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          </section>

          <section id="settings-section-api" className="settings-section">
            <div className="settings-section-head">
              <h3>API & Model</h3>
              <p>Connect any OpenAI-compatible endpoint and pick a model.</p>
            </div>
            <div className="settings-section-body">
              <label className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">Base URL</span>
                <input
                  type="text"
                  value={config.api_base_url}
                  onChange={(e) => setConfig({ ...config, api_base_url: e.target.value })}
                  placeholder="https://api.openai.com/v1"
                />
              </label>

              <div className="settings-field-grid">
                <label className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">API Key</span>
                  <input
                    type="password"
                    value={config.api_key}
                    onChange={(e) => setConfig({ ...config, api_key: e.target.value })}
                    placeholder="sk-..."
                  />
                </label>

                <label className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">Model</span>
                  <select
                    value={config.model}
                    onChange={(e) => setConfig({ ...config, model: e.target.value })}
                  >
                    {modelCatalog.map((name) => (
                      <option key={name} value={name}>{name}</option>
                    ))}
                  </select>
                </label>
              </div>

              <div className="settings-field-grid">
                <div className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">Model catalog</span>
                  <button type="button" className="settings-btn" onClick={handleFetchModels} disabled={loadingModels}>
                    {loadingModels ? "Loading models…" : "Fetch Models From API"}
                  </button>
                  {modelsError && <div className="settings-error">{modelsError}</div>}
                </div>

                <div className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">Add model manually</span>
                  <div className="settings-inline-row">
                    <input
                      type="text"
                      value={manualModel}
                      onChange={(e) => setManualModel(e.target.value)}
                      placeholder="gpt-4.1-mini"
                    />
                    <button className="settings-btn" onClick={handleAddManualModel} type="button">
                      Add
                    </button>
                  </div>
                </div>
              </div>
            </div>
          </section>

          <section id="settings-section-system" className="settings-section">
            <div className="settings-section-head">
              <h3>System Message</h3>
              <p>Instructions sent to the model at the start of every conversation.</p>
            </div>
            <div className="settings-section-body">
              <div className="settings-field-row settings-field-row-stacked">
                <div className="field-title-row">
                  <span className="settings-field-label">Content</span>
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
              </div>
            </div>
          </section>

          <section id="settings-section-advanced" className="settings-section">
            <div
              className="settings-section-head settings-section-head-toggle"
              onClick={() => setAdvancedOpen((prev) => !prev)}
              role="button"
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setAdvancedOpen((prev) => !prev);
                }
              }}
            >
              <div>
                <h3>Model Advanced Settings</h3>
                <p>Sampling parameters — leave empty to use API defaults.</p>
              </div>
              <span className={`settings-toggle-icon ${advancedOpen ? "open" : ""}`}>
                ▼
              </span>
            </div>

            {advancedOpen && (
              <div className="settings-section-body">
                <div className="settings-field-grid settings-field-grid-3">
                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">Temperature</span>
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
                      placeholder="default"
                    />
                  </label>

                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">Top P</span>
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
                      placeholder="default"
                    />
                  </label>

                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">Max Completion Tokens</span>
                    <input
                      type="number"
                      min={1}
                      step={1}
                      value={config.model_settings?.max_complete_tokens ?? ""}
                      onChange={(e) => {
                        const v = e.target.value;
                        setConfig((prev) => ({
                          ...prev,
                          model_settings: updateModelSettings(prev.model_settings, {
                            max_complete_tokens: v === "" ? undefined : Math.max(1, Math.floor(Number(v))),
                          }),
                        }));
                      }}
                      placeholder="default limit"
                    />
                  </label>
                </div>
              </div>
            )}
          </section>

          <section id="settings-section-runtime" className="settings-section">
            <div className="settings-section-head">
              <h3>Runtime & Debug</h3>
              <p>Where logs are written and how requests are monitored.</p>
            </div>
            <div className="settings-section-body">
              <label className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">Logger Output (debug build only)</span>
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
              </label>
              {sessionId && (
                <div className="settings-field-row">
                  <span className="settings-field-label">API request monitor</span>
                  <button type="button" className="settings-btn" onClick={() => setShowMonitor(true)}>
                    Launch Monitor
                  </button>
                </div>
              )}
            </div>
          </section>
        </div>
      </div>

      {isMessageEditorOpen && (
        <Portal>
          <div className="prompt-editor-overlay" role="dialog" aria-modal="true" aria-label="Edit system message">
            <div className="prompt-editor-shell">
              <div className="prompt-editor-header">
                <h3>System Message Editor</h3>
                <div className="prompt-editor-actions">
                  <button type="button" className="settings-btn" onClick={closeMessageEditor} disabled={messageSaving}>
                    Cancel
                  </button>
                  <button type="button" className="settings-btn settings-btn-primary" onClick={applyMessageEditor} disabled={messageSaving}>
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
        </Portal>
      )}

      {showMonitor && (
        <Portal>
          <MonitorPanel sessionId={sessionId!} onClose={() => setShowMonitor(false)} />
        </Portal>
      )}
    </div>
  );
}