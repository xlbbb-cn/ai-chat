import { useState, useEffect } from "react";
import { listSkills, saveSkill, deleteSkill } from "../api";
import type { Skill } from "../types";
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

  useEffect(() => {
    listSkills().then(setSkills).catch(console.error);
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
  }

  function startEdit(skill: Skill) {
    setEditing({ ...skill });
    setOriginalName(skill.name);
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
            System Prompt
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
            <button className="btn-secondary" onClick={() => { setEditing(null); setOriginalName(null); }}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="skills-list">
            {skills.map((skill) => {
              const isActive = activeSkillIds.includes(skill.name);
              return (
                <div
                  key={skill.name}
                  className={`skill-item ${isActive ? "active" : ""}`}
                  onClick={() => onToggle(skill.name, !isActive)}
                >
                  <div className="skill-content">
                    <div className="skill-title-row">
                      <label className="toggle-switch">
                        <input
                          type="checkbox"
                          checked={isActive}
                          readOnly
                        />
                        <span className="toggle-switch-slider" />
                      </label>
                      <span className="skill-name" title={skill.name}>
                        {skill.name}
                      </span>
                    </div>
                    <span className="skill-desc" title={skill.description}>{skill.description}</span>
                    <div className="skill-actions-bottom" onClick={(e) => e.stopPropagation()}>
                      <button className="mcp-action-btn" onClick={() => startEdit(skill)} title="Edit">
                        ✎
                      </button>
                      <button className="mcp-action-btn danger" onClick={() => handleDelete(skill.name)} title="Delete">
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
