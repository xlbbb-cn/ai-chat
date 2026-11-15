//! The `experience` tool — the only entry point to global experience packages.
//!
//! Actions:
//!
//! * `search` — deterministic registry query (matcher hits, tags, title).
//! * `load`   — inject the package guide + tool schemas into the turn.
//! * `invoke` — start the package's tool process once and read its result.
//! * `pack`   — turn workspace learnings into a global package (verified).
//!
//! Isolation is deliberate: packages are never reachable through `use_skill`,
//! the workspace skills root, the `run_cmd` library or the MCP roster, and the
//! tool itself is only advertised while self-evolution mode is on.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use super::emit_tool_progress;
use super::process::run_program_capture;
use crate::experience::{self, Experience, ToolSpec};
use crate::AppConfig;

/// Confirmation kind recorded in `auto_accept_confirm_kinds` when the user
/// opts into auto-accepting first runs. Kept separate from the command-risk
/// kinds on purpose.
const CONFIRM_KIND: &str = "experience_run";

pub(crate) fn tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "experience",
            "description": "Global experience packages (app-data `<app_data>/experiences/<slug>/`). \
                A package is NOT a skill: never load it with `use_skill`, never expect it in the \
                workspace skills folder, the `run_cmd` library or the MCP panel. It bundles a \
                `guide.md` plus executable tools and runs as its own short-lived tool process.\n\n\
                - `search` {query?, ext?, tag?} — deterministic lookup (matcher/extension/tag/title). \
                Call it before falling back to ad-hoc exploration.\n\
                - `load` {slug} — inject the guide and the exact tool schemas/arguments.\n\
                - `invoke` {slug, tool, args} — run one tool; the app spawns the package process, \
                enforces its timeout and returns its output. The first run of a package asks the \
                user (approval is bound to the package fingerprint).\n\
                - `pack` {slug, title, when[], guide_md, files[], tools[], contract?, verify?} — \
                promote what you learned in this workspace into a global package: files are copied \
                from the workspace into `bin/`, `when` are program-side matchers (`ext:docx`, \
                `magic:504b0304:word/`, `mime:…`, `name:*.pdf`, `tag:…`), and `verify` must pass \
                before the package is stored as verified.\n\n\
                Only available while self-evolution mode is on.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["search", "load", "invoke", "pack"],
                        "description": "Which experience operation to run."
                    },
                    "query": {
                        "type": "string",
                        "description": "`search`: free-text match on title/slug/guide."
                    },
                    "ext": {
                        "type": "string",
                        "description": "`search`: file extension the package must declare, e.g. 'docx'."
                    },
                    "tag": {
                        "type": "string",
                        "description": "`search`: `tag:` matcher the package must declare."
                    },
                    "slug": {
                        "type": "string",
                        "description": "Package id (letters, digits, '-' and '_')."
                    },
                    "tool": {
                        "type": "string",
                        "description": "`invoke`: tool name declared by the package (see `load`)."
                    },
                    "args": {
                        "type": "object",
                        "description": "`invoke`: tool arguments; only the names declared in the package are accepted."
                    },
                    "title": { "type": "string", "description": "`pack`: one-line title." },
                    "when": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "`pack`: program-side matchers, e.g. [\"ext:docx\",\"magic:504b0304:word/\"]."
                    },
                    "guide_md": {
                        "type": "string",
                        "description": "`pack`: the guide the model will read on `load` — when to use the package, exact commands, pitfalls, verification."
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "`pack`: workspace-relative files to copy into the package `bin/`, e.g. [\"./.data/scripts/docx2text.py\"]."
                    },
                    "tools": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "`pack`: [{name, description, command:[program,...], args:{name:{type,required}}}]."
                    },
                    "contract": {
                        "type": "string",
                        "enum": ["argv", "json"],
                        "description": "`pack`/`invoke`: 'argv' (default, arguments on the command line) or 'json' (one JSON envelope on stdin, exactly one JSON envelope on stdout)."
                    },
                    "verify": {
                        "type": "object",
                        "description": "`pack`: {tool, args} run once after install; a non-zero exit keeps the package as 'draft'."
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
    mission_id: Option<&str>,
    session_id: Option<&str>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> String {
    if !config.self_evolution_mode {
        return "Error: the experience tool is only available while self-evolution mode is on."
            .to_string();
    }
    let Some(store) = store_root(app) else {
        return "Error: the app-data directory is unavailable, so no experience store exists."
            .to_string();
    };

    match args["action"].as_str().unwrap_or("") {
        "search" => search(&store, args),
        "load" => load(&store, args),
        "invoke" => invoke(app, &store, args, mission_id, session_id, cancel).await,
        "pack" => pack(app, &store, args, workspace_dir).await,
        other => format!(
            "Error: unknown experience action '{other}' (expected search | load | invoke | pack)."
        ),
    }
}

/// Reply for a file the app cannot read as text, when a package claims it.
pub(crate) fn unsupported_hint(app: &AppHandle, path: &Path) -> Option<String> {
    let store = store_root(app)?;
    let hits = experience::resolve_for_path(&store, path);
    if hits.is_empty() {
        return None;
    }
    Some(experience::render_unsupported(path, &hits))
}

// ─── Panel commands ──────────────────────────────────────────────────────────

/// Registry view for the Experience panel: what exists, what it claims, whether
/// the current build is trusted, and which build it is.
#[tauri::command]
pub(crate) fn list_experiences(app: AppHandle) -> Result<Value, String> {
    let store = store_root(&app).ok_or_else(|| "the app-data directory is unavailable".to_string())?;
    let rows: Vec<Value> = experience::scan(&store)
        .into_iter()
        .map(|item| {
            json!({
                "slug": item.slug(),
                "title": item.manifest.title,
                "state": item.manifest.state,
                "contract": item.manifest.contract,
                "when": item.manifest.when,
                "tools": item.manifest.tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>(),
                "has_guide": item.guide_path().is_some(),
                "fingerprint": item.fingerprint(),
                "trusted": trust_allows(&store, &item),
                "dir": item.dir.display().to_string(),
                "source_workspace": item.manifest.source_workspace,
            })
        })
        .collect();
    Ok(json!({ "store": store.display().to_string(), "experiences": rows }))
}

/// Archive a package so it stops resolving. Never a hard delete: the folder
/// moves to `<store>/.archive/<slug>.bak.<n>` and can be restored by hand.
#[tauri::command]
pub(crate) fn delete_experience(app: AppHandle, slug: String) -> Result<(), String> {
    let store = store_root(&app).ok_or_else(|| "the app-data directory is unavailable".to_string())?;
    experience::validate_slug(&slug)?;
    let target = store.join(&slug);
    if !target.is_dir() {
        return Err(format!("no experience package '{slug}'"));
    }
    let archive = store.join(".archive");
    fs::create_dir_all(&archive).map_err(|err| format!("cannot create .archive: {err}"))?;
    let backup = next_backup_path(&archive.join(&slug));
    fs::rename(&target, &backup).map_err(|err| format!("cannot archive '{slug}': {err}"))?;
    Ok(())
}

// ─── search / load ───────────────────────────────────────────────────────────

fn search(store: &Path, args: &Value) -> String {
    let rows = experience::search(
        store,
        args["query"].as_str().unwrap_or(""),
        args["ext"].as_str().unwrap_or(""),
        args["tag"].as_str().unwrap_or(""),
    );
    if rows.is_empty() {
        return format!(
            "No experience package matched. Store: `{}`.\n\
             Nothing has been packed here yet — solve the problem in this workspace, then call \
             `experience` `pack` to make it global.",
            store.display()
        );
    }
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
}

fn load(store: &Path, args: &Value) -> String {
    let slug = args["slug"].as_str().unwrap_or("").trim();
    let Some(experience) = experience::load(store, slug) else {
        return format!(
            "Error: no experience package '{slug}' under `{}`. Use `search` first.",
            store.display()
        );
    };
    format!(
        "Experience package loaded (this is NOT a skill — do not call `use_skill`).\n{}",
        experience::render_block(&experience)
    )
}

// ─── invoke ──────────────────────────────────────────────────────────────────

fn text_of(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        other => Err(format!(
            "argument values must be strings/numbers/booleans, got {}",
            other
        )),
    }
}

/// Map declared arguments onto `--name value` pairs (the `argv` contract).
///
/// Only declared names are accepted: an undeclared key is an error instead of
/// being silently dropped or concatenated into the command line.
fn map_args(spec: &ToolSpec, supplied: &Value) -> Result<Vec<String>, String> {
    let declared = spec.args.as_object().cloned().unwrap_or_default();
    let given = supplied.as_object().cloned().unwrap_or_default();

    for key in given.keys() {
        if !declared.contains_key(key) {
            let known: Vec<&String> = declared.keys().collect();
            return Err(format!(
                "undeclared argument '{key}' (declared: {:?})",
                known
            ));
        }
    }

    let mut out = Vec::new();
    for (name, schema) in declared.iter() {
        let required = schema
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match given.get(name) {
            None | Some(Value::Null) => {
                if required {
                    return Err(format!("missing required argument '{name}'"));
                }
            }
            Some(Value::Bool(true)) => out.push(format!("--{name}")),
            Some(Value::Bool(false)) => {}
            Some(Value::Array(items)) => {
                for item in items {
                    out.push(format!("--{name}"));
                    out.push(text_of(item)?);
                }
            }
            Some(scalar) => {
                out.push(format!("--{name}"));
                out.push(text_of(scalar)?);
            }
        }
    }
    Ok(out)
}

/// Resolve the declared `command` into a program + argument vector.
///
/// A leading entry that names a file inside the package becomes an absolute
/// path; everything else stays a bare program name resolved through `PATH`.
/// Nothing is ever handed to a shell.
fn build_command(
    experience: &Experience,
    spec: &ToolSpec,
    supplied: &Value,
) -> Result<(String, Vec<String>, String), String> {
    let mut parts = spec.command.clone();
    if parts.is_empty() {
        return Err(format!("tool '{}' declares no command", spec.name));
    }
    let mut program = parts.remove(0);
    let bundled = experience.dir.join(&program);
    if bundled.is_file() {
        program = bundled.to_string_lossy().to_string();
    }

    let mut argv: Vec<String> = Vec::new();
    for part in parts {
        let candidate = experience.dir.join(&part);
        if candidate.is_file() {
            argv.push(candidate.to_string_lossy().to_string());
        } else {
            argv.push(part);
        }
    }
    argv.extend(map_args(spec, supplied)?);

    let preview = format!(
        "{} {}\n(cwd: {})",
        program,
        argv.join(" "),
        experience.dir.display()
    );
    Ok((program, argv, preview))
}

fn trust_path(store: &Path) -> PathBuf {
    store.join(".trust.json")
}

fn trust_allows(store: &Path, experience: &Experience) -> bool {
    let Ok(raw) = fs::read_to_string(trust_path(store)) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    value
        .get(experience.slug())
        .and_then(|entry| entry.get("fingerprint"))
        .and_then(Value::as_str)
        .map(|stored| stored == experience.fingerprint())
        .unwrap_or(false)
}

fn remember_trust(store: &Path, experience: &Experience) {
    let path = trust_path(store);
    let mut value: Value = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}));
    if !value.is_object() {
        value = json!({});
    }
    if let Some(map) = value.as_object_mut() {
        map.insert(
            experience.slug().to_string(),
            json!({
                "fingerprint": experience.fingerprint(),
                "at": chrono::Local::now().to_rfc3339(),
            }),
        );
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, serde_json::to_string_pretty(&value).unwrap_or_default());
}

/// First run (or any run after the package changed) needs the user's consent.
///
/// Approval is bound to the package fingerprint, so editing a package — which
/// is how it gains new executable code — invalidates the old approval.
async fn ensure_trusted(
    app: &AppHandle,
    store: &Path,
    experience: &Experience,
    preview: &str,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(), String> {
    if trust_allows(store, experience) {
        return Ok(());
    }
    let confirmation = super::request_tool_confirmation(
        app,
        format!(
            "Experience package `{}` wants to run its tool process.\n\n\
             Package: {}\nFingerprint: {}",
            experience.slug(),
            experience.dir.display(),
            experience.fingerprint()
        ),
        "experience".to_string(),
        preview.to_string(),
        CONFIRM_KIND,
        "none",
        cancel,
    )
    .await;
    if !confirmation.confirmed {
        return Err(format!(
            "⛔ Experience `{}` was not approved to run. Nothing was executed.",
            experience.slug()
        ));
    }
    remember_trust(store, experience);
    Ok(())
}

fn format_capture(code: i32, stdout: &str, stderr: &str) -> String {
    let mut out = String::new();
    if !stdout.trim().is_empty() {
        out.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("STDERR:\n");
        out.push_str(stderr.trim_end());
    }
    if code != 0 {
        out.push_str(&format!("\nExit code: {code}"));
    }
    if out.trim().is_empty() {
        out = "(no output)".to_string();
    }
    out
}

/// Strict `json` contract: exactly one JSON object on stdout.
///
/// `{"content":[{"type":"text","text":…}],"is_error":false}` — anything else
/// (plain text, extra lines, unknown content types) is refused outright rather
/// than wrapped leniently, so a package cannot look like it is following the
/// contract when it is not.
fn parse_json_contract(stdout: &str) -> Result<String, String> {
    let trimmed = stdout.trim();
    let value: Value = serde_json::from_str(trimmed).map_err(|err| {
        format!(
            "Error: contract \"json\" requires exactly one JSON object on stdout ({err}). Got:\n{}",
            trimmed.chars().take(400).collect::<String>()
        )
    })?;
    let Some(content) = value.get("content").and_then(Value::as_array) else {
        return Err(
            "Error: contract \"json\" requires a `content` array of {type:\"text\",text} items."
                .to_string(),
        );
    };
    let mut text = String::new();
    for item in content {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "text" {
            return Err(format!(
                "Error: contract \"json\" only supports text content blocks (got '{kind}')."
            ));
        }
        let Some(body) = item.get("text").and_then(Value::as_str) else {
            return Err("Error: a text content block is missing its `text` string.".to_string());
        };
        text.push_str(body);
    }
    let is_error = value
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(if is_error {
        format!("Error: {text}")
    } else {
        text
    })
}

async fn invoke(
    app: &AppHandle,
    store: &Path,
    args: &Value,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> String {
    let slug = args["slug"].as_str().unwrap_or("").trim();
    let Some(experience) = experience::load(store, slug) else {
        return format!("Error: no experience package '{slug}' under `{}`.", store.display());
    };
    let tool_name = args["tool"].as_str().unwrap_or("").trim();
    let Some(spec) = experience.tool(tool_name) else {
        let known: Vec<String> = experience
            .manifest
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        return format!(
            "Error: package '{slug}' has no tool '{tool_name}' (available: {:?}). Call `load` to see the schemas.",
            known
        );
    };

    let supplied = args.get("args").cloned().unwrap_or_else(|| json!({}));
    let (program, argv, preview) = match build_command(&experience, spec, &supplied) {
        Ok(parts) => parts,
        Err(err) => return format!("Error: {err}"),
    };

    if let Err(err) = ensure_trusted(app, store, &experience, &preview, cancel).await {
        return err;
    }

    let _ = emit_tool_progress(
        app,
        mission_id,
        session_id,
        format!("🧪 *Experience {}::{}*\n\n", slug, tool_name),
    );

    let contract = experience
        .manifest
        .contract
        .trim()
        .to_ascii_lowercase();
    let timeout_secs = args["timeout_seconds"]
        .as_u64()
        .unwrap_or(60)
        .clamp(1, 600);
    let cancel_owned = cancel.cloned();

    match contract.as_str() {
        "argv" => {
            let capture = match run_program_capture(
                program,
                argv,
                Some(experience.dir.clone()),
                timeout_secs,
                None,
                cancel_owned,
            )
            .await
            {
                Ok(capture) => capture,
                Err(err) => return err,
            };
            format_capture(capture.code, &capture.stdout, &capture.stderr)
        }
        "json" => {
            let payload = match serde_json::to_vec(&json!({
                "tool": tool_name,
                "args": supplied,
            })) {
                Ok(bytes) => bytes,
                Err(err) => return format!("Error: cannot encode the request: {err}"),
            };
            let capture = match run_program_capture(
                program,
                argv,
                Some(experience.dir.clone()),
                timeout_secs,
                Some(payload),
                cancel_owned,
            )
            .await
            {
                Ok(capture) => capture,
                Err(err) => return err,
            };
            if capture.code != 0 {
                return format!(
                    "Error: '{slug}' exited with code {}.\n{}",
                    capture.code,
                    format_capture(capture.code, &capture.stdout, &capture.stderr)
                );
            }
            // Strict: a malformed envelope is reported, never wrapped.
            parse_json_contract(&capture.stdout).unwrap_or_else(|err| err)
        }
        "mcp" => format!(
            "Error: contract \"mcp\" is not enabled in this build of the experience tool. \
             Re-pack the package with `contract: \"json\"` (one JSON envelope in/out) or \"argv\"."
        ),
        other => format!(
            "Error: unknown contract '{other}' on package '{slug}' (expected argv | json | mcp)."
        ),
    }
}

// ─── pack ────────────────────────────────────────────────────────────────────

fn next_backup_path(target: &Path) -> PathBuf {
    let mut index = 1;
    loop {
        let candidate = target.with_file_name(format!(
            "{}.bak.{index}",
            target
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "experience".to_string())
        ));
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}

/// Resolve a workspace-relative path and refuse anything escaping the root.
fn resolve_workspace_file(workspace: &Path, raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim().trim_start_matches("./");
    if trimmed.is_empty() {
        return Err("empty file path".to_string());
    }
    let joined = workspace.join(trimmed);
    let canonical = joined
        .canonicalize()
        .map_err(|err| format!("cannot resolve '{raw}': {err}"))?;
    let root = workspace
        .canonicalize()
        .map_err(|err| format!("cannot resolve the workspace root: {err}"))?;
    if !canonical.starts_with(&root) {
        return Err(format!("'{raw}' escapes the workspace root"));
    }
    if !canonical.is_file() {
        return Err(format!("'{raw}' is not a file"));
    }
    Ok(canonical)
}

fn copy_into_bin(source: &Path, bin: &Path) -> Result<String, String> {
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("cannot derive a file name from {}", source.display()))?;
    fs::create_dir_all(bin).map_err(|err| format!("cannot create bin/: {err}"))?;
    fs::copy(source, bin.join(name)).map_err(|err| format!("cannot copy '{name}': {err}"))?;
    Ok(name.to_string())
}

fn normalize_tools(raw: &Value, package_dir: &Path) -> Result<Vec<Value>, String> {
    let Some(items) = raw.as_array() else {
        return Ok(Vec::new());
    };
    let mut tools = Vec::new();
    for item in items {
        let name = item["name"].as_str().unwrap_or("").trim();
        experience::validate_slug(name)?;
        let command: Vec<String> = item["command"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if command.is_empty() {
            return Err(format!("tool '{name}' needs a non-empty `command` array"));
        }
        // Relative entries that look like files must really exist in the
        // package, so a typo fails at pack time instead of at invoke time.
        for part in command.iter().skip(1) {
            let candidate = package_dir.join(part);
            if part.contains('/') || part.contains('\\') {
                if !candidate.is_file() {
                    return Err(format!(
                        "tool '{name}' references '{part}', which is not in the package (did you list it in `files`?)"
                    ));
                }
            }
        }
        tools.push(json!({
            "name": name,
            "description": item["description"].as_str().unwrap_or(""),
            "command": command,
            "args": item.get("args").cloned().unwrap_or_else(|| json!({})),
        }));
    }
    Ok(tools)
}

async fn pack(
    app: &AppHandle,
    store: &Path,
    args: &Value,
    workspace_dir: &Path,
) -> String {
    let slug = args["slug"].as_str().unwrap_or("").trim().to_string();
    if let Err(err) = experience::validate_slug(&slug) {
        return format!("Error: {err}");
    }
    let when: Vec<String> = args["when"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default();
    for matcher in &when {
        if experience::parse_matcher(matcher).is_none() {
            return format!(
                "Error: '{matcher}' is not a valid matcher (ext:…, mime:…, magic:<hex>[:needle], name:…, tag:…)."
            );
        }
    }
    let guide_md = args["guide_md"].as_str().unwrap_or("");
    if guide_md.trim().is_empty() {
        return "Error: pack requires `guide_md` — the guide is what `load` injects.".to_string();
    }
    let contract = args["contract"].as_str().unwrap_or("argv").trim().to_string();
    if contract != "argv" && contract != "json" {
        return format!("Error: contract '{contract}' cannot be packed (use argv or json).");
    }

    let files: Vec<String> = args["files"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if let Err(err) = fs::create_dir_all(store) {
        return format!("Error: cannot create the experience store: {err}");
    }
    let staging = store.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        chrono::Local::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    if let Err(err) = fs::create_dir_all(&staging) {
        return format!("Error: cannot create a staging folder: {err}");
    }

    let copied: Result<Vec<String>, String> = files
        .iter()
        .map(|raw| {
            let source = resolve_workspace_file(workspace_dir, raw)?;
            copy_into_bin(&source, &staging.join("bin"))
        })
        .collect();
    let copied = match copied {
        Ok(names) => names,
        Err(err) => {
            let _ = fs::remove_dir_all(&staging);
            return format!("Error: {err}");
        }
    };

    let tools = match normalize_tools(&args["tools"], &staging) {
        Ok(tools) => tools,
        Err(err) => {
            let _ = fs::remove_dir_all(&staging);
            return format!("Error: {err}");
        }
    };

    if let Err(err) = fs::write(staging.join("guide.md"), guide_md) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot write guide.md: {err}");
    }

    let mut manifest = json!({
        "schema": 1,
        "slug": slug,
        "title": args["title"].as_str().unwrap_or(&slug),
        "when": when,
        "state": "verified",
        "source_workspace": workspace_dir.display().to_string(),
        "source_hash": "",
        "distilled_from": [],
        "guide": { "file": "guide.md", "max_bytes": 8192 },
        "contract": contract,
        "tools": tools,
    });
    if !args["verify"].is_null() {
        manifest["verify"] = args["verify"].clone();
    }
    if let Err(err) = fs::write(
        staging.join(experience::MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    ) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot write the manifest: {err}");
    }

    let target = store.join(&slug);
    if target.exists() {
        let backup = next_backup_path(&target);
        if let Err(err) = fs::rename(&target, &backup) {
            let _ = fs::remove_dir_all(&staging);
            return format!("Error: cannot back up the existing package: {err}");
        }
    }
    if let Err(err) = fs::rename(&staging, &target) {
        let _ = fs::remove_dir_all(&staging);
        return format!("Error: cannot install the package: {err}");
    }

    let installed = match experience::load_dir(&target) {
        Ok(installed) => installed,
        Err(err) => return format!("Error: the installed package is unreadable: {err}"),
    };

    // The fingerprint is written back so the panel (and `search`) can show
    // exactly which build is installed.
    let fingerprint = installed.fingerprint();
    let mut final_manifest = installed.manifest.clone();
    final_manifest.source_hash = fingerprint.clone();
    let _ = fs::write(
        target.join(experience::MANIFEST_FILE),
        serde_json::to_string_pretty(&final_manifest).unwrap_or_default(),
    );

    let mut report = format!(
        "✅ Packed experience `{}` at `{}`\n\
         - tools: {:?}\n- files: {:?}\n- matchers: {:?}\n- fingerprint: {}\n\
         The package is global: a new workspace resolves it from this store, never from the old workspace.",
        slug,
        target.display(),
        installed
            .manifest
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>(),
        copied,
        installed.manifest.when,
        fingerprint
    );

    if let Some(verify) = installed.manifest.verify.clone() {
        let result = verify_package(app, store, &slug, &verify, workspace_dir).await;
        match result {
            Ok(text) => report.push_str(&format!("\n\n[verify passed]\n{text}")),
            Err(err) => {
                // Keep the package, but mark it unverified: nothing may claim
                // "verified" without a passing self-check.
                let mut demoted = installed.manifest.clone();
                demoted.state = "draft".to_string();
                let _ = fs::write(
                    target.join(experience::MANIFEST_FILE),
                    serde_json::to_string_pretty(&demoted).unwrap_or_default(),
                );
                report.push_str(&format!(
                    "\n\n⚠️ verify failed — the package was stored as `draft`, not `verified`:\n{err}"
                ));
            }
        }
    }

    report
}

async fn verify_package(
    app: &AppHandle,
    store: &Path,
    slug: &str,
    verify: &experience::VerifySpec,
    _workspace_dir: &Path,
) -> Result<String, String> {
    let Some(experience) = experience::load(store, slug) else {
        return Err("package disappeared before verification".to_string());
    };
    let Some(spec) = experience.tool(&verify.tool) else {
        return Err(format!(
            "verify names tool '{}', which the package does not declare",
            verify.tool
        ));
    };
    // A freshly packed package is being written by us, so the first run does
    // not need a dialog: the user's approval happens on the first *invoke*.
    let _ = app;
    let (program, argv, _preview) = build_command(&experience, spec, &verify.args)?;
    let capture = run_program_capture(
        program,
        argv,
        Some(experience.dir.clone()),
        120,
        None,
        None,
    )
    .await?;
    if capture.code != 0 {
        return Err(format_capture(capture.code, &capture.stdout, &capture.stderr));
    }
    Ok(format_capture(capture.code, &capture.stdout, &capture.stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(value: Value) -> ToolSpec {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn argv_mapping_is_declared_order_and_declared_names_only() {
        let tool = spec(json!({
            "name": "extract",
            "command": ["python", "bin/x.py"],
            "args": {
                "path": { "type": "string", "required": true },
                "pages": { "type": "string" },
                "flag": { "type": "boolean" }
            }
        }));
        let argv = map_args(
            &tool,
            &json!({ "path": "a.docx", "pages": ["1", "2"], "flag": true }),
        )
        .unwrap();
        // `serde_json::Map` is ordered, so the mapping is deterministic:
        // booleans become switches, arrays repeat.
        assert_eq!(
            argv,
            vec!["--flag", "--pages", "1", "--pages", "2", "--path", "a.docx"]
        );

        assert!(map_args(&tool, &json!({ "path": "a.docx", "extra": "x" })).is_err());
        assert!(map_args(&tool, &json!({})).is_err());
        assert_eq!(
            map_args(&tool, &json!({ "path": "a.docx", "flag": false })).unwrap(),
            vec!["--path", "a.docx"]
        );
    }

    #[test]
    fn json_contract_is_strict() {
        assert_eq!(
            parse_json_contract(r#"{"content":[{"type":"text","text":"ok"}]}"#).unwrap(),
            "ok"
        );
        // Plain text is refused, never wrapped.
        assert!(parse_json_contract("plain text output").is_err());
        // Unknown block types are refused.
        assert!(parse_json_contract(r#"{"content":[{"type":"image"}]}"#).is_err());
        // Trailing garbage is refused.
        assert!(parse_json_contract("{\"content\":[]}\ntrailing").is_err());
        // Missing `text` is refused.
        assert!(parse_json_contract(r#"{"content":[{"type":"text"}]}"#).is_err());
        // `is_error` surfaces as an error string.
        let flagged =
            parse_json_contract(r#"{"content":[{"type":"text","text":"boom"}],"is_error":true}"#)
                .unwrap();
        assert!(flagged.starts_with("Error: boom"));
    }

    #[test]
    fn bundled_commands_become_absolute_and_stay_shell_free() {
        let dir = std::env::temp_dir().join(format!("rabbit-exp-tool-{}", std::process::id()));
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin").join("x.py"), "print(1)").unwrap();
        let experience = Experience {
            dir: dir.clone(),
            manifest: serde_json::from_str(
                r#"{"schema":1,"slug":"t","title":"t","when":[],"contract":"argv",
                    "tools":[{"name":"extract","command":["python","bin/x.py"],
                              "args":{"path":{"type":"string","required":true}}}]}"#,
            )
            .unwrap(),
        };
        let tool = experience.tool("extract").unwrap().clone();
        let (program, argv, preview) =
            build_command(&experience, &tool, &json!({ "path": "a b.docx" })).unwrap();
        assert_eq!(program, "python");
        // The bundled file becomes an absolute path (separators are whatever
        // the manifest spelled), so compare the resolved file, not the string.
        assert!(Path::new(&argv[0]).is_file(), "{}", argv[0]);
        assert!(argv[0].starts_with(&dir.to_string_lossy().to_string()));
        assert!(argv[0].ends_with("x.py"));
        // One argv entry, so a space inside the value can never be re-split.
        assert_eq!(argv[1], "--path");
        assert_eq!(argv[2], "a b.docx");
        assert!(preview.contains("cwd:"));

        fs::remove_dir_all(&dir).ok();
    }
}
