import { useState, useEffect, useRef } from "react";
import { fetchModels, fetchModelMetadata, getAppVersion, getConfig, saveConfig } from "../api";
import type { AppConfig, GlassEffect, ModelMetadata, ModelSettings, Palette, RequestHeader } from "../types";
import { LOCALE_LABELS, LOCALES, useI18n, type Locale, type MessageKey } from "../i18n";
import { matchModelCatalog, isDeepSeekModel } from "../utils/modelMetadata";
import { FontAwesomeIcon, faCaretDown, faCheck, faSpinner, faXmark } from "../icons";
import { MarkdownPreview } from "./MarkdownPreview";
import { MonitorPanel } from "./MonitorPanel";
import { AgentMissionPanel } from "./AgentMissionPanel";
import { MemoryPanel } from "./MemoryPanel";
import { Portal } from "./Portal";
import { Dropdown } from "./Dropdown";
import { UpdatePanel } from "./UpdatePanel";
import "./SettingsPanel.css";

interface Props {
  onClose: () => void;
  onConfigSaved?: (config: AppConfig) => void;
  onThemePreview?: (theme: "auto" | "light" | "dark" | undefined) => void;
  onPalettePreview?: (palette: Palette) => void;
  onGlassPreview?: (glass: GlassEffect) => void;
  sessionId?: string;
  /** A turn is streaming — the interaction monitor waits for it to finish. */
  turnActive?: boolean;
}

/**
 * Colour schemes offered next to the light / dark mode. Each one ships a
 * light and a dark token set — the swatch shows its light variant.
 */
const PALETTES: { id: Palette; labelKey: MessageKey }[] = [
  { id: "default", labelKey: "settings.appearance.paletteDefault" },
  { id: "sakura", labelKey: "settings.appearance.paletteSakura" },
  { id: "cream", labelKey: "settings.appearance.paletteCream" },
];

/**
 * Frosted-glass strengths. `standard` is the design's own blur (20px), so it
 * is what the row shows when the config carries no value; the block that the
 * attribute selects lives in `src/App.css` (`[data-glass="..."]`).
 */
const GLASS_LEVELS: { id: GlassEffect; labelKey: MessageKey }[] = [
  { id: "off", labelKey: "settings.appearance.glassOff" },
  { id: "subtle", labelKey: "settings.appearance.glassSubtle" },
  { id: "standard", labelKey: "settings.appearance.glassStandard" },
  { id: "strong", labelKey: "settings.appearance.glassStrong" },
];

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

/**
 * Tools that self-evolution mode owns. The assistant needs `file_actions` to
 * edit its own skills and `memory` to record what it learned, so enabling the
 * mode switches them on and they cannot be switched off again while it is on.
 * The backend enforces the same invariant (`AppConfig::effective_tools`).
 */
const SELF_EVOLUTION_REQUIRED_TOOLS = ["file_actions", "memory"];

/**
 * Apply the self-evolution invariant to a tool selection: the two required
 * tools are added when the mode is on. Turning the mode off leaves the
 * selection alone — the tools simply become optional again.
 */
function withSelfEvolutionTools(
  tools: string[] | undefined,
  selfEvolution: boolean
): string[] {
  const current = tools ?? [];
  if (!selfEvolution) return current;
  const next = [...current];
  for (const tool of SELF_EVOLUTION_REQUIRED_TOOLS) {
    if (!next.includes(tool)) next.push(tool);
  }
  return next;
}

function updateModelSettings(
  settings: ModelSettings | undefined,
  patch: Partial<ModelSettings>
): ModelSettings {
  return { ...(settings ?? {}), ...patch };
}

/**
 * Header rows are edited as a list; blank rows are dropped and names trimmed
 * on save, so a half-typed entry never reaches the backend.
 */
function normalizeCustomHeaders(headers: RequestHeader[] | undefined): RequestHeader[] {
  return (headers ?? [])
    .map((header) => ({ name: header.name.trim(), value: header.value }))
    .filter((header) => header.name.length > 0);
}

/** A blank conversation-id header name means "use the default". */
function normalizeConversationHeader(name: string | undefined): string | undefined {
  const trimmed = name?.trim();
  return trimmed ? trimmed : undefined;
}

/**
 * Turn-guard fields (rounds / tool calls / failure limit) are whole numbers
 * where `0` means "no limit". An empty box clears the override so the backend
 * default applies; anything unparsable is treated the same way.
 */
function parseLimitInput(value: string): number | undefined {
  if (value.trim() === "") return undefined;
  const parsed = Math.floor(Number(value));
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : undefined;
}

const SETTINGS_SECTIONS: { id: string; labelKey: MessageKey }[] = [
  { id: "appearance", labelKey: "settings.sections.appearance" },
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

export function SettingsPanel({ onClose, onConfigSaved, onThemePreview, onPalettePreview, onGlassPreview, sessionId, turnActive = false }: Props) {
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
  const [isMessageEditorOpen, setIsMessageEditorOpen] = useState(false);
  const [messageDraft, setMessageDraft] = useState("");
  const [messageSaving, setMessageSaving] = useState(false);
  const [showMonitor, setShowMonitor] = useState(false);
  const [showAgentMonitor, setShowAgentMonitor] = useState(false);
  const [showMemoryMonitor, setShowMemoryMonitor] = useState(false);
  const [showUpdatePanel, setShowUpdatePanel] = useState(false);
  const [appVersion, setAppVersion] = useState("");
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
  /**
   * DS-Format is forced on for DeepSeek-family models by the backend
   * (`AppConfig::ds_format_for`), so the switch mirrors that state instead of
   * pretending the user can turn it off.
   */
  const dsFormatAuto = isDeepSeekModel(config.model, currentMetadata);

  function addCustomHeader() {
    setConfig((prev) => ({
      ...prev,
      custom_headers: [...(prev.custom_headers ?? []), { name: "", value: "" }],
    }));
  }

  function updateCustomHeader(index: number, patch: Partial<RequestHeader>) {
    setConfig((prev) => {
      const headers = [...(prev.custom_headers ?? [])];
      if (!headers[index]) return prev;
      headers[index] = { ...headers[index], ...patch };
      return { ...prev, custom_headers: headers };
    });
  }

  function removeCustomHeader(index: number) {
    setConfig((prev) => ({
      ...prev,
      custom_headers: (prev.custom_headers ?? []).filter((_, i) => i !== index),
    }));
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
        // Fetch with what the form holds, not what was last saved: an API key
        // can be tried (and its model list listed) before "Save Changes".
        fetchModels({
          apiBaseUrl: config.api_base_url,
          apiKey: config.api_key,
          customHeaders: normalizeCustomHeaders(config.custom_headers),
        }),
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
      // `selected_tools` is owned by the tools panel and `workspace_dir` by the
      // titlebar switcher — neither belongs to this form. Re-read both before
      // saving so that toggling a tool or switching workspace while this panel
      // was open is never rolled back by "Save Changes".
      const persisted = await getConfig();
      const normalized: AppConfig = {
        ...config,
        workspace_dir: persisted.workspace_dir,
        conversation_id_header: normalizeConversationHeader(config.conversation_id_header),
        custom_headers: normalizeCustomHeaders(config.custom_headers),
        model_catalog: modelCatalog,
        selected_tools: withSelfEvolutionTools(
          persisted.selected_tools ?? [],
          config.self_evolution_mode ?? false
        ),
      };
      await saveConfig(normalized);
      setConfig(normalized);
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
        // Owned by the titlebar switcher — never roll a switch back.
        workspace_dir: persisted.workspace_dir,
        conversation_id_header: normalizeConversationHeader(config.conversation_id_header),
        custom_headers: normalizeCustomHeaders(config.custom_headers),
        // Owned by the tools panel — never roll those back.
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
    setMessageSaving(true);
    try {
      const persisted = await getConfig();
      const normalized: AppConfig = {
        ...config,
        // Owned by the titlebar switcher — never roll a switch back.
        workspace_dir: persisted.workspace_dir,
        conversation_id_header: normalizeConversationHeader(config.conversation_id_header),
        custom_headers: normalizeCustomHeaders(config.custom_headers),
        system_message: messageDraft,
        model_catalog: modelCatalog,
      };
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
                <span className="settings-field-label">{t("settings.appearance.palette")}</span>
                <div className="palette-grid" role="radiogroup" aria-label={t("settings.appearance.palette")}>
                  {PALETTES.map((palette) => {
                    const active = (config.palette ?? "default") === palette.id;
                    return (
                      <button
                        key={palette.id}
                        type="button"
                        role="radio"
                        aria-checked={active}
                        className={`palette-option${active ? " active" : ""}`}
                        onClick={() => {
                          setConfig((prev) => ({ ...prev, palette: palette.id }));
                          // Apply immediately so the switch is visible in real time.
                          onPalettePreview?.(palette.id);
                        }}
                      >
                        <span className={`palette-swatch palette-swatch-${palette.id}`} aria-hidden="true">
                          <span className="palette-chip" />
                          <span className="palette-chip" />
                          <span className="palette-chip" />
                          <span className="palette-chip" />
                        </span>
                        <span className="palette-option-label">{t(palette.labelKey)}</span>
                        {active && <FontAwesomeIcon icon={faCheck} className="palette-option-check" />}
                      </button>
                    );
                  })}
                </div>
                <small className="settings-field-hint">{t("settings.appearance.paletteHint")}</small>
              </div>

              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.appearance.glass")}</span>
                <div className="theme-segmented">
                  {GLASS_LEVELS.map((level) => (
                    <button
                      key={level.id}
                      type="button"
                      className={`theme-seg-btn${(config.glass_effect ?? "standard") === level.id ? " active" : ""}`}
                      onClick={() => {
                        setConfig((prev) => ({ ...prev, glass_effect: level.id }));
                        // Apply immediately so the blur change is visible in real time.
                        onGlassPreview?.(level.id);
                      }}
                    >
                      {t(level.labelKey)}
                    </button>
                  ))}
                </div>
                <small className="settings-field-hint">{t("settings.appearance.glassHint")}</small>
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
                  <Dropdown
                    label={t("settings.api.model")}
                    compact
                    value={config.model}
                    onChange={(model) => setConfig({ ...config, model })}
                    options={modelCatalog.map((name) => ({ value: name, label: name }))}
                  />
                </label>
              </div>

              <div className="settings-field-grid">
                <div className="settings-field-row settings-field-row-stacked">
                  <span className="settings-field-label">{t("settings.api.modelCatalog")}</span>
                  <button
                    type="button"
                    className="settings-btn settings-btn-tinted"
                    onClick={handleFetchModels}
                    disabled={loadingModels}
                    aria-busy={loadingModels}
                  >
                    {loadingModels && <FontAwesomeIcon icon={faSpinner} spin aria-hidden="true" />}
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
                      {currentMetadata.max_output_tokens ? (
                        <span className="model-capability-badge muted">
                          {t("settings.api.capabilityMaxOutput", {
                            tokens: formatTokenCount(currentMetadata.max_output_tokens),
                          })}
                        </span>
                      ) : null}
                      {capabilityBadges.length === 0 &&
                        !currentMetadata.context_length &&
                        !currentMetadata.max_output_tokens && (
                          <span className="model-capability-badge muted">{t("common.none")}</span>
                        )}
                    </div>
                    <small className="settings-field-hint">
                      {t("settings.api.capabilitiesMatched", {
                        name: currentMetadata.model_name,
                        percent: Math.round((currentMetadata.match_score ?? 1) * 100),
                      })}
                      {currentMetadata.vendor ? ` · ${currentMetadata.vendor}` : ""}
                      {currentMetadata.sources?.length
                        ? ` · ${currentMetadata.sources.join(", ")}`
                        : ""}
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

              <div className="settings-divider" />

              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.api.conversationHeader")}</span>
                <input
                  type="text"
                  value={config.conversation_id_header ?? ""}
                  onChange={(e) =>
                    setConfig((prev) => ({ ...prev, conversation_id_header: e.target.value }))
                  }
                  placeholder="x-conversation-id"
                />
                <small className="settings-field-hint">
                  {t("settings.api.conversationHeaderHint")}
                </small>
              </div>

              <div className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.api.customHeaders")}</span>
                <small className="settings-field-hint">{t("settings.api.customHeadersHint")}</small>
                {(config.custom_headers ?? []).length > 0 && (
                  <div className="settings-header-list">
                    {(config.custom_headers ?? []).map((header, index) => (
                      <div key={index} className="settings-header-row">
                        <input
                          type="text"
                          value={header.name}
                          placeholder={t("settings.api.customHeaderName")}
                          aria-label={t("settings.api.customHeaderName")}
                          onChange={(e) => updateCustomHeader(index, { name: e.target.value })}
                        />
                        <input
                          type="text"
                          value={header.value}
                          placeholder={t("settings.api.customHeaderValue")}
                          aria-label={t("settings.api.customHeaderValue")}
                          onChange={(e) => updateCustomHeader(index, { value: e.target.value })}
                        />
                        <button
                          type="button"
                          className="settings-btn settings-btn-danger"
                          onClick={() => removeCustomHeader(index)}
                          title={t("settings.api.removeHeader")}
                          aria-label={t("settings.api.removeHeader")}
                        >
                          <FontAwesomeIcon icon={faXmark} />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                <div>
                  <button type="button" className="settings-btn" onClick={addCustomHeader}>
                    {t("settings.api.addHeader")}
                  </button>
                </div>
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

                <div className="settings-divider" />

                <div className="settings-field-grid settings-field-grid-3">
                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">{t("settings.advanced.maxRounds")}</span>
                    <input
                      type="number"
                      min={0}
                      step={1}
                      value={config.main_agent_max_rounds ?? ""}
                      onChange={(e) =>
                        setConfig((prev) => ({
                          ...prev,
                          main_agent_max_rounds: parseLimitInput(e.target.value),
                        }))
                      }
                      placeholder="0"
                    />
                    <small className="settings-field-hint">
                      {t("settings.advanced.maxRoundsHint")}
                    </small>
                  </label>

                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">{t("settings.advanced.maxToolCalls")}</span>
                    <input
                      type="number"
                      min={0}
                      step={1}
                      value={config.main_agent_max_tool_calls ?? ""}
                      onChange={(e) =>
                        setConfig((prev) => ({
                          ...prev,
                          main_agent_max_tool_calls: parseLimitInput(e.target.value),
                        }))
                      }
                      placeholder="0"
                    />
                    <small className="settings-field-hint">
                      {t("settings.advanced.maxToolCallsHint")}
                    </small>
                  </label>

                  <label className="settings-field-row settings-field-row-stacked">
                    <span className="settings-field-label">{t("settings.advanced.toolFailureLimit")}</span>
                    <input
                      type="number"
                      min={0}
                      step={1}
                      value={config.tool_failure_limit ?? ""}
                      onChange={(e) =>
                        setConfig((prev) => ({
                          ...prev,
                          tool_failure_limit: parseLimitInput(e.target.value),
                        }))
                      }
                      placeholder="0"
                    />
                    <small className="settings-field-hint">
                      {t("settings.advanced.toolFailureLimitHint")}
                    </small>
                  </label>
                </div>

                <label className="settings-checkbox">
                  <input
                    type="checkbox"
                    checked={dsFormatAuto || (config.ds_format ?? false)}
                    disabled={dsFormatAuto}
                    onChange={(e) =>
                      setConfig((prev) => ({
                        ...prev,
                        ds_format: e.target.checked,
                      }))
                    }
                  />
                  <div className="settings-checkbox-copy">
                    <span>{t("settings.advanced.dsFormat")}</span>
                    <small>
                      {dsFormatAuto
                        ? t("settings.advanced.dsFormatAuto", { model: config.model })
                        : t("settings.advanced.dsFormatHint")}
                    </small>
                  </div>
                </label>

                <label className="settings-checkbox">
                  <input
                    type="checkbox"
                    checked={config.enhanced_figures ?? true}
                    onChange={(e) =>
                      setConfig((prev) => ({
                        ...prev,
                        enhanced_figures: e.target.checked,
                      }))
                    }
                  />
                  <div className="settings-checkbox-copy">
                    <span>{t("settings.advanced.enhancedFigures")}</span>
                    <small>{t("settings.advanced.enhancedFiguresHint")}</small>
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
              <label className="settings-checkbox">
                <input
                  type="checkbox"
                  checked={config.background_tools ?? true}
                  onChange={(e) =>
                    setConfig((prev) => ({
                      ...prev,
                      background_tools: e.target.checked,
                    }))
                  }
                />
                <div className="settings-checkbox-copy">
                  <span>{t("settings.runtime.backgroundTools")}</span>
                  <small>{t("settings.runtime.backgroundToolsHint")}</small>
                </div>
              </label>
              <label className="settings-checkbox">
                <input
                  type="checkbox"
                  checked={config.sub_agent_main_help ?? true}
                  onChange={(e) =>
                    setConfig((prev) => ({
                      ...prev,
                      sub_agent_main_help: e.target.checked,
                    }))
                  }
                />
                <div className="settings-checkbox-copy">
                  <span>{t("settings.runtime.subAgentMainHelp")}</span>
                  <small>{t("settings.runtime.subAgentMainHelpHint")}</small>
                </div>
              </label>
              <label className="settings-checkbox">
                <input
                  type="checkbox"
                  checked={config.single_instance ?? true}
                  onChange={(e) =>
                    setConfig((prev) => ({
                      ...prev,
                      single_instance: e.target.checked,
                    }))
                  }
                />
                <div className="settings-checkbox-copy">
                  <span>{t("settings.runtime.singleInstance")}</span>
                  <small>{t("settings.runtime.singleInstanceHint")}</small>
                </div>
              </label>

              <div className="settings-divider" />

              <label className="settings-field-row settings-field-row-stacked">
                <span className="settings-field-label">{t("settings.runtime.loggerOutput")}</span>
                <Dropdown
                  label={t("settings.runtime.loggerOutput")}
                  compact
                  value={config.logger_output ?? "file"}
                  onChange={(logger_output) =>
                    setConfig((prev) => ({
                      ...prev,
                      logger_output: logger_output as "file" | "println",
                    }))
                  }
                  options={[
                    { value: "file", label: t("settings.runtime.loggerFile") },
                    { value: "println", label: t("settings.runtime.loggerPrintln") },
                  ]}
                />
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

              <div className="settings-divider" />

              <label className="settings-checkbox">
                <input
                  type="checkbox"
                  checked={config.self_evolution_mode ?? false}
                  onChange={(e) =>
                    setConfig((prev) => {
                      const enabled = e.target.checked;
                      return {
                        ...prev,
                        self_evolution_mode: enabled,
                        selected_tools: withSelfEvolutionTools(
                          prev.selected_tools,
                          enabled
                        ),
                      };
                    })
                  }
                />
                <div className="settings-checkbox-copy">
                  <span>{t("settings.runtime.selfEvolution")}</span>
                  <small>{t("settings.runtime.selfEvolutionHint")}</small>
                </div>
              </label>

              {(sessionId || config.self_evolution_mode) && <div className="settings-divider" />}

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
              {config.self_evolution_mode && (
                <div className="settings-field-row">
                  <span className="settings-field-label">{t("settings.runtime.evolutionMemory")}</span>
                  <button type="button" className="settings-btn" onClick={() => setShowMemoryMonitor(true)}>
                    {t("settings.runtime.launchMonitor")}
                  </button>
                </div>
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
          <MonitorPanel sessionId={sessionId!} busy={turnActive} onClose={() => setShowMonitor(false)} />
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

      {showMemoryMonitor && (
        <Portal>
          <div className="mission-monitor-overlay" role="dialog" aria-modal="true" aria-label={t("settings.evolutionMemoryMonitorAria")}>
            <div className="mission-monitor-modal memory-monitor-modal">
              <MemoryPanel onClose={() => setShowMemoryMonitor(false)} />
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
