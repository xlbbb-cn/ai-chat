import { useState, useEffect, useCallback } from "react";
import {
  listApiRequests,
  getApiRequest,
  deleteApiRequest,
  clearApiRequests,
  type ApiRequestRecord,
  type ApiRequestDetail,
} from "../api";
import "./RequestMonitorPanel.css";

interface Props {
  onClose: () => void;
}

function formatMs(ms: number) {
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
}

function prettyJson(raw: string) {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

export function RequestMonitorPanel({ onClose }: Props) {
  const [records, setRecords] = useState<ApiRequestRecord[]>([]);
  const [selected, setSelected] = useState<ApiRequestDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<"request" | "response" | "tools">("request");

  const refresh = useCallback(() => {
    listApiRequests().then(setRecords).catch(console.error);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  async function handleSelect(id: number) {
    setLoading(true);
    try {
      const detail = await getApiRequest(id);
      setSelected(detail);
      setActiveTab("request");
    } catch (err) {
      console.error(err);
    } finally {
      setLoading(false);
    }
  }

  async function handleDelete(e: React.MouseEvent, id: number) {
    e.stopPropagation();
    await deleteApiRequest(id);
    if (selected?.id === id) setSelected(null);
    setRecords((prev) => prev.filter((r) => r.id !== id));
  }

  async function handleClear() {
    if (!confirm("Clear all request logs?")) return;
    await clearApiRequests();
    setRecords([]);
    setSelected(null);
  }

  return (
    <div className="monitor-panel">
      <div className="monitor-header">
        <h2>Request Monitor</h2>
        <div className="monitor-header-actions">
          <button className="monitor-btn" onClick={refresh} title="Refresh">↻</button>
          <button className="monitor-btn danger" onClick={handleClear} title="Clear all">🗑 Clear</button>
          <button className="close-btn" onClick={onClose}>✕</button>
        </div>
      </div>

      <div className="monitor-body">
        {/* Left: request list */}
        <div className="monitor-list">
          {records.length === 0 && (
            <p className="monitor-empty">No API requests logged yet.</p>
          )}
          {records.map((r) => (
            <div
              key={r.id}
              className={`monitor-item ${selected?.id === r.id ? "active" : ""} ${r.error ? "error" : ""}`}
              onClick={() => handleSelect(r.id)}
            >
              <div className="monitor-item-top">
                <span className="monitor-model">{r.model}</span>
                <span className="monitor-duration">{formatMs(r.duration_ms)}</span>
              </div>
              <div className="monitor-item-mid">
                <span className="monitor-tokens">{r.prompt_tokens}↑ {r.completion_tokens}↓</span>
                <span className={`monitor-finish ${r.finish_reason}`}>{r.error ? "error" : r.finish_reason}</span>
              </div>
              {r.response_preview && !r.error && (
                <div className="monitor-preview">{r.response_preview}</div>
              )}
              {r.error && <div className="monitor-preview error-text">{r.error.slice(0, 120)}</div>}
              <div className="monitor-item-footer">
                <span className="monitor-ts">{r.timestamp}</span>
                <button
                  className="monitor-del-btn"
                  onClick={(e) => handleDelete(e, r.id)}
                  title="Delete"
                >✕</button>
              </div>
            </div>
          ))}
        </div>

        {/* Right: detail */}
        <div className="monitor-detail">
          {!selected && !loading && (
            <div className="monitor-detail-empty">Select a request to inspect</div>
          )}
          {loading && <div className="monitor-detail-empty">Loading…</div>}
          {selected && !loading && (
            <>
              <div className="monitor-detail-meta">
                <span><strong>Model:</strong> {selected.model}</span>
                <span><strong>Finish:</strong> {selected.finish_reason}</span>
                <span><strong>Tokens:</strong> {selected.prompt_tokens}↑ / {selected.completion_tokens}↓</span>
                <span><strong>Duration:</strong> {formatMs(selected.duration_ms)}</span>
                <span><strong>Session:</strong> {selected.session_id.slice(0, 8)}…</span>
                <span><strong>Time:</strong> {selected.timestamp}</span>
              </div>
              {selected.error && (
                <div className="monitor-error-box">{selected.error}</div>
              )}
              <div className="monitor-tabs">
                <button
                  className={activeTab === "request" ? "active" : ""}
                  onClick={() => setActiveTab("request")}
                >Request</button>
                <button
                  className={activeTab === "response" ? "active" : ""}
                  onClick={() => setActiveTab("response")}
                >Response</button>
                {selected.tool_calls && (
                  <button
                    className={activeTab === "tools" ? "active" : ""}
                    onClick={() => setActiveTab("tools")}
                  >Tool Calls</button>
                )}
              </div>
              <div className="monitor-code-wrap">
                {activeTab === "request" && (
                  <pre className="monitor-code">{prettyJson(selected.request_body)}</pre>
                )}
                {activeTab === "response" && (
                  <pre className="monitor-code">{selected.response_content || "(empty)"}</pre>
                )}
                {activeTab === "tools" && (
                  <pre className="monitor-code">{prettyJson(selected.tool_calls)}</pre>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
