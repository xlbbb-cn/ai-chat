import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import type { ModelMetadata } from "../types";
import { Portal } from "./Portal";
import "./ModelSelect.css";

interface Props {
    /** Model ids offered in the list (config `model_catalog`). */
    models: string[];
    /** Currently selected model id. */
    value: string;
    /** Capability metadata per model id, from the llm-metadata catalogue. */
    metadata: Record<string, ModelMetadata>;
    disabled?: boolean;
    onChange: (model: string) => void;
}

/**
 * Capability badges rendered inside every option. The labels reuse the
 * `settings.api.capability*` strings (Settings → API shows the same set) so the
 * two views can never drift apart.
 */
const CAPABILITY_CHIPS: { flag: keyof ModelMetadata; icon: string; labelKey: MessageKey }[] = [
    { flag: "supports_tools", icon: "🛠", labelKey: "settings.api.capabilityTools" },
    { flag: "supports_reasoning", icon: "🧠", labelKey: "settings.api.capabilityReasoning" },
    { flag: "supports_vision", icon: "👁", labelKey: "settings.api.capabilityVision" },
    { flag: "supports_files", icon: "📄", labelKey: "settings.api.capabilityFiles" },
    { flag: "supports_audio", icon: "🔊", labelKey: "settings.api.capabilityAudio" },
];

/** Badges for one model, in display order (empty when nothing is known). */
function chipsFor(metadata: ModelMetadata | undefined) {
    return metadata ? CAPABILITY_CHIPS.filter((chip) => metadata[chip.flag]) : [];
}

/** Popup height is capped, so long catalogues scroll instead of overflowing. */
const MAX_POPUP_HEIGHT = 320;
/**
 * Width of the popup when the trigger is narrower. Wide enough for a model id
 * plus four labelled capability badges on a single line.
 */
const MIN_POPUP_WIDTH = 380;
const TYPE_AHEAD_RESET_MS = 900;
/** Shortest the 2px scroll thumb may get, so long catalogues stay grabbable. */
const MIN_THUMB_PERCENT = 8;

/**
 * Model picker whose options carry the model's capability badges.
 *
 * A native `<select>` cannot render markup inside `<option>`, so this is a
 * small listbox on top of the existing `Portal`. The trigger keeps the look of
 * the previous `.model-select`, and the popup opens upwards — the input row
 * lives at the bottom of the window, where there is no room below.
 *
 * Keyboard: Enter/Space/Arrow opens, arrows move, Enter picks, Esc closes, and
 * typing jumps to the first matching model (parity with the native select).
 */
export function ModelSelect({ models, value, metadata, disabled, onChange }: Props) {
    const { t } = useI18n();
    const listId = useId();
    const [open, setOpen] = useState(false);
    const [highlight, setHighlight] = useState(-1);
    /** Fixed-position geometry of the popup, measured from the trigger. */
    const [anchor, setAnchor] = useState<{ left: number; bottom: number; width: number } | null>(null);
    const triggerRef = useRef<HTMLButtonElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const typeAheadRef = useRef({ buffer: "", at: 0 });
    /** Geometry of the 2px scroll indicator, in percent of the track. */
    const [scroll, setScroll] = useState({ thumb: 100, offset: 0 });

    /**
     * Native scrollbars are hidden and drawn in the DOM instead: WebKit on
     * macOS ignores `::-webkit-scrollbar`, so CSS alone cannot deliver the 2px
     * bar the design calls for. `thumb`/`offset` are percentages of the track.
     */
    const measureScroll = useCallback(() => {
        const el = listRef.current;
        if (!el) return;

        const { scrollHeight, clientHeight, scrollTop } = el;
        if (scrollHeight <= clientHeight + 1) {
            setScroll({ thumb: 100, offset: 0 });
            return;
        }

        const thumb = Math.max((clientHeight / scrollHeight) * 100, MIN_THUMB_PERCENT);
        const maxScroll = scrollHeight - clientHeight;
        setScroll({ thumb, offset: (scrollTop / maxScroll) * (100 - thumb) });
    }, []);

    const close = useCallback((refocus = false) => {
        setOpen(false);
        if (refocus) triggerRef.current?.focus();
    }, []);

    const openList = useCallback(
        (index: number) => {
            setHighlight(index >= 0 && index < models.length ? index : 0);
            setOpen(true);
        },
        [models.length],
    );

    const commit = useCallback(
        (index: number) => {
            const next = models[index];
            if (next !== undefined && next !== value) onChange(next);
            close(true);
        },
        [models, onChange, value, close],
    );

    // Keep the popup glued to the trigger: the input area can be resized, the
    // message list can scroll, and the window itself can move.
    useLayoutEffect(() => {
        if (!open) {
            setAnchor(null);
            return;
        }

        const place = () => {
            const rect = triggerRef.current?.getBoundingClientRect();
            if (!rect) return;
            const width = Math.min(
                Math.max(rect.width, MIN_POPUP_WIDTH),
                Math.max(window.innerWidth - 24, MIN_POPUP_WIDTH),
            );
            setAnchor({
                // Right-aligned with the trigger, so the wider popup grows
                // leftwards instead of drifting over the send button.
                left: Math.max(12, Math.min(rect.right - width, window.innerWidth - width - 12)),
                // The popup grows upwards from just above the trigger.
                bottom: window.innerHeight - rect.top + 8,
                width,
            });
        };

        place();
        window.addEventListener("resize", place);
        window.addEventListener("scroll", place, true);
        return () => {
            window.removeEventListener("resize", place);
            window.removeEventListener("scroll", place, true);
        };
    }, [open]);

    // Dismiss on outside click / Escape while open.
    useEffect(() => {
        if (!open) return;

        const onPointerDown = (event: PointerEvent) => {
            const target = event.target as Node;
            if (triggerRef.current?.contains(target) || listRef.current?.contains(target)) return;
            close();
        };
        const onKeyDown = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            // Esc belongs to the popup while it is open.
            event.stopPropagation();
            close(true);
        };

        document.addEventListener("pointerdown", onPointerDown, true);
        document.addEventListener("keydown", onKeyDown, true);
        return () => {
            document.removeEventListener("pointerdown", onPointerDown, true);
            document.removeEventListener("keydown", onKeyDown, true);
        };
    }, [open, close]);

    // Keep the highlighted option visible while arrowing through the list.
    useEffect(() => {
        if (!open || highlight < 0) return;
        listRef.current
            ?.querySelector<HTMLElement>(`[data-option-index="${highlight}"]`)
            ?.scrollIntoView({ block: "nearest" });
    }, [open, highlight]);

    // Re-measure the scroll indicator whenever the popup (re)opens or the
    // catalogue changes — a different model count means a different thumb.
    // `anchor` is part of the deps because the list only mounts one render
    // after `open` flips (the popup is rendered once its geometry is known).
    useEffect(() => {
        if (open && anchor) measureScroll();
    }, [open, anchor, models.length, measureScroll]);

    // A stream can start while the popup is open (e.g. the retry button) — the
    // picker is disabled then, so it must not stay floating over the chat.
    useEffect(() => {
        if (disabled && open) close();
    }, [disabled, open, close]);

    const move = useCallback(
        (delta: number) => {
            setHighlight((prev) => {
                if (models.length === 0) return -1;
                if (prev < 0) return delta > 0 ? 0 : models.length - 1;
                return (prev + delta + models.length) % models.length;
            });
        },
        [models.length],
    );

    /** Native selects jump to the first entry matching what you type. */
    const handleTypeAhead = useCallback(
        (key: string) => {
            const now = Date.now();
            const state = typeAheadRef.current;
            state.buffer = now - state.at > TYPE_AHEAD_RESET_MS ? key : state.buffer + key;
            state.at = now;

            const needle = state.buffer.toLowerCase();
            const index = models.findIndex((model) => model.toLowerCase().startsWith(needle));
            if (index >= 0) {
                if (open) setHighlight(index);
                else openList(index);
            }
        },
        [models, open, openList],
    );

    function handleTriggerKeyDown(event: React.KeyboardEvent<HTMLButtonElement>) {
        if (disabled) return;

        if (!open) {
            if (event.key === "Enter" || event.key === " " || event.key === "ArrowDown") {
                event.preventDefault();
                openList(models.indexOf(value));
                return;
            }
            if (event.key === "ArrowUp") {
                event.preventDefault();
                openList(models.length - 1);
                return;
            }
        } else {
            if (event.key === "ArrowDown") {
                event.preventDefault();
                move(1);
                return;
            }
            if (event.key === "ArrowUp") {
                event.preventDefault();
                move(-1);
                return;
            }
            if (event.key === "Home" || event.key === "End") {
                event.preventDefault();
                setHighlight(event.key === "Home" ? 0 : models.length - 1);
                return;
            }
            if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                commit(highlight);
                return;
            }
            if (event.key === "Tab") {
                close();
                return;
            }
        }

        if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
            handleTypeAhead(event.key);
        }
    }

    return (
        <>
            <button
                ref={triggerRef}
                type="button"
                className="model-select-trigger"
                title={t("app.selectModel")}
                aria-label={t("app.selectModel")}
                aria-haspopup="listbox"
                aria-expanded={open}
                aria-controls={open ? listId : undefined}
                aria-activedescendant={open && highlight >= 0 ? `${listId}-${highlight}` : undefined}
                disabled={disabled}
                onClick={() => (open ? close() : openList(models.indexOf(value)))}
                onKeyDown={handleTriggerKeyDown}
            >
                <span className="model-select-value">{value}</span>
                <span className="model-select-caret" aria-hidden="true">
                    ▾
                </span>
            </button>

            {open && anchor && (
                <Portal>
                    <div
                        className="model-select-popup-shell"
                        style={{
                            left: `${anchor.left}px`,
                            bottom: `${anchor.bottom}px`,
                            width: `${anchor.width}px`,
                            maxHeight: `${MAX_POPUP_HEIGHT}px`,
                        }}
                    >
                        <div
                            ref={listRef}
                            id={listId}
                            className="model-select-popup"
                            role="listbox"
                            aria-label={t("app.selectModel")}
                            onScroll={measureScroll}
                        >
                            {models.map((model, index) => {
                                const chips = chipsFor(metadata[model]);
                                return (
                                    <div
                                        key={model}
                                        id={`${listId}-${index}`}
                                        data-option-index={index}
                                        role="option"
                                        aria-selected={model === value}
                                        className={`model-select-option${index === highlight ? " highlighted" : ""}${model === value ? " selected" : ""
                                            }`}
                                        onPointerEnter={() => setHighlight(index)}
                                        onClick={() => commit(index)}
                                    >
                                        <span className="model-select-option-name">{model}</span>
                                        {chips.length > 0 && (
                                            <span className="model-select-option-chips">
                                                {chips.map((chip) => (
                                                    // Icon only — the capability name lives in the
                                                    // tooltip and the accessible label.
                                                    <span
                                                        key={chip.flag}
                                                        className="model-capability-chip"
                                                        role="img"
                                                        title={t(chip.labelKey)}
                                                        aria-label={t(chip.labelKey)}
                                                    >
                                                        {chip.icon}
                                                    </span>
                                                ))}
                                            </span>
                                        )}
                                    </div>
                                );
                            })}
                        </div>
                        {scroll.thumb < 100 && (
                            <div className="model-select-scrollbar" aria-hidden="true">
                                <div
                                    className="model-select-scrollbar-thumb"
                                    style={{ height: `${scroll.thumb}%`, top: `${scroll.offset}%` }}
                                />
                            </div>
                        )}
                    </div>
                </Portal>
            )}
        </>
    );
}
