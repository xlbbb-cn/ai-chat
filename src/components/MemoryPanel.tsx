import { useEffect, useState } from "react";
import { listArchivedSkills, listEvolutionMemory, readEvolutionEntry } from "../api";
import type { ArchivedSkill, EvolutionMemoryEntry } from "../types";
import { useI18n } from "../i18n";
import {
  FontAwesomeIcon,
  faBoxArchive,
  faChevronDown,
  faChevronRight,
  faXmark,
} from "../icons";
import "./MemoryPanel.css";

interface Props {
  onClose: () => void;
  /** Bumped after a workspace switch so the panel re-reads its data. */
  refreshKey?: number;
}

/**
 * Read-only view of the self-evolution memory.
 *
 * The rows are the exact ranking the prompt uses (`rank` and `injected` come
 * from the same backend function), so this panel answers "what does the model
 * actually see, and in what order". Nothing here mutates memory: tidying is an
 * explicit chat request that goes through `memory consolidate`.
 */
export function MemoryPanel({ onClose, refreshKey = 0 }: Props) {
  const { t } = useI18n();
  const [entries, setEntries] = useState<EvolutionMemoryEntry[]>([]);
  const [archives, setArchives] = useState<ArchivedSkill[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [detail, setDetail] = useState("");
  const [detailError, setDetailError] = useState("");
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    Promise.all([listEvolutionMemory(), listArchivedSkills()])
      .then(([rows, archived]) => {
        if (cancelled) return;
        setEntries(rows);
        setArchives(archived);
      })
      .catch(console.error)
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [refreshKey]);

  async function toggleDetail(key: string) {
    if (expanded === key) {
      setExpanded(null);
      setDetail("");
      setDetailError("");
      return;
    }
    setExpanded(key);
    setDetail("");
    setDetailError("");
    try {
      setDetail(await readEvolutionEntry(key));
    } catch (err) {
      setDetailError(`${t("memoryPanel.detailFailed")} ${String(err)}`);
    }
  }

  const injected = entries.filter((entry) => entry.injected).length;
  const totalChars = entries.reduce((sum, entry) => sum + entry.chars, 0);
  const archivedEntries = archives.reduce((sum, archive) => sum + archive.entries, 0);

  return (
    <div className="memory-panel">
      <div className="memory-header">
        <h2>{t("memoryPanel.title")}</h2>
        <button className="close-btn" onClick={onClose} aria-label={t("common.close")}>
          <FontAwesomeIcon icon={faXmark} />
        </button>
      </div>

      <div className="memory-summary">
        <span>{t("memoryPanel.summary", { count: entries.length, injected })}</span>
        {/* Mirrors EVOLUTION_CONTEXT_BUDGET_CHARS in `tools/memory.rs`. */}
        <span>{t("memoryPanel.budget", { chars: totalChars, budget: 4000 })}</span>
      </div>

      <div className="memory-list">
        {loading && <p className="memory-empty">{t("memoryPanel.loading")}</p>}
        {!loading && entries.length === 0 && (
          <p className="memory-empty">{t("memoryPanel.empty")}</p>
        )}
        {entries.map((entry) => (
          <div
            className={`memory-item${entry.injected ? "" : " is-omitted"}`}
            key={entry.key}
          >
            <button
              type="button"
              className="memory-row"
              onClick={() => void toggleDetail(entry.key)}
              aria-expanded={expanded === entry.key}
            >
              <span className="memory-rank">#{entry.rank}</span>
              <span className="memory-main">
                <span className="memory-title">{entry.title || entry.key}</span>
                <span className="memory-meta">
                  {entry.kind && <span className="memory-badge">{entry.kind}</span>}
                  <span className="memory-badge">{entry.importance}★</span>
                  <span>{t("memoryPanel.evidence", { count: entry.evidence })}</span>
                  <span>{t("memoryPanel.uses", { count: entry.uses })}</span>
                  {entry.pinned && (
                    <span className="memory-badge">{t("memoryPanel.pinned")}</span>
                  )}
                  {entry.source === "auto" && (
                    <span className="memory-badge">{t("memoryPanel.auto")}</span>
                  )}
                  {entry.tags.map((tag) => (
                    <span className="memory-tag" key={tag}>
                      {tag}
                    </span>
                  ))}
                </span>
              </span>
              <FontAwesomeIcon
                icon={expanded === entry.key ? faChevronDown : faChevronRight}
              />
            </button>
            {expanded === entry.key && (
              <pre className="memory-detail">
                {detailError || detail || t("memoryPanel.loading")}
              </pre>
            )}
          </div>
        ))}
      </div>

      {archives.length > 0 && (
        <div className="memory-archive">
          <div className="memory-archive-header">
            {t("memoryPanel.archivedTitle", { count: archivedEntries })}
          </div>
          {archives.map((archive) => (
            <div className="memory-archive-row" key={archive.name}>
              <FontAwesomeIcon icon={faBoxArchive} />
              <span className="memory-archive-name">{archive.name}</span>
              <span className="memory-archive-meta">
                {t("memoryPanel.archivedMeta", {
                  entries: archive.entries,
                  chars: archive.chars,
                })}
                {archive.skill_exists ? "" : t("memoryPanel.archivedMissing")}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
