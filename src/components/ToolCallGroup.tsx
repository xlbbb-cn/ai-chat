import type { Message, ToolCallEntry } from "../types";
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
  const runningEntry = entries.find((e) => e.status === "running");
  const isRunning = runningEntry !== undefined;
  const doneCount = entries.filter((e) => e.status === "done").length;
  const errorCount = entries.filter((e) => e.status === "error").length;

  let summaryLabel: string;
  if (isRunning) {
    summaryLabel = `[${runningEntry!.agent_name}] ${runningEntry!.description}`;
  } else if (errorCount > 0) {
    summaryLabel = `${entries.length} 个工具调用 — ${errorCount} 个失败`;
  } else {
    summaryLabel = `${doneCount} 个工具调用完成`;
  }

  return (
    <div className={`tool-call-group${isRunning ? " tool-call-group--running" : ""}`}>
      <details className="tool-call-group-details">
        <summary className="tool-call-group-summary">
          <span className={`tool-call-group-indicator${isRunning ? " running" : ""}`} />
          <span className="tool-call-group-label">{summaryLabel}</span>
          <span className="tool-call-group-count">{entries.length} 步</span>
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
