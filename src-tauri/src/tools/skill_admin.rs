//! Maintenance-only skill administration.
//!
//! The evolution maintenance turn may rewrite and merge the assistant's own
//! skills, but a generic `file_actions` write cannot tell a self-evolution
//! skill from one the user authored in the Skills panel, and it cannot enable
//! the merged result. `skill_merge` is the one narrow operation that does all
//! of it atomically enough to be safe:
//!
//! 1. validate the target and every source against the workspace status file
//!    (only `source: self-evolution` is accepted; `user` / unknown is refused),
//! 2. write the merged `skill.md` (a `.bak.N` copy of the old file is kept),
//! 3. enable the merged skill in `skills.status`,
//! 4. back up each source folder as `<folder>.bak.N` and delete it.
//!
//! The tool is advertised only to the evolution maintenance turn and the
//! dispatcher additionally checks the mission id, so the main agent and every
//! sub-agent cannot reach it.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use super::file_actions::backup_protected_path;
use super::emit_tool_progress;
use crate::skills;
use crate::workspace;
use crate::AppConfig;

/// The `skill_merge` tool definition handed to the maintenance loop.
pub(crate) fn merge_tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "skill_merge",
            "description": "Merge one or more self-evolution skills into a target skill folder. \
                Writes the merged SKILL.md (keeping a .bak.N copy of the old file), enables the \
                target in the workspace skill status, then backs up and deletes every source \
                folder as <folder>.bak.N. Only skills whose status source is 'self-evolution' \
                are accepted — a skill authored in the Skills panel ('user') or one with an \
                unknown source is refused. This is the only supported way to rewrite or merge \
                skills during evolution maintenance; never delete a skill with file_actions.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Skill folder to keep (or create). Use the folder name shown by file_actions, e.g. 'deploy-docker'."
                    },
                    "sources": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Skill folders/names to fold into the target and delete. Pass [] when only rewriting/enabling the target."
                    },
                    "skill_md": {
                        "type": "string",
                        "description": "The complete merged SKILL.md: YAML frontmatter (name, description) plus the full body. Keep every concrete command, path and gotcha from the sources."
                    }
                },
                "required": ["target", "sources", "skill_md"]
            }
        }
    })
}

fn is_self_evolution(workspace_dir: &Path, skill_name: &str) -> bool {
    skills::skill_source(workspace_dir, skill_name).as_deref() == Some("self-evolution")
}

/// Compare two paths after resolving symlinks / Windows case differences.
/// Non-existent paths fall back to a plain comparison.
fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn source_label(workspace_dir: &Path, skill_name: &str) -> String {
    match skills::skill_source(workspace_dir, skill_name).as_deref() {
        Some("self-evolution") => "self-evolution".to_string(),
        Some("user") => "user (Skills panel)".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => "unknown".to_string(),
    }
}

fn unique_sources(args: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    args["sources"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .filter(|name| seen.insert(name.to_string()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Tolerate a model that wraps the document in a Markdown code fence even
/// though the prompt asks for the raw SKILL.md. The written file must start
/// with the YAML frontmatter, never with ```.
fn clean_skill_md(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_string();
    };
    let body_start = rest.find('\n').map(|index| index + 1).unwrap_or(0);
    let body = rest[body_start..].trim_end();
    let body = body.strip_suffix("```").unwrap_or(body);
    body.trim().to_string()
}

/// Handle one `skill_merge` call. Synchronous: every step is short file I/O.
pub(crate) fn handle_skill_merge(
    app: &AppHandle,
    args: &Value,
    workspace_dir: &Path,
    config: &AppConfig,
    mission_id: Option<&str>,
) -> String {
    if !config.self_evolution_mode {
        return "Error: skill_merge is only available while self-evolution mode is on.".to_string();
    }

    let target = args["target"].as_str().unwrap_or("").trim().to_string();
    let _ = emit_tool_progress(
        app,
        mission_id,
        format!("🧩 *Merging skills into {}*\n\n", target),
    );

    merge_skill(workspace_dir, args)
}

/// Core merge logic without the Tauri progress side effect, so it is testable.
fn merge_skill(workspace_dir: &Path, args: &Value) -> String {
    let target = args["target"].as_str().unwrap_or("").trim().to_string();
    if target.is_empty() {
        return "Error: 'target' is required.".to_string();
    }
    if let Err(error) = skills::validate_skill_name(&target) {
        return format!("Error: invalid target: {error}");
    }

    let skill_md = clean_skill_md(args["skill_md"].as_str().unwrap_or(""));
    if skill_md.trim().is_empty() {
        return "Error: 'skill_md' is required (the complete merged SKILL.md).".to_string();
    }
    let skill = match skills::parse_skill_md(&skill_md) {
        Ok(skill) => skill,
        Err(error) => return format!("Error: merged skill_md is invalid: {error}"),
    };
    if let Err(error) = skills::validate_skill_name(&skill.name) {
        return format!("Error: invalid skill name in skill_md: {error}");
    }

    let skills_root = workspace::skills_root(workspace_dir);
    let target_dir = skills_root.join(&target);

    // The merged frontmatter name must not shadow a skill that lives in a
    // different folder (especially a user-authored one): the status file is
    // keyed by that name, so writing here would silently retarget it.
    if let Ok(existing) = skills::resolve_skill_by_name(&skills_root, &skill.name) {
        if !same_path(&existing.skill_dir, &target_dir) {
            return format!(
                "Error: merged skill name '{}' already belongs to folder '{}'. Use that folder as the target, or choose another skill name.",
                skill.name,
                existing
                    .skill_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("?")
            );
        }
        if !is_self_evolution(workspace_dir, &existing.skill.name) {
            return format!(
                "Error: merged skill name '{}' belongs to a {} skill — only self-evolution skills may be merged.",
                skill.name,
                source_label(workspace_dir, &existing.skill.name)
            );
        }
    }

    // The target may be new. If it already exists, it must be one of ours.
    if target_dir.exists() {
        if !target_dir.is_dir() {
            return format!(
                "Error: target '{}' exists but is not a skill folder.",
                target_dir.display()
            );
        }
        let existing = match skills::resolve_skill_by_name(&skills_root, &target) {
            Ok(existing) => existing,
            Err(error) => {
                return format!(
                    "Error: target folder '{}' exists but is not a readable skill: {error}",
                    target
                );
            }
        };
        if !is_self_evolution(workspace_dir, &existing.skill.name) {
            return format!(
                "Error: target skill '{}' is {} — only self-evolution skills may be merged.",
                existing.skill.name,
                source_label(workspace_dir, &existing.skill.name)
            );
        }
    }

    let sources = unique_sources(args);
    let mut resolved_sources = Vec::new();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    for source in &sources {
        if let Err(error) = skills::validate_skill_name(source) {
            return format!("Error: invalid source '{source}': {error}");
        }
        let resolved = match skills::resolve_skill_by_name(&skills_root, source) {
            Ok(resolved) => resolved,
            Err(error) => return format!("Error: source skill '{source}' not found: {error}"),
        };
        if same_path(&resolved.skill_dir, &target_dir) {
            return format!(
                "Error: source '{}' resolves to the target folder; remove it from sources.",
                source
            );
        }
        if !is_self_evolution(workspace_dir, &resolved.skill.name) {
            return format!(
                "Error: source skill '{}' is {} — only self-evolution skills may be merged.",
                resolved.skill.name,
                source_label(workspace_dir, &resolved.skill.name)
            );
        }
        let dir_key = resolved
            .skill_dir
            .canonicalize()
            .unwrap_or_else(|_| resolved.skill_dir.clone());
        if seen_dirs.insert(dir_key) {
            resolved_sources.push(resolved);
        }
    }

    // 1) The merged skill must exist before any source is removed.
    if let Err(error) = fs::create_dir_all(&target_dir) {
        return format!("Error creating target folder '{}': {error}", target_dir.display());
    }
    let target_file = target_dir.join("skill.md");
    let target_backup = match backup_protected_path(&target_file, Some(&skills_root)) {
        Ok(backup) => backup,
        Err(error) => return format!("Error: {error}"),
    };
    if target_file.exists() && target_backup.is_none() {
        return format!(
            "Error: target file '{}' exists but was not backed up; refusing to overwrite it.",
            target_file.display()
        );
    }
    if let Err(error) = fs::write(&target_file, skill_md) {
        return format!("Error writing '{}': {error}", target_file.display());
    }
    if let Err(error) = skills::enable_skill_for_workspace(workspace_dir, &skill.name) {
        return format!(
            "Error: merged skill was written but could not be enabled: {error}"
        );
    }

    // 2) Only now retire the sources; every folder keeps a full backup.
    let mut archived = Vec::new();
    for source in &resolved_sources {
        match backup_protected_path(&source.skill_dir, Some(&skills_root)) {
            Ok(Some(path)) => {
                archived.push(format!("{} → {}", source.skill.name, path.display()));
            }
            Ok(None) => {
                return format!(
                    "Error: merged skill '{}' was written and enabled, but source '{}' was not backed up (it is not under the skills root); refusing to delete it.",
                    skill.name, source.skill.name
                );
            }
            Err(error) => {
                return format!(
                    "Error: merged skill '{}' was written and enabled, but backing up source '{}' failed: {error}",
                    skill.name, source.skill.name
                );
            }
        }
        if let Err(error) = fs::remove_dir_all(&source.skill_dir) {
            return format!(
                "Error: merged skill '{}' was written and enabled, but deleting source '{}' failed: {error}",
                skill.name, source.skill.name
            );
        }
    }

    let mut report = format!(
        "Merged {} source skill(s) into '{}' (folder '{}') and enabled it.",
        resolved_sources.len(),
        skill.name,
        target
    );
    if let Some(backup) = target_backup {
        report.push_str(&format!("\nPrevious target file backed up to {}.", backup.display()));
    }
    if archived.is_empty() {
        report.push_str("\nNo source folders were deleted.");
    } else {
        report.push_str("\nArchived sources:\n");
        for line in archived {
            report.push_str("- ");
            report.push_str(&line);
            report.push('\n');
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-skill-merge-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(workspace: &Path, folder: &str, name: &str, body: &str) {
        let dir = workspace::skills_root(workspace).join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("skill.md"),
            format!("---\nname: {name}\ndescription: {name} description\n---\n\n{body}\n"),
        )
        .unwrap();
    }

    fn write_status(workspace: &Path, entries: &[(&str, &str)]) {
        let skills: serde_json::Map<String, Value> = entries
            .iter()
            .map(|(name, source)| {
                (
                    name.to_string(),
                    json!({ "enabled": true, "source": source }),
                )
            })
            .collect();
        fs::write(
            workspace::skills_status_path(workspace),
            json!({ "version": 1, "skills": skills }).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn merges_self_evolution_skills_and_keeps_backups() {
        let workspace = temp_workspace("merge");
        write_skill(&workspace, "alpha", "alpha", "alpha body");
        write_skill(&workspace, "beta", "beta", "beta body");
        write_status(
            &workspace,
            &[("alpha", "self-evolution"), ("beta", "self-evolution")],
        );

        let report = merge_skill(
            &workspace,
            &json!({
                "target": "alpha",
                "sources": ["beta"],
                "skill_md": "---\nname: alpha\ndescription: merged\n---\n\nalpha body\n\nbeta body\n",
            }),
        );
        assert!(report.starts_with("Merged 1"), "{report}");

        let skills_root = workspace::skills_root(&workspace);
        let merged = fs::read_to_string(skills_root.join("alpha").join("skill.md")).unwrap();
        assert!(merged.contains("beta body"));
        assert!(skills_root.join("alpha").join("skill.md.bak.1").is_file());
        assert!(skills_root.join("beta.bak.1").is_dir());
        assert!(!skills_root.join("beta").exists());

        let status: crate::skills::SkillsStatus = serde_json::from_str(
            &fs::read_to_string(workspace::skills_status_path(&workspace)).unwrap(),
        )
        .unwrap();
        assert!(status.skills.get("alpha").unwrap().enabled);

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn refuses_to_merge_a_user_authored_skill() {
        let workspace = temp_workspace("user");
        write_skill(&workspace, "alpha", "alpha", "alpha body");
        write_skill(&workspace, "user-skill", "user-skill", "user body");
        write_status(
            &workspace,
            &[("alpha", "self-evolution"), ("user-skill", "user")],
        );

        let report = merge_skill(
            &workspace,
            &json!({
                "target": "alpha",
                "sources": ["user-skill"],
                "skill_md": "---\nname: alpha\ndescription: merged\n---\n\nalpha body\n\nuser body\n",
            }),
        );
        assert!(report.contains("user"), "{report}");

        let skills_root = workspace::skills_root(&workspace);
        assert!(skills_root.join("user-skill").join("skill.md").is_file());
        let alpha = fs::read_to_string(skills_root.join("alpha").join("skill.md")).unwrap();
        assert!(alpha.contains("alpha body"));
        assert!(!alpha.contains("user body"));

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn strips_a_wrapping_markdown_fence() {
        let raw = "```markdown\n---\nname: x\ndescription: d\n---\n\nbody\n```";
        let cleaned = clean_skill_md(raw);
        assert!(cleaned.starts_with("---\nname: x"));
        assert!(cleaned.ends_with("body"));
    }
}
