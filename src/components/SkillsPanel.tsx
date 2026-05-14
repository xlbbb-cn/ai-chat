import { useState, useEffect } from "react";
import { listSkills, saveSkill, deleteSkill } from "../api";
import type { Skill } from "../types";
import "./SkillsPanel.css";

interface Props {
  activeSkillId: string | null;
  onSelect: (id: string | null) => void;
  onClose: () => void;
}

const emptySkill = (): Skill => ({
  id: crypto.randomUUID(),
  name: "",
  description: "",
  system_prompt: "",
  allow_commands: false,
});

export function SkillsPanel({ activeSkillId, onSelect, onClose }: Props) {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [editing, setEditing] = useState<Skill | null>(null);

  useEffect(() => {
    listSkills().then(setSkills).catch(console.error);
  }, []);

  async function handleSave() {
    if (!editing) return;
    await saveSkill(editing);
    const updated = await listSkills();
    setSkills(updated);
    setEditing(null);
  }

  async function handleDelete(id: string) {
    await deleteSkill(id);
    setSkills((s) => s.filter((x) => x.id !== id));
    if (activeSkillId === id) onSelect(null);
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
              placeholder="e.g. Code Reviewer"
            />
          </label>
          <label>
            Description
            <input
              value={editing.description}
              onChange={(e) => setEditing({ ...editing, description: e.target.value })}
              placeholder="Short description"
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
          <label style={{ flexDirection: 'row', alignItems: 'center', gap: '8px', cursor: 'pointer' }}>
            <input
              type="checkbox"
              checked={editing.allow_commands ?? false}
              onChange={(e) => setEditing({ ...editing, allow_commands: e.target.checked })}
            />
            Allow command execution (bash / python / powershell)
          </label>
          <div className="editor-actions">
            <button className="btn-primary" onClick={handleSave}>Save</button>
            <button className="btn-secondary" onClick={() => setEditing(null)}>Cancel</button>
          </div>
        </div>
      ) : (
        <>
          <div className="skills-list">
            <div
              className={`skill-item ${activeSkillId === null ? "active" : ""}`}
              onClick={() => onSelect(null)}
            >
              <span className="skill-name">No Skill</span>
              <span className="skill-desc">Default assistant behavior</span>
            </div>
            {skills.map((skill) => (
              <div
                key={skill.id}
                className={`skill-item ${activeSkillId === skill.id ? "active" : ""}`}
                onClick={() => onSelect(skill.id)}
              >
                <span className="skill-name">
                  {skill.name}
                  {skill.allow_commands && <span title="Command execution enabled" style={{ marginLeft: '6px', fontSize: '0.75rem', opacity: 0.7 }}>⚙️</span>}
                </span>
                <span className="skill-desc">{skill.description}</span>
                <div className="skill-actions" onClick={(e) => e.stopPropagation()}>
                  <button onClick={() => setEditing(skill)}>Edit</button>
                  <button className="danger" onClick={() => handleDelete(skill.id)}>Delete</button>
                </div>
              </div>
            ))}
          </div>
          <div className="skills-footer">
            <button className="btn-primary" onClick={() => setEditing(emptySkill())}>
              + New Skill
            </button>
          </div>
        </>
      )}
    </div>
  );
}
