import type { Message, ToolCallEntry } from "../types";
import { useState } from "react";
import "./ToolCallGroup.css";

interface Props {
  message: Message;
}

function statusIcon(status: ToolCallEntry["status"]): string {
  if (status === "running") return "⟳";
  if (status === "done") return "✓";
  return "✕";
}

export function ToolCallGroup({ message }: Props) {
  const entries = message.tool_calls ?? [];
  const [open, setOpen] = useState(false);
  if (entries.length === 0) return null;
  const runningEntry = entries.find((e) => e.status === "running");
  const isRunning = runningEntry !== undefined;
  const doneCount = entries.filter((e) => e.status === "done").length;
  const errorCount = entries.filter((e) => e.status === "error").length;

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
          {!open && runningEntry && (
            <span className="tool-call-group-preview" aria-hidden="true">
              <span className="tool-call-preview-row tool-call-preview-main">
                <span
                  className={`tool-call-preview-icon tool-call-preview-icon--${runningEntry.status}`}
                >
                  {statusIcon(runningEntry.status)}
                </span>
                <span className="tool-call-preview-name">[{runningEntry.agent_name}]</span>
                <span className="tool-call-preview-desc">{runningEntry.description}</span>
              </span>
              <span className="tool-call-preview-row tool-call-preview-sub">
                {runningEntry.summary ?? runningEntry.error ?? "\u00A0"}
              </span>
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
