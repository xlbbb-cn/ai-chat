import { useEffect, useState } from "react";
import { deleteExperience, listExperiences } from "../api";
import type { ExperienceRow } from "../api";
import { useI18n } from "../i18n";
import {
  FontAwesomeIcon,
  faBoxArchive,
  faChevronDown,
  faChevronRight,
  faXmark,
} from "../icons";
import "./ExperiencePanel.css";

interface Props {
  onClose: () => void;
  /** Bumped after a workspace switch so the panel re-reads its data. */
  refreshKey?: number;
}

/**
 * Read-only view of the global experience store (`<app_data>/experiences/`).
 *
 * These packages are not skills: they never appear in the skills folder, the
 * `run_cmd` library or the MCP panel, and only the `experience` tool reaches
 * them. Nothing here edits a package — a package is created by `experience
 * pack` inside a self-evolution turn; this panel only lists, inspects and
 * archives one (archiving moves it to `.archive/`, it never deletes bytes).
 */
export function ExperiencePanel({ onClose, refreshKey = 0 }: Props) {
  const { t } = useI18n();
  const [rows, setRows] = useState<ExperienceRow[]>([]);
  const [store, setStore] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirmSlug, setConfirmSlug] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    listExperiences()
      .then((registry) => {
        if (cancelled) return;
        setRows(registry.experiences);
        setStore(registry.store);
      })
      .catch((err) => {
        if (!cancelled) setError(`${t("experiencePanel.loadFailed")} ${String(err)}`);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [refreshKey, reloadKey, t]);

  /** Interpreters plus recorded modules — one number for the row badge. */
  function dependencyCount(row: ExperienceRow): number {
    const modules = Object.values(row.requires?.packages ?? {}).reduce(
      (total, list) => total + list.length,
      0,
    );
    return (row.requires?.tools.length ?? 0) + modules;
  }

  /**
   * A package is a folder, so "installed" is a fact about this workspace: the parts that
   * `experience install` already copied here. No probing, no process starts.
   */
  function installedLabel(row: ExperienceRow): string {
    const done: string[] = [];
    if (row.installed?.skill) done.push(t("experiencePanel.partSkill"));
    if (row.installed?.mcp) done.push(t("experiencePanel.partMcp"));
    for (const name of row.installed?.scripts ?? []) done.push(name);
    return done.length > 0 ? done.join(" · ") : t("experiencePanel.installedNone");
  }

  async function archive(slug: string) {
    setBusy(slug);
    setError("");
    try {
      await deleteExperience(slug);
      setConfirmSlug(null);
      setExpanded(null);
      setReloadKey((value) => value + 1);
    } catch (err) {
      setError(`${t("experiencePanel.deleteFailed")} ${String(err)}`);
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="experience-panel">
      <div className="experience-header">
        <h2>{t("experiencePanel.title")}</h2>
        <button className="close-btn" onClick={onClose} aria-label={t("common.close")}>
          <FontAwesomeIcon icon={faXmark} />
        </button>
      </div>

      <div className="experience-summary">
        <span>{t("experiencePanel.summary", { count: rows.length })}</span>
        <span className="experience-store" title={store}>
          {store}
        </span>
      </div>

      {error && <p className="experience-error">{error}</p>}

      <div className="experience-list">
        {loading && <p className="experience-empty">{t("experiencePanel.loading")}</p>}
        {!loading && rows.length === 0 && (
          <p className="experience-empty">{t("experiencePanel.empty")}</p>
        )}
        {rows.map((row) => {
          const open = expanded === row.slug;
          const confirming = confirmSlug === row.slug;
          return (
            <div className="experience-item" key={row.slug}>
              <button
                type="button"
                className="experience-row"
                onClick={() => setExpanded(open ? null : row.slug)}
                aria-expanded={open}
              >
                <span className="experience-main">
                  <span className="experience-title">{row.title || row.slug}</span>
                  <span className="experience-meta">
                    <span className="experience-badge">{row.state || "draft"}</span>
                    {row.when.length > 0 && (
                      <span className="experience-badge">{row.when[0]}</span>
                    )}
                    {row.when.length > 1 && (
                      <span className="experience-badge">+{row.when.length - 1}</span>
                    )}
                    <span>
                      {row.tools.length > 0
                        ? t("experiencePanel.toolCount", { count: row.tools.length })
                        : t("experiencePanel.noTools")}
                    </span>
                    {dependencyCount(row) > 0 && (
                      <span className="experience-badge" title={row.requires_summary}>
                        {t("experiencePanel.deps", { count: dependencyCount(row) })}
                      </span>
                    )}
                    <span className="experience-slug">{row.slug}</span>
                  </span>
                </span>
                <FontAwesomeIcon icon={open ? faChevronDown : faChevronRight} />
              </button>

              {open && (
                <div className="experience-detail">
                  <dl className="experience-facts">
                    <dt>{t("experiencePanel.labelWhen")}</dt>
                    <dd>{row.when.length > 0 ? row.when.join(", ") : "—"}</dd>
                    <dt>{t("experiencePanel.labelTools")}</dt>
                    <dd>{row.tools.length > 0 ? row.tools.join(", ") : "—"}</dd>
                    <dt>{t("experiencePanel.labelRequires")}</dt>
                    <dd>
                      {row.requires_summary || t("experiencePanel.requiresNone")}
                      {row.requires?.notes ? (
                        <span className="experience-note">{row.requires.notes}</span>
                      ) : null}
                      {row.requires?.checked ? (
                        <span className="experience-note">
                          {t("experiencePanel.requiresFrom", { host: row.requires.checked })}
                        </span>
                      ) : null}
                    </dd>
                    <dt>{t("experiencePanel.labelParts")}</dt>
                    <dd>
                      {row.tools.length > 0
                        ? row.tools.join(" · ")
                        : t("experiencePanel.partsNone")}
                    </dd>
                    <dt>{t("experiencePanel.labelInstalled")}</dt>
                    <dd>{installedLabel(row)}</dd>
                    <dt>{t("experiencePanel.labelSource")}</dt>
                    <dd>{row.source_workspace || "—"}</dd>
                    <dt>{t("experiencePanel.labelDir")}</dt>
                    <dd className="experience-path">{row.dir}</dd>
                  </dl>
                  <div className="experience-actions">
                    <button
                      type="button"
                      className={`experience-archive-btn${confirming ? " is-confirming" : ""}`}
                      disabled={busy === row.slug}
                      onClick={() => {
                        if (!confirming) {
                          setConfirmSlug(row.slug);
                          return;
                        }
                        void archive(row.slug);
                      }}
                    >
                      <FontAwesomeIcon icon={faBoxArchive} />
                      {confirming
                        ? t("experiencePanel.deleteConfirm", { slug: row.slug })
                        : t("experiencePanel.delete")}
                    </button>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
