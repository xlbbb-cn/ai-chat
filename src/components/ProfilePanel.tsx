import { useState, useEffect } from "react";
import { listProfiles, saveProfile, deleteProfile, applyProfile, getConfig, listMcpServers, listSubAgents, getAgentOrchestration } from "../api";
import type { Profile } from "../types";
import "./ProfilePanel.css";

interface Props {
    onClose: () => void;
    onProfileApplied?: () => void;
}

export function ProfilePanel({ onClose, onProfileApplied }: Props) {
    const [profiles, setProfiles] = useState<Profile[]>([]);
    const [newProfileName, setNewProfileName] = useState("");
    const [saving, setSaving] = useState(false);
    const [applying, setApplying] = useState<string | null>(null);

    useEffect(() => {
        loadProfiles();
    }, []);

    async function loadProfiles() {
        try {
            const list = await listProfiles();
            setProfiles(list);
        } catch (err) {
            console.error("Failed to load profiles:", err);
        }
    }

    async function handleSaveProfile() {
        if (!newProfileName.trim()) return;

        setSaving(true);
        try {
            // Gather current configuration
            const [config, mcpServers, agents, orchestration] = await Promise.all([
                getConfig(),
                listMcpServers(),
                listSubAgents(),
                getAgentOrchestration(),
            ]);

            const profile: Profile = {
                name: newProfileName.trim(),
                selected_skills: config.selected_skills || [],
                selected_tools: config.selected_tools || [],
                agents: agents,
                orchestration: orchestration,
                mcp_servers: mcpServers,
                created_at: new Date().toISOString(),
                updated_at: new Date().toISOString(),
            };

            await saveProfile(profile);
            setNewProfileName("");
            await loadProfiles();
        } catch (err) {
            console.error("Failed to save profile:", err);
        } finally {
            setSaving(false);
        }
    }

    async function handleApplyProfile(name: string) {
        setApplying(name);
        try {
            await applyProfile(name);
            onProfileApplied?.();
        } catch (err) {
            console.error("Failed to apply profile:", err);
        } finally {
            setApplying(null);
        }
    }

    async function handleDeleteProfile(name: string) {
        if (!confirm(`Delete profile "${name}"?`)) return;

        try {
            await deleteProfile(name);
            await loadProfiles();
        } catch (err) {
            console.error("Failed to delete profile:", err);
        }
    }

    return (
        <div className="profile-panel">
            <div className="profile-header">
                <h2>Profiles</h2>
                <button className="close-btn" onClick={onClose}>✕</button>
            </div>

            <div className="profile-body">
                <div className="profile-create">
                    <input
                        type="text"
                        value={newProfileName}
                        onChange={(e) => setNewProfileName(e.target.value)}
                        placeholder="New profile name"
                        onKeyDown={(e) => e.key === "Enter" && handleSaveProfile()}
                    />
                    <button
                        className="btn-primary"
                        onClick={handleSaveProfile}
                        disabled={!newProfileName.trim() || saving}
                    >
                        {saving ? "Saving..." : "Save Current Config"}
                    </button>
                </div>

                <div className="profile-list">
                    {profiles.length === 0 ? (
                        <div className="profile-empty">No profiles saved</div>
                    ) : (
                        profiles.map((profile) => (
                            <div key={profile.name} className="profile-item">
                                <div className="profile-info">
                                    <div className="profile-name">{profile.name}</div>
                                    <div className="profile-meta">
                                        <span>{profile.selected_skills.length} skills</span>
                                        <span>{profile.selected_tools.length} tools</span>
                                        <span>{profile.agents.length} agents</span>
                                        <span>{profile.mcp_servers.length} MCP servers</span>
                                    </div>
                                    <div className="profile-date">
                                        Updated: {new Date(profile.updated_at).toLocaleString()}
                                    </div>
                                </div>
                                <div className="profile-actions">
                                    <button
                                        className="btn-secondary"
                                        onClick={() => handleApplyProfile(profile.name)}
                                        disabled={applying !== null}
                                    >
                                        {applying === profile.name ? "Applying..." : "Apply"}
                                    </button>
                                    <button
                                        className="btn-danger"
                                        onClick={() => handleDeleteProfile(profile.name)}
                                    >
                                        Delete
                                    </button>
                                </div>
                            </div>
                        ))
                    )}
                </div>
            </div>
        </div>
    );
}
