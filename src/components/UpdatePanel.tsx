import { useCallback, useEffect, useRef, useState } from "react";
import {
    checkUpdate,
    downloadUpdate,
    onUpdateDownloadProgress,
    openReleasePage,
    openUpdateFile,
    revealUpdateFile,
} from "../api";
import type { UpdateAsset, UpdateInfo } from "../types";
import { MarkdownPreview } from "./MarkdownPreview";
import "./UpdatePanel.css";

interface Props {
    /** Result of the startup check, shown until the panel refreshes it. */
    initialInfo?: UpdateInfo | null;
    onClose: () => void;
    /** Lets the caller (chat banner) react to a fresh check result. */
    onInfo?: (info: UpdateInfo) => void;
}

function formatBytes(bytes: number): string {
    if (!bytes) return "";
    const units = ["B", "KB", "MB", "GB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
        value /= 1024;
        unit += 1;
    }
    return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

function formatDate(iso?: string): string {
    if (!iso) return "";
    const date = new Date(iso);
    return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString();
}

/** Split a path so a long folder name cannot push the file name out of view. */
function splitPath(path: string): { dir: string; file: string } {
    const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return index === -1
        ? { dir: "", file: path }
        : { dir: path.slice(0, index), file: path.slice(index + 1) };
}

export function UpdatePanel({ initialInfo, onClose, onInfo }: Props) {
    const [info, setInfo] = useState<UpdateInfo | null>(initialInfo ?? null);
    const [checking, setChecking] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [downloading, setDownloading] = useState<string | null>(null);
    const [progress, setProgress] = useState<{ downloaded: number; total: number } | null>(null);
    const [savedPath, setSavedPath] = useState<string | null>(null);
    const mountedRef = useRef(true);
    const onInfoRef = useRef(onInfo);
    onInfoRef.current = onInfo;

    useEffect(() => {
        mountedRef.current = true;
        return () => {
            mountedRef.current = false;
        };
    }, []);

    useEffect(() => {
        const unlisten = onUpdateDownloadProgress((payload) => {
            if (!mountedRef.current) return;
            setProgress({ downloaded: payload.downloaded, total: payload.total });
        });
        return () => {
            unlisten.then((fn) => fn());
        };
    }, []);

    const runCheck = useCallback(async () => {
        setChecking(true);
        setError(null);
        try {
            const result = await checkUpdate();
            if (!mountedRef.current) return;
            setInfo(result);
            onInfoRef.current?.(result);
        } catch (err) {
            if (mountedRef.current) setError(typeof err === "string" ? err : String(err));
        } finally {
            if (mountedRef.current) setChecking(false);
        }
    }, []);

    useEffect(() => {
        // Always refresh on open so the panel never shows a stale release.
        void runCheck();
    }, [runCheck]);

    const handleDownload = useCallback(async (asset: UpdateAsset) => {
        setDownloading(asset.name);
        setSavedPath(null);
        setError(null);
        setProgress({ downloaded: 0, total: asset.size });
        try {
            const path = await downloadUpdate(asset.name, asset.download_url);
            if (mountedRef.current) setSavedPath(path);
        } catch (err) {
            if (mountedRef.current) setError(typeof err === "string" ? err : String(err));
        } finally {
            if (mountedRef.current) {
                setDownloading(null);
                setProgress(null);
            }
        }
    }, []);

    const handleOpen = useCallback(async (action: () => Promise<void>) => {
        try {
            await action();
        } catch (err) {
            setError(typeof err === "string" ? err : String(err));
        }
    }, []);

    const percent = progress
        ? progress.total > 0
            ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100))
            : 0
        : 0;
    const busy = checking || downloading !== null;

    return (
        <div className="update-overlay" role="dialog" aria-modal="true" aria-label="Software update">
            <div className="update-panel">
                <header className="update-panel-head">
                    <div className="update-panel-heading">
                        <h3>Software Update</h3>
                        <p>
                            {info
                                ? `Installed v${info.current_version} · ${info.platform}`
                                : "Checks the newest published GitHub release for this repository."}
                        </p>
                    </div>
                    <div className="update-panel-actions">
                        <button type="button" className="update-btn" onClick={runCheck} disabled={busy}>
                            {checking ? "Checking…" : "Check again"}
                        </button>
                        {/* Closing stays possible: the download runs in the backend either way. */}
                        <button type="button" className="update-btn" onClick={onClose}>
                            Close
                        </button>
                    </div>
                </header>

                <div className="update-panel-body">
                    {error && <div className="update-error">{error}</div>}

                    {!info && checking && <div className="update-status">Checking for updates…</div>}

                    {info && !info.has_update && (
                        <div className="update-status update-status-ok">
                            <span className="update-dot" />
                            <span>
                                You are on the latest published version (v{info.latest_version})
                                {info.published_at ? `, released ${formatDate(info.published_at)}` : ""}.
                            </span>
                        </div>
                    )}

                    {info && info.has_update && (
                        <>
                            <div className="update-status update-status-new">
                                <span className="update-dot" />
                                <span>Version {info.latest_version} is available</span>
                                {info.published_at && <span className="update-muted">· {formatDate(info.published_at)}</span>}
                                {info.prerelease && <span className="update-badge">pre-release</span>}
                            </div>

                            {!info.has_platform_asset && (
                                <div className="update-hint">
                                    This release has no bundle for <code>{info.platform}</code>. Choose a file below or open the release page.
                                </div>
                            )}

                            <ul className="update-assets">
                                {info.assets.map((asset) => (
                                    <li key={asset.name} className={`update-asset${asset.recommended ? " recommended" : ""}`}>
                                        <div className="update-asset-info">
                                            <span className="update-asset-name" title={asset.name}>
                                                {asset.name}
                                            </span>
                                            <span className="update-asset-meta">
                                                {asset.kind}
                                                {asset.size ? ` · ${formatBytes(asset.size)}` : ""}
                                                {asset.recommended && <span className="update-badge">recommended</span>}
                                            </span>
                                        </div>
                                        <button
                                            type="button"
                                            className={`update-btn${asset.recommended ? " update-btn-primary" : ""}`}
                                            onClick={() => handleDownload(asset)}
                                            disabled={busy}
                                        >
                                            {downloading === asset.name ? "Downloading…" : "Download"}
                                        </button>
                                    </li>
                                ))}
                                {info.assets.length === 0 && (
                                    <li className="update-asset empty">No downloadable files attached to this release.</li>
                                )}
                            </ul>

                            {progress && (
                                <div
                                    className="update-progress"
                                    role="progressbar"
                                    aria-valuemin={0}
                                    aria-valuemax={100}
                                    aria-valuenow={percent}
                                >
                                    <div className="update-progress-track">
                                        <div className="update-progress-fill" style={{ width: `${percent}%` }} />
                                    </div>
                                    <span className="update-progress-label">
                                        {percent}%
                                        {progress.total > 0
                                            ? ` · ${formatBytes(progress.downloaded)} / ${formatBytes(progress.total)}`
                                            : ` · ${formatBytes(progress.downloaded)}`}
                                    </span>
                                </div>
                            )}

                            {savedPath && (
                                <div className="update-saved">
                                    <div className="update-saved-info">
                                        <span className="update-saved-name">{splitPath(savedPath).file}</span>
                                        <span className="update-saved-dir" title={savedPath}>
                                            {splitPath(savedPath).dir}
                                        </span>
                                    </div>
                                    <div className="update-saved-actions">
                                        <button
                                            type="button"
                                            className="update-btn update-btn-primary"
                                            onClick={() => handleOpen(() => openUpdateFile(savedPath))}
                                        >
                                            Run installer
                                        </button>
                                        <button
                                            type="button"
                                            className="update-btn"
                                            onClick={() => handleOpen(() => revealUpdateFile(savedPath))}
                                        >
                                            Show in folder
                                        </button>
                                    </div>
                                </div>
                            )}

                            <div className="update-notes">
                                <div className="update-notes-title">Release notes</div>
                                <div className="update-notes-body">
                                    {info.release_notes.trim() ? (
                                        <MarkdownPreview content={info.release_notes} />
                                    ) : (
                                        <p className="update-notes-empty">This release has no notes.</p>
                                    )}
                                </div>
                            </div>
                        </>
                    )}

                    {info && (
                        <div className="update-footer">
                            <button
                                type="button"
                                className="update-btn"
                                onClick={() => handleOpen(() => openReleasePage(info.release_url))}
                            >
                                Open release page
                            </button>
                            <span className="update-muted">Draft releases are not published and are skipped.</span>
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
}
