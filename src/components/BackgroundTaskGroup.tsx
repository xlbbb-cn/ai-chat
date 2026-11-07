import { useEffect, useState } from "react";
import { FontAwesomeIcon, faChevronDown, faChevronUp, faSpinner, faXmark } from "../icons";
import { listServeProcesses, stopServeProcess } from "../api";
import type { BackgroundTaskState, ServeProcessInfo } from "../types";
import { useI18n } from "../i18n";
import { formatUptime } from "../utils/time";
import "./BackgroundTaskGroup.css";

interface Props {
    /** Session-scoped background tool tasks, live from `background-task-state`. */
    tasks: BackgroundTaskState[];
    /** Rounds currently blocked waiting on a background result. */
    waiting: number;
    /** Stop a running task — its whole process tree is terminated. */
    onCancelTask?: (taskId: string) => void;
}

/** Poll fast while a serve process runs, idle slowly otherwise. */
const FAST_POLL_MS = 2000;
const SLOW_POLL_MS = 5000;

/**
 * Live background work: the tasks a turn started with `run_in_background` and
 * the serve processes `run_shell` keeps alive.
 *
 * Only what is *running right now* appears. A finished task delivers its result
 * to the model, and a stopped serve process is past tense — neither belongs in
 * a live monitor, so as the last of them ends the whole box unmounts.
 *
 * (This is the only place either is shown: the composer and the tools panel no
 * longer list them.)
 */
export function BackgroundTaskGroup({ tasks, waiting, onCancelTask }: Props) {
    const { t } = useI18n();
    const [open, setOpen] = useState(false);
    const [serves, setServes] = useState<ServeProcessInfo[]>([]);
    const [stopping, setStopping] = useState<string | null>(null);
    const [now, setNow] = useState(() => Date.now());

    const runningServes = serves.filter((process) => process.state === "running");
    const runningTasks = tasks.filter((task) => task.status === "running");
    const hasRunningServe = runningServes.length > 0;
    const live = hasRunningServe || runningTasks.length > 0 || waiting > 0;

    // Serve processes push no event — the model may launch one mid-turn and the
    // process may exit on its own — so the registry is polled. The cadence
    // follows what is happening: watch closely while one runs, idle otherwise.
    useEffect(() => {
        let cancelled = false;
        const refresh = async () => {
            try {
                const list = await listServeProcesses();
                if (!cancelled) setServes(list);
            } catch (err) {
                console.error("listServeProcesses failed", err);
            }
        };
        void refresh();
        const timer = window.setInterval(refresh, hasRunningServe ? FAST_POLL_MS : SLOW_POLL_MS);
        return () => {
            cancelled = true;
            window.clearInterval(timer);
        };
    }, [hasRunningServe]);

    // Keep the elapsed column counting while something is live: the backend
    // only reports elapsed when a task changes state.
    useEffect(() => {
        if (!live) return;
        setNow(Date.now());
        const id = window.setInterval(() => setNow(Date.now()), 1000);
        return () => window.clearInterval(id);
    }, [live]);

    async function stopServe(id: string) {
        setStopping(id);
        try {
            await stopServeProcess(id);
            setServes(await listServeProcesses());
        } catch (err) {
            console.error("stopServeProcess failed", err);
        } finally {
            setStopping(null);
        }
    }

    if (!live) return null;

    return (
        <section className="background-group" aria-label={t("backgroundGroup.title")}>
            <button
                type="button"
                className="background-group-header"
                onClick={() => setOpen((value) => !value)}
                aria-expanded={open}
            >
                <span className="background-group-dot" aria-hidden="true" />
                <span className="background-group-title">{t("backgroundGroup.title")}</span>
                {runningTasks.length > 0 && (
                    <span className="background-group-badge">
                        {t("backgroundGroup.running", { count: runningTasks.length })}
                    </span>
                )}
                {waiting > 0 && (
                    <span className="background-group-badge waiting">
                        {t("backgroundGroup.waiting", { count: waiting })}
                    </span>
                )}
                <span className="background-group-chevron">
                    <FontAwesomeIcon icon={open ? faChevronUp : faChevronDown} />
                </span>
            </button>

            {open && (
                <div className="background-group-body">
                    {runningServes.length > 0 && (
                        <div className="background-group-section">
                            <div className="background-group-section-title">
                                {t("backgroundGroup.serves")}
                            </div>
                            <ul className="background-group-list">
                                {runningServes.map((process) => (
                                    <li key={process.id} className="background-entry">
                                        <div className="background-entry-row">
                                            <span className="background-serve-dot" aria-hidden="true" />
                                            <span className="background-entry-name" title={process.id}>
                                                {process.id}
                                            </span>
                                            <span className="background-entry-meta">
                                                {t("backgroundGroup.serveMeta", {
                                                    pid: process.pid,
                                                    uptime: formatUptime(process.uptime_secs),
                                                })}
                                            </span>
                                            <button
                                                type="button"
                                                className="background-entry-action"
                                                onClick={() => void stopServe(process.id)}
                                                disabled={stopping === process.id}
                                                title={t("backgroundGroup.stopServe")}
                                                aria-label={t("backgroundGroup.stopServe")}
                                            >
                                                <FontAwesomeIcon icon={faXmark} />
                                            </button>
                                        </div>
                                        <code className="background-entry-code">
                                            {process.shell_type} · {process.code}
                                        </code>
                                        {process.recent_output.trim() !== "" && (
                                            <pre className="background-entry-output">
                                                {process.recent_output}
                                            </pre>
                                        )}
                                    </li>
                                ))}
                            </ul>
                        </div>
                    )}

                    {runningTasks.length > 0 && (
                        <div className="background-group-section">
                            <div className="background-group-section-title">
                                {t("backgroundGroup.tasks")}
                            </div>
                            <ul className="background-group-list">
                                {runningTasks.map((task) => (
                                    <li key={task.id} className="background-entry">
                                        <div className="background-entry-row">
                                            <span className="background-entry-icon running" aria-hidden="true">
                                                <FontAwesomeIcon icon={faSpinner} spin />
                                            </span>
                                            <span className="background-entry-name" title={task.label}>
                                                {task.label}
                                            </span>
                                            <span className="background-entry-meta">{task.tool}</span>
                                            <span className="background-entry-meta">
                                                {formatUptime(
                                                    Math.max(0, Math.round((now - task.started_at_ms) / 1000)),
                                                )}
                                            </span>
                                            {onCancelTask && (
                                                <button
                                                    type="button"
                                                    className="background-entry-action"
                                                    onClick={() => onCancelTask(task.id)}
                                                    title={t("backgroundGroup.cancelTask")}
                                                    aria-label={t("backgroundGroup.cancelTask")}
                                                >
                                                    <FontAwesomeIcon icon={faXmark} />
                                                </button>
                                            )}
                                        </div>
                                        {task.preview && task.preview.trim() !== "" && (
                                            <div className="background-entry-preview">{task.preview}</div>
                                        )}
                                    </li>
                                ))}
                            </ul>
                        </div>
                    )}

                    {!hasRunningServe && runningTasks.length === 0 && (
                        <div className="background-group-hint">{t("backgroundGroup.hint")}</div>
                    )}
                </div>
            )}
        </section>
    );
}
