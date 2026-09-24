import { useState, useEffect } from "react";
import { listProfiles, saveProfile, deleteProfile, applyProfile, getConfig, listMcpServers, listSubAgents, getAgentOrchestration } from "../api";
import type { Profile } from "../types";
import { useI18n } from "../i18n";
import { FontAwesomeIcon, faXmark } from "../icons";
import "./ProfilePanel.css";

interface Props {
    onClose: () => void;
    onProfileApplied?: () => void;
}

export function ProfilePanel({ onClose, onProfileApplied }: Props) {
    const { t, locale } = useI18n();
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
        if (!confirm(t("profile.deleteConfirm", { name }))) return;

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
                <h2>{t("profile.title")}</h2>
                <button className="close-btn" onClick={onClose} aria-label={t("common.close")}>
                    <FontAwesomeIcon icon={faXmark} />
                </button>
            </div>

            <div className="profile-body">
                <div className="profile-create">
                    <input
                        type="text"
                        value={newProfileName}
                        onChange={(e) => setNewProfileName(e.target.value)}
                        placeholder={t("profile.newPlaceholder")}
                        onKeyDown={(e) => e.key === "Enter" && handleSaveProfile()}
                    />
                    <button
                        className="btn-primary"
                        onClick={handleSaveProfile}
                        disabled={!newProfileName.trim() || saving}
                    >
                        {saving ? t("common.saving") : t("profile.saveCurrent")}
                    </button>
                </div>

                <div className="profile-list">
                    {profiles.length === 0 ? (
                        <div className="profile-empty">{t("profile.empty")}</div>
                    ) : (
                        profiles.map((profile) => (
                            <div key={profile.name} className="profile-item">
                                <div className="profile-info">
                                    <div className="profile-name">{profile.name}</div>
                                    <div className="profile-meta">
                                        <span>{t("profile.metaSkills", { count: profile.selected_skills.length })}</span>
                                        <span>{t("profile.metaTools", { count: profile.selected_tools.length })}</span>
                                        <span>{t("profile.metaAgents", { count: profile.agents.length })}</span>
                                        <span>{t("profile.metaMcp", { count: profile.mcp_servers.length })}</span>
                                    </div>
                                    <div className="profile-date">
                                        {t("profile.updated", { date: new Date(profile.updated_at).toLocaleString(locale) })}
                                    </div>
                                </div>
                                <div className="profile-actions">
                                    <button
                                        className="btn-secondary"
                                        onClick={() => handleApplyProfile(profile.name)}
                                        disabled={applying !== null}
                                    >
                                        {applying === profile.name ? t("common.applying") : t("common.apply")}
                                    </button>
                                    <button
                                        className="btn-danger"
                                        onClick={() => handleDeleteProfile(profile.name)}
                                    >
                                        {t("common.delete")}
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
