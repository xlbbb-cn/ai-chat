/**
 * Minimal, dependency-free i18n layer for the frontend.
 *
 * - `I18nProvider` (wired in `main.tsx`) holds the active locale and re-renders
 *   the tree when it changes.
 * - Components read `{ t, locale }` from `useI18n()`.
 * - Code that runs outside React (e.g. the markdown-it code-block rule) can use
 *   the module-level `t()`, which always reflects the active locale.
 *
 * Locales: English, Simplified Chinese (`zh-CN`) and Traditional Chinese
 * (`zh-TW`). Missing keys fall back to English, and finally to the key itself.
 */
import {
    createContext,
    useCallback,
    useContext,
    useEffect,
    useMemo,
    useState,
    type ReactNode,
} from "react";
import en, { type MessageKey, type Messages, type PluralValue } from "./messages/en";
import zhCN from "./messages/zh-CN";
import zhTW from "./messages/zh-TW";

export type { MessageKey, Messages, PluralValue };

export type Locale = "en" | "zh-CN" | "zh-TW";

/** Locale order used by the language picker. */
export const LOCALES: readonly Locale[] = ["en", "zh-CN", "zh-TW"];

/** Labels are always shown in their own language, never translated. */
export const LOCALE_LABELS: Record<Locale, string> = {
    en: "English",
    "zh-CN": "简体中文",
    "zh-TW": "繁體中文",
};

export const FALLBACK_LOCALE: Locale = "en";

const DICTS: Record<Locale, Messages> = {
    en,
    "zh-CN": zhCN,
    "zh-TW": zhTW,
};

/** Cache so the first paint already uses the last chosen language. */
const STORAGE_KEY = "ai-chat.locale";

export type TranslateVars = Record<string, string | number | null | undefined>;

export function isLocale(value: unknown): value is Locale {
    return typeof value === "string" && (LOCALES as readonly string[]).includes(value);
}

/** Best-effort match of the OS/browser language to a bundled locale. */
export function detectSystemLocale(): Locale {
    const candidates =
        typeof navigator === "undefined"
            ? []
            : [navigator.language, ...(navigator.languages ?? [])];
    for (const candidate of candidates) {
        const tag = candidate?.toLowerCase();
        if (!tag) continue;
        if (tag.startsWith("zh")) {
            // Traditional is used in Taiwan / Hong Kong / Macau.
            return /hant|tw|hk|mo/.test(tag) ? "zh-TW" : "zh-CN";
        }
        if (tag.startsWith("en")) return "en";
    }
    return FALLBACK_LOCALE;
}

function readCachedLocale(): Locale {
    try {
        const raw = window.localStorage.getItem(STORAGE_KEY);
        if (isLocale(raw)) return raw;
    } catch {
        /* storage can be unavailable (private mode) — fall through */
    }
    return detectSystemLocale();
}

function lookup(dict: Messages, path: string): string | PluralValue | undefined {
    let node: unknown = dict;
    for (const part of path.split(".")) {
        if (!node || typeof node !== "object") return undefined;
        node = (node as Record<string, unknown>)[part];
    }
    if (typeof node === "string") return node;
    if (node && typeof node === "object" && "one" in node && "other" in node) {
        return node as PluralValue;
    }
    return undefined;
}

function interpolate(template: string, vars?: TranslateVars): string {
    if (!vars) return template;
    return template.replace(/\{(\w+)\}/g, (match, name: string) => {
        const value = vars[name];
        return value === undefined || value === null ? match : String(value);
    });
}

/** Resolve one message key for a locale. Falls back to English, then to the key. */
export function translate(locale: Locale, key: MessageKey, vars?: TranslateVars): string {
    const dict = DICTS[locale] ?? DICTS[FALLBACK_LOCALE];
    const node = lookup(dict, key) ?? lookup(DICTS[FALLBACK_LOCALE], key);
    const template =
        typeof node === "string"
            ? node
            : node
                ? Number(vars?.count) === 1
                    ? node.one
                    : node.other
                : undefined;
    return template === undefined ? key : interpolate(template, vars);
}

// ─── Non-React access ────────────────────────────────────────────────────────

let activeLocale: Locale = readCachedLocale();

export function getActiveLocale(): Locale {
    return activeLocale;
}

/**
 * Translate without a hook. Use only where a hook cannot: the value is read
 * from the module-level locale, so callers do not automatically re-render.
 */
export function t(key: MessageKey, vars?: TranslateVars): string {
    return translate(activeLocale, key, vars);
}

/**
 * `true` when a streamed reply was cut short by the user. The marker is written
 * in the language active at the time, so every locale's wording is checked.
 */
export function isGenerationStopped(content: string): boolean {
    return LOCALES.some((locale) => content.includes(translate(locale, "app.generationStopped")));
}

// ─── React bindings ──────────────────────────────────────────────────────────

export interface I18n {
    locale: Locale;
    setLocale: (locale: Locale) => void;
    t: (key: MessageKey, vars?: TranslateVars) => string;
}

const I18nContext = createContext<I18n>({
    locale: FALLBACK_LOCALE,
    setLocale: () => { },
    t: (key) => key,
});

export function I18nProvider({ children }: { children: ReactNode }) {
    const [locale, setLocaleState] = useState<Locale>(readCachedLocale);

    // Assigned during render (not in an effect) so helpers invoked while children
    // render — e.g. the markdown-it fence rule — already see the new language.
    activeLocale = locale;

    const setLocale = useCallback((next: Locale) => {
        if (!isLocale(next)) return;
        setLocaleState((prev) => (prev === next ? prev : next));
        try {
            window.localStorage.setItem(STORAGE_KEY, next);
        } catch {
            /* ignore unavailable storage */
        }
    }, []);

    useEffect(() => {
        document.documentElement.setAttribute("lang", locale);
    }, [locale]);

    const value = useMemo<I18n>(
        () => ({ locale, setLocale, t: (key, vars) => translate(locale, key, vars) }),
        [locale, setLocale],
    );

    return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18n {
    return useContext(I18nContext);
}
