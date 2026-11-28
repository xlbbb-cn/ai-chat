import type { Message, ToolCallEntry } from "../types";
import { useEffect, useState } from "react";
import { useI18n, type MessageKey, type TranslateVars } from "../i18n";
import { FontAwesomeIcon, faChevronRight, faTriangleExclamation } from "../icons";
import { formatBytes } from "../utils/format";
import "./ToolCallGroup.css";

type Translate = (key: MessageKey, vars?: TranslateVars) => string;

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

/**
 * Per-status light, rendered inside a fixed-width slot: a red dot for a
 * failure, a pulsing green dot while the task runs, and nothing at all for a
 * finished step — success is the quiet default. Skipped keeps a dim neutral dot
 * (the task never ran, which is not a failure).
 */
function StatusLight({ status }: { status: ToolCallEntry["status"] }) {
  if (status === "done") return null;
  return <span className={`tool-call-light tool-call-light--${status}`} aria-hidden="true" />;
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

/** `812ms` / `0.8s` / `12s` — a call's own time, never a raw millisecond count. */
function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms / 1000)}s`;
}

/**
 * What the call produced, as one muted line under the row:
 * `lines 40–80 · 0.8s`, `18 matches · 3 files`, `+42 −3`.
 *
 * Only what the backend measured is rendered — an absent field shows nothing
 * rather than a zero — and the wording stays here: the Rust side sends numbers,
 * never "40 行". A failure renders its error line instead of these, because a
 * count of the error message says nothing.
 */
function metricsFor(entry: ToolCallEntry, t: Translate): string {
  const outcome = entry.outcome;
  if (!outcome || entry.status === "error") return "";
  const parts: string[] = [];
  // A search's hit count is the interesting figure; the rows it came back as
  // would only repeat it.
  if (outcome.matches != null) {
    parts.push(
      t("toolGroup.metrics.matches", {
        count: outcome.matches,
        files: outcome.files ?? 0,
      }),
    );
  } else if (outcome.lines != null && (outcome.line_from == null || outcome.line_from <= 1)) {
    parts.push(t("toolGroup.metrics.lines", { count: outcome.lines }));
  } else if (
    outcome.line_from != null &&
    outcome.line_to != null &&
    outcome.line_to > outcome.line_from
  ) {
    parts.push(
      t("toolGroup.metrics.linesRange", { from: outcome.line_from, to: outcome.line_to }),
    );
  }
  if (outcome.added != null || outcome.removed != null) {
    parts.push(`+${outcome.added ?? 0} −${outcome.removed ?? 0}`);
  }
  // A size carries the shape of the result when there is no line count to do
  // it — a `write` reports the bytes it wrote, not its one-line reply.
  if (outcome.bytes != null && outcome.lines == null) {
    parts.push(formatBytes(outcome.bytes));
  }
  if (outcome.truncated) parts.push(t("toolGroup.metrics.truncated"));
  if (outcome.duration_ms > 0) parts.push(formatDuration(outcome.duration_ms));
  return parts.join(" · ");
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
    // A finished failure states its reason first: what it attempted is already
    // on row 1, and the reason it broke is what a reader needs.
    const failure = (entry.error ?? "").trim();
    if (failure) return failure;
    const text = (entry.summary ?? "").trim();
    if (text) return text;
    if (entry.status === "running" && entry.started_at) {
      return t("toolGroup.waitingOutput", {
        seconds: Math.max(0, Math.round((now - entry.started_at) / 1000)),
      });
    }
    return (entry.activity ?? "").trim();
  };

  // Rows 2–3 preview the running tool call; once nothing is running the last
  // entry stays put, so the next call replaces it instead of leaving a gap.
  // A running sub-agent always wins: with parallel tasks the first running row
  // would otherwise hide the task that actually has progress to show.
  const previewEntry =
    entries.find((e) => e.status === "running" && e.kind === "agent") ??
    runningEntry ??
    entries[entries.length - 1];
  // The collapsed card reports the outcome too (`✓ lines 40–80 · 0.8s`): the
  // row that just finished is the one the user is waiting to hear about, and
  // opening the group to check it would defeat the preview.
  const previewResult =
    previewEntry.status === "running" ? "" : metricsFor(previewEntry, t);
  const previewSub = [detailFor(previewEntry), previewResult].filter(Boolean).join(" · ");
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
                <span className="tool-call-preview-row tool-call-preview-main">
                  <span className="tool-call-preview-name" title={previewEntry.task_id}>
                    [<AgentLabel entry={previewEntry} />]
                  </span>
                  <span className="tool-call-preview-desc">{previewEntry.description}</span>
                </span>
                {/* Outside the masked text row so the fade does not dim the
                    status light; sits at the right edge like the expanded rows. */}
                <span className="tool-call-preview-icon">
                  <StatusLight status={previewEntry.status} />
                </span>
              </span>
              {/* The row simply re-renders on a content swap — no transition. */}
              <span className={previewSubClass}>{previewSub}</span>
            </span>
          )}
        </summary>
        <ul className="tool-call-group-list">
          {entries.map((entry) => (
            <li
              key={entry.task_id}
              className={`tool-call-entry tool-call-entry--${entry.status}`}
            >
              <span className="tool-call-entry-icon"><StatusLight status={entry.status} /></span>
              <span className="tool-call-entry-name" title={entry.task_id}>
                <AgentLabel entry={entry} />
              </span>
              <span className="tool-call-entry-desc">{entry.description}</span>
              {/* Row 2 is either the reason a call failed or what it was
                  doing. Both at once would fight for the same grid cell, and
                  the failure text is the one a reader came for. */}
              {entry.error ? (
                <span className="tool-call-entry-error" title={entry.error}>
                  {entry.error}
                </span>
              ) : (
                (() => {
                  const detail = detailFor(entry);
                  if (!entry.cwd && !detail) return null;
                  return (
                    <span className="tool-call-entry-summary">
                      {entry.cwd ? `in ${entry.cwd}` : ""}
                      {entry.cwd && detail ? " · " : ""}
                      {detail}
                    </span>
                  );
                })()
              )}
              {(() => {
                const metrics = metricsFor(entry, t);
                if (!metrics) return null;
                return <span className="tool-call-entry-meta">{metrics}</span>;
              })()}
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
              {/* The head of the result, on demand: the row stays one line, and
                  the full error / output is one click away instead of in the
                  transcript. */}
              {(() => {
                const output = entry.outcome?.preview ?? entry.outcome?.error;
                if (!output) return null;
                return (
                  <details className="tool-call-entry-output">
                    <summary>
                      {t(
                        entry.status === "error"
                          ? "toolGroup.output.showError"
                          : "toolGroup.output.show",
                      )}
                    </summary>
                    <pre>{output}</pre>
                  </details>
                );
              })()}
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
