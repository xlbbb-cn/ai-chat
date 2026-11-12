//! Markdown-backed definitions for `llm` sub-agents.
//!
//! `sub_agents.json` is configuration only. For an `llm` agent it stores the
//! runtime knobs (enabled, model, tools, limits) plus the relative `file` that
//! holds the definition. The definition itself — `name`, `description`,
//! `emoji`, `color` and the system prompt — lives in
//! `<app_data>/agents/<category>/<slug>.md` with YAML frontmatter, mirroring
//! the format used by the skill packages:
//!
//! ```text
//! ---
//! name: 代码审查员
//! description: 专业代码审查专家……
//! emoji: 👀
//! color: purple
//! ---
//!
//! <system prompt>
//! ```
//!
//! Files are the source of truth. A file the app did not create (anything
//! outside `agents/custom/`) is never deleted from the panel, and a save only
//! rewrites a file when its parsed definition actually changed — toggling
//! `enabled` or editing tools must not touch the original collection.
//!
//! Discovery is explicit: `build_index` scans the tree on the panel's Refresh
//! action and **replaces** `agents/index.yaml`; opening the panel reads that
//! manifest (`load_index` / `append_index_agents`), while runtime resolution
//! (`resolve_configured`) only reads files referenced by `sub_agents.json` and
//! never lists the directory.

use super::{default_max_iterations, normalize_agent_type, SubAgent, AGENT_TYPE_LLM};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

/// Tools a newly discovered `llm` agent starts with.
///
/// Discovered agents are disabled by default, so granting the read/write tool
/// does not run anything on its own. `run_cmd` / `run_shell` stay opt-in.
pub const DISCOVERED_DEFAULT_TOOLS: [&str; 2] = ["file_actions", "memory"];

/// Where the app puts agents created from the panel.
pub const CUSTOM_CATEGORY: &str = "custom";

/// The definition parsed out of one agent markdown file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub emoji: Option<String>,
    pub color: Option<String>,
    /// Markdown body — the agent's system prompt.
    pub prompt: String,
}

/// One markdown file discovered under the agents root.
#[derive(Debug, Clone)]
pub struct ScannedAgent {
    pub relative: String,
    pub definition: AgentDefinition,
    /// Set when the file exists but its frontmatter could not be parsed. The
    /// entry is still listed (disabled) so the panel can show and repair it.
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    /// The persistent `sub_agents.json` needs a rewrite (an inline prompt was
    /// migrated to a file, or a stale file-backed entry was dropped).
    pub changed: bool,
    pub migrated: usize,
    pub removed_missing: usize,
}

// ─── Discovery manifest ───────────────────────────────────────────────────────

/// File name of the discovery manifest inside `agents/`.
pub const INDEX_FILE: &str = "index.yaml";

/// One discovered agent file, as written to `agents/index.yaml`.
///
/// The markdown file stays authoritative for the definition and prompt; the
/// manifest only records which files exist and keeps a metadata snapshot for
/// a broken/unreadable file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentIndexEntry {
    #[serde(default)]
    pub id: String,
    pub file: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The checked-in discovery manifest. Refresh scans `agents/**/*.md` and
/// **replaces** this file; opening the panel only reads it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIndex {
    #[serde(default = "default_index_version")]
    pub version: u32,
    #[serde(default)]
    pub refreshed_at: String,
    #[serde(default)]
    pub agents: Vec<AgentIndexEntry>,
}

fn default_index_version() -> u32 {
    1
}

impl Default for AgentIndex {
    fn default() -> Self {
        Self {
            version: default_index_version(),
            refreshed_at: String::new(),
            agents: Vec::new(),
        }
    }
}

// ─── Paths ───────────────────────────────────────────────────────────────────

/// `<app_data>/agents`, next to `sub_agents.json`.
pub fn agents_root(agents_config_path: &Path) -> PathBuf {
    agents_config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("agents")
}

/// `<app_data>/agents/index.yaml`.
pub fn index_path(root: &Path) -> PathBuf {
    root.join(INDEX_FILE)
}

/// Read the discovery manifest. A missing or malformed file is `None` (the
/// panel then shows only configured agents until the next Refresh replaces it).
pub fn load_index(root: &Path) -> Option<AgentIndex> {
    let raw = fs::read_to_string(index_path(root)).ok()?;
    serde_yaml::from_str::<AgentIndex>(&raw).ok()
}

/// Replace `agents/index.yaml` with the given manifest.
pub fn write_index(root: &Path, index: &AgentIndex) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let body = serde_yaml::to_string(index).map_err(|e| e.to_string())?;
    let content = format!(
        "# Generated by the Sub Agents panel on Refresh; this file is replaced on the next refresh.\n{body}"
    );
    fs::write(index_path(root), content).map_err(|e| e.to_string())
}

/// Scan `agents/**/*.md` and build the manifest that Refresh writes.
pub fn build_index(root: &Path) -> AgentIndex {
    AgentIndex {
        version: default_index_version(),
        refreshed_at: chrono::Utc::now().to_rfc3339(),
        agents: scan(root)
            .into_iter()
            .map(|scanned| AgentIndexEntry {
                id: id_for_relative(&scanned.relative),
                file: scanned.relative,
                name: scanned.definition.name,
                description: scanned.definition.description,
                emoji: scanned.definition.emoji,
                color: scanned.definition.color,
                error: scanned.parse_error,
            })
            .collect(),
    }
}

/// Normalize a config-supplied relative path to a safe `category/file.md`
/// string with forward slashes.
///
/// Rejects absolute paths, drive prefixes, `..` and anything that is not a
/// `.md` file. The returned value is lexical only; `ensure_inside` additionally
/// checks symlinks before a write.
pub fn normalize_relative(relative: &str) -> Result<String, String> {
    let trimmed = relative.trim().replace('\\', "/");
    if trimmed.is_empty() {
        return Err("The agent file path cannot be empty.".to_string());
    }
    // A drive-qualified path is absolute even on a host that does not parse it
    // as one (`C:/x/agent.md` is three ordinary components on Linux).
    {
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return Err(
                "The agent file path must stay inside the agents directory.".to_string()
            );
        }
    }

    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(&trimmed).components() {
        match component {
            Component::Normal(part) => {
                let value = part.to_string_lossy().to_string();
                if value == ".bak" || value.contains(".bak.") {
                    return Err("Backup files cannot be used as agent definitions.".to_string());
                }
                parts.push(value);
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(
                    "The agent file path must stay inside the agents directory.".to_string()
                );
            }
        }
    }

    let joined = parts.join("/");
    if joined.is_empty() {
        return Err("The agent file path cannot be empty.".to_string());
    }
    if !joined.to_ascii_lowercase().ends_with(".md") {
        return Err("An agent definition must be a .md file.".to_string());
    }
    Ok(joined)
}

/// Resolve a relative path against the agents root (lexical check only).
pub fn resolve(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = normalize_relative(relative)?;
    Ok(root.join(relative))
}

/// Agent id derived from the file path: `engineering/code-reviewer.md` →
/// `engineering/code-reviewer`.
pub fn id_for_relative(relative: &str) -> String {
    let normalized = relative.trim().replace('\\', "/");
    if normalized.to_ascii_lowercase().ends_with(".md") {
        normalized[..normalized.len() - 3].to_string()
    } else {
        normalized
    }
}

/// File name (without directories) of a relative path.
fn name_from_relative(relative: &str) -> String {
    id_for_relative(relative)
        .rsplit('/')
        .next()
        .unwrap_or("agent")
        .to_string()
}

pub fn is_custom_path(relative: &str) -> bool {
    relative
        .replace('\\', "/")
        .split('/')
        .next()
        .map(|category| category.eq_ignore_ascii_case(CUSTOM_CATEGORY))
        .unwrap_or(false)
}

fn is_backup_name(name: &str) -> bool {
    name.contains(".bak.")
}

/// Filename slug derived from an agent name (80 characters max); falls back to
/// a random suffix so two unnamed agents never share a file.
fn slugify(name: &str) -> String {
    let slug = crate::util::slugify(name, 80);
    if slug.is_empty() {
        format!("agent-{}", &Uuid::new_v4().to_string()[..8])
    } else {
        slug
    }
}

/// First free `category/<slug>.md` path for a new agent.
pub fn unique_agent_path(root: &Path, category: &str, name: &str) -> String {
    let slug = slugify(name);
    let category = category.trim_matches('/');
    let mut counter = 1u32;
    loop {
        let file_name = if counter == 1 {
            format!("{slug}.md")
        } else {
            format!("{slug}-{counter}.md")
        };
        let relative = if category.is_empty() {
            file_name
        } else {
            format!("{category}/{file_name}")
        };
        if !root.join(&relative).exists() {
            return relative;
        }
        counter += 1;
        if counter > 9_999 {
            return format!(
                "{}agent-{}.md",
                category_with_slash(category),
                &Uuid::new_v4().to_string()[..8]
            );
        }
    }
}

fn category_with_slash(category: &str) -> String {
    if category.is_empty() {
        String::new()
    } else {
        format!("{category}/")
    }
}

// ─── Markdown parsing / serialization ────────────────────────────────────────

fn split_frontmatter(content: &str) -> Result<(&str, &str), String> {
    let content = content.trim_start_matches('\u{feff}');
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or("missing YAML frontmatter")?;
    let end = rest.find("\n---").ok_or("unclosed frontmatter")?;
    let frontmatter = &rest[..end];
    let body = rest[end + 4..].trim();
    Ok((frontmatter, body))
}

fn yaml_scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn scalar(map: &Mapping, key: &str) -> Option<String> {
    map.get(key).and_then(yaml_scalar_to_string)
}

fn set_scalar(map: &mut Mapping, key: &str, value: &str) {
    map.insert(
        Value::String(key.to_string()),
        Value::String(value.to_string()),
    );
}

fn set_optional_scalar(map: &mut Mapping, key: &str, value: Option<&str>) {
    match value.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => set_scalar(map, key, text),
        None => {
            map.remove(key);
        }
    }
}

/// Parse an agent markdown file. The body becomes the system prompt.
pub fn parse_agent_md(content: &str) -> Result<AgentDefinition, String> {
    let (frontmatter, body) = split_frontmatter(content)?;
    let map: Mapping = serde_yaml::from_str(frontmatter).map_err(|e| e.to_string())?;
    Ok(AgentDefinition {
        name: scalar(&map, "name")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        description: scalar(&map, "description")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        emoji: scalar(&map, "emoji").filter(|value| !value.trim().is_empty()),
        color: scalar(&map, "color").filter(|value| !value.trim().is_empty()),
        prompt: body.to_string(),
    })
}

/// Serialize a definition, merging unknown frontmatter keys from `existing`.
pub fn serialize_agent_md(
    existing: Option<&str>,
    definition: &AgentDefinition,
) -> Result<String, String> {
    let mut map: Mapping = existing
        .and_then(|content| split_frontmatter(content).ok())
        .and_then(|(frontmatter, _)| serde_yaml::from_str::<Mapping>(frontmatter).ok())
        .unwrap_or_default();

    set_scalar(&mut map, "name", definition.name.trim());
    set_scalar(&mut map, "description", definition.description.trim());
    set_optional_scalar(&mut map, "emoji", definition.emoji.as_deref());
    set_optional_scalar(&mut map, "color", definition.color.as_deref());

    let yaml = serde_yaml::to_string(&map).map_err(|e| e.to_string())?;
    let yaml = yaml.trim_end_matches('\n');
    let prompt = definition.prompt.trim();
    if prompt.is_empty() {
        Ok(format!("---\n{yaml}\n---\n"))
    } else {
        Ok(format!("---\n{yaml}\n---\n\n{prompt}\n"))
    }
}

fn definitions_match(current: &AgentDefinition, next: &AgentDefinition) -> bool {
    current.name.trim() == next.name.trim()
        && current.description.trim() == next.description.trim()
        && current
            .emoji
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            == next
                .emoji
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        && current
            .color
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            == next
                .color
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        && current.prompt.trim() == next.prompt.trim()
}

// ─── Scanning ────────────────────────────────────────────────────────────────

/// Recursively collect every `*.md` file under `root`, sorted by path.
///
/// Hidden directories/files, symlinks and `.bak.N` backups are ignored.
pub fn scan(root: &Path) -> Vec<ScannedAgent> {
    let mut result = Vec::new();
    scan_dir(root, root, 0, &mut result);
    result.sort_by(|left, right| left.relative.cmp(&right.relative));
    result
}

fn scan_dir(root: &Path, dir: &Path, depth: usize, out: &mut Vec<ScannedAgent>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || is_backup_name(&name) {
            continue;
        }
        if file_type.is_dir() {
            scan_dir(root, &path, depth + 1, out);
            continue;
        }
        if !file_type.is_file() || !name.to_ascii_lowercase().ends_with(".md") {
            continue;
        }
        let Some(relative) = relative_path(root, &path) else {
            continue;
        };
        if let Some(scanned) = read_scanned_agent_at(&path, &relative) {
            out.push(scanned);
        }
    }
}

/// Read one agent file that is known to exist. Returns `None` when it cannot
/// be read at all; a parse error is returned as a `ScannedAgent` with a
/// fallback name so the panel can display and repair it.
fn read_scanned_agent_at(path: &Path, relative: &str) -> Option<ScannedAgent> {
    let fallback_name = name_from_relative(relative);
    let Ok(content) = fs::read_to_string(path) else {
        return Some(ScannedAgent {
            relative: relative.to_string(),
            definition: AgentDefinition {
                name: fallback_name,
                ..AgentDefinition::default()
            },
            parse_error: Some("The file is not valid UTF-8 text.".to_string()),
        });
    };
    match parse_agent_md(&content) {
        Ok(mut definition) => {
            if definition.name.trim().is_empty() {
                definition.name = fallback_name;
            }
            Some(ScannedAgent {
                relative: relative.to_string(),
                definition,
                parse_error: None,
            })
        }
        Err(error) => Some(ScannedAgent {
            relative: relative.to_string(),
            definition: AgentDefinition {
                name: fallback_name,
                ..AgentDefinition::default()
            },
            parse_error: Some(error),
        }),
    }
}

/// Read one agent by its relative path (no directory scan). `None` means the
/// file does not exist; unreadable/broken files still return a fallback.
pub fn read_scanned_agent(root: &Path, relative: &str) -> Option<ScannedAgent> {
    let relative = normalize_relative(relative).ok()?;
    let path = resolve(root, &relative).ok()?;
    let metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    read_scanned_agent_at(&path, &relative)
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

// ─── Write / delete ──────────────────────────────────────────────────────────

/// Canonicalize the nearest existing ancestor of `target` and make sure it is
/// inside the canonical agents root (guards against symlinked directories).
fn ensure_inside(root: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let canonical_root = fs::canonicalize(root).map_err(|e| e.to_string())?;

    let mut existing = target.to_path_buf();
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| "The agent path has no existing parent directory.".to_string())?
            .to_path_buf();
    }
    let canonical = fs::canonicalize(&existing).map_err(|e| e.to_string())?;
    if !canonical.starts_with(&canonical_root) {
        return Err("The agent path must stay inside the agents directory.".to_string());
    }
    Ok(())
}

fn backup_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
    else {
        return Ok(());
    };
    for counter in 1..=999u32 {
        let candidate = path.with_file_name(format!("{name}.bak.{counter}"));
        if !candidate.exists() {
            return fs::copy(path, candidate)
                .map(|_| ())
                .map_err(|e| e.to_string());
        }
    }
    Ok(())
}

/// Write a definition. The file is only touched when the parsed definition
/// differs, so enabling an agent never rewrites the original collection file.
pub fn write_definition(
    root: &Path,
    relative: &str,
    definition: &AgentDefinition,
) -> Result<(), String> {
    let relative = normalize_relative(relative)?;
    let target = resolve(root, &relative)?;
    ensure_inside(root, &target)?;

    let existing = fs::read_to_string(&target).ok();
    if let Some(content) = &existing {
        if let Ok(current) = parse_agent_md(content) {
            if definitions_match(&current, definition) {
                return Ok(());
            }
        }
        backup_file(&target)?;
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serialize_agent_md(existing.as_deref(), definition)?;
    fs::write(&target, content).map_err(|e| e.to_string())
}

/// Delete a definition file. Only `agents/custom/**` is deletable: the rest of
/// the collection is read-only from the app.
pub fn delete_definition(root: &Path, relative: &str) -> Result<(), String> {
    let relative = normalize_relative(relative)?;
    if !is_custom_path(&relative) {
        return Err(
            "Only agents under agents/custom can be deleted. Disable this agent instead."
                .to_string(),
        );
    }
    let target = resolve(root, &relative)?;
    ensure_inside(root, &target)?;
    if target.is_file() {
        backup_file(&target)?;
        fs::remove_file(&target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ─── Config ↔ files ──────────────────────────────────────────────────────────

pub fn definition_from_agent(agent: &SubAgent) -> AgentDefinition {
    AgentDefinition {
        name: agent.name.clone(),
        description: agent.description.clone(),
        emoji: agent.emoji.clone(),
        color: agent.color.clone(),
        prompt: agent.system_prompt.clone(),
    }
}

fn apply_definition(agent: &mut SubAgent, scanned: ScannedAgent) {
    let name = scanned.definition.name.trim();
    agent.name = if name.is_empty() {
        name_from_relative(&scanned.relative)
    } else {
        name.to_string()
    };
    agent.description = scanned.definition.description;
    agent.emoji = scanned.definition.emoji;
    agent.color = scanned.definition.color;
    agent.system_prompt = scanned.definition.prompt;
    // A broken file must never run with an empty prompt just because the
    // agent was enabled before the file was edited outside the app.
    if scanned.parse_error.is_some() {
        agent.enabled = false;
    }
    agent.parse_error = scanned.parse_error;
    agent.file = Some(scanned.relative);
    agent.agent_type = AGENT_TYPE_LLM.to_string();
}

fn virtual_agent(scanned: ScannedAgent, taken_ids: &[String]) -> SubAgent {
    let base = id_for_relative(&scanned.relative);
    let mut id = base.clone();
    let mut counter = 2u32;
    while taken_ids.iter().any(|taken| taken == &id) {
        id = format!("{base}#{counter}");
        counter += 1;
    }

    SubAgent {
        id,
        name: if scanned.definition.name.trim().is_empty() {
            name_from_relative(&scanned.relative)
        } else {
            scanned.definition.name.clone()
        },
        description: scanned.definition.description.clone(),
        agent_type: AGENT_TYPE_LLM.to_string(),
        system_prompt: scanned.definition.prompt.clone(),
        emoji: scanned.definition.emoji.clone(),
        color: scanned.definition.color.clone(),
        file: Some(scanned.relative.clone()),
        parse_error: scanned.parse_error.clone(),
        configured: false,
        model: None,
        max_complete_tokens: None,
        max_tokens: None,
        temperature: None,
        allowed_tools: DISCOVERED_DEFAULT_TOOLS
            .iter()
            .map(|tool| tool.to_string())
            .collect(),
        allowed_skills: Vec::new(),
        max_iterations: default_max_iterations(),
        enable_mcp: false,
        cli: Default::default(),
        cloud: Default::default(),
        require_approval: true,
        enabled: false,
    }
}

/// Resolve the **configured** roster only. No directory scan: an `llm` entry
/// reads the file it references, legacy inline prompts are migrated, and a
/// stale file reference is dropped. Files that are not in `sub_agents.json`
/// are discovered exclusively by `build_index` when the user clicks Refresh.
pub fn resolve_configured(config: &mut super::AgentsConfig, root: &Path) -> SyncReport {
    let mut report = SyncReport::default();
    let mut kept: Vec<SubAgent> = Vec::new();
    let existing = std::mem::take(&mut config.agents);

    for mut agent in existing {
        agent.configured = true;
        let kind = normalize_agent_type(&agent.agent_type);
        agent.agent_type = kind.clone();
        if kind != AGENT_TYPE_LLM {
            kept.push(agent);
            continue;
        }

        let declared_value = agent
            .file
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let declared = declared_value
            .as_deref()
            .and_then(|value| normalize_relative(value).ok());
        let invalid_declared = declared_value.is_some() && declared.is_none();

        if let Some(relative) = declared.as_deref() {
            if let Some(scanned) = read_scanned_agent(root, relative) {
                apply_definition(&mut agent, scanned);
                kept.push(agent);
                continue;
            }
        }

        // No readable file. Migrate an inline prompt (legacy JSON, old export)
        // into a file, otherwise drop the stale (or invalid) file reference.
        if !agent.system_prompt.trim().is_empty() || !agent.name.trim().is_empty() {
            let relative = declared
                .clone()
                .unwrap_or_else(|| unique_agent_path(root, CUSTOM_CATEGORY, &agent.name));
            let definition = definition_from_agent(&agent);
            match write_definition(root, &relative, &definition) {
                Ok(()) => {
                    agent.file = Some(relative);
                    agent.system_prompt.clear();
                    agent.parse_error = None;
                    report.migrated += 1;
                    report.changed = true;
                    kept.push(agent);
                }
                Err(_) => {
                    // Keep the inline definition as a runtime fallback so a
                    // read-only agents directory never loses the prompt.
                    kept.push(agent);
                }
            }
        } else if invalid_declared || declared.is_some() {
            report.removed_missing += 1;
            report.changed = true;
        } else {
            // A hand-edited empty llm entry: keep it so the panel can repair it.
            kept.push(agent);
        }
    }

    config.agents = kept;
    report
}

/// Append the manifest's discovered agents to a resolved roster.
///
/// Configured entries win (deduped by `file`); a manifest entry whose file
/// disappeared is skipped. Reading one indexed file is not discovery: the
/// directory is never listed here. The manifest's metadata is only a fallback
/// for an indexed file that is present but broken.
pub fn append_index_agents(agents: &mut Vec<SubAgent>, index: &AgentIndex, root: &Path) {
    let mut seen_files: HashSet<String> = agents
        .iter()
        .filter_map(|agent| agent.file.as_deref())
        .map(|file| file.replace('\\', "/"))
        .collect();
    let mut taken_ids: Vec<String> = agents.iter().map(|agent| agent.id.clone()).collect();

    for entry in &index.agents {
        let Ok(file) = normalize_relative(&entry.file) else {
            continue;
        };
        if !seen_files.insert(file.clone()) {
            continue;
        }
        let Some(mut scanned) = read_scanned_agent(root, &file) else {
            continue;
        };
        if scanned.parse_error.is_some() {
            if !entry.name.trim().is_empty() {
                scanned.definition.name = entry.name.clone();
            }
            if !entry.description.trim().is_empty() {
                scanned.definition.description = entry.description.clone();
            }
            if entry.emoji.is_some() {
                scanned.definition.emoji = entry.emoji.clone();
            }
            if entry.color.is_some() {
                scanned.definition.color = entry.color.clone();
            }
        }
        let agent = virtual_agent(scanned, &taken_ids);
        taken_ids.push(agent.id.clone());
        agents.push(agent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rabbit-chat-studio-agents-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).expect("temp agents root");
        path
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let content = "---\nname: 审查员\ndescription: 代码审查\nemoji: 👀\ncolor: purple\n---\n\n你是审查员。\n";
        let definition = parse_agent_md(content).expect("parse");
        assert_eq!(definition.name, "审查员");
        assert_eq!(definition.description, "代码审查");
        assert_eq!(definition.emoji.as_deref(), Some("👀"));
        assert_eq!(definition.color.as_deref(), Some("purple"));
        assert_eq!(definition.prompt, "你是审查员。");
    }

    #[test]
    fn serialization_preserves_unknown_frontmatter_keys() {
        let existing = "---\nname: old\ndescription: old\nemoji: 👀\ncolor: red\ncustom_key: keep-me\n---\n\nold body\n";
        let definition = AgentDefinition {
            name: "new".to_string(),
            description: "new description".to_string(),
            emoji: Some("🤖".to_string()),
            color: None,
            prompt: "new body".to_string(),
        };
        let serialized = serialize_agent_md(Some(existing), &definition).expect("serialize");
        assert!(serialized.contains("custom_key: keep-me"));
        assert!(serialized.contains("name: new"));
        assert!(!serialized.contains("color:"));
        let reparsed = parse_agent_md(&serialized).expect("reparse");
        assert_eq!(reparsed.name, "new");
        assert_eq!(reparsed.emoji.as_deref(), Some("🤖"));
        assert_eq!(reparsed.prompt, "new body");
    }

    #[test]
    fn relative_paths_reject_escapes() {
        assert!(normalize_relative("../outside.md").is_err());
        assert!(normalize_relative("custom/../../outside.md").is_err());
        assert!(normalize_relative("C:\\absolute\\agent.md").is_err());
        assert!(normalize_relative("/absolute/agent.md").is_err());
        assert!(normalize_relative("custom/agent.txt").is_err());
        assert_eq!(
            normalize_relative("custom\\agent.md").unwrap(),
            "custom/agent.md"
        );
    }

    #[test]
    fn write_only_rewrites_changed_definitions() {
        let root = temp_root("write");
        let relative = "custom/reviewer.md";
        let definition = AgentDefinition {
            name: "Reviewer".to_string(),
            description: "reviews".to_string(),
            emoji: None,
            color: None,
            prompt: "be precise".to_string(),
        };
        write_definition(&root, relative, &definition).expect("first write");
        let path = root.join("custom/reviewer.md");
        let first_mtime = fs::metadata(&path).unwrap().modified().unwrap();

        write_definition(&root, relative, &definition).expect("no-op write");
        let second_mtime = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(first_mtime, second_mtime);

        let changed = AgentDefinition {
            prompt: "be very precise".to_string(),
            ..definition
        };
        write_definition(&root, relative, &changed).expect("changed write");
        assert!(root.join("custom/reviewer.md.bak.1").is_file());
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("be very precise"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_custom_files_can_be_deleted() {
        let root = temp_root("delete");
        let definition = AgentDefinition {
            name: "Reviewer".to_string(),
            description: String::new(),
            emoji: None,
            color: None,
            prompt: "review".to_string(),
        };
        write_definition(&root, "engineering/reviewer.md", &definition).expect("write library");
        assert!(delete_definition(&root, "engineering/reviewer.md").is_err());
        assert!(root.join("engineering/reviewer.md").is_file());

        write_definition(&root, "custom/reviewer.md", &definition).expect("write custom");
        delete_definition(&root, "custom/reviewer.md").expect("delete custom");
        assert!(!root.join("custom/reviewer.md").is_file());
        assert!(root.join("custom/reviewer.md.bak.1").is_file());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_discovers_nested_files_and_skips_backups() {
        let root = temp_root("scan");
        write_definition(
            &root,
            "engineering/one.md",
            &AgentDefinition {
                name: "One".to_string(),
                ..AgentDefinition::default()
            },
        )
        .unwrap();
        fs::write(
            root.join("engineering/one.md.bak.1"),
            "---\nname: backup\n---\n",
        )
        .unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden/two.md"), "---\nname: hidden\n---\n").unwrap();

        let scanned = scan(&root);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].relative, "engineering/one.md");
        assert_eq!(scanned[0].definition.name, "One");

        fs::remove_dir_all(&root).ok();
    }

    fn configured_agent(file: &str) -> SubAgent {
        SubAgent {
            id: id_for_relative(file),
            name: String::new(),
            description: String::new(),
            agent_type: AGENT_TYPE_LLM.to_string(),
            system_prompt: String::new(),
            model: None,
            max_complete_tokens: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Vec::new(),
            allowed_skills: Vec::new(),
            max_iterations: 10,
            enable_mcp: false,
            emoji: None,
            color: None,
            file: Some(file.to_string()),
            parse_error: None,
            configured: true,
            cli: Default::default(),
            cloud: Default::default(),
            require_approval: true,
            enabled: true,
        }
    }

    #[test]
    fn resolve_configured_migrates_inline_prompts_without_discovering_files() {
        let root = temp_root("resolve");
        write_definition(
            &root,
            "engineering/one.md",
            &AgentDefinition {
                name: "One".to_string(),
                description: "first".to_string(),
                ..AgentDefinition::default()
            },
        )
        .unwrap();

        let mut config = super::super::AgentsConfig::default();
        config.agents.push(super::super::SubAgent {
            id: "legacy".to_string(),
            name: "Legacy".to_string(),
            description: "old".to_string(),
            agent_type: AGENT_TYPE_LLM.to_string(),
            system_prompt: "old prompt".to_string(),
            emoji: None,
            color: None,
            file: None,
            parse_error: None,
            configured: true,
            model: None,
            max_complete_tokens: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Vec::new(),
            allowed_skills: Vec::new(),
            max_iterations: 10,
            enable_mcp: false,
            cli: Default::default(),
            cloud: Default::default(),
            require_approval: true,
            enabled: true,
        });

        let report = resolve_configured(&mut config, &root);
        assert_eq!(report.migrated, 1);
        assert!(root.join("custom/legacy.md").is_file());
        assert_eq!(config.agents.len(), 1, "resolve must not discover files");

        let legacy = &config.agents[0];
        assert_eq!(legacy.file.as_deref(), Some("custom/legacy.md"));
        assert!(legacy.system_prompt.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn refresh_writes_and_replaces_the_index() {
        let root = temp_root("index");
        write_definition(
            &root,
            "engineering/one.md",
            &AgentDefinition {
                name: "One".to_string(),
                ..AgentDefinition::default()
            },
        )
        .unwrap();
        write_definition(
            &root,
            "custom/two.md",
            &AgentDefinition {
                name: "Two".to_string(),
                ..AgentDefinition::default()
            },
        )
        .unwrap();

        // A stale manifest must be replaced, not appended to.
        write_index(
            &root,
            &AgentIndex {
                refreshed_at: "old".to_string(),
                agents: vec![AgentIndexEntry {
                    id: "stale".to_string(),
                    file: "stale.md".to_string(),
                    name: "Stale".to_string(),
                    ..AgentIndexEntry::default()
                }],
                ..AgentIndex::default()
            },
        )
        .unwrap();

        write_index(&root, &build_index(&root)).unwrap();
        let first = load_index(&root).expect("index parses");
        assert_eq!(first.agents.len(), 2);
        assert!(first.agents.iter().all(|entry| entry.file != "stale.md"));
        assert!(root.join(INDEX_FILE).is_file());
        assert!(first.agents.iter().all(|entry| entry.file != INDEX_FILE));

        write_definition(
            &root,
            "engineering/three.md",
            &AgentDefinition {
                name: "Three".to_string(),
                ..AgentDefinition::default()
            },
        )
        .unwrap();
        write_index(&root, &build_index(&root)).unwrap();
        let second = load_index(&root).expect("replaced index parses");
        assert_eq!(second.agents.len(), 3);
        assert!(second
            .agents
            .iter()
            .any(|entry| entry.file == "engineering/three.md"));
        assert!(!second.refreshed_at.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn index_entries_add_virtual_agents_and_skip_missing_files() {
        let root = temp_root("append");
        write_definition(
            &root,
            "engineering/one.md",
            &AgentDefinition {
                name: "One".to_string(),
                ..AgentDefinition::default()
            },
        )
        .unwrap();
        write_definition(
            &root,
            "custom/two.md",
            &AgentDefinition {
                name: "Two".to_string(),
                ..AgentDefinition::default()
            },
        )
        .unwrap();
        let index = build_index(&root);

        let mut agents = vec![configured_agent("engineering/one.md")];
        append_index_agents(&mut agents, &index, &root);
        assert_eq!(agents.len(), 2);
        assert_eq!(
            agents
                .iter()
                .filter(|agent| agent.file.as_deref() == Some("engineering/one.md"))
                .count(),
            1,
            "configured entries are not duplicated from the index"
        );

        let discovered = agents
            .iter()
            .find(|agent| agent.file.as_deref() == Some("custom/two.md"))
            .expect("indexed agent");
        assert!(!discovered.configured);
        assert!(!discovered.enabled);
        assert_eq!(
            discovered.allowed_tools,
            vec!["file_actions".to_string(), "memory".to_string()]
        );

        // The file disappeared but the index is stale: the panel must not show a ghost.
        fs::remove_file(root.join("custom/two.md")).unwrap();
        let mut agents = vec![configured_agent("engineering/one.md")];
        append_index_agents(&mut agents, &index, &root);
        assert_eq!(agents.len(), 1);

        fs::remove_dir_all(&root).ok();
    }
}
