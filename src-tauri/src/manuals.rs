//! The manuals system: pointer-style prompt injection for heavyweight actions.
//!
//! Design (recorded note `design-subagent-prompt-injection`): the system
//! prompt never carries the full reference for an enhanced action. Instead a
//! tiny MANUALS pointer block announces that the manual exists as a plain file
//! in the workspace (NOT a skill — it never appears in any skill list or the
//! `/` mention popup), and the model reads the file with `file_actions` right
//! before performing the action. Skill-like injection, lazy: the cost is paid
//! only when the action actually happens.
//!
//! Every manual body lives in `prompt.rs`; this module owns the names, the
//! workspace file layout and the seeding.

use std::fs;
use std::path::{Path, PathBuf};

use crate::prompt;

/// One manual the pointer block can announce. `key` is also the file stem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manual {
    /// Drawing with the enhanced figure blocks (`svg` / `carve` / `flow` / `html-fig`).
    EnhancedFigures,
    /// Authoring / editing workspace skills by hand.
    CreateSkill,
    /// Building a self-owned MCP server with `mcp_self`.
    CreateMcpServer,
    /// Explicitly creating a sub-agent (`llm` / `cli` / `cloud`) with `agent_admin`.
    CreateSubagent,
    /// Authoring a workspace plugin (manifest, stdio protocol, service attach).
    CreatePlugin,
}

impl Manual {
    /// File stem and stable handle (`enhanced-figures`, `create-skill`, …).
    pub fn name(self) -> &'static str {
        match self {
            Manual::EnhancedFigures => "enhanced-figures",
            Manual::CreateSkill => "create-skill",
            Manual::CreateMcpServer => "create-mcp-server",
            Manual::CreateSubagent => "create-subagent",
            Manual::CreatePlugin => "create-plugin",
        }
    }

    /// The one-line description shown in the pointer block.
    fn title_and_description(self) -> (&'static str, &'static str) {
        match self {
            Manual::EnhancedFigures => (
                "Enhanced figures",
                "draw syntax for `flow` / `svg` / `carve` / `html-fig` blocks — read before \
                 drawing the first figure of the session",
            ),
            Manual::CreateSkill => (
                "Skill creation",
                "how to author/edit a workspace skill (frontmatter, `.data/skills.status` \
                 registration) — read before creating or editing a skill",
            ),
            Manual::CreateMcpServer => (
                "MCP server creation",
                "the JSON-RPC handshake and the `mcp_self` workflow for a self-built server — \
                 read before writing a server script",
            ),
            Manual::CreateSubagent => (
                "Sub-agent creation",
                "field shapes for explicitly creating an `llm` / `cli` / `cloud` agent with \
                 `agent_admin` — read before the first `save` of the session",
            ),
            Manual::CreatePlugin => (
                "Plugin creation",
                "plugin folder + `plugin.json`, the stdio protocol, service attach and \
                 `config.schema.yaml` — read before writing a plugin",
            ),
        }
    }

    /// The full manual body (the words live in `prompt.rs`).
    pub fn body(self) -> String {
        match self {
            Manual::EnhancedFigures => prompt::enhanced_figures_manual(),
            Manual::CreateSkill => prompt::skill_manual(),
            Manual::CreateMcpServer => prompt::mcp_manual(),
            Manual::CreateSubagent => prompt::subagent_manual(),
            Manual::CreatePlugin => prompt::plugin_manual(),
        }
    }
}

/// Workspace folder the manuals are seeded into: `<workspace>/.data/manuals`.
pub fn manuals_root(workspace: &Path) -> PathBuf {
    workspace.join(prompt::MANUALS_DIR)
}

fn manual_path(workspace: &Path, manual: Manual) -> PathBuf {
    manuals_root(workspace).join(format!("{}.md", manual.name()))
}

/// Workspace-relative display path used in the pointer block.
fn manual_display_path(manual: Manual) -> String {
    format!("{}/{}.md", prompt::MANUALS_DIR, manual.name())
}

/// Make sure the given manuals exist as files. A missing manual is seeded from
/// its static body; an existing file is left alone so user edits survive, and
/// a re-seed is always one `file_actions` delete away.
pub(crate) fn seed_manuals(workspace: &Path, manuals: &[Manual]) {
    for manual in manuals {
        let path = manual_path(workspace, *manual);
        if path.exists() {
            continue;
        }
        let dir = manuals_root(workspace);
        if fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let _ = fs::write(&path, manual.body());
    }
}

/// Build the MANUALS pointer block and seed the announced manuals.
///
/// `manuals` is the relevance-filtered set (the caller decides which features
/// are on). Empty input returns an empty string so the prompt stays
/// byte-identical to a session without manuals.
pub(crate) fn pointer_for(workspace: &Path, manuals: &[Manual]) -> String {
    if manuals.is_empty() {
        return String::new();
    }
    seed_manuals(workspace, manuals);
    let lines: Vec<String> = manuals
        .iter()
        .map(|manual| {
            let (title, description) = manual.title_and_description();
            prompt::pointer_line(title, description, &manual_display_path(*manual))
        })
        .collect();
    let mut block = prompt::pointer_block(&lines);
    block.push('\n');
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_names_match_file_paths() {
        for manual in [
            Manual::EnhancedFigures,
            Manual::CreateSkill,
            Manual::CreateMcpServer,
            Manual::CreateSubagent,
            Manual::CreatePlugin,
        ] {
            let line = {
                let (title, description) = manual.title_and_description();
                prompt::pointer_line(title, description, &manual_display_path(manual))
            };
            assert!(
                line.contains(&format!("`{}/{}.md`", prompt::MANUALS_DIR, manual.name())),
                "pointer line must name the seeded file: {line}"
            );
        }
    }

    #[test]
    fn empty_pointer_is_empty() {
        let tmp = std::env::temp_dir().join(format!(
            "ai-chat-manuals-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(pointer_for(&tmp, &[]), "");
        assert!(!tmp.join(prompt::MANUALS_DIR).exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn seeding_writes_missing_files_only() {
        let tmp = std::env::temp_dir().join(format!(
            "ai-chat-manuals-seed-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        pointer_for(&tmp, &[Manual::CreateSkill]);
        let path = manual_path(&tmp, Manual::CreateSkill);
        assert!(path.exists());
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(".data/skills.status"));

        // An edited manual is never overwritten by a later seed.
        fs::write(&path, "user edit").unwrap();
        pointer_for(&tmp, &[Manual::CreateSkill, Manual::CreateSubagent]);
        assert_eq!(fs::read_to_string(&path).unwrap(), "user edit");
        assert!(manual_path(&tmp, Manual::CreateSubagent).exists());
        let _ = fs::remove_dir_all(&tmp);
    }
}
