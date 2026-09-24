import { useState, useEffect } from "react";
import { listSkills, saveSkill, deleteSkill } from "../api";
import type { Skill } from "../types";
import { useI18n } from "../i18n";
import { FontAwesomeIcon, faPen, faXmark } from "../icons";
import { MarkdownPreview } from "./MarkdownPreview";
import { Portal } from "./Portal";
import "./SkillsPanel.css";

interface Props {
  activeSkillIds: string[];
  onToggle: (name: string, active: boolean) => void;
  onClose: () => void;
}

const emptySkill = (): Skill => ({
  name: "",
  description: "",
  system_prompt: "",
  allowed_commands: [],
});

export function SkillsPanel({ activeSkillIds, onToggle, onClose }: Props) {
  const { t } = useI18n();
  const [skills, setSkills] = useState<Skill[]>([]);
  const [editing, setEditing] = useState<Skill | null>(null);
  const [originalName, setOriginalName] = useState<string | null>(null);
  const [isPromptEditorOpen, setIsPromptEditorOpen] = useState(false);
  const [promptDraft, setPromptDraft] = useState("");

  useEffect(() => {
    listSkills()
      .then((skills) => setSkills(skills.sort((a, b) => a.name.localeCompare(b.name))))
      .catch(console.error);
  }, []);

  async function handleSave() {
    if (!editing || !editing.name.trim()) return;
    await saveSkill(editing);
    // If name changed, delete the old file
    if (originalName && originalName !== editing.name) {
      await deleteSkill(originalName).catch(() => { });
      if (activeSkillIds.includes(originalName)) {
        onToggle(originalName, false);
        onToggle(editing.name, true);
      }
    }
    const updated = await listSkills();
    setSkills(updated);
    setEditing(null);
    setOriginalName(null);
    setIsPromptEditorOpen(false);
  }

  function startEdit(skill: Skill) {
    setEditing({ ...skill });
    setOriginalName(skill.name);
    setIsPromptEditorOpen(false);
  }

  function openPromptEditor() {
    if (!editing) return;
    setPromptDraft(editing.system_prompt ?? "");
    setIsPromptEditorOpen(true);
  }

  function closePromptEditor() {
    setIsPromptEditorOpen(false);
  }

  async function applyPromptEditor() {
    if (!editing) return;
    const updatedSkill: Skill = { ...editing, system_prompt: promptDraft };
    setEditing(updatedSkill);

    // Persist immediately for prompt-only editing when no rename operation is pending.
    if (!originalName || originalName === updatedSkill.name) {
      await saveSkill(updatedSkill);
      const updated = await listSkills();
      setSkills(updated);
    }

    setIsPromptEditorOpen(false);
  }

  async function handleDelete(name: string) {
    await deleteSkill(name);
    setSkills((s) => s.filter((x) => x.name !== name));
    if (activeSkillIds.includes(name)) onToggle(name, false);
  }

  return (
    <div className="skills-panel">
      <div className="skills-header">
        <h2>{t("skills.title")}</h2>
        <button className="close-btn" onClick={onClose} aria-label={t("common.close")}>
          <FontAwesomeIcon icon={faXmark} />
        </button>
      </div>

      {editing ? (
        <div className="skill-editor">
          <label>
            {t("skills.name")}
            <input
              value={editing.name}
              onChange={(e) => setEditing({ ...editing, name: e.target.value })}
              placeholder={t("skills.namePlaceholder")}
            />
          </label>
          <label>
            {t("skills.description")}
            <input
              value={editing.description}
              onChange={(e) => setEditing({ ...editing, description: e.target.value })}
              placeholder={t("skills.descriptionPlaceholder")}
            />
          </label>
          <label>
            <div className="field-title-row">
              <span>{t("skills.systemPrompt")}</span>
              <button type="button" className="inline-edit-btn" onClick={openPromptEditor}>
                {t("common.edit")}
              </button>
            </div>
            <textarea
              rows={8}
              value={editing.system_prompt}
              onChange={(e) => setEditing({ ...editing, system_prompt: e.target.value })}
              placeholder={t("skills.systemPromptPlaceholder")}
            />
          </label>
          <label>
            {t("skills.version")}
            <input
              value={editing.version ?? ""}
              onChange={(e) =>
                setEditing({ ...editing, version: e.target.value || undefined })
              }
              placeholder={t("skills.versionPlaceholder")}
            />
          </label>
          <label>
            {t("skills.allowedCommands")}
            <input
              value={(editing.allowed_commands ?? []).join(", ")}
              onChange={(e) => {
                const cmds = e.target.value
                  .split(",")
                  .map((s) => s.trim())
                  .filter(Boolean);
                setEditing({ ...editing, allowed_commands: cmds });
              }}
              placeholder={t("skills.allowedCommandsPlaceholder")}
            />
            <span style={{ fontSize: "0.78rem", opacity: 0.65 }}>
              {t("skills.allowedCommandsHint")}
            </span>
          </label>
          <div className="editor-actions">
            <button className="btn-primary" onClick={handleSave} disabled={!editing.name.trim()}>
              {t("common.save")}
            </button>
            <button className="btn-secondary" onClick={() => { setEditing(null); setOriginalName(null); setIsPromptEditorOpen(false); }}>
              {t("common.cancel")}
            </button>
          </div>

          {isPromptEditorOpen && (
            <Portal>
              <div className="prompt-editor-overlay" role="dialog" aria-modal="true" aria-label={t("skills.promptEditor.aria")}>
                <div className="prompt-editor-shell">
                  <div className="prompt-editor-header">
                    <h3>{t("skills.promptEditor.title")}</h3>
                    <div className="prompt-editor-actions">
                      <button type="button" className="btn-secondary" onClick={closePromptEditor}>
                        {t("common.cancel")}
                      </button>
                      <button type="button" className="btn-primary" onClick={applyPromptEditor}>
                        {t("common.done")}
                      </button>
                    </div>
                  </div>

                  <div className="prompt-editor-body">
                    <div className="prompt-column">
                      <span>{t("app.markdownColumn")}</span>
                      <textarea
                        className="prompt-editor-textarea"
                        value={promptDraft}
                        onChange={(e) => setPromptDraft(e.target.value)}
                        placeholder={t("skills.promptEditor.placeholder")}
                      />
                    </div>

                    <div className="prompt-column">
                      <span>{t("app.previewColumn")}</span>
                      <div className="prompt-preview">
                        {promptDraft.trim() ? (
                          <MarkdownPreview content={promptDraft} />
                        ) : (
                          <p className="prompt-preview-empty">{t("app.markdownPreviewEmpty")}</p>
                        )}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </Portal>
          )}
        </div>
      ) : (
        <>
          <div className="skills-list">
            {skills.map((skill) => {
              const isActive = activeSkillIds.includes(skill.name);
              return (
                <div
                  key={skill.name}
                  className={`skill-item ${!isActive ? "disabled" : ""}`}
                >
                  <div className="skill-row">
                    <label className="skill-toggle" title={isActive ? t("common.disable") : t("common.enable")}>
                      <input
                        type="checkbox"
                        checked={isActive}
                        onChange={(e) => onToggle(skill.name, e.target.checked)}
                      />
                      <span className="skill-toggle-slider" />
                    </label>
                    <div className="skill-info">
                      <span
                        className={`skill-name ${isActive ? "active" : ""}`}
                        onClick={() => onToggle(skill.name, !isActive)}
                      >
                        {skill.name}
                      </span>
                      <span className="skill-transport">{skill.description}</span>
                    </div>
                    <div className="skill-actions">
                      <button className="skill-action-btn" onClick={() => startEdit(skill)} title={t("common.edit")}>
                        <FontAwesomeIcon icon={faPen} />
                      </button>
                      <button className="skill-action-btn danger" onClick={() => handleDelete(skill.name)} title={t("common.delete")}>
                        <FontAwesomeIcon icon={faXmark} />
                      </button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <div className="skills-footer">
            <button className="btn-primary" onClick={() => { setEditing(emptySkill()); setOriginalName(null); }}>
              {t("skills.newSkill")}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
