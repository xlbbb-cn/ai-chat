import { useState, useEffect, useRef } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { fetchModels, fetchModelMetadata, getAppVersion, getConfig, getWorkspaceDir, saveConfig, listProfiles, saveProfile, deleteProfile, applyProfile, listMcpServers, listSubAgents, getAgentOrchestration } from "../api";
import type { AppConfig, ModelMetadata, ModelSettings, Profile } from "../types";
import { LOCALE_LABELS, LOCALES, useI18n, type Locale, type MessageKey } from "../i18n";
import { matchModelCatalog } from "../utils/modelMetadata";
import { FontAwesomeIcon, faCaretDown, faXmark } from "../icons";
import { MarkdownPreview } from "./MarkdownPreview";
import { MonitorPanel } from "./MonitorPanel";
import { AgentMissionPanel } from "./AgentMissionPanel";
import { Portal } from "./Portal";
import { UpdatePanel } from "./UpdatePanel";
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
  model_context_lengths: {},
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

const SETTINGS_SECTIONS: { id: string; labelKey: MessageKey }[] = [
  { id: "appearance", labelKey: "settings.sections.appearance" },
  { id: "workspace", labelKey: "settings.sections.workspace" },
  { id: "api", labelKey: "settings.sections.api" },
  { id: "system", labelKey: "settings.sections.system" },
  { id: "advanced", labelKey: "settings.sections.advanced" },
  { id: "runtime", labelKey: "settings.sections.runtime" },
  { id: "about", labelKey: "settings.sections.about" },
];

/**
 * Capability flags shown as badges for the selected model, in display order.
 * The labels come from `config.model_metadata` (see `fetch_model_metadata`).
 */
const CAPABILITY_FLAGS: { flag: keyof ModelMetadata; labelKey: MessageKey }[] = [
  { flag: "supports_tools", labelKey: "settings.api.capabilityTools" },
  { flag: "supports_reasoning", labelKey: "settings.api.capabilityReasoning" },
  { flag: "supports_vision", labelKey: "settings.api.capabilityVision" },
  { flag: "supports_files", labelKey: "settings.api.capabilityFiles" },
  { flag: "supports_audio", labelKey: "settings.api.capabilityAudio" },
  { flag: "open_weights", labelKey: "settings.api.capabilityOpenWeights" },
];

/** Compact token count for capability badges: `128K`, `1M`, `512`. */
function formatTokenCount(tokens: number): string {
  if (tokens >= 1_000_000) return `${Math.round(tokens / 100_000) / 10}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return String(tokens);
}

export function SettingsPanel({ onClose, onConfigSaved, onThemePreview, sessionId }: Props) {
  const { t, locale, setLocale } = useI18n();
  const [config, setConfig] = useState<AppConfig>(defaultConfig);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [metadataError, setMetadataError] = useState<string | null>(null);
  const [metadataSummary, setMetadataSummary] = useState<{ matched: number; total: number } | null>(null);
  const [manualModel, setManualModel] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(true);
  const [workspaceDirActual, setWorkspaceDirActual] = useState("");
  const [pickingWorkspace, setPickingWorkspace] = useState(false);
  const [isMessageEditorOpen, setIsMessageEditorOpen] = useState(false);
  const [messageDraft, setMessageDraft] = useState("");
  const [messageSaving, setMessageSaving] = useState(false);
  const [showMonitor, setShowMonitor] = useState(false);
  const [showAgentMonitor, setShowAgentMonitor] = useState(false);
  const [showUpdatePanel, setShowUpdatePanel] = useState(false);
  const [appVersion, setAppVersion] = useState("");
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
    getAppVersion().then(setAppVersion).catch(console.error);
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
  /** Capability metadata of the selected model, when it was matched. */
  const currentMetadata = config.model_metadata?.[config.model];
  const capabilityBadges = currentMetadata
    ? CAPABILITY_FLAGS.filter((capability) => currentMetadata[capability.flag]).map((capability) => ({
      key: capability.flag as string,
      label: t(capability.labelKey),
    }))
    : [];
  if (currentMetadata?.deprecated) {
    capabilityBadges.push({ key: "deprecated", label: t("settings.api.capabilityDeprecated") });
  }

  async function handlePickWorkspace() {
    if (pickingWorkspace) return;
    setPickingWorkspace(true);
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: t("settings.workspace.dirLabel"),
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
    setMetadataError(null);
    setMetadataSummary(null);
    try {
      // The capability catalogue is an independent public feed, so treat it as
      // best-effort: an unreachable mirror must never block the model list.
      const [remoteModels, metadataEntries] = await Promise.all([
        fetchModels(),
        fetchModelMetadata().catch((err) => {
          setMetadataError(String(err));
          return [] as ModelMetadata[];
        }),
      ]);

      const merged = mergeModels([], remoteModels.map((m) => m.id));

      // Names rarely line up exactly (gateway prefixes, date stamps, renames),
      // so match each remote id against the closest catalogue entry.
      const capabilities = matchModelCatalog(merged, metadataEntries);

      // Context windows the provider reported (OpenRouter, Groq, vLLM…) win;
      // the catalogue fills the gap for endpoints that report none.
      const providerLengths: Record<string, number> = {};
      for (const m of remoteModels) {
        if (m.context_length && m.context_length > 0) {
          providerLengths[m.id] = m.context_length;
        }
      }
      if (metadataEntries.length > 0) {
        setMetadataSummary({ matched: Object.keys(capabilities).length, total: merged.length });
      }

      setConfig((prev) => {
        const learned = { ...(prev.model_context_lengths ?? {}) };
        for (const [id, meta] of Object.entries(capabilities)) {
          if (meta.context_length && !learned[id]) learned[id] = meta.context_length;
        }
        return {
          ...prev,
          model_catalog: merged,
          model: merged.includes(prev.model) ? prev.model : (merged[0] ?? prev.model),
          model_context_lengths: { ...learned, ...providerLengths },
          // Keep previously learned capabilities when the catalogue was
          // unreachable, otherwise replace them with this refresh.
          model_metadata: Object.keys(capabilities).length ? capabilities : prev.model_metadata,
        };
      });
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

  /**
   * Switch the interface language and persist it right away: the picker is a
   * preference, not a form field, so it must not depend on “Save Changes”.
   * The switch itself never waits for IPC — a failed write only means the
   * choice is not remembered for the next launch.
   */
  async function handleLanguageChange(next: Locale) {
    setLocale(next);
    setConfig((prev) => ({ ...prev, language: next }));
    try {
      const persisted = await getConfig();
      const normalized: AppConfig = {
        ...config,
        language: next,
        // Owned by the skills / tools panels — never roll those back.
        selected_skills: persisted.selected_skills ?? [],
        selected_tools: persisted.selected_tools ?? [],
      };
      await saveConfig(normalized);
      setConfig(normalized);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
      onConfigSaved?.(normalized);
    } catch (e) {
      console.error("Failed to persist the language preference", e);
    }
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
    <div className="settings-page" role="dialog" aria-modal="true" aria-label={t("settings.aria")}>
      <header className="settings-page-header">
        <div className="settings-page-heading">
          <h2>{t("settings.title")}</h2>
          <p>{t("settings.subtitle")}</p>
        </div>
        <div className="settings-page-actions">
          <button type="button" className="settings-btn" onClick={onClose}>
            {t("common.close")}
          </button>
          <button type="button" className="settings-btn settings-btn-primary" onClick={handleSave} disabled={saving}>
            {saving ? t("common.saving") : saved ? t("common.saved") : t("common.saveChanges")}
          </button>
        </div>
      </header>

      <div className="settings-layout">
        <nav className="settings-nav" aria-label={t("settings.navAria")}>
          {SETTINGS_SECTIONS.map((section) => (
            <button
              key={section.id}
              type="button"
              className={`settings-nav-item${activeSection === section.id ? " active" : ""}`}
              onClick={() => scrollToSection(section.id)}
            >
              {t(section.labelKey)}
            </button>
          ))}
        </nav>

        <div className="settings-content" ref={contentRef} onScroll={handleSectionScroll}>
          <section id="settings-section-appearance" className="settings-section">
            <div className="settings-section-head">
              <h3>{t("settings.appearance.title")}</h3>
              <p>{t("settings.appearance.subtitle")}</p>
            </div>
            <div className="settings-section-body">
              <div className="settings-field-row">
                <span className="settings-field-label">{t("settings.appearance.colorMode")}</span>
                <div className="theme-segmented">
                  {(["auto", "light", "dark"] as const).map((theme) => (
                    <button
                      key={theme}
                      type="button"
                      className={`theme-seg-btn${(config.theme ?? "auto") === theme ? " active" : ""}`}
                      onClick={() => {
                        setConfig((prev) => ({ ...prev, theme }));
                        // Apply immediately so the switch is visible in real time.
                        onThemePreview?.(theme);
                      }}
                    >
                      {theme === "auto"
                        ? t("settings.appearance.themeAuto")
                        : theme === "light"
                          ? t("settings.appearance.themeLight")
                          : t("settings.appearance.themeDark")}
                    </button>
                  ))}
                </div>
              </div>

              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.appearance.language")}</span>
                <div className="theme-segmented">
                  {LOCALES.map((value) => (
                    <button
                      key={value}
                      type="button"
                      className={`theme-seg-btn${(config.language ?? locale) === value ? " active" : ""}`}
                      onClick={() => void handleLanguageChange(value)}
                    >
                      {LOCALE_LABELS[value]}
                    </button>
                  ))}
                </div>
                <small className="settings-field-hint">{t("settings.appearance.languageHint")}</small>
              </div>
            </div>
          </section>

          <section id="settings-section-workspace" className="settings-section">
            <div className="settings-section-head">
              <h3>{t("settings.workspace.title")}</h3>
              <p>{t("settings.workspace.subtitle")}</p>
            </div>
            <div className="settings-section-body">
              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.workspace.dirLabel")}</span>
                <div className="workspace-dir-row">
                  <input
                    type="text"
                    readOnly
                    value={config.workspace_dir ?? ""}
                    placeholder={workspaceDirActual || t("settings.workspace.defaultDir")}
                    title={config.workspace_dir || workspaceDirActual || t("settings.workspace.defaultDir")}
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
                    title={t("settings.workspace.browseTitle")}
                  >
                    {pickingWorkspace ? t("common.picking") : t("common.browse")}
                  </button>
                  {config.workspace_dir && (
                    <button
                      type="button"
                      className="settings-btn workspace-dir-clear"
                      onClick={handleClearWorkspace}
                      title={t("settings.workspace.resetTitle")}
                      aria-label={t("settings.workspace.resetAria")}
                    >
                      {t("settings.workspace.reset")}
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
                  <span>{t("settings.workspace.selfEvolution")}</span>
                  <small>{t("settings.workspace.selfEvolutionHint")}</small>
                </div>
              </label>

              <div className="settings-divider" />

              <div className="profile-section">
                <div className="profile-section-title">{t("settings.workspace.profilesTitle")}</div>
                <div className="profile-create-row">
                  <input
                    type="text"
                    value={newProfileName}
                    onChange={(e) => setNewProfileName(e.target.value)}
                    placeholder={t("settings.workspace.profileNamePlaceholder")}
                    onKeyDown={(e) => e.key === "Enter" && handleSaveProfile()}
                  />
                  <button
                    type="button"
                    className="settings-btn"
                    onClick={handleSaveProfile}
                    disabled={!newProfileName.trim() || profileSaving}
                  >
                    {profileSaving ? t("common.saving") : t("settings.workspace.saveCurrent")}
                  </button>
                </div>
                {profiles.length > 0 && (
                  <div className="profile-list">
                    {profiles.map((profile) => (
                      <div key={profile.name} className="profile-item">
                        <div className="profile-item-info">
                          <span className="profile-item-name">{profile.name}</span>
                          <span className="profile-item-meta">
                            {t("settings.workspace.profileMeta", {
                              skills: profile.selected_skills.length,
                              tools: profile.selected_tools.length,
                              agents: profile.agents.length,
                            })}
                          </span>
                        </div>
                        <div className="profile-item-actions">
                          <button
                            type="button"
                            className="settings-btn"
                            onClick={() => handleApplyProfile(profile.name)}
                            disabled={profileApplying !== null}
                            title={t("settings.workspace.applyTitle")}
                          >
                            {profileApplying === profile.name ? t("common.applying") : t("common.apply")}
                          </button>
                          <button
                            type="button"
                            className="settings-btn settings-btn-danger"
                            onClick={() => handleDeleteProfile(profile.name)}
                            aria-label={t("settings.workspace.deleteAria", { name: profile.name })}
                          >
                            <FontAwesomeIcon icon={faXmark} />
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
              <h3>{t("settings.api.title")}</h3>
              <p>{t("settings.api.subtitle")}</p>
            </div>
            <div className="settings-section-body">
              <label className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.api.baseUrl")}</span>
                <input
                  type="text"
                  value={config.api_base_url}
                  onChange={(e) => setConfig({ ...config, api_base_url: e.target.value })}
                  placeholder="https://api.openai.com/v1"
                />
              </label>

              <div className="settings-field-grid">
                <label className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">{t("settings.api.apiKey")}</span>
                  <input
                    type="password"
                    value={config.api_key}
                    onChange={(e) => setConfig({ ...config, api_key: e.target.value })}
                    placeholder="sk-..."
                  />
                </label>

                <label className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">{t("settings.api.model")}</span>
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
                  <span className="settings-field-label">{t("settings.api.modelCatalog")}</span>
                  <button type="button" className="settings-btn" onClick={handleFetchModels} disabled={loadingModels}>
                    {loadingModels ? t("settings.api.loadingModels") : t("settings.api.fetchModels")}
                  </button>
                  {modelsError && <div className="settings-error">{modelsError}</div>}
                  {metadataSummary && (
                    <small className="settings-field-hint">
                      {t("settings.api.capabilitiesSummary", {
                        matched: metadataSummary.matched,
                        total: metadataSummary.total,
                      })}
                    </small>
                  )}
                  {metadataError && (
                    <small className="settings-field-hint settings-warning">
                      {t("settings.api.capabilitiesError", { error: metadataError })}
                    </small>
                  )}
                </div>

                <div className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">{t("settings.api.addModelManually")}</span>
                  <div className="settings-inline-row">
                    <input
                      type="text"
                      value={manualModel}
                      onChange={(e) => setManualModel(e.target.value)}
                      placeholder={t("settings.api.addModelPlaceholder")}
                    />
                    <button
                      className="settings-btn"
                      onClick={handleAddManualModel}
                      type="button"
                      title={t("settings.api.addModelTitle")}
                    >
                      {t("common.add")}
                    </button>
                  </div>
                </div>
              </div>

              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">
                  {t("settings.api.contextWindow", { model: config.model })}
                </span>
                <input
                  type="number"
                  min={1}
                  step={1024}
                  value={config.model_context_lengths?.[config.model] ?? ""}
                  onChange={(e) => {
                    const v = e.target.value;
                    setConfig((prev) => {
                      const lengths = { ...(prev.model_context_lengths ?? {}) };
                      if (v === "") {
                        delete lengths[prev.model];
                      } else {
                        const n = Math.max(1, Math.floor(Number(v)));
                        if (Number.isFinite(n)) lengths[prev.model] = n;
                      }
                      return { ...prev, model_context_lengths: lengths };
                    });
                  }}
                  placeholder={t("settings.api.contextPlaceholder")}
                />
                <small className="settings-field-hint">
                  {t("settings.api.contextHint")}
                </small>
              </div>

              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.api.capabilities")}</span>
                {currentMetadata ? (
                  <div className="model-capabilities">
                    <div className="model-capability-badges">
                      {capabilityBadges.map((badge) => (
                        <span
                          key={badge.key}
                          className={`model-capability-badge${badge.key === "deprecated" ? " danger" : ""}`}
                        >
                          {badge.label}
                        </span>
                      ))}
                      {currentMetadata.context_length ? (
                        <span className="model-capability-badge muted">
                          {t("settings.api.capabilityContext", {
                            tokens: formatTokenCount(currentMetadata.context_length),
                          })}
                        </span>
                      ) : null}
                      {capabilityBadges.length === 0 && !currentMetadata.context_length && (
                        <span className="model-capability-badge muted">{t("common.none")}</span>
                      )}
                    </div>
                    <small className="settings-field-hint">
                      {t("settings.api.capabilitiesMatched", {
                        name: currentMetadata.model_name,
                        percent: Math.round((currentMetadata.match_score ?? 1) * 100),
                      })}
                      {currentMetadata.vendor ? ` · ${currentMetadata.vendor}` : ""}
                    </small>
                    {currentMetadata.description && (
                      <small className="settings-field-hint">{currentMetadata.description}</small>
                    )}
                  </div>
                ) : (
                  <small className="settings-field-hint">
                    {t("settings.api.capabilitiesEmpty", { model: config.model })}
                  </small>
                )}
              </div>
            </div>
          </section>

          <section id="settings-section-system" className="settings-section">
            <div className="settings-section-head">
              <h3>{t("settings.system.title")}</h3>
              <p>{t("settings.system.subtitle")}</p>
            </div>
            <div className="settings-section-body">
              <div className="settings-field-row settings-field-row-stacked">
                <div className="field-title-row">
                  <span className="settings-field-label">{t("settings.system.content")}</span>
                  <button type="button" className="inline-edit-btn" onClick={openMessageEditor}>
                    {t("common.edit")}
                  </button>
                </div>
                <textarea
                  rows={4}
                  value={config.system_message ?? ""}
                  onChange={(e) => setConfig({ ...config, system_message: e.target.value })}
                  placeholder={t("settings.system.placeholder")}
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
                <h3>{t("settings.advanced.title")}</h3>
                <p>{t("settings.advanced.subtitle")}</p>
              </div>
              <span className={`settings-toggle-icon ${advancedOpen ? "open" : ""}`}>
                <FontAwesomeIcon icon={faCaretDown} />
              </span>
            </div>

            {advancedOpen && (
              <div className="settings-section-body">
                <div className="settings-field-grid settings-field-grid-3">
                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">{t("settings.advanced.temperature")}</span>
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
                      placeholder={t("settings.advanced.placeholderDefault")}
                    />
                  </label>

                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">{t("settings.advanced.topP")}</span>
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
                      placeholder={t("settings.advanced.placeholderDefault")}
                    />
                  </label>

                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">{t("settings.advanced.maxCompletionTokens")}</span>
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
                      placeholder={t("settings.advanced.placeholderDefaultLimit")}
                    />
                  </label>
                </div>

                <label className="settings-checkbox">
                  <input
                    type="checkbox"
                    checked={config.ds_format ?? false}
                    onChange={(e) =>
                      setConfig((prev) => ({
                        ...prev,
                        ds_format: e.target.checked,
                      }))
                    }
                  />
                  <div className="settings-checkbox-copy">
                    <span>{t("settings.advanced.dsFormat")}</span>
                    <small>{t("settings.advanced.dsFormatHint")}</small>
                  </div>
                </label>
              </div>
            )}
          </section>

          <section id="settings-section-runtime" className="settings-section">
            <div className="settings-section-head">
              <h3>{t("settings.runtime.title")}</h3>
              <p>{t("settings.runtime.subtitle")}</p>
            </div>
            <div className="settings-section-body">
              <label className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.runtime.loggerOutput")}</span>
                <select
                  value={config.logger_output ?? "file"}
                  onChange={(e) =>
                    setConfig((prev) => ({
                      ...prev,
                      logger_output: e.target.value as "file" | "println",
                    }))
                  }
                >
                  <option value="file">{t("settings.runtime.loggerFile")}</option>
                  <option value="println">{t("settings.runtime.loggerPrintln")}</option>
                </select>
              </label>
              <label className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.runtime.retention")}</span>
                <input
                  type="number"
                  min={0}
                  max={3650}
                  step={1}
                  value={config.log_retention_days ?? 90}
                  onChange={(e) => {
                    const v = e.target.value;
                    setConfig((prev) => ({
                      ...prev,
                      log_retention_days: v === "" ? 90 : Math.max(0, Math.floor(Number(v)) || 0),
                    }));
                  }}
                />
                <small className="settings-field-hint">
                  {t("settings.runtime.retentionHint")}
                </small>
              </label>
              {sessionId && (
                <>
                  <div className="settings-field-row">
                    <span className="settings-field-label">{t("settings.runtime.interactionMonitor")}</span>
                    <button type="button" className="settings-btn" onClick={() => setShowMonitor(true)}>
                      {t("settings.runtime.launchMonitor")}
                    </button>
                  </div>
                  <div className="settings-field-row">
                    <span className="settings-field-label">{t("settings.runtime.agentMissions")}</span>
                    <button type="button" className="settings-btn" onClick={() => setShowAgentMonitor(true)}>
                      {t("settings.runtime.launchMonitor")}
                    </button>
                  </div>
                </>
              )}
            </div>
          </section>

          <section id="settings-section-about" className="settings-section">
            <div className="settings-section-head">
              <h3>{t("settings.about.title")}</h3>
              <p>{t("settings.about.subtitle")}</p>
            </div>
            <div className="settings-section-body">
              <div className="settings-field-row">
                <span className="settings-field-label">{t("settings.about.currentVersion")}</span>
                <div className="settings-inline-row">
                  <code>{appVersion || "—"}</code>
                </div>
              </div>

              <label className="settings-checkbox">
                <input
                  type="checkbox"
                  checked={config.check_updates_on_startup ?? true}
                  onChange={(e) =>
                    setConfig((prev) => ({
                      ...prev,
                      check_updates_on_startup: e.target.checked,
                    }))
                  }
                />
                <div className="settings-checkbox-copy">
                  <span>{t("settings.about.checkOnStartup")}</span>
                  <small>{t("settings.about.checkOnStartupHint")}</small>
                </div>
              </label>

              <label className="settings-checkbox">
                <input
                  type="checkbox"
                  checked={config.include_prerelease_updates ?? false}
                  onChange={(e) =>
                    setConfig((prev) => ({
                      ...prev,
                      include_prerelease_updates: e.target.checked,
                    }))
                  }
                />
                <div className="settings-checkbox-copy">
                  <span>{t("settings.about.prerelease")}</span>
                  <small>{t("settings.about.prereleaseHint")}</small>
                </div>
              </label>

              <div className="settings-field-row">
                <span className="settings-field-label">{t("settings.about.updates")}</span>
                <button type="button" className="settings-btn" onClick={() => setShowUpdatePanel(true)}>
                  {t("settings.about.checkNow")}
                </button>
              </div>
            </div>
          </section>
        </div>
      </div>

      {isMessageEditorOpen && (
        <Portal>
          <div className="prompt-editor-overlay" role="dialog" aria-modal="true" aria-label={t("settings.messageEditor.aria")}>
            <div className="prompt-editor-shell">
              <div className="prompt-editor-header">
                <h3>{t("settings.messageEditor.title")}</h3>
                <div className="prompt-editor-actions">
                  <button type="button" className="settings-btn" onClick={closeMessageEditor} disabled={messageSaving}>
                    {t("common.cancel")}
                  </button>
                  <button type="button" className="settings-btn settings-btn-primary" onClick={applyMessageEditor} disabled={messageSaving}>
                    {messageSaving ? t("common.saving") : t("common.done")}
                  </button>
                </div>
              </div>

              <div className="prompt-editor-body">
                <div className="prompt-column">
                  <span>{t("app.markdownColumn")}</span>
                  <textarea
                    className="prompt-editor-textarea"
                    value={messageDraft}
                    onChange={(e) => setMessageDraft(e.target.value)}
                    placeholder={t("settings.messageEditor.placeholder")}
                  />
                </div>

                <div className="prompt-column">
                  <span>{t("app.previewColumn")}</span>
                  <div className="prompt-preview">
                    {messageDraft.trim() ? (
                      <MarkdownPreview content={messageDraft} />
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

      {showMonitor && (
        <Portal>
          <MonitorPanel sessionId={sessionId!} onClose={() => setShowMonitor(false)} />
        </Portal>
      )}

      {showAgentMonitor && (
        <Portal>
          <div className="mission-monitor-overlay" role="dialog" aria-modal="true" aria-label={t("settings.agentMissionsMonitorAria")}>
            <div className="mission-monitor-modal">
              <AgentMissionPanel sessionId={sessionId!} onClose={() => setShowAgentMonitor(false)} />
            </div>
          </div>
        </Portal>
      )}

      {showUpdatePanel && (
        <Portal>
          <UpdatePanel onClose={() => setShowUpdatePanel(false)} />
        </Portal>
      )}
    </div>
  );
}