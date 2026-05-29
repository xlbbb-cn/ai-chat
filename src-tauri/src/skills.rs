use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::State;

use crate::AppState;

/// YAML frontmatter parsed from a SKILL.md file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Allowlist of executable names the skill may run (empty = unrestricted).
    #[serde(
        rename = "allowed-commands",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub allowed_commands: Vec<String>,
    #[serde(
        rename = "allowed-tools",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// Full skill representation passed to/from the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Allowlist of executable names the skill may run (empty = unrestricted).
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub system_prompt: String,
}

pub fn parse_skill_md(content: &str) -> Result<Skill, String> {
    let content = content.trim_start_matches('\u{feff}');
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or("missing YAML frontmatter")?;
    let end = rest.find("\n---").ok_or("unclosed frontmatter")?;
    let frontmatter = &rest[..end];
    let body = rest[end + 4..]
        .trim_start_matches('\n')
        .trim_start_matches('\r')
        .trim();
    let meta: SkillMeta = serde_yaml::from_str(frontmatter).map_err(|e| e.to_string())?;
    Ok(Skill {
        name: meta.name,
        description: meta.description,
        version: meta.version,
        author: meta.author,
        allowed_commands: meta.allowed_commands,
        allowed_tools: meta.allowed_tools,
        context: meta.context,
        agent: meta.agent,
        license: meta.license,
        system_prompt: body.to_string(),
    })
}

pub fn skill_to_md(skill: &Skill) -> Result<String, String> {
    let meta = SkillMeta {
        name: skill.name.clone(),
        description: skill.description.clone(),
        version: skill.version.clone(),
        author: skill.author.clone(),
        allowed_commands: skill.allowed_commands.clone(),
        allowed_tools: skill.allowed_tools.clone(),
        context: skill.context.clone(),
        agent: skill.agent.clone(),
        license: skill.license.clone(),
    };
    let frontmatter = serde_yaml::to_string(&meta).map_err(|e| e.to_string())?;
    Ok(format!(
        "---\n{}---\n\n{}",
        frontmatter, skill.system_prompt
    ))
}

pub fn load_skill_by_name(skills_dir: &PathBuf, name: &str) -> Result<Skill, String> {
    let path = skills_dir.join(name).join("skill.md");
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    parse_skill_md(&content)
}

/// Collect skill roots eligible for self-evolution edits.
/// Self-evolution is restricted to `workspace/skills`.
/// App-managed skills remain readable and listable, but are not writable
/// by tool execution.
pub fn collect_self_evolution_roots(
    managed_skills_dir: &Path,
    workspace_dir: Option<&Path>,
    enabled: bool,
) -> Vec<PathBuf> {
    let _ = managed_skills_dir;
    if !enabled {
        return Vec::new();
    }

    workspace_dir
        .map(|ws_dir| vec![ws_dir.join("skills")])
        .unwrap_or_default()
}

#[tauri::command]
pub fn list_skills(state: State<'_, AppState>) -> Vec<Skill> {
    let mut skills = Vec::new();

    let read_dir_skills = |dir: &PathBuf| -> Vec<Skill> {
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| {
                        let skill_path = e.path().join("skill.md");
                        let content = fs::read_to_string(skill_path).ok()?;
                        parse_skill_md(&content).ok()
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    skills.extend(read_dir_skills(&state.skills_dir));

    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let ws_skills_dir = workspace_dir.join("skills");
    skills.extend(read_dir_skills(&ws_skills_dir));

    // Deduplicate by name, preferring workspace implementations over app-data implementations
    let mut unique_skills = std::collections::HashMap::new();
    for skill in skills {
        unique_skills.insert(skill.name.clone(), skill);
    }
    
    unique_skills.into_values().collect()
}

#[tauri::command]
pub fn save_skill(state: State<'_, AppState>, skill: Skill) -> Result<(), String> {
    let md = skill_to_md(&skill)?;
    let skill_dir = state.skills_dir.join(&skill.name);
    fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    fs::write(skill_dir.join("skill.md"), md).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_skill(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let skill_dir = state.skills_dir.join(&name);
    fs::remove_dir_all(skill_dir).map_err(|e| e.to_string())
}
