//! The `experience` tool: query the global store and *install* a package into the
//! workspace, where the ordinary tools run it.
//!
//! There is deliberately no execution here. A package is a portable folder; `install`
//! copies its parts into `<workspace>/.data/{skills,scripts,mcp}` and from then on
//! `use_skill`, `run_shell`/`run_cmd` and `mcp_self` do the running — the same audited
//! paths as any other workspace artifact. `requires` is metadata for whoever installs
//! the dependencies (the model or the user), never something this code executes.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use crate::experience::{self, Experience};
use crate::AppConfig;

pub(crate) fn tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "experience",
            "description": "Global experience packages (app-data, survives every workspace switch). \
                A package is a portable folder holding a method (`guide.md`), optional skill/script/\
                MCP parts, and the environment it needs; this tool never runs it. \
                Use `search` before solving a problem from scratch, `load` to read the method, \
                `install` to copy the parts into this workspace — then run them with the ordinary \
                tools: `use_skill` for the skill part, `run_shell`/`run_cmd` for the scripts, \
                `mcp_self` for the server. `pack` snapshots workspace artifacts into a package, \
                `distill` folds several packages into one, `forget` archives one. \
                Available only while self-evolution mode is on.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["search", "load", "install", "pack", "distill", "forget"]
                    },
                    "slug": {
                        "type": "string",
                        "description": "Package id (lowercase, dashes); the install target name too."
                    },
                    "query": {
                        "type": "string",
                        "description": "search: free text over titles and matchers."
                    },
                    "ext": {
                        "type": "string",
                        "description": "search: match a file extension, e.g. docx."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "install: replace parts that already exist in the workspace (the old copy is kept as .bak.<n>)."
                    },
                    "title": { "type": "string" },
                    "when": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "matchers: ext:docx, mime:…, magic:504b0304[:word/], name:*.pdf, tag:…"
                    },
                    "guide_md": {
                        "type": "string",
                        "description": "pack/distill: the method — how to use it, what to watch out for."
                    },
                    "skill": {
                        "type": "string",
                        "description": "pack: workspace skill folder name under .data/skills."
                    },
                    "scripts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "pack: workspace script names under .data/scripts."
                    },
                    "mcp": {
                        "type": "string",
                        "description": "pack: workspace MCP server folder name under .data/mcp."
                    },
                    "sources": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "distill: package ids to fold together (at least two)."
                    },
                    "keep_sources": {
                        "type": "boolean",
                        "description": "distill: leave the sources in the store instead of archiving them."
                    }
                },
                "required": ["action"]
            }
        }
    })
}

fn app_data_root(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(dir) = app.path().app_data_dir() {
        return Some(dir);
    }
    app.state::<crate::AppState>()
        .config_path
        .parent()
        .map(Path::to_path_buf)
}

fn store_root(app: &AppHandle) -> Option<PathBuf> {
    app_data_root(app).map(|dir| experience::root_for(&dir))
}

pub(crate) async fn handle(
    app: &AppHandle,
    args: &Value,
    workspace_dir: &Path,
    config: &AppConfig,
    _mission_id: Option<&str>,
    _session_id: Option<&str>,
    _cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> String {
    if !config.self_evolution_mode {
        return "Error: the experience tool is only available while self-evolution mode is on."
            .to_string();
    }
    let Some(store) = store_root(app) else {
        return "Error: the app-data directory is unavailable.".to_string();
    };
    match args["action"].as_str().unwrap_or("") {
        "search" => search(
            &store,
            args["query"].as_str().unwrap_or(""),
            args["ext"].as_str().unwrap_or(""),
        ),
        "load" => load_action(&store, args["slug"].as_str().unwrap_or("").trim()),
        "install" => install(&store, args, workspace_dir),
        "pack" => pack(&store, args, workspace_dir),
        "distill" => distill(&store, args),
        "forget" => forget(&store, args["slug"].as_str().unwrap_or("").trim()),
        other => format!(
            "Error: unknown experience action '{other}' \
             (expected search | load | install | pack | distill | forget)."
        ),
    }
}

/// A file the model is about to read turned out to be binary: if a package claims that
/// format, say which one to install instead of leaving the model to guess.
pub(crate) fn unsupported_hint(app: &AppHandle, path: &Path) -> Option<String> {
    let store = store_root(app)?;
    let hits = experience::resolve_for_path(&store, path);
    let first = hits.first()?;
    let mut out = String::from("A global experience package claims this format:\n");
    out.push_str(&format!(
        "- `{}` — install it with `experience {{\"action\":\"install\",\"slug\":\"{}\"}}`, \
         then load its guide and run the script it placed in `.data/scripts/`.\n",
        first.experience.slug(),
        first.experience.slug(),
    ));
    if hits.len() > 1 {
        let others: Vec<&str> = hits.iter().skip(1).map(|hit| hit.experience.slug()).collect();
        out.push_str(&format!("- also claiming it: {}\n", others.join(", ")));
    }
    Some(out)
}

fn search(store: &Path, query: &str, ext: &str) -> String {
    let rows = experience::search(store, query, ext, "");
    if rows.is_empty() {
        return format!(
            "No experience package matched. Store: {}",
            store.display()
        );
    }
    match serde_json::to_string_pretty(&rows) {
        Ok(text) => text,
        Err(err) => format!("Error: cannot render the result: {err}"),
    }
}

/// Parts of a package, as they exist on disk: `skill/`, `scripts/`, `mcp/`.
fn parts_of(package: &Experience) -> Vec<&'static str> {
    let mut parts = Vec::new();
    if package.dir.join("skill").is_dir() {
        parts.push("skill");
    }
    if package.dir.join("scripts").is_dir() {
        parts.push("scripts");
    }
    if package.dir.join("mcp").is_dir() {
        parts.push("mcp");
    }
    parts
}

fn load_action(store: &Path, slug: &str) -> String {
    let Some(package) = experience::load(store, slug) else {
        return format!("Error: no experience package named '{slug}' in the store.");
    };
    let mut out = String::new();
    out.push_str(&format!("--- Experience: {} ---\n", package.slug()));
    out.push_str(&format!("[Experience root: {}]\n", package.dir.display()));
    if !package.manifest.when.is_empty() {
        out.push_str(&format!("[Matches] {}\n", package.manifest.when.join(", ")));
    }
    if !package.manifest.requires.is_empty() {
        out.push_str(&format!(
            "[Requires] {} — install these yourself, the package cannot\n",
            package.manifest.requires.summary()
        ));
    }
    let parts = parts_of(&package);
    out.push_str(&format!(
        "[Parts] {}\n",
        if parts.is_empty() {
            "none (method only)".to_string()
        } else {
            parts.join(", ")
        }
    ));
    out.push_str(&format!(
        "[Install] experience {{\"action\":\"install\",\"slug\":\"{}\"}}\n\n",
        package.slug()
    ));
    out.push_str(package.guide_text().trim());
    out.push('\n');
    out
}

fn install(store: &Path, args: &Value, workspace_dir: &Path) -> String {
    let slug = args["slug"].as_str().unwrap_or("").trim();
    let Some(package) = experience::load(store, slug) else {
        return format!("Error: no experience package named '{slug}' in the store.");
    };
    let overwrite = args["overwrite"].as_bool().unwrap_or(false);
    let mut done: Vec<String> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    // Skill part → `.data/skills/<slug>/`, loaded afterwards with `use_skill`.
    let skill_source = package.dir.join("skill");
    if skill_source.is_dir() {
        let target = workspace_dir.join(".data").join("skills").join(slug);
        if target.exists() && !overwrite {
            conflicts.push(format!("skill `{slug}`"));
        } else {
            match replace_directory(&skill_source, &target, overwrite, true) {
                Ok(()) => done.push(format!(
                    "skill `{slug}` → `.data/skills/{slug}/` (then `use_skill {slug}`)"
                )),
                Err(err) => return format!("Error: cannot install the skill part: {err}"),
            }
        }
    }

    // Scripts → `.data/scripts/<name>` flat, because `run_shell`/`run_cmd` resolve a
    // bare first word against that folder; a collision is refused rather than renamed.
    let scripts_source = package.dir.join("scripts");
    if scripts_source.is_dir() {
        let target_dir = workspace_dir.join(".data").join("scripts");
        if let Ok(entries) = fs::read_dir(&scripts_source) {
            for entry in entries.flatten() {
                let source = entry.path();
                if !source.is_file() {
                    continue;
                }
                let Some(name) = source.file_name().map(|value| value.to_string_lossy().to_string())
                else {
                    continue;
                };
                if name.starts_with('.') || name.contains(".bak.") {
                    continue;
                }
                let target = target_dir.join(&name);
                if target.exists() && !overwrite {
                    conflicts.push(format!("script `{name}`"));
                    continue;
                }
                if let Err(err) = fs::create_dir_all(&target_dir) {
                    return format!("Error: cannot create `.data/scripts`: {err}");
                }
                if target.exists() {
                    let _ = fs::rename(&target, next_backup_path(&target));
                }
                if let Err(err) = fs::copy(&source, &target) {
                    return format!("Error: cannot install the script `{name}`: {err}");
                }
                done.push(format!(
                    "script `{name}` → `.data/scripts/{name}` (then `run_shell {name} …`)"
                ));
            }
        }
    }

    // MCP part → `.data/mcp/<slug>/`; registering it stays with `mcp_self`, so there is
    // exactly one writer for the roster and the user sees the usual confirmation.
    let mcp_source = package.dir.join("mcp");
    if mcp_source.is_dir() {
        let target = workspace_dir.join(".data").join("mcp").join(slug);
        if target.exists() && !overwrite {
            conflicts.push(format!("mcp folder `{slug}`"));
        } else {
            match replace_directory(&mcp_source, &target, overwrite, false) {
                Ok(()) => done.push(format!(
                    "mcp `{slug}` → `.data/mcp/{slug}/` (then register it with `mcp_self`)"
                )),
                Err(err) => return format!("Error: cannot install the mcp part: {err}"),
            }
        }
    }

    if done.is_empty() && conflicts.is_empty() {
        return format!(
            "Error: package '{slug}' has no installable parts \
             (expected `skill/`, `scripts/` or `mcp/` next to experience.json)."
        );
    }

    let mut report = String::new();
    if !done.is_empty() {
        report.push_str(&format!("Installed `{slug}` into this workspace:\n"));
        for line in &done {
            report.push_str(&format!("- {line}\n"));
        }
    }
    if !conflicts.is_empty() {
        report.push_str(&format!(
            "\nAlready present (pass `overwrite: true` to replace, the old copy is kept as \
             `.bak.<n>`): {}\n",
            conflicts.join(", ")
        ));
    }
    if !package.manifest.requires.is_empty() {
        report.push_str(&format!(
            "\nEnvironment (install it yourself, the package does not): {}\n",
            package.manifest.requires.summary()
        ));
    }
    report.push_str("\nNothing was executed — run it with the tools named above.");
    report
}

fn pack(store: &Path, args: &Value, workspace_dir: &Path) -> String {
    let slug = args["slug"].as_str().unwrap_or("").trim();
    if let Err(err) = experience::validate_slug(slug) {
        return format!("Error: {err}");
    }
    let target = store.join(slug);
    if target.exists() {
        return format!("Error: a package named '{slug}' already exists; choose another slug.");
    }

    let data = workspace_dir.join(".data");
    let skill = args["skill"].as_str().unwrap_or("").trim();
    let mcp = args["mcp"].as_str().unwrap_or("").trim();
    let scripts: Vec<String> = args["scripts"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|value| value.as_str())
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let skill_source = if skill.is_empty() { None } else { Some(data.join("skills").join(skill)) };
    let mcp_source = if mcp.is_empty() { None } else { Some(data.join("mcp").join(mcp)) };
    if let Some(source) = &skill_source {
        if !source.is_dir() {
            return format!("Error: no workspace skill `{}` (expected {})", skill, source.display());
        }
    }
    if let Some(source) = &mcp_source {
        if !source.is_dir() {
            return format!("Error: no workspace mcp folder `{}` (expected {})", mcp, source.display());
        }
    }
    for name in &scripts {
        if !data.join("scripts").join(name).is_file() {
            return format!("Error: no workspace script `{name}` under `.data/scripts`.");
        }
    }
    if skill_source.is_none() && mcp_source.is_none() && scripts.is_empty() {
        return "Error: `pack` needs at least one of `skill`, `scripts`, `mcp` — a package with \
                no payload is a note, not an experience."
            .to_string();
    }

    let staging = store.join(format!(".tmp-pack-{}-{}", std::process::id(), nanos()));
    if let Err(err) = fs::create_dir_all(&staging) {
        return format!("Error: cannot create the staging folder: {err}");
    }
    if let Some(source) = &skill_source {
        if let Err(err) = copy_directory(source, &staging.join("skill")) {
            let _ = fs::remove_dir_all(&staging);
            return format!("Error: cannot copy the skill: {err}");
        }
    }
    if !scripts.is_empty() {
        let folder = staging.join("scripts");
        if let Err(err) = fs::create_dir_all(&folder) {
            let _ = fs::remove_dir_all(&staging);
            return format!("Error: cannot create the scripts folder: {err}");
        }
        for name in &scripts {
            let source = data.join("scripts").join(name);
            if let Err(err) = fs::copy(&source, folder.join(name)) {
                let _ = fs::remove_dir_all(&staging);
                return format!("Error: cannot copy the script `{name}`: {err}");
            }
        }
    }
    if let Some(source) = &mcp_source {
        if let Err(err) = copy_directory(source, &staging.join("mcp")) {
            let _ = fs::remove_dir_all(&staging);
            return format!("Error: cannot copy the mcp folder: {err}");
        }
    }

    let guide = args["guide_md"]
        .as_str()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| {
            format!(
                "# {slug}\n\nPacked from a workspace that solved this once. \
                 Run `install`, then use the tools named in the manifest parts.\n"
            )
        });
    if let Err(err) = fs::write(staging.join("guide.md"), format!("{guide}\n")) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot write the guide: {err}");
    }

    let mut requires = experience::scan_requirements(&[], &experience::payload_files(&staging));
    // No command line to read the runtimes from any more: infer them from the payload.
    for file in experience::payload_files(&staging) {
        let name = match file.extension().and_then(|value| value.to_str()).unwrap_or("") {
            "py" => Some("python"),
            "js" | "mjs" | "cjs" => Some("node"),
            "ps1" => Some("pwsh"),
            "sh" => Some("sh"),
            _ => None,
        };
        if let Some(name) = name {
            if !requires.tools.iter().any(|tool| tool == name) {
                requires.tools.push(name.to_string());
            }
        }
    }

    let when: Vec<String> = args["when"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|value| value.as_str())
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let title = args["title"]
        .as_str()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| slug.to_string());

    let manifest = json!({
        "schema": 1,
        "slug": slug,
        "title": title,
        "when": when,
        "state": "verified",
        "source_workspace": workspace_dir.display().to_string(),
        "requires": requires,
    });
    if let Err(err) = fs::write(
        staging.join(experience::MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string()),
    ) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot write the manifest: {err}");
    }

    if let Err(err) = fs::rename(&staging, &target) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot install the package: {err}");
    }

    let installed = experience::load(store, slug);
    let summary = installed
        .as_ref()
        .map(|package| package.manifest.requires.summary())
        .unwrap_or_default();
    let mut report = format!(
        "Packed `{slug}` ({}):\n",
        if skill_source.is_some() {
            format!("skill, {} script(s), mcp", scripts.len())
        } else if mcp_source.is_some() {
            format!("{} script(s), mcp", scripts.len())
        } else {
            format!("{} script(s)", scripts.len())
        }
    );
    if let Some(package) = &installed {
        report.push_str(&format!("- store: {}\n", package.dir.display()));
    }
    if !summary.is_empty() {
        report.push_str(&format!("- requires: {summary}\n"));
    }
    report.push_str("- share it by copying the folder; `requires` tells the other machine what to install");
    report
}

fn distill(store: &Path, args: &Value) -> String {
    let target_slug = args["slug"].as_str().unwrap_or("").trim();
    if let Err(err) = experience::validate_slug(target_slug) {
        return format!("Error: {err}");
    }
    let sources: Vec<String> = args["sources"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|value| value.as_str())
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if sources.len() < 2 {
        return "Error: `distill` needs at least two sources \
                (`sources: [\"a\", \"b\"]`)."
            .to_string();
    }
    if sources.iter().any(|slug| slug == target_slug) {
        return "Error: the distilled package needs its own new slug.".to_string();
    }
    if store.join(target_slug).exists() {
        return format!("Error: a package named '{target_slug}' already exists.");
    }

    let mut packages: Vec<Experience> = Vec::new();
    for slug in &sources {
        match experience::load(store, slug) {
            Some(package) => packages.push(package),
            None => return format!("Error: no experience package '{slug}' in the store."),
        }
    }

    let staging = store.join(format!(".tmp-distill-{}-{}", std::process::id(), nanos()));
    if let Err(err) = fs::create_dir_all(&staging) {
        return format!("Error: cannot create the staging folder: {err}");
    }

    let mut when: Vec<String> = Vec::new();
    let mut guide = String::new();
    let mut script_names: Vec<String> = Vec::new();
    let mut has_skill = false;
    let mut has_mcp = false;
    let mut copied_scripts: Vec<String> = Vec::new();

    if let Some(text) = args["guide_md"].as_str() {
        if !text.trim().is_empty() {
            guide.push_str(text.trim());
            guide.push_str("\n\n");
        }
    }
    for package in &packages {
        for matcher in &package.manifest.when {
            if !when.iter().any(|existing| existing == matcher) {
                when.push(matcher.clone());
            }
        }
        if guide.is_empty() || args["guide_md"].as_str().unwrap_or("").trim().is_empty() {
            let title = if package.manifest.title.trim().is_empty() {
                package.slug().to_string()
            } else {
                package.manifest.title.clone()
            };
            guide.push_str(&format!("## {title} (`{}`)\n\n", package.slug()));
            let body = package.guide_text();
            guide.push_str(if body.trim().is_empty() { "_(no guide)_" } else { body.trim() });
            guide.push_str("\n\n");
        }

        // Payload: a skill is one folder per package, scripts are flattened (they are
        // invoked by bare name), and only one MCP server can survive.
        let skill_source = package.dir.join("skill");
        if skill_source.is_dir() && !has_skill {
            if let Err(err) = copy_directory(&skill_source, &staging.join("skill")) {
                let _ = fs::remove_dir_all(&staging);
                return format!("Error: cannot copy the skill of '{}': {err}", package.slug());
            }
            has_skill = true;
        }
        let scripts_source = package.dir.join("scripts");
        if let Ok(entries) = fs::read_dir(&scripts_source) {
            for entry in entries.flatten() {
                let source = entry.path();
                if !source.is_file() {
                    continue;
                }
                let Some(name) = source.file_name().map(|value| value.to_string_lossy().to_string())
                else {
                    continue;
                };
                if copied_scripts.iter().any(|existing| existing == &name) {
                    script_names.push(name);
                    continue;
                }
                if let Err(err) = fs::create_dir_all(staging.join("scripts")) {
                    let _ = fs::remove_dir_all(&staging);
                    return format!("Error: cannot create the scripts folder: {err}");
                }
                if let Err(err) = fs::copy(&source, staging.join("scripts").join(&name)) {
                    let _ = fs::remove_dir_all(&staging);
                    return format!("Error: cannot copy the script `{name}`: {err}");
                }
                copied_scripts.push(name);
            }
        }
        let mcp_source = package.dir.join("mcp");
        if mcp_source.is_dir() {
            if has_mcp {
                let _ = fs::remove_dir_all(&staging);
                return "Error: only one package with an `mcp/` part can anchor a distillation"
                    .to_string();
            }
            if let Err(err) = copy_directory(&mcp_source, &staging.join("mcp")) {
                let _ = fs::remove_dir_all(&staging);
                return format!("Error: cannot copy the mcp folder of '{}': {err}", package.slug());
            }
            has_mcp = true;
        }
    }

    if let Err(err) = fs::write(staging.join("guide.md"), format!("{}\n", guide.trim())) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot write the guide: {err}");
    }
    let requires = experience::scan_requirements(&[], &experience::payload_files(&staging));
    let manifest = json!({
        "schema": 1,
        "slug": target_slug,
        "title": args["title"].as_str().unwrap_or(target_slug),
        "when": when,
        "state": "draft",
        "distilled_from": sources.clone(),
        "requires": requires,
    });
    if let Err(err) = fs::write(
        staging.join(experience::MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string()),
    ) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot write the manifest: {err}");
    }
    let target = store.join(target_slug);
    if let Err(err) = fs::rename(&staging, &target) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot install the package: {err}");
    }

    let keep = args["keep_sources"].as_bool().unwrap_or(false);
    let mut archived: Vec<String> = Vec::new();
    if !keep {
        for slug in &sources {
            if archive_package(store, slug).is_ok() {
                archived.push(slug.clone());
            }
        }
    }
    let mut report = format!(
        "Distilled `{target_slug}` from {} package(s):\n- skill: {}\n- scripts: {}\n- mcp: {}\n",
        sources.len(),
        if has_skill { "yes" } else { "no" },
        if copied_scripts.is_empty() {
            "none".to_string()
        } else {
            copied_scripts.join(", ")
        },
        if has_mcp { "yes" } else { "no" },
    );
    if script_names.is_empty() {
        report.push_str("- every script name was unique\n");
    } else {
        report.push_str(&format!(
            "- dropped duplicate script names (first package wins): {}\n",
            script_names.join(", ")
        ));
    }
    report.push_str(&format!(
        "- state: `draft` — run it once in a workspace, then `pack` it again as `{target_slug}` \
         to mark it verified\n"
    ));
    if archived.is_empty() {
        report.push_str("- sources kept (`keep_sources: true`)\n");
    } else {
        report.push_str(&format!("- archived sources: {archived:?}\n"));
    }
    report
}

fn forget(store: &Path, slug: &str) -> String {
    match archive_package(store, slug) {
        Ok(path) => format!(
            "Archived `{slug}` to {}. It no longer resolves on this machine; move the folder \
             back to restore it.",
            path.display()
        ),
        Err(err) => format!("Error: {err}"),
    }
}

/// Move a package out of resolution without deleting it. Shared by `forget` and the
/// panel's archive button.
fn archive_package(store: &Path, slug: &str) -> Result<PathBuf, String> {
    experience::validate_slug(slug)?;
    let source = store.join(slug);
    if !source.is_dir() {
        return Err(format!("no experience package '{slug}'"));
    }
    let archive = store.join(".archive");
    fs::create_dir_all(&archive).map_err(|err| format!("cannot create `.archive`: {err}"))?;
    let target = next_backup_path(&archive.join(slug));
    fs::rename(&source, &target).map_err(|err| format!("cannot archive '{slug}': {err}"))?;
    Ok(target)
}

/// `<path>.bak.<n>`, the convention every other backup in this app follows.
fn next_backup_path(path: &Path) -> PathBuf {
    let mut counter = 1u32;
    loop {
        let candidate = PathBuf::from(format!("{}.bak.{counter}", path.display()));
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}

fn copy_directory(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|err| format!("cannot create {}: {err}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|err| format!("cannot read {}: {err}", from.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.contains(".bak.") {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        if source.is_dir() {
            copy_directory(&source, &target)?;
        } else if source.is_file() {
            fs::copy(&source, &target)
                .map_err(|err| format!("cannot copy {}: {err}", source.display()))?;
        }
    }
    Ok(())
}

/// Replace `target` with a copy of `source`, backing up what was there (`backup` false
/// means the caller already decided the target is free).
fn replace_directory(
    source: &Path,
    target: &Path,
    overwrite: bool,
    backup: bool,
) -> Result<(), String> {
    if target.exists() {
        if !overwrite {
            return Err(format!("{} already exists", target.display()));
        }
        if backup {
            let keep = next_backup_path(target);
            fs::rename(target, &keep)
                .map_err(|err| format!("cannot back up {}: {err}", target.display()))?;
        } else {
            fs::remove_dir_all(target)
                .map_err(|err| format!("cannot replace {}: {err}", target.display()))?;
        }
    }
    copy_directory(source, target)
}

pub(crate) fn list_experiences(app: AppHandle) -> Result<Value, String> {
    let store = store_root(&app).ok_or_else(|| "the app-data directory is unavailable".to_string())?;
    let rows: Vec<Value> = experience::scan(&store)
        .into_iter()
        .map(|package| {
            let parts = parts_of(&package);
            json!({
                "slug": package.slug(),
                "title": package.manifest.title,
                "state": package.manifest.state,
                // Kept for the existing panel: a package is installed, not trusted.
                "when": package.manifest.when,
                "tools": parts,
                "has_guide": !package.guide_text().trim().is_empty(),
                "fingerprint": package.fingerprint(),
                "dir": package.dir.display().to_string(),
                "source_workspace": package.manifest.source_workspace,
                "requires": package.manifest.requires,
                "requires_summary": package.manifest.requires.summary(),
                "parts": parts.clone(),
            })
        })
        .collect();
    Ok(json!({ "store": store.display().to_string(), "experiences": rows }))
}

pub(crate) fn delete_experience(app: AppHandle, slug: String) -> Result<(), String> {
    let store = store_root(&app).ok_or_else(|| "the app-data directory is unavailable".to_string())?;
    archive_package(&store, slug.trim()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("rabbit-exp-tool-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = root.join("experiences");
        let workspace = root.join("ws");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(workspace.join(".data")).unwrap();
        (store, workspace)
    }

    fn write_package(store: &Path, slug: &str) {
        let dir = store.join(slug);
        fs::create_dir_all(dir.join("skill")).unwrap();
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::create_dir_all(dir.join("mcp")).unwrap();
        fs::write(dir.join("guide.md"), "how to use it").unwrap();
        fs::write(dir.join("skill").join("skill.md"), "---\nname: docx-text\n---\nbody").unwrap();
        fs::write(dir.join("scripts").join("docx2text.py"), "print('x')").unwrap();
        fs::write(dir.join("mcp").join("server.js"), "// server").unwrap();
        fs::write(
            dir.join(experience::MANIFEST_FILE),
            serde_json::to_string(&json!({
                "schema": 1,
                "slug": slug,
                "title": "DOCX",
                "when": ["ext:docx"],
                "state": "verified",
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn install_copies_parts_into_the_workspace_and_refuses_to_clobber() {
        let (store, workspace) = workspace("install");
        write_package(&store, "docx-text");

        let report = install(&store, &json!({ "action": "install", "slug": "docx-text" }), &workspace);
        assert!(report.contains("Installed `docx-text`"), "{report}");
        assert!(workspace.join(".data/skills/docx-text/skill.md").is_file());
        assert!(workspace.join(".data/scripts/docx2text.py").is_file());
        assert!(workspace.join(".data/mcp/docx-text/server.js").is_file());

        // A second run must not overwrite the workspace copy without `overwrite`.
        let again = install(&store, &json!({ "action": "install", "slug": "docx-text" }), &workspace);
        assert!(again.contains("Already present"), "{again}");
        assert!(again.contains("script `docx2text.py`"), "{again}");

        // `overwrite` replaces and keeps the previous copy as `.bak.1`.
        let forced = install(
            &store,
            &json!({ "action": "install", "slug": "docx-text", "overwrite": true }),
            &workspace,
        );
        assert!(forced.contains("Installed `docx-text`"), "{forced}");
        let _ = fs::remove_dir_all(workspace.parent().unwrap());
    }

    #[test]
    fn pack_snapshots_workspace_artifacts_into_the_store() {
        let (store, workspace) = workspace("pack");
        fs::create_dir_all(workspace.join(".data/skills/docx-text")).unwrap();
        fs::write(workspace.join(".data/skills/docx-text/skill.md"), "---\nname: docx-text\n---").unwrap();
        fs::create_dir_all(workspace.join(".data/scripts")).unwrap();
        fs::write(workspace.join(".data/scripts/docx2text.py"), "import docx\nprint('x')\n").unwrap();

        let report = pack(
            &store,
            &json!({
                "action": "pack",
                "slug": "docx-text",
                "title": "DOCX",
                "when": ["ext:docx"],
                "skill": "docx-text",
                "scripts": ["docx2text.py"],
                "guide_md": "run docx2text"
            }),
            &workspace,
        );
        assert!(report.contains("Packed `docx-text`"), "{report}");

        let package = experience::load(&store, "docx-text").expect("package");
        assert!(package.dir.join("skill/skill.md").is_file());
        assert!(package.dir.join("scripts/docx2text.py").is_file());
        assert_eq!(package.manifest.when, vec!["ext:docx".to_string()]);
        // The runtime is inferred from the payload now that there is no command line.
        assert!(package.manifest.requires.tools.iter().any(|tool| tool == "python"));
        assert!(package
            .manifest
            .requires
            .packages
            .get("python")
            .map(|modules| modules.iter().any(|module| module == "docx"))
            .unwrap_or(false));

        // And installing it back into a fresh workspace restores the same layout.
        let other = workspace.parent().unwrap().join("ws2");
        fs::create_dir_all(other.join(".data")).unwrap();
        let installed = install(&store, &json!({ "slug": "docx-text" }), &other);
        assert!(installed.contains("Installed `docx-text`"), "{installed}");
        assert!(other.join(".data/scripts/docx2text.py").is_file());
        let _ = fs::remove_dir_all(workspace.parent().unwrap());
    }

    #[test]
    fn distill_merges_payloads_and_archives_sources() {
        let (store, _workspace) = workspace("distill");
        write_package(&store, "docx-text");
        write_package(&store, "pdf-text");
        // Two mcp parts cannot merge into one package.
        let blocked = distill(&store, &json!({ "slug": "both", "sources": ["docx-text", "pdf-text"] }));
        assert!(blocked.contains("only one package with an `mcp/` part"), "{blocked}");

        let _ = fs::remove_dir_all(store.join("pdf-text/mcp"));
        let report = distill(&store, &json!({ "slug": "doc-text", "sources": ["docx-text", "pdf-text"] }));
        assert!(report.starts_with("Distilled `doc-text`"), "{report}");
        assert!(!store.join("docx-text").exists());
        let merged = experience::load(&store, "doc-text").expect("distilled package");
        assert!(merged.dir.join("skill/skill.md").is_file());
        assert!(merged.dir.join("mcp/server.js").is_file());
        assert_eq!(merged.manifest.when, vec!["ext:docx".to_string()]);
        let _ = fs::remove_dir_all(store.parent().unwrap());
    }
}
