import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { FontAwesomeIcon, type IconDefinition } from "../icons";
import { Portal } from "./Portal";

/**
 * One row of a dropdown list.
 *
 * `label` is what the row shows and — while it is a plain string — what the
 * trigger displays once the row is selected, so a field whose options are
 * their own value (a model id, a path) needs no extra prop.
 */
export interface DropdownOption {
    value: string;
    label: ReactNode;
    /** Right-hand content on the row: capability badges, counts, meta. */
    extra?: ReactNode;
    /** Tooltip on the row, and on the trigger when this row is selected. */
    title?: string;
    /** Plain text for type-ahead; defaults to `label` when that is a string. */
    text?: string;
    disabled?: boolean;
}

interface Props {
    options: DropdownOption[];
    value: string;
    onChange: (value: string) => void;
    /** Accessible name — the field label the control sits next to. */
    label: string;
    /** Trigger tooltip; defaults to `label`. */
    title?: string;
    /** Compact-zone sizing (13px): Settings and the composer row. */
    compact?: boolean;
    /** Leading glyph inside the trigger (the thinking-depth brain). */
    icon?: IconDefinition;
    disabled?: boolean;
    /** Shown by the trigger when `value` matches no option. */
    placeholder?: string;
    /** Extra class on the trigger, for width constraints at the call site. */
    className?: string;
    /** Floor for the popup width — the model picker needs room for badges. */
    minPopupWidth?: number;
    maxPopupHeight?: number;
    /** `auto` flips up when there is no room below; `up` pins it upwards. */
    placement?: "auto" | "up" | "down";
    /** Put a filter input at the top of the popup that narrows the list. */
    searchable?: boolean;
    /** Placeholder / accessible name of that filter input (localized by the caller). */
    searchPlaceholder?: string;
    /** Row shown while the filter matches nothing (localized by the caller). */
    emptyText?: string;
}

/** Gap between the trigger and the popup, and the popup's viewport margin. */
const GAP = 6;
const EDGE = 12;
const DEFAULT_MAX_POPUP_HEIGHT = 320;
const TYPE_AHEAD_RESET_MS = 900;
/** Shortest the 2px scroll thumb may get, so long lists stay grabbable. */
const MIN_THUMB_PERCENT = 8;
/** A popup never shrinks below this, even when the viewport is cramped. */
const MIN_VISIBLE_LIST = 120;

type Anchor = {
    left: number;
    width: number;
    maxHeight: number;
    top?: number;
    bottom?: number;
};

/**
 * The one dropdown in the app: a field-shaped trigger plus a listbox in the
 * shared `Portal`.
 *
 * Every native `<select>` was replaced by this, because the list a `<select>`
 * opens is drawn by the platform — GTK on Linux, a menu on macOS, Windows'
 * own popup — so "one design system" and "the OS decides how the list looks"
 * cannot both be true. The trigger keeps the field anatomy of `DESIGN.md`
 * (34px, 1px `--c-border-i`, 4px radius, `--c-field` fill, the `--icon-chevron`
 * glyph token, accent border on focus/open); the popup keeps the popup
 * anatomy (`--c-popup`, 4px padding, rows at `--c-accent-soft` when active).
 *
 * Keyboard contract is the native one: Enter/Space/ArrowDown opens, arrows and
 * Home/End move, Enter/Space picks, Esc closes, Tab closes, and typing jumps
 * to the first matching option.
 */
export function Dropdown({
    options,
    value,
    onChange,
    label,
    title,
    compact,
    icon,
    disabled,
    placeholder,
    className,
    minPopupWidth,
    maxPopupHeight = DEFAULT_MAX_POPUP_HEIGHT,
    placement = "auto",
    searchable = false,
    searchPlaceholder,
    emptyText,
}: Props) {
    const listId = useId();
    const [open, setOpen] = useState(false);
    const [highlight, setHighlight] = useState(-1);
    /** Fixed-position geometry of the popup, measured from the trigger. */
    const [anchor, setAnchor] = useState<Anchor | null>(null);
    const triggerRef = useRef<HTMLButtonElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const searchRef = useRef<HTMLInputElement>(null);
    /** Focus the filter once per opening, not on every geometry refresh. */
    const searchFocusedRef = useRef(false);
    const typeAheadRef = useRef({ buffer: "", at: 0 });
    /** Current filter text; reset whenever the popup closes. */
    const [query, setQuery] = useState("");
    /** Geometry of the 2px scroll indicator, in percent of the track. */
    const [scroll, setScroll] = useState({ thumb: 100, offset: 0 });

    const selectedIndex = options.findIndex((option) => option.value === value);
    const selected = selectedIndex >= 0 ? options[selectedIndex] : undefined;
    /** Text for type-ahead and filtering — the visible label when it is a string. */
    const textOf = useCallback(
        (option: DropdownOption) =>
            option.text ?? (typeof option.label === "string" ? option.label : option.value),
        [],
    );
    /** Options actually rendered — narrowed by the filter, when there is one. */
    const list = useMemo(() => {
        if (!searchable) return options;
        const needle = query.trim().toLowerCase();
        if (!needle) return options;
        return options.filter((option) => textOf(option).toLowerCase().includes(needle));
    }, [searchable, options, query, textOf]);

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
        // A filter is a one-shot view into the list, never a persisted setting:
        // reopening the popup starts from the full catalogue again.
        setQuery("");
        if (refocus) triggerRef.current?.focus();
    }, []);

    const openList = useCallback(
        (index: number) => {
            setHighlight(index >= 0 && index < list.length ? index : 0);
            setOpen(true);
        },
        [list.length],
    );

    const commit = useCallback(
        (index: number) => {
            const option = list[index];
            if (!option || option.disabled) return;
            if (option.value !== value) onChange(option.value);
            close(true);
        },
        [list, onChange, value, close],
    );

    // Keep the popup glued to the trigger: panels scroll, the window resizes
    // and the composer row can grow while the list is open.
    useLayoutEffect(() => {
        if (!open) {
            setAnchor(null);
            return;
        }

        const place = () => {
            const rect = triggerRef.current?.getBoundingClientRect();
            if (!rect) return;

            const width = Math.min(
                Math.max(rect.width, minPopupWidth ?? 0),
                Math.max(window.innerWidth - EDGE * 2, MIN_VISIBLE_LIST),
            );
            const roomBelow = window.innerHeight - rect.bottom - GAP - EDGE;
            const roomAbove = rect.top - GAP - EDGE;
            const up =
                placement === "up" ||
                (placement === "auto" && roomBelow < maxPopupHeight && roomAbove > roomBelow);
            const room = Math.max(up ? roomAbove : roomBelow, MIN_VISIBLE_LIST);
            const maxHeight = Math.min(maxPopupHeight, room);
            // Left-aligned with the trigger and clamped to the viewport, so a
            // popup that is wider than its trigger grows towards the room.
            const left = Math.max(EDGE, Math.min(rect.left, window.innerWidth - width - EDGE));

            setAnchor(
                up
                    ? { left, width, maxHeight, bottom: window.innerHeight - rect.top + GAP }
                    : { left, width, maxHeight, top: rect.bottom + GAP },
            );
        };

        place();
        window.addEventListener("resize", place);
        window.addEventListener("scroll", place, true);
        return () => {
            window.removeEventListener("resize", place);
            window.removeEventListener("scroll", place, true);
        };
    }, [open, minPopupWidth, maxPopupHeight, placement]);

    // Dismiss on outside pointer and on Escape, while the list is open.
    useEffect(() => {
        if (!open) return;

        const onPointerDown = (event: PointerEvent) => {
            const target = event.target as Node;
            if (
                triggerRef.current?.contains(target) ||
                listRef.current?.contains(target) ||
                searchRef.current?.contains(target)
            ) return;
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
    }, [open, highlight, list]);

    // Re-measure whenever the popup (re)opens or the option count changes —
    // `anchor` is a dependency because the list mounts one render after `open`
    // flips (the popup waits for its geometry).
    useEffect(() => {
        if (open && anchor) measureScroll();
    }, [open, anchor, list.length, measureScroll]);

    // A streaming turn disables the field — the popup must not linger over it.
    useEffect(() => {
        if (disabled && open) close();
    }, [disabled, open, close]);

    // The filter input mounts one render after `open` flips (the popup waits for
    // its geometry), so focus once both are ready and never steal focus again
    // while the user is typing.
    useEffect(() => {
        if (!open) {
            searchFocusedRef.current = false;
            return;
        }
        if (!searchable || searchFocusedRef.current || !anchor) return;
        searchFocusedRef.current = true;
        searchRef.current?.focus();
    }, [searchable, open, anchor]);

    // A changed filter means a different list: park the highlight on its first
    // row (or back on the selected row when the filter is cleared).
    useEffect(() => {
        if (searchable) setHighlight(query ? 0 : selectedIndex);
    }, [searchable, query, selectedIndex]);

    const move = useCallback(
        (delta: number) => {
            setHighlight((prev) => {
                if (list.length === 0) return -1;
                if (prev < 0) return delta > 0 ? 0 : list.length - 1;
                return (prev + delta + list.length) % list.length;
            });
        },
        [list.length],
    );

    /** Native selects jump to the first entry matching what you type. */
    const handleTypeAhead = useCallback(
        (key: string) => {
            const now = Date.now();
            const state = typeAheadRef.current;
            state.buffer = now - state.at > TYPE_AHEAD_RESET_MS ? key : state.buffer + key;
            state.at = now;

            const needle = state.buffer.toLowerCase();
            const index = list.findIndex((option) =>
                textOf(option).toLowerCase().startsWith(needle),
            );
            if (index >= 0) {
                if (open) setHighlight(index);
                else openList(index);
            }
        },
        [list, open, openList, textOf],
    );

    function handleTriggerKeyDown(event: React.KeyboardEvent<HTMLButtonElement>) {
        if (disabled) return;

        if (!open) {
            if (event.key === "Enter" || event.key === " " || event.key === "ArrowDown") {
                event.preventDefault();
                openList(selectedIndex);
                return;
            }
            if (event.key === "ArrowUp") {
                event.preventDefault();
                openList(list.length - 1);
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
                setHighlight(event.key === "Home" ? 0 : list.length - 1);
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
            if (searchable) {
                // Typing on the closed trigger opens the filter with that
                // character already in it; the input owns the keys from then on.
                event.preventDefault();
                setQuery(event.key);
                openList(0);
                return;
            }
            handleTypeAhead(event.key);
        }
    }

    /**
     * Keyboard contract while the filter input owns the focus. Escape is
     * deliberately absent: the document capture listener closes the popup and
     * refocuses the trigger, exactly as it does for every other dropdown.
     */
    function handleSearchKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
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
        if (event.key === "Home") {
            event.preventDefault();
            setHighlight(0);
            return;
        }
        if (event.key === "End") {
            event.preventDefault();
            setHighlight(list.length - 1);
            return;
        }
        if (event.key === "Enter") {
            event.preventDefault();
            commit(highlight);
            return;
        }
        if (event.key === "Tab") {
            close();
        }
    }

    const triggerClass = [
        "dropdown-trigger",
        compact ? "dropdown-trigger--compact" : "",
        icon ? "dropdown-trigger--icon" : "",
        className ?? "",
    ]
        .filter(Boolean)
        .join(" ");

    return (
        <>
            <button
                ref={triggerRef}
                type="button"
                className={triggerClass}
                title={title ?? (selected?.title ?? label)}
                aria-label={label}
                aria-haspopup="listbox"
                aria-expanded={open}
                aria-controls={open ? listId : undefined}
                aria-activedescendant={
                    open && highlight >= 0 && list.length > 0
                        ? `${listId}-${highlight}`
                        : undefined
                }
                disabled={disabled}
                onClick={() => (open ? close() : openList(selectedIndex))}
                onKeyDown={handleTriggerKeyDown}
            >
                {icon && (
                    <FontAwesomeIcon icon={icon} className="dropdown-icon" aria-hidden="true" />
                )}
                <span className="dropdown-value">
                    {selected ? selected.label : (placeholder ?? "")}
                </span>
                <span className="dropdown-chevron" aria-hidden="true" />
            </button>

            {open && anchor && (
                <Portal>
                    <div
                        className={`dropdown-popup${searchable ? " dropdown-popup--searchable" : ""}`}
                        style={{
                            left: `${anchor.left}px`,
                            width: `${anchor.width}px`,
                            maxHeight: `${anchor.maxHeight}px`,
                            ...(anchor.top !== undefined ? { top: `${anchor.top}px` } : {}),
                            ...(anchor.bottom !== undefined ? { bottom: `${anchor.bottom}px` } : {}),
                        }}
                    >
                        {searchable && (
                            <div className="dropdown-search">
                                <input
                                    ref={searchRef}
                                    type="text"
                                    className="dropdown-search-input"
                                    value={query}
                                    onChange={(event) => setQuery(event.target.value)}
                                    onKeyDown={handleSearchKeyDown}
                                    placeholder={searchPlaceholder ?? ""}
                                    aria-label={searchPlaceholder ?? label}
                                    aria-controls={listId}
                                    aria-activedescendant={
                                        highlight >= 0 && list.length > 0
                                            ? `${listId}-${highlight}`
                                            : undefined
                                    }
                                    autoComplete="off"
                                    spellCheck={false}
                                />
                            </div>
                        )}
                        <div
                            ref={listRef}
                            id={listId}
                            className="dropdown-list"
                            role="listbox"
                            aria-label={label}
                            onScroll={measureScroll}
                        >
                            {list.map((option, index) => (
                                <div
                                    key={option.value}
                                    id={`${listId}-${index}`}
                                    data-option-index={index}
                                    role="option"
                                    aria-selected={option.value === value}
                                    aria-disabled={option.disabled || undefined}
                                    className={[
                                        "dropdown-option",
                                        option.extra ? "dropdown-option--rich" : "",
                                        index === highlight ? "active" : "",
                                        option.value === value ? "selected" : "",
                                    ]
                                        .filter(Boolean)
                                        .join(" ")}
                                    title={option.title}
                                    onPointerEnter={() => setHighlight(index)}
                                    onClick={() => commit(index)}
                                >
                                    <span className="dropdown-option-label">{option.label}</span>
                                    {option.extra && (
                                        <span className="dropdown-option-extra">{option.extra}</span>
                                    )}
                                </div>
                            ))}
                            {searchable && list.length === 0 && (
                                <div className="dropdown-empty" role="presentation">
                                    {emptyText}
                                </div>
                            )}
                        </div>
                        {scroll.thumb < 100 && (
                            <div className="dropdown-scrollbar" aria-hidden="true">
                                <div
                                    className="dropdown-scrollbar-thumb"
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
