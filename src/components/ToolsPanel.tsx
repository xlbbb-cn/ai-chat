import { useState, useEffect } from "react";
import { getConfig, saveConfig } from "../api";
import type { AppConfig, AutoAcceptKind, ConfirmKind } from "../types";
import { useI18n, type MessageKey } from "../i18n";
import { FontAwesomeIcon, faAnglesLeft, faChevronDown, faChevronUp } from "../icons";
import "./ToolsPanel.css";

interface Props {
    onClose: () => void;
    onToolsChange: (tools: string[]) => void;
}

/** Tool ids with the message keys for their label / description. */
const AVAILABLE_TOOLS: { id: string; nameKey: MessageKey; descriptionKey: MessageKey }[] = [
    {
        id: "file_actions",
        nameKey: "tools.items.fileActions.name",
        descriptionKey: "tools.items.fileActions.description",
    },
    {
        id: "run_cmd",
        nameKey: "tools.items.runCmd.name",
        descriptionKey: "tools.items.runCmd.description",
    },
    {
        id: "run_shell",
        nameKey: "tools.items.runShell.name",
        descriptionKey: "tools.items.runShell.description",
    },
    {
        id: "calculator",
        nameKey: "tools.items.calculator.name",
        descriptionKey: "tools.items.calculator.description",
    },
    {
        id: "memory",
        nameKey: "tools.items.memory.name",
        descriptionKey: "tools.items.memory.description",
    },
    {
        id: "todo_list",
        nameKey: "tools.items.todoList.name",
        descriptionKey: "tools.items.todoList.description",
    },

    {
        id: "timer",
        nameKey: "tools.items.timer.name",
        descriptionKey: "tools.items.timer.description",
    },
];

const AVAILABLE_TOOL_IDS = new Set(AVAILABLE_TOOLS.map((t) => t.id));

/**
 * Tools that self-evolution mode owns. The assistant needs `file_actions` to
 * edit its own skills and `memory` to record what it learned, so while the
 * mode is on they cannot be switched off. The backend enforces the same
 * invariant (`AppConfig::effective_tools`), so this is only the UI mirror of
 * it — the switches stay visible and checked, just locked.
 */
const SELF_EVOLUTION_LOCKED_TOOLS = ["file_actions", "memory"];

/**
 * Tools a selected tool silently pulls in, mirrored from `TOOL_DEPENDENCIES`
 * in `src-tauri/src/lib.rs`. The calculator's tool description tells the model
 * to compute anything its expression grammar cannot express with `run_cmd`, so
 * enabling the calculator enables `run_cmd` with it — and `run_cmd` stays
 * locked while the calculator is on. The backend enforces the same invariant
 * on every config write, so this is only the UI mirror.
 */
const TOOL_DEPENDENCIES: Record<string, string> = { calculator: "run_cmd" };

/**
 * Confirmation kinds that can actually reach the approval dialog. The `kind`
 * strings are matched against `auto_accept_confirm_kinds` by the Rust side,
 * which sends them from `RiskLevel::confirm_kind()` in `src-tauri/src/tools/risk.rs`
 * (L0-L3 ask for approval, L4-L6 never prompt). Keep in sync.
 */
const AUTO_ACCEPT_OPTIONS: Array<{
    kind: ConfirmKind;
    labelKey: MessageKey;
    descriptionKey: MessageKey;
}> = [
        {
            kind: "dangerous",
            labelKey: "tools.autoAcceptItems.dangerous.label",
            descriptionKey: "tools.autoAcceptItems.dangerous.description",
        },
        {
            kind: "system_config",
            labelKey: "tools.autoAcceptItems.systemConfig.label",
            descriptionKey: "tools.autoAcceptItems.systemConfig.description",
        },
        {
            kind: "user_software",
            labelKey: "tools.autoAcceptItems.userSoftware.label",
            descriptionKey: "tools.autoAcceptItems.userSoftware.description",
        },
        {
            kind: "user_data",
            labelKey: "tools.autoAcceptItems.userData.label",
            descriptionKey: "tools.autoAcceptItems.userData.description",
        },
    ];

/**
 * Kinds that are preserved in `auto_accept_confirm_kinds` but are not offered
 * here: `cli` / `cloud` agents are approved per agent from the Sub Agents
 * panel (`require_approval`), so their kind has no Tools-panel switch.
 */
const HIDDEN_AUTO_ACCEPT_KINDS = ["external_agent"];

/** Every kind the panel can toggle individually. */
const ALL_AUTO_ACCEPT_KINDS: ConfirmKind[] = AUTO_ACCEPT_OPTIONS.map((option) => option.kind);

const AUTO_ACCEPT_KIND_SET = new Set<string>(AUTO_ACCEPT_OPTIONS.map((option) => option.kind));

/**
 * Wildcard stored in `auto_accept_confirm_kinds`: the Rust matcher treats it as
 * "any kind", so the one-click "auto-approve everything" switch also covers
 * kinds added in a future version instead of silently missing them.
 */
const AUTO_ACCEPT_ALL = "*";

/**
 * Drop values the backend would never emit (e.g. the retired `sudo` /
 * `elevation` kinds) so a stale config cannot keep dead entries around, while
 * preserving the `"*"` wildcard and the kinds hidden from this panel.
 */
function sanitizeAutoAcceptKinds(values: readonly string[] | undefined): AutoAcceptKind[] {
    return (values ?? []).filter(
        (value): value is AutoAcceptKind =>
            value === AUTO_ACCEPT_ALL ||
            AUTO_ACCEPT_KIND_SET.has(value) ||
            HIDDEN_AUTO_ACCEPT_KINDS.includes(value),
    );
}

export function ToolsPanel({ onClose, onToolsChange }: Props) {
    const { t } = useI18n();
    const [config, setConfig] = useState<AppConfig | null>(null);
    const [expandedTool, setExpandedTool] = useState<string | null>(null);
    const [autoAcceptExpanded, setAutoAcceptExpanded] = useState(true);
    // Raw text of the parallel-count field while it is being edited: clamping
    // on every keystroke would turn "delete, type 6" into 4 → 46 → 8.
    const [parallelCountDraft, setParallelCountDraft] = useState<string | null>(null);

    useEffect(() => {
        getConfig()
            .then((cfg) => {
                const selected = (cfg.selected_tools ?? []).filter((id) => AVAILABLE_TOOL_IDS.has(id));
                const sanitized = {
                    ...cfg,
                    selected_tools: selected,
                    auto_accept_confirm_kinds: sanitizeAutoAcceptKinds(cfg.auto_accept_confirm_kinds),
                };
                setConfig(sanitized);
                setAutoAcceptExpanded(false);
                onToolsChange(selected);
            })
            .catch(console.error);
    }, []);

    async function toggleTool(toolId: string) {
        if (!config) return;
        // Self-evolution mode owns these two — they stay on.
        if (config.self_evolution_mode && SELF_EVOLUTION_LOCKED_TOOLS.includes(toolId)) return;
        // A required tool cannot be switched off while the tool that needs it
        // is selected (the calculator keeps `run_cmd` on).
        const dependencyLocked = Object.entries(TOOL_DEPENDENCIES).some(
            ([tool, required]) => toolId === required && (config.selected_tools ?? []).includes(tool)
        );
        if (dependencyLocked) return;
        const current = config.selected_tools ?? [];
        let updated = current.includes(toolId)
            ? current.filter(t => t !== toolId)
            : [...current, toolId];

        // Enabling a tool with a dependency enables the requirement too: the
        // calculator's description advertises `run_cmd` as its fallback, so
        // the pair must reach the selection together.
        const required = TOOL_DEPENDENCIES[toolId];
        if (required && updated.includes(toolId) && !updated.includes(required)) {
            updated = [...updated, required];
        }

        const newConfig = { ...config, selected_tools: updated };
        setConfig(newConfig);
        await saveConfig(newConfig);
        onToolsChange(updated);
    }

    async function persistAutoAcceptKinds(kinds: AutoAcceptKind[]) {
        if (!config) return;
        const newConfig = { ...config, auto_accept_confirm_kinds: kinds };
        setConfig(newConfig);
        await saveConfig(newConfig);
    }

    /** Persist one footer setting (parallel tool calls) without touching the rest. */
    async function persistConfig(patch: Partial<AppConfig>) {
        if (!config) return;
        const newConfig = { ...config, ...patch };
        setConfig(newConfig);
        await saveConfig(newConfig);
    }

    /** Commit the count field: one write, clamped to 1–8. */
    async function commitParallelCount() {
        const raw = parallelCountDraft;
        setParallelCountDraft(null);
        if (raw === null) return;
        const parsed = parseInt(raw, 10);
        if (Number.isNaN(parsed)) return;
        const next = Math.min(8, Math.max(1, parsed));
        if (next === (config?.max_parallel_tools ?? 4)) return;
        await persistConfig({ max_parallel_tools: next });
    }

    async function toggleAutoAccept(kind: ConfirmKind) {
        if (!config) return;
        const current = sanitizeAutoAcceptKinds(config.auto_accept_confirm_kinds);
        // The wildcard means "every kind on": expand it to the explicit list
        // first, so unchecking a single kind keeps the remaining ones enabled.
        const base: AutoAcceptKind[] = current.includes(AUTO_ACCEPT_ALL)
            ? [...ALL_AUTO_ACCEPT_KINDS]
            : current;
        const updated = base.includes(kind)
            ? base.filter((value) => value !== kind)
            : [...base, kind];

        await persistAutoAcceptKinds(updated);
    }

    async function toggleAutoAcceptAll() {
        if (!config) return;
        const current = sanitizeAutoAcceptKinds(config.auto_accept_confirm_kinds);
        const allEnabled =
            current.includes(AUTO_ACCEPT_ALL) ||
            ALL_AUTO_ACCEPT_KINDS.every((kind) => current.includes(kind));

        await persistAutoAcceptKinds(allEnabled ? [] : [AUTO_ACCEPT_ALL]);
    }

    if (!config) {
        return <div className="tools-panel">{t("common.loading")}</div>;
    }

    const selectedTools = config.selected_tools ?? [];
    const selfEvolution = config.self_evolution_mode ?? false;
    const isSelfEvolutionLocked = (toolId: string) =>
        selfEvolution && SELF_EVOLUTION_LOCKED_TOOLS.includes(toolId);
    const isDependencyLocked = (toolId: string) =>
        Object.entries(TOOL_DEPENDENCIES).some(
            ([tool, required]) => toolId === required && selectedTools.includes(tool)
        );
    const isLockedTool = (toolId: string) =>
        isSelfEvolutionLocked(toolId) || isDependencyLocked(toolId);
    const lockReason = (toolId: string) =>
        isSelfEvolutionLocked(toolId)
            ? t("tools.lockedBySelfEvolution")
            : t("tools.lockedByDependency");
    const autoAcceptKinds = sanitizeAutoAcceptKinds(config.auto_accept_confirm_kinds);
    const autoAcceptAll = autoAcceptKinds.includes(AUTO_ACCEPT_ALL);
    const allAutoAcceptEnabled =
        autoAcceptAll || ALL_AUTO_ACCEPT_KINDS.every((kind) => autoAcceptKinds.includes(kind));
    const isAutoAcceptEnabled = (kind: ConfirmKind) => autoAcceptAll || autoAcceptKinds.includes(kind);

    return (
        <div className="tools-panel">
            <div className="panel-header">
                <h2>{t("tools.title")}</h2>
                <button className="close-btn" onClick={onClose} aria-label={t("common.collapse")}>
                    <FontAwesomeIcon icon={faAnglesLeft} />
                </button>
            </div>
            <div className="tools-description">
                {t("tools.builtIn")}
            </div>
            <div className="tools-list">
                {AVAILABLE_TOOLS.map(tool => {
                    const locked = isLockedTool(tool.id);
                    const enabled = selectedTools.includes(tool.id);
                    return (
                        <div
                            key={tool.id}
                            className={`tool-item ${!enabled ? "disabled" : ""}`}
                        >
                            <div className="tool-row">
                                <label
                                    className="toggle-switch"
                                    title={
                                        locked
                                            ? lockReason(tool.id)
                                            : enabled
                                                ? t("common.disable")
                                                : t("common.enable")
                                    }
                                >
                                    <input
                                        type="checkbox"
                                        checked={enabled}
                                        disabled={locked}
                                        onChange={() => toggleTool(tool.id)}
                                    />
                                    <span className="toggle-switch-slider" />
                                </label>
                                <div className="tool-info">
                                    <span
                                        className="tool-name"
                                        onClick={() => toggleTool(tool.id)}
                                    >
                                        {t(tool.nameKey)}
                                        {locked && (
                                            <span className="tool-locked">{t("tools.lockedBadge")}</span>
                                        )}
                                    </span>
                                    <span className="tool-transport">
                                        {t(tool.descriptionKey).split("\n")[0]}
                                    </span>
                                </div>
                                <div className="tool-actions">
                                    <button
                                        className="tool-action-btn"
                                        onClick={(e) => {
                                            e.stopPropagation();
                                            setExpandedTool(expandedTool === tool.id ? null : tool.id);
                                        }}
                                    >
                                        <FontAwesomeIcon
                                            icon={expandedTool === tool.id ? faChevronUp : faChevronDown}
                                        />
                                    </button>
                                </div>
                            </div>
                            {expandedTool === tool.id && (
                                <div className="tool-desc-expanded">
                                    {t(tool.descriptionKey)}
                                </div>
                            )}
                        </div>
                    );
                })}
            </div>

            <div className="tools-footer-section">
                <button
                    type="button"
                    className="section-toggle"
                    onClick={() => setAutoAcceptExpanded((expanded) => !expanded)}
                    aria-expanded={autoAcceptExpanded}
                >
                    <span>{t("tools.autoAccept")}</span>
                    <span className="section-toggle-chevron">
                        <FontAwesomeIcon icon={autoAcceptExpanded ? faChevronUp : faChevronDown} />
                    </span>
                </button>

                {autoAcceptExpanded && (
                    <div className="auto-accept-list">
                        <label
                            className="auto-accept-option auto-accept-option-all"
                            title={t("tools.autoAcceptItems.all.description")}
                        >
                            <input
                                className="auto-accept-checkbox"
                                type="checkbox"
                                checked={allAutoAcceptEnabled}
                                onChange={() => void toggleAutoAcceptAll()}
                            />
                            <div className="auto-accept-option-body">
                                <span className="auto-accept-name">{t("tools.autoAcceptItems.all.label")}</span>
                            </div>
                        </label>
                        {AUTO_ACCEPT_OPTIONS.map((option) => {
                            const checked = isAutoAcceptEnabled(option.kind);
                            return (
                                <label
                                    key={option.kind}
                                    className="auto-accept-option"
                                    title={t(option.descriptionKey)}
                                >
                                    <input
                                        className="auto-accept-checkbox"
                                        type="checkbox"
                                        checked={checked}
                                        onChange={() => void toggleAutoAccept(option.kind)}
                                    />
                                    <div className="auto-accept-option-body">
                                        <span className="auto-accept-name">{t(option.labelKey)}</span>
                                    </div>
                                </label>
                            );
                        })}
                    </div>
                )}
            </div>

            {/* Parallel tool calls live with the tools themselves: the switch is
                a property of how the tool layer runs, not of the app runtime. */}
            <div className="tools-footer-section tools-parallel-section">
                <label
                    className="tools-parallel-toggle"
                    title={t("tools.parallelToolsHint")}
                >
                    <input
                        className="auto-accept-checkbox"
                        type="checkbox"
                        checked={config.parallel_tools ?? true}
                        onChange={(e) =>
                            void persistConfig({ parallel_tools: e.target.checked })
                        }
                    />
                    <span className="tools-parallel-label">{t("tools.parallelTools")}</span>
                </label>
                {(config.parallel_tools ?? true) && (
                    <label className="tools-parallel-count" title={t("tools.parallelCountHint")}>
                        <span>{t("tools.parallelCount")}</span>
                        <input
                            type="number"
                            min={1}
                            max={8}
                            step={1}
                            value={parallelCountDraft ?? String(config.max_parallel_tools ?? 4)}
                            onChange={(e) => setParallelCountDraft(e.target.value)}
                            onBlur={() => void commitParallelCount()}
                            onKeyDown={(e) => {
                                if (e.key === "Enter") e.currentTarget.blur();
                            }}
                        />
                    </label>
                )}
            </div>
        </div>
    );
}
