import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
    message as messageDialog,
    open as openDialog,
    save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import {
    FontAwesomeIcon,
    faEllipsis,
    faWindowMaximize,
    faWindowMinimize,
    faWindowRestore,
    faXmark,
} from "../icons";
import {
    exportProfileTo,
    getAppVersion,
    getWorkspaceDir,
    importProfileFrom,
    openAppDataDir,
    openMarkdownFile,
} from "../api";
import { useI18n } from "../i18n";

/**
 * The app ships frameless (`decorations: false`), so the drag region and the
 * window controls are ours. The bar sits above every overlay and uses the same
 * tokens as the rest of the chrome — see DESIGN.md → Window chrome.
 */

/** `null` when the UI is opened in a plain browser (Vite preview). */
const APP_WINDOW =
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
        ? getCurrentWindow()
        : null;

/** macOS puts the controls on the left, Windows/Linux on the right. */
const IS_MAC = navigator.userAgent.includes("Mac");

/** `…/workspaces/acme` → `acme`; falls back to the full path. */
function workspaceLabel(dir: string): string {
    return dir.split(/[\\/]/).filter(Boolean).pop() || dir;
}

export function TitleBar() {
    const { t } = useI18n();
    const [maximized, setMaximized] = useState(false);
    const [menuOpen, setMenuOpen] = useState(false);
    const [workspace, setWorkspace] = useState("");
    const menuRef = useRef<HTMLDivElement>(null);
    const triggerRef = useRef<HTMLButtonElement>(null);

    // The maximize glyph must follow the real window state — it also changes
    // when the user double-clicks the drag region or uses the OS shortcut.
    useEffect(() => {
        if (!APP_WINDOW) return;
        let disposed = false;
        let unlisten: (() => void) | undefined;
        const sync = () => {
            APP_WINDOW?.isMaximized()
                .then((value) => {
                    if (!disposed) setMaximized(value);
                })
                .catch(() => { });
        };
        sync();
        APP_WINDOW.onResized(sync)
            .then((fn) => {
                if (disposed) fn();
                else unlisten = fn;
            })
            .catch(() => { });
        return () => {
            disposed = true;
            unlisten?.();
        };
    }, []);

    // The Rust side writes `AI Chat — <workspace>` into the native title (the
    // taskbar text). With no native titlebar, surface the workspace here.
    useEffect(() => {
        let disposed = false;
        const load = () => {
            getWorkspaceDir()
                .then((dir) => {
                    if (!disposed) setWorkspace(dir);
                })
                .catch(() => { });
        };
        load();
        const unlisten = listen<string>("workspace-changed", load);
        return () => {
            disposed = true;
            unlisten.then((fn) => fn()).catch(() => { });
        };
    }, []);

    const closeMenu = useCallback((refocus = false) => {
        setMenuOpen(false);
        if (refocus) triggerRef.current?.focus();
    }, []);

    // Dismiss on outside click / Escape while open (same pattern as ModelSelect).
    useEffect(() => {
        if (!menuOpen) return;
        const onPointerDown = (event: PointerEvent) => {
            if (menuRef.current?.contains(event.target as Node)) return;
            setMenuOpen(false);
        };
        const onKeyDown = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.stopPropagation();
            closeMenu(true);
        };
        document.addEventListener("pointerdown", onPointerDown, true);
        document.addEventListener("keydown", onKeyDown, true);
        return () => {
            document.removeEventListener("pointerdown", onPointerDown, true);
            document.removeEventListener("keydown", onKeyDown, true);
        };
    }, [menuOpen, closeMenu]);

    /** Run a menu action, closing the popup first and surfacing failures. */
    const run = useCallback(
        async (action: () => Promise<void>) => {
            closeMenu();
            try {
                await action();
            } catch (err) {
                await messageDialog(String(err), { title: "AI Chat", kind: "error" })
                    .catch(() => { });
            }
        },
        [closeMenu],
    );

    const handleOpenDataDir = () => run(openAppDataDir);

    const handleSaveProfile = () =>
        run(async () => {
            const path = await saveDialog({
                defaultPath: "ai-chat.profile.zip",
                filters: [{ name: "Profile", extensions: ["zip"] }],
            });
            if (path) await exportProfileTo(path);
        });

    const handleRestoreProfile = () =>
        run(async () => {
            const path = await openDialog({
                multiple: false,
                directory: false,
                filters: [{ name: "Profile", extensions: ["zip"] }],
            });
            if (typeof path === "string") await importProfileFrom(path);
        });

    const handleMarkdownEdit = () =>
        run(async () => {
            const path = await openDialog({
                multiple: false,
                directory: false,
                filters: [{ name: "Markdown", extensions: ["md", "markdown", "mdown", "mkd"] }],
            });
            if (typeof path === "string") await openMarkdownFile(path);
        });

    const handleAbout = () =>
        run(async () => {
            const version = await getAppVersion();
            await messageDialog(
                `About AI Chat\n\nAI Chat\nOpenAI-compatible desktop assistant\nVersion ${version}`,
                { title: "AI Chat", kind: "info" },
            );
        });

    const controls = (
        <div className="titlebar-controls">
            <button
                type="button"
                className="titlebar-btn"
                onClick={() => void APP_WINDOW?.minimize()}
                title={t("app.windowMinimize")}
                aria-label={t("app.windowMinimize")}
                disabled={!APP_WINDOW}
            >
                <FontAwesomeIcon icon={faWindowMinimize} />
            </button>
            <button
                type="button"
                className="titlebar-btn"
                onClick={() => void APP_WINDOW?.toggleMaximize()}
                title={maximized ? t("app.windowRestore") : t("app.windowMaximize")}
                aria-label={maximized ? t("app.windowRestore") : t("app.windowMaximize")}
                disabled={!APP_WINDOW}
            >
                <FontAwesomeIcon icon={maximized ? faWindowRestore : faWindowMaximize} />
            </button>
            <button
                type="button"
                className="titlebar-btn is-close"
                onClick={() => void APP_WINDOW?.close()}
                title={t("app.windowClose")}
                aria-label={t("app.windowClose")}
                disabled={!APP_WINDOW}
            >
                <FontAwesomeIcon icon={faXmark} />
            </button>
        </div>
    );

    return (
        <header className="titlebar" data-tauri-drag-region>
            {IS_MAC && controls}
            <div className="titlebar-drag" data-tauri-drag-region>
                <span className="titlebar-title" data-tauri-drag-region>
                    AI Chat
                </span>
                {workspace && (
                    <span className="titlebar-workspace" title={workspace} data-tauri-drag-region>
                        {workspaceLabel(workspace)}
                    </span>
                )}
            </div>
            <div className="titlebar-menu-wrap" ref={menuRef}>
                <button
                    ref={triggerRef}
                    type="button"
                    className={`titlebar-btn ${menuOpen ? "active" : ""}`}
                    onClick={() => setMenuOpen((open) => !open)}
                    title={t("app.appMenu")}
                    aria-label={t("app.appMenu")}
                    aria-haspopup="menu"
                    aria-expanded={menuOpen}
                >
                    <FontAwesomeIcon icon={faEllipsis} />
                </button>
                {menuOpen && (
                    <div className="titlebar-menu" role="menu">
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleOpenDataDir}
                        >
                            {t("app.appMenuOpenDataDir")}
                        </button>
                        <div className="titlebar-menu-sep" role="separator" />
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleSaveProfile}
                        >
                            {t("app.appMenuSaveProfile")}
                        </button>
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleRestoreProfile}
                        >
                            {t("app.appMenuRestoreProfile")}
                        </button>
                        <div className="titlebar-menu-sep" role="separator" />
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleMarkdownEdit}
                        >
                            {t("app.appMenuMarkdownEdit")}
                        </button>
                        <div className="titlebar-menu-sep" role="separator" />
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleAbout}
                        >
                            {t("app.appMenuAbout")}
                        </button>
                    </div>
                )}
            </div>
            {!IS_MAC && controls}
        </header>
    );
}
