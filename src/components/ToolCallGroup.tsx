import type { Message, ToolCallEntry } from "../types";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useI18n } from "../i18n";
import { FontAwesomeIcon, faCheck, faChevronRight, faMinus, faSpinner, faTriangleExclamation, faXmark } from "../icons";
import "./ToolCallGroup.css";

interface Props {
  message: Message;
  /**
   * Answer an inline `run_shell` confirmation (`request_id`, `allowed`).
   * Absent for persisted history — the buttons only render for live turns.
   */
  onConfirm?: (requestId: string, allowed: boolean) => void;
  /** Stop a running background tool task (`run_in_background`). */
  onCancelBackground?: (taskId: string) => void;
}

/** Per-status glyph; the running spinner is the only animated one. */
function StatusIcon({ status }: { status: ToolCallEntry["status"] }) {
  if (status === "running") return <FontAwesomeIcon icon={faSpinner} spin />;
  if (status === "done") return <FontAwesomeIcon icon={faCheck} />;
  // Skipped is neutral: the task never ran, which is not a failure.
  if (status === "skipped") return <FontAwesomeIcon icon={faMinus} />;
  return <FontAwesomeIcon icon={faXmark} />;
}

/**
 * A sub-agent row carries its appearance-order number (`#1`, `#2`, …) so the
 * tool group can be matched with the planner / mission panel without showing
 * the raw task id. Planner, selector, aggregator and plain tool rows have no
 * `agent_index` and render exactly as before.
 */
function AgentLabel({ entry }: { entry: ToolCallEntry }) {
  if (entry.agent_index == null) return <>{entry.agent_name}</>;
  return (
    <>
      <span className="tool-call-agent-index">#{entry.agent_index}</span>
      {" · "}
      {entry.agent_name}
    </>
  );
}

interface FlipRowProps {
  /** Identity of the content currently shown; changing it triggers the flip. */
  flipKey: string;
  className?: string;
  children: ReactNode;
}

/**
 * Preview row (rows 2–3). A content swap plays a card flip: the outgoing
 * render is parked on the hidden back face and the card is animated from
 * edge-on, so the first frame still matches what was on screen before the
 * swap and the new content never hard-cuts into place.
 */
function FlipRow({ flipKey, className, children }: FlipRowProps) {
  // Previous render, kept around so it can rotate out as the back face.
  const lastRenderRef = useRef<{ key: string; node: ReactNode }>({ key: flipKey, node: children });
  const [outgoing, setOutgoing] = useState<ReactNode>(null);

  if (lastRenderRef.current.key !== flipKey) {
    // Render-phase adjustment: park the outgoing render on the back face and
    // re-render immediately with the flip armed.
    setOutgoing(lastRenderRef.current.node);
  }
  lastRenderRef.current = { key: flipKey, node: children };

  return (
    <span className={className}>
      <span
        className={`tool-call-flip${outgoing !== null ? " tool-call-flip--live" : ""}`}
        onAnimationEnd={(e) => {
          if (e.animationName === "tool-flip-in") setOutgoing(null);
        }}
      >
        <span className="tool-call-flip-face">{children}</span>
        <span className="tool-call-flip-face tool-call-flip-face--back">{outgoing}</span>
      </span>
    </span>
  );
}

export function ToolCallGroup({ message, onConfirm, onCancelBackground }: Props) {
  const { t } = useI18n();
  const entries = message.tool_calls ?? [];
  const [open, setOpen] = useState(false);
  // One-second tick while anything runs: "waiting 12s" / "thinking · 320 chars"
  // must keep counting even when the backend sends no tokens for a minute.
  const hasRunning = entries.some((entry) => entry.status === "running");
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!hasRunning) return;
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [hasRunning]);
  if (entries.length === 0) return null;
  const runningEntry = entries.find((e) => e.status === "running");
  const isRunning = runningEntry !== undefined;
  const doneCount = entries.filter((e) => e.status === "done").length;
  const runningCount = entries.filter((e) => e.status === "running").length;
  const errorCount = entries.filter((e) => e.status === "error").length;
  const skippedCount = entries.filter((e) => e.status === "skipped").length;
  // At most one confirmation can be pending at a time (the backend blocks the
  // tool call until it is answered), so the first entry that carries one wins.
  const confirmEntry = entries.find((e) => e.confirm !== undefined);
  const confirmInfo = confirmEntry?.confirm;

  /**
   * Row 3 text for one entry.
   *
   * While a task runs, what is happening *now* wins: a live reasoning counter,
   * then the tool it is executing. Streamed output stays visible between rounds
   * (it accumulates), and a plain wait timer covers the gaps before the first
   * signal. A sub-agent often emits no content for a whole round — without the
   * live signals the row looks frozen while the model is actually working.
   */
  const detailFor = (entry: ToolCallEntry): string => {
    if (entry.status === "running") {
      if (entry.thinking && entry.thinking.chars > 0) {
        return t("toolGroup.thinkingLive", {
          count: entry.thinking.chars,
          seconds: Math.max(0, Math.round((now - entry.thinking.started_at) / 1000)),
        });
      }
      const activity = (entry.activity ?? "").trim();
      if (activity) return activity;
    }
    const text = (entry.summary ?? entry.error ?? "").trim();
    if (text) return text;
    if (entry.status === "running" && entry.started_at) {
      return t("toolGroup.waitingOutput", {
        seconds: Math.max(0, Math.round((now - entry.started_at) / 1000)),
      });
    }
    return (entry.activity ?? "").trim();
  };

  // Rows 2–3 preview the running tool call; once nothing is running the last
  // entry stays put, so the next call gives the flip something to swap with.
  // A running sub-agent always wins: with parallel tasks the first running row
  // would otherwise hide the task that actually has progress to show.
  const previewEntry =
    entries.find((e) => e.status === "running" && e.kind === "agent") ??
    runningEntry ??
    entries[entries.length - 1];
  const previewSub = detailFor(previewEntry);
  const previewSubClass = previewSub
    ? "tool-call-preview-row tool-call-preview-sub"
    : "tool-call-preview-row tool-call-preview-sub tool-call-preview-sub--empty";

  // Row 1 of the collapsed state. A running group must say so — reporting
  // "completed" while a task is still working was half of the frozen-row bug.
  // Failures dominate otherwise, then skipped steps (they explain a report that
  // is smaller than the plan).
  const titleLabel =
    runningCount > 0
      ? t("toolGroup.running", {
          running: runningCount,
          done: doneCount,
          count: entries.length,
        })
      : errorCount > 0
        ? t("toolGroup.completedWithErrors", { count: entries.length, done: doneCount, errors: errorCount })
        : skippedCount > 0
          ? t("toolGroup.completedWithSkipped", { count: entries.length, done: doneCount, skipped: skippedCount })
          : t("toolGroup.completed", { count: doneCount });

  return (
    <div className={`tool-call-group${isRunning ? " tool-call-group--running" : ""}`}>
      <details
        className="tool-call-group-details"
        open={open}
        onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}
      >
        <summary className="tool-call-group-summary">
          <span className="tool-call-group-head">
            <span className="tool-call-group-label">{titleLabel}</span>
            <span className="tool-call-group-count">{t("toolGroup.steps", { count: entries.length })}</span>
            <span className="tool-call-group-chevron">
              <FontAwesomeIcon icon={faChevronRight} />
            </span>
          </span>
          {!open && (
            <span className="tool-call-group-preview" aria-hidden="true">
              <span className="tool-call-preview-line">
                <FlipRow
                  className="tool-call-preview-row tool-call-preview-main"
                  flipKey={previewEntry.task_id}
                >
                  <span className="tool-call-preview-name" title={previewEntry.task_id}>
                    [<AgentLabel entry={previewEntry} />]
                  </span>
                  <span className="tool-call-preview-desc">{previewEntry.description}</span>
                </FlipRow>
                {/* Outside the masked text row so the fade does not dim the
                    glyph; sits at the right edge like the expanded rows. */}
                <span
                  className={`tool-call-preview-icon tool-call-preview-icon--${previewEntry.status}`}
                >
                  <StatusIcon status={previewEntry.status} />
                </span>
              </span>
              <FlipRow
                className={previewSubClass}
                // A sub-agent row streams text continuously: flipping on every
                // 400 ms chunk would keep the line permanently reversed. Only
                // ordinary tool rows swap (args → result) with a flip.
                flipKey={
                  previewEntry.kind === "agent"
                    ? previewEntry.task_id
                    : `${previewEntry.task_id}|${previewSub}`
                }
              >
                {previewSub}
              </FlipRow>
            </span>
          )}
        </summary>
        <ul className="tool-call-group-list">
          {entries.map((entry) => (
            <li
              key={entry.task_id}
              className={`tool-call-entry tool-call-entry--${entry.status}`}
            >
              <span className="tool-call-entry-icon"><StatusIcon status={entry.status} /></span>
              <span className="tool-call-entry-name" title={entry.task_id}>
                <AgentLabel entry={entry} />
              </span>
              <span className="tool-call-entry-desc">{entry.description}</span>
              {(() => {
                const detail = detailFor(entry);
                if (!entry.cwd && !detail) return null;
                return (
                  <span className="tool-call-entry-summary">
                    {entry.cwd ? `in ${entry.cwd}` : ""}
                    {entry.cwd && detail ? " · " : ""}
                    {detail}
                  </span>
                );
              })()}
              {entry.help && (
                <span className={`tool-call-entry-help tool-call-entry-help--${entry.help.status}`}>
                  {entry.help.status === "error"
                    ? `${t("toolGroup.mainAgentHelpFailed")}: ${entry.help.error ?? ""}`
                    : `${t("toolGroup.mainAgentHelp")}: ${entry.help.request}`}
                  {entry.help.status === "done" && entry.help.answer_chars != null
                    ? ` ${t("toolGroup.mainAgentHelpChars", { count: entry.help.answer_chars })}`
                    : ""}
                </span>
              )}
              {entry.mbus && (entry.mbus.sent > 0 || entry.mbus.received > 0) && (
                <span className="tool-call-entry-mbus">
                  <span className="tool-call-entry-mbus-count">
                    {t("toolGroup.mbusCounts", {
                      sent: entry.mbus.sent,
                      received: entry.mbus.received,
                    })}
                  </span>
                  {entry.mbus.last && (
                    <span className="tool-call-entry-mbus-last">
                      {entry.mbus.last.direction === "out" ? "→ " : "← "}
                      {entry.mbus.last.label}: {entry.mbus.last.body}
                    </span>
                  )}
                </span>
              )}
              {entry.error && (
                <span className="tool-call-entry-error">{entry.error}</span>
              )}
              {entry.background && entry.status === "running" && onCancelBackground && (
                <button
                  type="button"
                  className="inline-edit-btn tool-call-entry-cancel"
                  title={t("app.background.cancel")}
                  onClick={() => onCancelBackground(entry.task_id)}
                >
                  {t("app.background.cancel")}
                </button>
              )}
            </li>
          ))}
        </ul>
      </details>
      {confirmInfo && onConfirm && (
        <div className="tool-call-confirm" role="alertdialog" aria-label={t("toolGroup.confirmTitle")}>
          <div className="tool-call-confirm-head">
            <FontAwesomeIcon icon={faTriangleExclamation} className="tool-call-confirm-icon" />
            <span className="tool-call-confirm-title">{t("toolGroup.confirmTitle")}</span>
          </div>
          <div className="tool-call-confirm-reason">
            <strong>{t("app.confirm.reason")}</strong> {confirmInfo.reason}
          </div>
          <pre className="tool-call-confirm-code">
            {confirmInfo.code.length > 400
              ? confirmInfo.code.slice(0, 400) + "…"
              : confirmInfo.code}
          </pre>
          <div className="tool-call-confirm-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={() => onConfirm(confirmInfo.request_id, false)}
            >
              {t("app.confirm.deny")}
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={() => onConfirm(confirmInfo.request_id, true)}
            >
              {t("app.confirm.allow")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
