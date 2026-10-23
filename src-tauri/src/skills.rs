use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub skill: Skill,
    pub skill_dir: PathBuf,
    pub skill_file: PathBuf,
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

/// Load one skill from the workspace skills root (`<workspace>/.data/skills`).
///
/// Names are resolved by folder name first, then by the skill's frontmatter
/// `name`. The fallback matters because the frontmatter name is the
/// model-facing identity (it is what the chat listing, the Skills panel and
/// `use_skill` all use), while the folder name can differ — e.g. a skill
/// folder taken verbatim from a zip download (`gracker-writing-main`) whose
/// frontmatter declares `name: gracker-writing`. Without the fallback every
/// chat request and skill-status validation for such a skill silently fails,
/// which made the skills selection drop to empty whenever the panel reloaded.
pub fn resolve_skill_by_name(skills_dir: &Path, name: &str) -> Result<ResolvedSkill, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("empty skill name".to_string());
    }

    let skill_dir = skills_dir.join(name);
    if let Some(skill_file) = find_skill_file(&skill_dir) {
        let content = fs::read_to_string(&skill_file).map_err(|e| e.to_string())?;
        let skill = parse_skill_md(&content)?;
        return Ok(ResolvedSkill {
            skill,
            skill_dir,
            skill_file,
        });
    }

    // No folder matches: scan the skill roots and match the frontmatter name
    // (exact match wins over a case-insensitive one).
    let mut ci_match: Option<ResolvedSkill> = None;
    for (folder, skill_file) in scan_skill_dirs(skills_dir) {
        let Ok(content) = fs::read_to_string(&skill_file) else {
            continue;
        };
        let Ok(skill) = parse_skill_md(&content) else {
            continue;
        };
        let declared = skill.name.trim();
        if declared == name {
            return Ok(ResolvedSkill {
                skill,
                skill_dir: skills_dir.join(&folder),
                skill_file,
            });
        }
        if declared.eq_ignore_ascii_case(name) {
            ci_match.get_or_insert(ResolvedSkill {
                skill,
                skill_dir: skills_dir.join(&folder),
                skill_file,
            });
        }
    }

    ci_match.ok_or_else(|| format!("skill '{}' not found", name))
}

/// The one and only skill root, and the only root self-evolution may write to.
///
/// There is exactly **one** skills directory in the whole app:
/// `<workspace>/.data/skills`. It is computed here and nowhere else, so the
/// assistant can never be pointed at a second location — the app-data folder
/// has no skills directory, and a workspace switch moves the whole thing.
///
/// Returns `None` while self-evolution mode is off (nothing is writable then).
pub fn self_evolution_skill_root(
    workspace_dir: Option<&Path>,
    enabled: bool,
) -> Option<PathBuf> {
    if !enabled {
        return None;
    }

    workspace_dir.map(crate::workspace::skills_root)
}

/// Model-facing description of the self-evolution capability.
///
/// Shared by the main chat and by sub-agents so both describe the same
/// contract: skills may be installed, edited and deleted **only** under
/// `<workspace>/.data/skills`, through the `file_actions` tool.
pub fn self_evolution_prompt(root: Option<&Path>) -> String {
    let Some(root) = root else {
        return String::new();
    };

    format!(
        "SELF-EVOLUTION: you may install, extend, edit and delete skills yourself. \
         The one skill directory is `{}` (workspace-relative: `./.data/skills`).\n\
         Use `file_actions` with workspace-relative paths under `./.data/skills/...`: \
         `mkdir` a skill folder, `write`/`patch` its `skill.md` to create or extend a skill, \
         `move`/`rename` to reorganise, `delete` to remove a skill. \
         A skill is a folder containing `skill.md`: YAML frontmatter with `name` and \
         `description`, then the instructions that are loaded into the model. \
         Every mutation creates a `.bak.<n>` backup automatically.\n\
         Skills live in exactly one directory — `./.data/skills`. The rest of the \
         workspace is yours to read and write, including the `.data` state folder \
         (memory, todos, skill status); only `.data/chat.db` is protected by the \
         application. There is no second skill directory: do not create or look \
         for one. Do not reference app-data skill paths or any path outside the \
         workspace root. Reuse exact paths returned by tools.\n\n\
         EVOLUTION MEMORY: you have a dedicated `evolution` memory scope (the `memory` tool \
         with scope \"evolution\") for facts that must survive across sessions and that you \
         have verified to be correct — for example:\n\
         - the runtime environment: which interpreter is the default one (`python` vs \
         `python3`), the package manager, the shell, the path to a project interpreter;\n\
         - the correct way to operate a tool: how to drive the browser, how to run the \
         project's tests, the exact command that works;\n\
         - the file locations you use often: important paths, output directories, config files.\n\
         Store such a fact with `memory add`, scope `evolution`, and a short descriptive key \
         as soon as you have confirmed it — and read the scope (`memory list` / `get` / \
         `search`) before starting work so you reuse what you already learned. \
         Record only facts you verified; never store guesses.",
        root.display()
    )
}

// ─── Skill status (`<workspace>/.data/skills.status`) ────────────────────────
//
// The skill *selection* used to live in the app-data `config.json`, which made
// it global: switching to a workspace where those skills do not resolve pruned
// the list for good. It is now per-workspace state, next to the skills it
// describes. The file is the single source of truth for which skills are
// enabled, and it never drops an entry on its own — a skill whose folder is
// gone stays recorded (disabled from the model's point of view) and simply
// comes back when the folder does.

/// Status-file schema version.
const SKILL_STATUS_VERSION: u32 = 1;
/// The skill was created or last edited through the Skills panel.
const SKILL_SOURCE_USER: &str = "user";
/// The skill directory was found without a matching panel record (typically
/// written by the assistant in self-evolution mode, or copied in by hand).
const SKILL_SOURCE_SELF_EVOLUTION: &str = "self-evolution";

/// Per-skill status metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillStatusEntry {
    /// Whether the skill is sent to the model on the next request.
    #[serde(default)]
    pub enabled: bool,
    /// Where the skill came from (`user` | `self-evolution`). Owned by the
    /// backend: a stale UI snapshot never overwrites it.
    #[serde(default)]
    pub source: String,
    /// Last time the skill was enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_at: Option<String>,
    /// Last time this entry was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Last time the skill was observed on disk (discovery or change).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
    /// `skill.md` modification time at the last observation, used to detect
    /// skills edited outside the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_mtime: Option<String>,
}

/// The workspace skill status file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsStatus {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Keyed by skill name so the UI can toggle one entry without touching the
    /// rest of the file.
    pub skills: BTreeMap<String, SkillStatusEntry>,
}

impl Default for SkillsStatus {
    fn default() -> Self {
        Self {
            version: SKILL_STATUS_VERSION,
            updated_at: None,
            skills: BTreeMap::new(),
        }
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Load the status file; a missing or unreadable file is an empty status.
fn load_status_file(path: &Path) -> SkillsStatus {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<SkillsStatus>(&content).ok())
        .unwrap_or_default()
}

fn write_status_file(path: &Path, status: &SkillsStatus) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(status).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

/// Locate the skill document inside a skill directory.
///
/// This app's own skills use a lower-case `skill.md`, while imported skill
/// packages commonly use the upper-case `SKILL.md` convention. Both are
/// accepted (the lower-case name first, so platform instances where the
/// filesystem is case-sensitive resolve app-authored files deterministically).
fn find_skill_file(skill_dir: &Path) -> Option<PathBuf> {
    for file_name in ["skill.md", "SKILL.md"] {
        let candidate = skill_dir.join(file_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Every skill directory on disk as `(folder name, skill file path)`.
fn scan_skill_dirs(skills_root: &Path) -> Vec<(String, PathBuf)> {
    if !skills_root.is_dir() {
        return Vec::new();
    }

    fs::read_dir(skills_root)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    if is_backup_artifact(&name) {
                        return None;
                    }
                    let skill_file = find_skill_file(&entry.path())?;
                    Some((name, skill_file))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Backup artifacts created by the tool layer (`<name>.bak.<n>`) sit next to
/// the original — a deleted skill folder is copied there before removal.
///
/// They are not skills: a backup folder still contains the original
/// `skill.md`, so scanning it would advertise the same skill twice.
fn is_backup_artifact(folder: &str) -> bool {
    let Some((_, suffix)) = folder.rsplit_once(".bak.") else {
        return false;
    };
    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
}

fn skill_md_mtime(skill_file: &Path) -> Option<String> {
    let modified = fs::metadata(skill_file).ok()?.modified().ok()?;
    let stamp: chrono::DateTime<chrono::Utc> = modified.into();
    Some(stamp.to_rfc3339())
}

/// Fold the skills currently on disk into `status`. Returns `true` when a new
/// entry was discovered or an existing one observed a changed file.
///
/// Entries are keyed by the skill's frontmatter `name` — the same identity the
/// Skills panel toggles and the chat deliverable (`use_skill`,
/// `filter_existing_skills`) resolves — not by the folder name, which may
/// differ (zip downloads, renamed folders). A skill file that fails to parse
/// falls back to the folder name so its metadata is still tracked.
fn refresh_skill_status(status: &mut SkillsStatus, skills_root: &Path) -> bool {
    let mut changed = false;
    let observed_at = now_rfc3339();

    for (folder, skill_file) in scan_skill_dirs(skills_root) {
        let mtime = skill_md_mtime(&skill_file);
        let name = fs::read_to_string(&skill_file)
            .ok()
            .and_then(|content| parse_skill_md(&content).ok())
            .map(|skill| skill.name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(folder);
        let name = name.trim().to_string();
        match status.skills.get_mut(&name) {
            Some(entry) => {
                if entry.file_mtime != mtime {
                    entry.file_mtime = mtime;
                    entry.last_seen_at = Some(observed_at.clone());
                    changed = true;
                }
            }
            None => {
                status.skills.insert(
                    name,
                    SkillStatusEntry {
                        enabled: false,
                        source: SKILL_SOURCE_SELF_EVOLUTION.to_string(),
                        enabled_at: None,
                        updated_at: Some(observed_at.clone()),
                        last_seen_at: Some(observed_at.clone()),
                        file_mtime: mtime,
                    },
                );
                changed = true;
            }
        }
    }

    changed
}

/// Read the workspace skill status, folding in any skill that appeared on disk.
///
/// Called on startup, after a workspace switch and whenever the Skills panel
/// opens. The merged view is persisted (best effort) so newly discovered
/// skills keep their metadata; entries are never removed here.
#[tauri::command]
pub fn get_skills_status(state: State<'_, AppState>) -> SkillsStatus {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let status_path = crate::workspace::skills_status_path(&workspace_dir);
    let mut status = load_status_file(&status_path);

    if refresh_skill_status(&mut status, &crate::workspace::skills_root(&workspace_dir)) {
        status.version = SKILL_STATUS_VERSION;
        status.updated_at = Some(now_rfc3339());
        let _ = write_status_file(&status_path, &status);
    }

    status
}

/// Persist the workspace skill status (enablement + metadata).
///
/// The stored file is the merge *base*: incoming entries are added/updated,
/// everything else is kept, so a stale UI snapshot can never drop another
/// entry or a selection.
///
/// Merge rules:
/// - `source` is backend-owned: it comes from the stored entry, and a skill
///   that appears without one is recorded as created outside the panel.
/// - `enabled_at` is stamped when an entry is switched on (or first enabled).
/// - Entries are only ever added or updated, never dropped — a deleted skill
///   folder must not erase the user's selection for when it returns.
#[tauri::command]
pub fn save_skills_status(
    state: State<'_, AppState>,
    status: SkillsStatus,
) -> Result<(), String> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let status_path = crate::workspace::skills_status_path(&workspace_dir);
    let stored = load_status_file(&status_path);
    let now = now_rfc3339();

    let mut merged = stored;
    merged.version = SKILL_STATUS_VERSION;
    merged.updated_at = Some(now.clone());

    for (name, incoming) in status.skills {
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }

        let previous = merged.skills.get(&name).cloned();
        let was_enabled = previous.as_ref().map(|entry| entry.enabled).unwrap_or(false);
        let source = previous
            .as_ref()
            .map(|entry| entry.source.clone())
            .filter(|source| !source.trim().is_empty())
            .unwrap_or_else(|| SKILL_SOURCE_SELF_EVOLUTION.to_string());

        let enabled_at = if incoming.enabled {
            match previous.as_ref().and_then(|entry| entry.enabled_at.clone()) {
                Some(existing) if was_enabled => Some(existing),
                _ => Some(now.clone()),
            }
        } else {
            previous.as_ref().and_then(|entry| entry.enabled_at.clone())
        };

        merged.skills.insert(
            name,
            SkillStatusEntry {
                enabled: incoming.enabled,
                source,
                enabled_at,
                updated_at: Some(now.clone()),
                last_seen_at: previous
                    .as_ref()
                    .and_then(|entry| entry.last_seen_at.clone())
                    .or(incoming.last_seen_at),
                file_mtime: previous
                    .as_ref()
                    .and_then(|entry| entry.file_mtime.clone())
                    .or(incoming.file_mtime),
            },
        );
    }

    write_status_file(&status_path, &merged)
}

/// Record that the Skills panel wrote this skill (`save_skill`).
fn mark_skill_source(workspace_dir: &Path, name: &str, source: &str) {
    let status_path = crate::workspace::skills_status_path(workspace_dir);
    let mut status = load_status_file(&status_path);
    let now = now_rfc3339();

    let entry = status.skills.entry(name.to_string()).or_default();
    entry.source = source.to_string();
    entry.updated_at = Some(now.clone());

    status.version = SKILL_STATUS_VERSION;
    status.updated_at = Some(now);
    let _ = write_status_file(&status_path, &status);
}

#[tauri::command]
pub fn list_skills(state: State<'_, AppState>) -> Vec<Skill> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let skills_root = crate::workspace::skills_root(&workspace_dir);

    // `.data/skills` is created on demand (Skills panel, self-evolution), so a
    // missing directory simply means there are no workspace skills yet.
    scan_skill_dirs(&skills_root)
        .into_iter()
        .filter_map(|(_, skill_file)| {
            let content = fs::read_to_string(skill_file).ok()?;
            parse_skill_md(&content).ok()
        })
        .collect()
}

#[tauri::command]
pub fn save_skill(state: State<'_, AppState>, skill: Skill) -> Result<(), String> {
    let md = skill_to_md(&skill)?;
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let skill_dir = crate::workspace::skills_root(&workspace_dir).join(&skill.name);
    fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    fs::write(skill_dir.join("skill.md"), md).map_err(|e| e.to_string())?;

    // The status entry survives the write: editing a skill keeps its
    // enablement, only the origin is stamped as panel-authored.
    mark_skill_source(&workspace_dir, &skill.name, SKILL_SOURCE_USER);
    Ok(())
}

#[tauri::command]
pub fn delete_skill(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let skill_dir = crate::workspace::skills_root(&workspace_dir).join(&name);
    fs::remove_dir_all(skill_dir).map_err(|e| e.to_string())
}

/// Return the subset of `names` that currently resolve to a loadable skill.
///
/// Read-only helper for the UI: the persisted selection keeps names whose
/// folder has gone missing, and this decides which of them can actually be
/// sent to the model right now. Resolution mirrors the chat runtime
/// (`resolve_skill_by_name`), so the returned names always load. It never
/// writes, so a temporarily absent skill is never dropped from the status
/// file.
#[tauri::command]
pub fn filter_existing_skills(state: State<'_, AppState>, names: Vec<String>) -> Vec<String> {
    if names.is_empty() {
        return names;
    }

    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let skills_root = crate::workspace::skills_root(&workspace_dir);

    names
        .into_iter()
        .filter(|name| resolve_skill_by_name(&skills_root, name).is_ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, folder: &str, declared_name: &str, file_name: &str) {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(file_name),
            format!(
                "---\nname: {declared_name}\ndescription: test skill\n---\n\nBody of {declared_name}.\n"
            ),
        )
        .unwrap();
    }

    struct TempSkillsRoot(PathBuf);

    impl TempSkillsRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ai-chat-skills-test-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempSkillsRoot(dir)
        }
    }

    impl Drop for TempSkillsRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A skill imported from a zip archive: folder `x-main` with an upper-case
    /// `SKILL.md` declaring `name: x`. It must be discoverable, listed, and
    /// resolvable both ways.
    #[test]
    fn resolves_zip_imported_skill_by_frontmatter_and_folder_name() {
        let root = TempSkillsRoot::new("zip-import");
        write_skill(&root.0, "gracker-writing-main", "gracker-writing", "SKILL.md");

        // Discoverable despite the upper-case file name.
        let scanned = scan_skill_dirs(&root.0);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].0, "gracker-writing-main");

        // Resolve by the frontmatter name (model-facing identity).
        let by_declared = resolve_skill_by_name(&root.0, "gracker-writing").unwrap();
        assert_eq!(by_declared.skill.name, "gracker-writing");

        // Resolve by the folder name too.
        let by_folder = resolve_skill_by_name(&root.0, "gracker-writing-main").unwrap();
        assert_eq!(by_folder.skill.name, "gracker-writing");

        // Unknown names still fail.
        assert!(resolve_skill_by_name(&root.0, "nope").is_err());
        assert!(resolve_skill_by_name(&root.0, "").is_err());
    }

    /// The reported bug: the panel toggles/status use the frontmatter `name`
    /// (`x`), so a pre-existing enabled entry under that name must keep its
    /// `enabled` flag when the on-disk folder is keyed differently
    /// (`x-main`). It must NOT be re-keyed to the folder name and reset to
    /// disabled, which is what emptied the selection on every panel open.
    #[test]
    fn refresh_keys_status_by_frontmatter_name_and_preserves_enablement() {
        let root = TempSkillsRoot::new("refresh-keys");
        write_skill(&root.0, "gracker-writing-main", "gracker-writing", "SKILL.md");

        // Status as left behind by an earlier session: the skill was enabled
        // through the panel under its frontmatter name; the folder entry
        // (non-parse state aside) may also exist from an older refresh.
        let mut status = SkillsStatus::default();
        status.skills.insert(
            "gracker-writing".to_string(),
            SkillStatusEntry {
                enabled: true,
                source: SKILL_SOURCE_SELF_EVOLUTION.to_string(),
                ..SkillStatusEntry::default()
            },
        );

        refresh_skill_status(&mut status, &root.0);

        let entry = status.skills.get("gracker-writing").expect("frontmatter entry");
        assert!(entry.enabled, "enablement must survive the refresh");
        assert!(entry.file_mtime.is_some(), "mtime must be refreshed");

        // No second entry under the folder name is created for the same skill.
        assert!(!status.skills.contains_key("gracker-writing-main"));
    }

    /// Workspace skills written by the app itself keep resolving exactly as
    /// before (folder name == frontmatter name, lower-case file).
    #[test]
    fn resolves_own_panel_skills_unchanged() {
        let root = TempSkillsRoot::new("panel-skill");
        write_skill(&root.0, "code-reviewer", "code-reviewer", "skill.md");

        let resolved = resolve_skill_by_name(&root.0, "code-reviewer").unwrap();
        assert_eq!(resolved.skill.name, "code-reviewer");
        assert_eq!(resolved.skill_dir, root.0.join("code-reviewer"));

        let mut status = SkillsStatus::default();
        assert!(refresh_skill_status(&mut status, &root.0), "new skill discovered");
        assert!(status.skills.contains_key("code-reviewer"));
        assert!(!status
            .skills
            .get("code-reviewer")
            .unwrap()
            .enabled, "newly discovered skills start disabled");
    }

    /// Deleting a skill in self-evolution mode leaves a `demo.bak.1` folder
    /// next to it. It still contains the original `skill.md`, so it must not be
    /// advertised as a second skill with the same name.
    #[test]
    fn backup_folders_are_not_scanned_as_skills() {
        let root = TempSkillsRoot::new("backup-artifact");
        write_skill(&root.0, "demo", "demo", "skill.md");
        write_skill(&root.0, "demo.bak.1", "demo", "skill.md");

        let scanned = scan_skill_dirs(&root.0);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].0, "demo");

        let mut status = SkillsStatus::default();
        refresh_skill_status(&mut status, &root.0);
        assert_eq!(status.skills.len(), 1);
        assert!(status.skills.contains_key("demo"));
    }
}
