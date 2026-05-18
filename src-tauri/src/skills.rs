use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::State;
use dirs::home_dir;

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
    #[serde(rename = "allowed-commands", default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_commands: Vec<String>,
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
    let body = rest[end + 4..].trim_start_matches('\n').trim_start_matches('\r').trim();
    let meta: SkillMeta = serde_yaml::from_str(frontmatter).map_err(|e| e.to_string())?;
    Ok(Skill {
        name: meta.name,
        description: meta.description,
        version: meta.version,
        author: meta.author,
        allowed_commands: meta.allowed_commands,
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
    };
    let frontmatter = serde_yaml::to_string(&meta).map_err(|e| e.to_string())?;
    Ok(format!("---\n{}---\n\n{}", frontmatter, skill.system_prompt))
}

pub fn load_skill_by_name(skills_dir: &PathBuf, name: &str) -> Result<Skill, String> {
    let path = skills_dir.join(name).join("skill.md");
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    parse_skill_md(&content)
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

    if let Some(home_dir) = home_dir() {
        let user_skills_dir = home_dir.join(".skills");
        skills.extend(read_dir_skills(&user_skills_dir));
    }

    skills
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
