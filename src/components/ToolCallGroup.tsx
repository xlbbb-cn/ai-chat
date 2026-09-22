import type { Message, ToolCallEntry } from "../types";
import { useRef, useState, type ReactNode } from "react";
import "./ToolCallGroup.css";

interface Props {
  message: Message;
}

function statusIcon(status: ToolCallEntry["status"]): string {
  if (status === "running") return "⟳";
  if (status === "done") return "✓";
  return "✕";
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

export function ToolCallGroup({ message }: Props) {
  const entries = message.tool_calls ?? [];
  const [open, setOpen] = useState(false);
  if (entries.length === 0) return null;
  const runningEntry = entries.find((e) => e.status === "running");
  const isRunning = runningEntry !== undefined;
  const doneCount = entries.filter((e) => e.status === "done").length;
  const errorCount = entries.filter((e) => e.status === "error").length;

  // Rows 2–3 preview the running tool call; once nothing is running the last
  // entry stays put, so the next call gives the flip something to swap with.
  // Row 3 only exists while the entry has a result — an empty row collapses
  // instead of holding a blank line, so the card is 2 rows tall meanwhile.
  const previewEntry = runningEntry ?? entries[entries.length - 1];
  const previewSub = (previewEntry.summary ?? previewEntry.error ?? "").trim();
  const previewSubClass = previewSub
    ? "tool-call-preview-row tool-call-preview-sub"
    : "tool-call-preview-row tool-call-preview-sub tool-call-preview-sub--empty";

  // Row 1 of the collapsed state: completion title.
  const titleLabel =
    errorCount > 0
      ? `${doneCount} tool calls completed — ${errorCount} failed`
      : `${doneCount} tool calls completed`;

  return (
    <div className={`tool-call-group${isRunning ? " tool-call-group--running" : ""}`}>
      <details
        className="tool-call-group-details"
        open={open}
        onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}
      >
        <summary className="tool-call-group-summary">
          <span className="tool-call-group-head">
            <span className={`tool-call-group-indicator${isRunning ? " running" : ""}`} />
            <span className="tool-call-group-label">{titleLabel}</span>
            <span className="tool-call-group-count">{entries.length} steps</span>
            <span className="tool-call-group-chevron">›</span>
          </span>
          {!open && (
            <span className="tool-call-group-preview" aria-hidden="true">
              <FlipRow
                className="tool-call-preview-row tool-call-preview-main"
                flipKey={previewEntry.task_id}
              >
                <span
                  className={`tool-call-preview-icon tool-call-preview-icon--${previewEntry.status}`}
                >
                  {statusIcon(previewEntry.status)}
                </span>
                <span className="tool-call-preview-name">[{previewEntry.agent_name}]</span>
                <span className="tool-call-preview-desc">{previewEntry.description}</span>
              </FlipRow>
              <FlipRow
                className={previewSubClass}
                flipKey={`${previewEntry.task_id}|${previewSub}`}
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
              <span className="tool-call-entry-icon">{statusIcon(entry.status)}</span>
              <span className="tool-call-entry-name">{entry.agent_name}</span>
              <span className="tool-call-entry-desc">{entry.description}</span>
              {entry.summary && (
                <span className="tool-call-entry-summary">{entry.summary}</span>
              )}
              {entry.error && (
                <span className="tool-call-entry-error">{entry.error}</span>
              )}
            </li>
          ))}
        </ul>
      </details>
    </div>
  );
}
