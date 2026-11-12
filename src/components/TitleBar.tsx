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
    faBolt,
    faEllipsis,
    faWindowMaximize,
    faWindowMinimize,
    faWindowRestore,
    faXmark,
} from "../icons";
import {
    exportBackupTo,
    getAppVersion,
    getInstanceInfo,
    getWorkspaceDir,
    getWorkspaceHistory,
    importBackupFrom,
    openAppDataDir,
    openIssuePage,
    openMarkdownFile,
    openWorkspaceDir,
    openWorkspaceTmpDir,
    setWorkspaceDir,
} from "../api";
import { useI18n } from "../i18n";
import { formatCountdown } from "../utils/time";
import type { SleepLockState, WorkspaceHistoryEntry } from "../types";

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

/** `/home/leonard/projects/acme` → `/home/leonard/projects` (`""` at the root). */
function workspaceParent(dir: string): string {
    const index = Math.max(dir.lastIndexOf("/"), dir.lastIndexOf("\\"));
    return index > 0 ? dir.slice(0, index) : "";
}

interface Props {
    /** Backend "keep the machine awake" lock. The titlebar badge is its only
     *  surface: it is informational, not a control. */
    sleepLock: SleepLockState;
    /** Wall-clock start of the running turn (`null` while the chat is idle),
     *  so the badge counts the turn up instead of showing a static label. */
    turnStartedAt: number | null;
    /** 1 s heartbeat owned by the app shell (the delay timers use it too). */
    now: number;
    /** Runs right before a workspace switch is applied. Every sidebar panel
     *  reads workspace-scoped state (skills, memory, tools, MCP, history), so
     *  the shell closes them first instead of letting them repaint against the
     *  workspace that is being replaced. */
    onBeforeWorkspaceSwitch?: () => void;
}

export function TitleBar({
    sleepLock,
    turnStartedAt,
    now,
    onBeforeWorkspaceSwitch,
}: Props) {
    const { t } = useI18n();
    const [maximized, setMaximized] = useState(false);
    const [menuOpen, setMenuOpen] = useState(false);
    const [workspace, setWorkspace] = useState("");
    const [workspaceMenuOpen, setWorkspaceMenuOpen] = useState(false);
    const [workspaceEntries, setWorkspaceEntries] = useState<WorkspaceHistoryEntry[]>([]);
    /** A shadow window works on a temporary config copy (`instance.rs`), so
     *  its workspace switch is window-local and the switcher says so. */
    const [shadow, setShadow] = useState(false);
    const menuRef = useRef<HTMLDivElement>(null);
    const triggerRef = useRef<HTMLButtonElement>(null);
    const workspaceMenuRef = useRef<HTMLDivElement>(null);
    const workspaceTriggerRef = useRef<HTMLButtonElement>(null);
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

    // The Rust side writes `Rabbit Chat Studio — <workspace>` into the native title (the
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
        // A shadow window (single-instance off) works on a temporary config
        // copy: switching its workspace only moves this window.
        getInstanceInfo()
            .then((info) => {
                if (!disposed) setShadow(info.role === "shadow");
            })
            .catch(() => { });
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

    const closeWorkspaceMenu = useCallback((refocus = false) => {
        setWorkspaceMenuOpen(false);
        if (refocus) workspaceTriggerRef.current?.focus();
    }, []);

    /**
     * A streaming reply owns the workspace it started in — its tool paths and
     * its database handle belong to that one — so switching waits for the turn
     * to end. The backend refuses the same case (`set_workspace_dir`).
     */
    const workspaceBusy = turnStartedAt !== null;

    /** Read the rows on open: another window (or Settings) may have changed
     *  the list since the last time the popup was shown. */
    const toggleWorkspaceMenu = useCallback(() => {
        if (workspaceMenuOpen) {
            setWorkspaceMenuOpen(false);
            return;
        }
        setWorkspaceMenuOpen(true);
        getWorkspaceHistory()
            .then(setWorkspaceEntries)
            .catch(() => setWorkspaceEntries([]));
    }, [workspaceMenuOpen]);

    // Keyboard path: opening lands on the current row, so Tab / Enter work
    // without a mouse and Escape hands focus back to the indicator.
    useEffect(() => {
        if (!workspaceMenuOpen) return;
        const items = workspaceMenuRef.current?.querySelectorAll<HTMLButtonElement>(
            ".titlebar-menu-item:not(:disabled)"
        );
        if (!items || items.length === 0) return;
        const all = Array.from(items);
        (all.find((item) => item.classList.contains("is-current")) ?? all[0]).focus();
    }, [workspaceMenuOpen]);

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

    // The workspace popup dismisses the same way as the app menu.
    useEffect(() => {
        if (!workspaceMenuOpen) return;
        const onPointerDown = (event: PointerEvent) => {
            if (workspaceMenuRef.current?.contains(event.target as Node)) return;
            setWorkspaceMenuOpen(false);
        };
        const onKeyDown = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.stopPropagation();
            closeWorkspaceMenu(true);
        };
        document.addEventListener("pointerdown", onPointerDown, true);
        document.addEventListener("keydown", onKeyDown, true);
        return () => {
            document.removeEventListener("pointerdown", onPointerDown, true);
            document.removeEventListener("keydown", onKeyDown, true);
        };
    }, [workspaceMenuOpen, closeWorkspaceMenu]);

    /** Run a menu action, closing the popup first and surfacing failures. */
    const run = useCallback(
        async (action: () => Promise<void>) => {
            closeMenu();
            try {
                await action();
            } catch (err) {
                await messageDialog(String(err), { title: "Rabbit Chat Studio", kind: "error" })
                    .catch(() => { });
            }
        },
        [closeMenu],
    );

    /**
     * The switcher applies immediately — that is its whole point, unlike the
     * Settings form which stages `workspace_dir` behind "Save Changes". The
     * command touches only that one field and returns the resolved path, while
     * `workspace-changed` refreshes everything else in the window.
     */
    const switchWorkspace = async (dir: string | null) => {
        closeWorkspaceMenu();
        onBeforeWorkspaceSwitch?.();
        try {
            setWorkspace(await setWorkspaceDir(dir));
        } catch (err) {
            await messageDialog(String(err), { title: "Rabbit Chat Studio", kind: "error" })
                .catch(() => { });
        }
    };

    /** "Choose another folder…" — the same native picker Settings uses. */
    const pickWorkspace = async () => {
        closeWorkspaceMenu();
        try {
            const selected = await openDialog({
                directory: true,
                multiple: false,
                title: t("app.workspacePickTitle"),
                defaultPath: workspace || undefined,
            });
            if (typeof selected === "string" && selected) await switchWorkspace(selected);
        } catch (err) {
            await messageDialog(String(err), { title: "Rabbit Chat Studio", kind: "error" })
                .catch(() => { });
        }
    };

    const handleOpenDataDir = () => run(openAppDataDir);

    const handleOpenWorkspaceDir = () => run(openWorkspaceDir);

    /** `<workspace>/.data/tmp` — the sub-agent deliverable exchange area. */
    const handleOpenDeliverablesDir = () => run(openWorkspaceTmpDir);

    const handleSaveBackup = () =>
        run(async () => {
            const path = await saveDialog({
                defaultPath: "rabbit-chat-studio.backup.zip",
                filters: [{ name: "Backup", extensions: ["zip"] }],
            });
            if (path) await exportBackupTo(path);
        });

    const handleRestoreBackup = () =>
        run(async () => {
            const path = await openDialog({
                multiple: false,
                directory: false,
                filters: [{ name: "Backup", extensions: ["zip"] }],
            });
            if (typeof path === "string") await importBackupFrom(path);
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

    /** GitHub's new-issue page — the bug-report entry, right above About. */
    const handleReportIssue = () => run(openIssuePage);

    const handleAbout = () =>
        run(async () => {
            const version = await getAppVersion();
            await messageDialog(
                `About Rabbit Chat Studio\n\nRabbit Chat Studio\nOpenAI-compatible desktop assistant\nVersion ${version}`,
                { title: "Rabbit Chat Studio", kind: "info" },
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
                    Rabbit Chat Studio
                </span>
                {workspace && (
                    <div className="titlebar-workspace-wrap" ref={workspaceMenuRef}>
                        <button
                            ref={workspaceTriggerRef}
                            type="button"
                            className={`titlebar-workspace ${workspaceMenuOpen ? "is-open" : ""}`}
                            title={workspace}
                            aria-label={t("app.workspaceSwitchAria", {
                                workspace: workspaceLabel(workspace),
                            })}
                            aria-haspopup="menu"
                            aria-expanded={workspaceMenuOpen}
                            onClick={toggleWorkspaceMenu}
                        >
                            {workspaceLabel(workspace)}
                        </button>
                        {workspaceMenuOpen && (
                            <div
                                className="titlebar-menu titlebar-workspace-menu"
                                role="menu"
                                aria-label={t("app.workspaceSwitch")}
                            >
                                {workspaceEntries.map((entry) => (
                                    <button
                                        key={entry.path}
                                        type="button"
                                        role="menuitemradio"
                                        aria-checked={entry.current}
                                        className={`titlebar-menu-item titlebar-workspace-item ${entry.current
                                            ? "is-current"
                                            : ""
                                            }`}
                                        title={entry.path}
                                        disabled={workspaceBusy}
                                        onClick={() => void switchWorkspace(entry.path)}
                                    >
                                        <span className="workspace-item-name">
                                            {workspaceLabel(entry.path)}
                                        </span>
                                        <span className="workspace-item-path">
                                            {workspaceParent(entry.path)}
                                        </span>
                                        {entry.current ? (
                                            <span className="workspace-item-note">
                                                {t("app.workspaceCurrent")}
                                            </span>
                                        ) : (
                                            !entry.exists && (
                                                <span className="workspace-item-note is-missing">
                                                    {t("app.workspaceMissing")}
                                                </span>
                                            )
                                        )}
                                    </button>
                                ))}
                                {workspaceBusy && (
                                    <p className="titlebar-menu-note">
                                        {t("app.workspaceBusy")}
                                    </p>
                                )}
                                {workspaceEntries.length > 0 && (
                                    <div className="titlebar-menu-sep" role="separator" />
                                )}
                                <button
                                    type="button"
                                    role="menuitem"
                                    className="titlebar-menu-item"
                                    disabled={workspaceBusy}
                                    onClick={() => void pickWorkspace()}
                                >
                                    {t("app.workspacePickOther")}
                                </button>
                                {shadow && (
                                    <p className="titlebar-menu-note">
                                        {t("app.workspaceShadowNote")}
                                    </p>
                                )}
                            </div>
                        )}
                    </div>
                )}
                {sleepLock.active && (
                    <span
                        className="sleep-lock-badge"
                        title={t("app.timers.sleepLockHint")}
                        data-tauri-drag-region
                    >
                        <FontAwesomeIcon icon={faBolt} />
                        {turnStartedAt !== null
                            ? t("app.timers.sleepLockRunning", {
                                  time: formatCountdown(now - turnStartedAt),
                              })
                            : t("app.timers.sleepLock")}
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
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleOpenWorkspaceDir}
                        >
                            {t("app.appMenuOpenWorkspaceDir")}
                        </button>
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleOpenDeliverablesDir}
                        >
                            {t("app.appMenuOpenDeliverables")}
                        </button>
                        <div className="titlebar-menu-sep" role="separator" />
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleSaveBackup}
                        >
                            {t("app.appMenuSaveBackup")}
                        </button>
                        <button
                            type="button"
                            role="menuitem"
                            className="titlebar-menu-item"
                            onClick={handleRestoreBackup}
                        >
                            {t("app.appMenuRestoreBackup")}
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
                            onClick={handleReportIssue}
                        >
                            {t("app.appMenuReportIssue")}
                        </button>
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
