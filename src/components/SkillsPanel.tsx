import { useState, useEffect } from "react";
import { listSkills, saveSkill, deleteSkill } from "../api";
import type { Skill } from "../types";
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
        <h2>Skills</h2>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      {editing ? (
        <div className="skill-editor">
          <label>
            Name
            <input
              value={editing.name}
              onChange={(e) => setEditing({ ...editing, name: e.target.value })}
              placeholder="e.g. code-reviewer"
            />
          </label>
          <label>
            Description
            <input
              value={editing.description}
              onChange={(e) => setEditing({ ...editing, description: e.target.value })}
              placeholder="Short description and trigger condition"
            />
          </label>
          <label>
            <div className="field-title-row">
              <span>System Prompt</span>
              <button type="button" className="inline-edit-btn" onClick={openPromptEditor}>
                Edit
              </button>
            </div>
            <textarea
              rows={8}
              value={editing.system_prompt}
              onChange={(e) => setEditing({ ...editing, system_prompt: e.target.value })}
              placeholder="You are a helpful assistant that…"
            />
          </label>
          <label>
            Version
            <input
              value={editing.version ?? ""}
              onChange={(e) =>
                setEditing({ ...editing, version: e.target.value || undefined })
              }
              placeholder="e.g. 1.0.0 (optional)"
            />
          </label>
          <label>
            Allowed Commands
            <input
              value={(editing.allowed_commands ?? []).join(", ")}
              onChange={(e) => {
                const cmds = e.target.value
                  .split(",")
                  .map((s) => s.trim())
                  .filter(Boolean);
                setEditing({ ...editing, allowed_commands: cmds });
              }}
              placeholder="e.g. curl, wget, git  (empty = unrestricted)"
            />
            <span style={{ fontSize: "0.78rem", opacity: 0.65 }}>
              Comma-separated executable names. Leave empty to allow all.
            </span>
          </label>
          <div className="editor-actions">
            <button className="btn-primary" onClick={handleSave} disabled={!editing.name.trim()}>
              Save
            </button>
            <button className="btn-secondary" onClick={() => { setEditing(null); setOriginalName(null); setIsPromptEditorOpen(false); }}>
              Cancel
            </button>
          </div>

          {isPromptEditorOpen && (
            <Portal>
              <div className="prompt-editor-overlay" role="dialog" aria-modal="true" aria-label="Edit system prompt">
                <div className="prompt-editor-shell">
                  <div className="prompt-editor-header">
                    <h3>System Prompt Editor</h3>
                    <div className="prompt-editor-actions">
                      <button type="button" className="btn-secondary" onClick={closePromptEditor}>
                        Cancel
                      </button>
                      <button type="button" className="btn-primary" onClick={applyPromptEditor}>
                        Done
                      </button>
                    </div>
                  </div>

                  <div className="prompt-editor-body">
                    <div className="prompt-column">
                      <span>Markdown</span>
                      <textarea
                        className="prompt-editor-textarea"
                        value={promptDraft}
                        onChange={(e) => setPromptDraft(e.target.value)}
                        placeholder="Write your system prompt in Markdown..."
                      />
                    </div>

                    <div className="prompt-column">
                      <span>Preview</span>
                      <div className="prompt-preview">
                        {promptDraft.trim() ? (
                          <MarkdownPreview content={promptDraft} />
                        ) : (
                          <p className="prompt-preview-empty">Markdown preview will appear here.</p>
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
                    <label className="skill-toggle" title={isActive ? "Disable" : "Enable"}>
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
                      <button className="skill-action-btn" onClick={() => startEdit(skill)} title="Edit">
                        ✎
                      </button>
                      <button className="skill-action-btn danger" onClick={() => handleDelete(skill.name)} title="Delete">
                        ✕
                      </button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <div className="skills-footer">
            <button className="btn-primary" onClick={() => { setEditing(emptySkill()); setOriginalName(null); }}>
              + New Skill
            </button>
          </div>
        </>
      )}
    </div>
  );
}
