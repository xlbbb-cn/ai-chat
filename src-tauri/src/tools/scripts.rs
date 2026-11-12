//! Workspace script library: `<workspace>/.data/scripts`.
//!
//! A script is a plain file (`*.ps1`, `*.sh`, `*.py`, `*.js`, …). The directory
//! is prepended to the `PATH` of `run_cmd` children **and** `run_cmd` resolves
//! a script by bare name itself, so a `.ps1` / `.py` file works on Windows
//! without relying on `PATHEXT`.
//!
//! Human/model-facing metadata lives in the script header as comment lines:
//!
//! ```text
//! # @description: Build the frontend and check the bundle
//! # @usage: build-frontend --release
//! ```
//!
//! `//`, `REM` and `::` are accepted as comment prefixes too, so the same
//! convention works for JavaScript, batch and PowerShell files.
//!
//! `.index.json` is application-managed (the model must not edit it): it keeps
//! use counts, the last exit code and the approved content hash for scripts
//! whose body scan found dangerous patterns.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::risk::{scan_script_body, BlacklistHit};
use crate::workspace;

/// Files above this size are not executed (and their header is not parsed).
pub(crate) const SCRIPT_MAX_BYTES: u64 = 512 * 1024;

/// Only the first lines of a script are scanned for `@description` / `@usage`.
const HEADER_LINES: usize = 40;
const HEADER_BYTES: u64 = 64 * 1024;

/// Application-managed statistics/approval file inside the library.
const INDEX_FILE: &str = ".index.json";
const INDEX_VERSION: u32 = 1;

/// Cap for the compact script block injected into the system prompt.
const PROMPT_MAX_SCRIPTS: usize = 12;
const PROMPT_MAX_CHARS: usize = 1_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Interpreter {
    PowerShell,
    Bash,
    Python,
    Node,
    Cmd,
}

impl Interpreter {
    /// Human-readable label used by `list` and the confirmation dialog.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Interpreter::PowerShell => "powershell",
            Interpreter::Bash => "bash",
            Interpreter::Python => "python",
            Interpreter::Node => "node",
            Interpreter::Cmd => "cmd",
        }
    }

    /// Program name used for the skill `allowed-commands` check.
    pub(crate) fn program_name(self) -> &'static str {
        match self {
            Interpreter::PowerShell => {
                if cfg!(windows) {
                    "powershell"
                } else {
                    "pwsh"
                }
            }
            Interpreter::Bash => "bash",
            Interpreter::Python => {
                if cfg!(windows) {
                    "python"
                } else {
                    "python3"
                }
            }
            Interpreter::Node => "node",
            Interpreter::Cmd => "cmd",
        }
    }
}

/// One script file discovered in the library.
#[derive(Debug, Clone)]
pub(crate) struct ScriptEntry {
    /// File name including the extension (`build-frontend.ps1`).
    pub file_name: String,
    /// Command name (`build-frontend`).
    pub name: String,
    pub interpreter: Interpreter,
    pub description: Option<String>,
    pub usage: Option<String>,
    pub uses: u64,
    pub last_used_at: Option<String>,
    pub last_exit: Option<i32>,
    /// True when the current content hash was approved by the user.
    pub approved: bool,
    pub size: u64,
    /// Non-fatal problem (unreadable header, oversized file, …).
    pub warning: Option<String>,
    pub path: PathBuf,
}

/// A resolved script plus the exact interpreter that will run it.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedScript {
    pub entry: ScriptEntry,
    pub path: PathBuf,
}

impl ResolvedScript {
    /// Program + arguments for `process::run_program_timed`.
    ///
    /// `rest` is the argument list after the script name. The path is passed as
    /// a separate argument (never concatenated into a command string), so
    /// spaces, quotes and backslashes round-trip unchanged.
    pub(crate) fn command_parts(&self, rest: &[String]) -> (String, Vec<String>) {
        let path = self.path.display().to_string();
        let mut args: Vec<String> = Vec::new();
        let program = match self.entry.interpreter {
            Interpreter::PowerShell => {
                args.extend(
                    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
                        .into_iter()
                        .map(String::from),
                );
                Interpreter::PowerShell.program_name().to_string()
            }
            Interpreter::Bash => Interpreter::Bash.program_name().to_string(),
            Interpreter::Python => Interpreter::Python.program_name().to_string(),
            Interpreter::Node => Interpreter::Node.program_name().to_string(),
            // Rust's `Command` runs `.cmd` / `.bat` through `cmd.exe`.
            Interpreter::Cmd => path.clone(),
        };
        if !matches!(self.entry.interpreter, Interpreter::Cmd) {
            args.push(path);
        }
        args.extend(rest.iter().cloned());
        (program, args)
    }
}

/// Result of scanning one script body.
#[derive(Debug, Clone)]
pub(crate) struct ScriptScan {
    pub file_name: String,
    pub hash: String,
    pub hits: Vec<BlacklistHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ScriptStat {
    #[serde(default)]
    uses: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_exit: Option<i32>,
    /// Hash of the body the user approved after a dangerous-pattern scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approved_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ScriptIndex {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    scripts: BTreeMap<String, ScriptStat>,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn index_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

fn lock_index() -> std::sync::MutexGuard<'static, ()> {
    // A poisoned lock must not disable the script library for the rest of the
    // process: the index is a cache of statistics, not a consistency boundary.
    index_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Directory of the active workspace's script library.
pub(crate) fn root(workspace_dir: &Path) -> PathBuf {
    workspace::scripts_root(workspace_dir)
}

/// Create the library directory if needed.
pub(crate) fn ensure_dir(workspace_dir: &Path) -> Result<PathBuf, String> {
    let root = root(workspace_dir);
    fs::create_dir_all(&root)
        .map_err(|e| format!("Failed to create '{}': {e}", root.display()))?;
    Ok(root)
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

fn interpreter_for_ext(ext: &str) -> Option<Interpreter> {
    match ext {
        "ps1" => Some(Interpreter::PowerShell),
        "sh" | "bash" => Some(Interpreter::Bash),
        "py" => Some(Interpreter::Python),
        "js" | "mjs" | "cjs" => Some(Interpreter::Node),
        "cmd" | "bat" => Some(Interpreter::Cmd),
        _ => None,
    }
}

/// Extensions tried when a script is called without its extension.
///
/// Shell scripts come first on both platforms ("shell preferred"): PowerShell
/// is the Windows shell, `sh`/`bash` are the Unix shells. Python and Node are
/// next; batch files are last because they are the least portable.
pub(crate) fn preferred_extensions() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["ps1", "sh", "bash", "py", "js", "mjs", "cjs", "cmd", "bat"]
    }
    #[cfg(not(windows))]
    {
        &["sh", "bash", "py", "js", "mjs", "cjs", "ps1", "cmd", "bat"]
    }
}

/// Strip a comment marker, returning the text after it.
fn strip_comment_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('#') {
        return Some(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("//") {
        return Some(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("::") {
        return Some(rest);
    }
    // `REM` / `rem` followed by whitespace or end of line.
    if let Some(prefix) = trimmed.get(..3) {
        if prefix.eq_ignore_ascii_case("rem") {
            let rest = &trimmed[3..];
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Some(rest);
            }
        }
    }
    None
}

fn strip_meta_key<'a>(comment: &'a str, key: &str) -> Option<&'a str> {
    let lower = comment.to_ascii_lowercase();
    if lower.starts_with(key) {
        let value = comment[key.len()..].trim();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    } else {
        None
    }
}

/// Read `@description` / `@usage` from the first comment lines of a script.
///
/// When no `@description` is present, the first ordinary comment line is used
/// as the description. This keeps hand-written scripts useful without forcing
/// the metadata header on every file.
pub(crate) fn parse_metadata(content: &str) -> (Option<String>, Option<String>) {
    let mut description: Option<String> = None;
    let mut usage: Option<String> = None;
    let mut first_comment: Option<String> = None;

    for line in content.lines().take(HEADER_LINES) {
        let Some(comment) = strip_comment_prefix(line) else {
            continue;
        };
        let comment = comment.trim();
        if comment.is_empty() {
            continue;
        }
        // A shebang is a comment after the `#` is stripped, but it is not a
        // description.
        if comment.starts_with('!') {
            continue;
        }
        if description.is_none() {
            if let Some(value) = strip_meta_key(comment, "@description:") {
                description = Some(value.to_string());
                continue;
            }
        }
        if usage.is_none() {
            if let Some(value) = strip_meta_key(comment, "@usage:") {
                usage = Some(value.to_string());
                continue;
            }
        }
        if first_comment.is_none() && !comment.starts_with('@') {
            first_comment = Some(comment.to_string());
        }
    }

    if description.is_none() {
        description = first_comment;
    }
    (description, usage)
}

fn read_header(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let mut reader = BufReader::new(file.take(HEADER_BYTES));
    let mut out = String::new();
    let mut line = String::new();
    for _ in 0..HEADER_LINES {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => out.push_str(&line),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(out)
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Content hash used to remember an approved script body.
pub(crate) fn content_hash(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    Ok(fnv1a_hex(&bytes))
}

fn index_path(root: &Path) -> PathBuf {
    root.join(INDEX_FILE)
}

fn load_index(root: &Path) -> ScriptIndex {
    fs::read_to_string(index_path(root))
        .ok()
        .and_then(|text| serde_json::from_str::<ScriptIndex>(&text).ok())
        .unwrap_or_default()
}

fn save_index(root: &Path, index: &ScriptIndex) -> Result<(), String> {
    let text = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
    let path = index_path(root);
    let temp = root.join(".index.json.tmp");
    fs::write(&temp, text).map_err(|e| format!("cannot write '{}': {e}", temp.display()))?;
    // `rename` replaces an existing file on Unix but not on Windows.
    #[cfg(windows)]
    {
        let _ = fs::remove_file(&path);
    }
    fs::rename(&temp, &path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("cannot replace '{}': {e}", path.display())
    })
}

/// List every runnable script, sorted by command name.
pub(crate) fn list(workspace_dir: &Path) -> Vec<ScriptEntry> {
    let Ok(root) = ensure_dir(workspace_dir) else {
        return Vec::new();
    };
    let index = load_index(&root);
    let Ok(read_dir) = fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for item in read_dir.flatten() {
        let path = item.path();
        let file_name = item.file_name().to_string_lossy().into_owned();
        // Hidden/app-managed entries (`.index.json`, temp files, backups).
        if file_name.starts_with('.') || file_name.contains(".bak.") {
            continue;
        }
        let Some(ext) = extension_of(&path) else {
            continue;
        };
        let Some(interpreter) = interpreter_for_ext(&ext) else {
            continue;
        };
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }

        let stat = index.scripts.get(&file_name).cloned().unwrap_or_default();
        let approved = stat
            .approved_hash
            .as_deref()
            .map(|hash| content_hash(&path).map(|current| current == hash).unwrap_or(false))
            .unwrap_or(false);

        let (description, usage, warning) = if metadata.len() > SCRIPT_MAX_BYTES {
            (
                None,
                None,
                Some(format!(
                    "file is {} bytes, above the {} byte inspection limit",
                    metadata.len(),
                    SCRIPT_MAX_BYTES
                )),
            )
        } else {
            match read_header(&path) {
                Ok(header) => {
                    let (description, usage) = parse_metadata(&header);
                    (description, usage, None)
                }
                Err(err) => (None, None, Some(format!("header unavailable: {err}"))),
            }
        };

        entries.push(ScriptEntry {
            name: path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(&file_name)
                .to_string(),
            file_name,
            interpreter,
            description,
            usage,
            uses: stat.uses,
            last_used_at: stat.last_used_at,
            last_exit: stat.last_exit,
            approved,
            size: metadata.len(),
            warning,
            path,
        });
    }

    entries.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.file_name.cmp(&b.file_name))
    });
    entries
}

/// Resolve a bare command name to a library script.
///
/// `None` means "not a library script" — the caller falls back to a
/// PATH-resolved program. An `Err` means the name matched a script but the
/// script cannot be run (too large, path escape, …).
pub(crate) fn resolve(
    workspace_dir: &Path,
    token: &str,
) -> Result<Option<ResolvedScript>, String> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    // A path is a program path, never a bare script name. `run_cmd` already
    // rejects it before this point; keep the guard here so the library cannot
    // be reached through a separator by another caller.
    if token.contains('/') || token.contains('\\') {
        return Ok(None);
    }

    let root = ensure_dir(workspace_dir)?;
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Exact file name (`build-frontend.ps1`).
    let exact = root.join(token);
    if exact.is_file() {
        candidates.push(exact);
    }

    // 2. Command name without extension (`build-frontend`): try the platform
    //    preference order. A token that already carries an unknown extension is
    //    not a library script.
    if Path::new(token).extension().is_none() {
        for ext in preferred_extensions() {
            let candidate = root.join(format!("{token}.{ext}"));
            if candidate.is_file() && !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }

    for candidate in candidates {
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(&canonical_root) {
            continue;
        }
        let Some(ext) = extension_of(&canonical) else {
            continue;
        };
        let Some(interpreter) = interpreter_for_ext(&ext) else {
            continue;
        };
        let metadata = fs::metadata(&canonical)
            .map_err(|e| format!("cannot read '{}': {e}", canonical.display()))?;
        if metadata.len() > SCRIPT_MAX_BYTES {
            return Err(format!(
                "Script '{}' is {} bytes, above the {} byte execution limit.",
                canonical.display(),
                metadata.len(),
                SCRIPT_MAX_BYTES
            ));
        }
        let file_name = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(token)
            .to_string();
        let header = read_header(&canonical).unwrap_or_default();
        let (description, usage) = parse_metadata(&header);
        let index = load_index(&root);
        let stat = index.scripts.get(&file_name).cloned().unwrap_or_default();
        let approved = stat
            .approved_hash
            .as_deref()
            .map(|hash| content_hash(&canonical).map(|current| current == hash).unwrap_or(false))
            .unwrap_or(false);

        return Ok(Some(ResolvedScript {
            entry: ScriptEntry {
                name: Path::new(&file_name)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or(&file_name)
                    .to_string(),
                file_name,
                interpreter,
                description,
                usage,
                uses: stat.uses,
                last_used_at: stat.last_used_at,
                last_exit: stat.last_exit,
                approved,
                size: metadata.len(),
                warning: None,
                path: canonical.clone(),
            },
            path: canonical,
        }));
    }

    Ok(None)
}

/// Script files referenced as command arguments (for example
/// `python .data/scripts/foo.py`). Their body is scanned too, so a script path
/// cannot bypass the library safety check by being passed to an interpreter.
///
/// Relative arguments are resolved against `base_dir` (the child's working
/// directory), while the containment check is always against the workspace's
/// script library.
pub(crate) fn referenced_scripts(
    workspace_dir: &Path,
    base_dir: &Path,
    args: &[String],
) -> Vec<PathBuf> {
    let root = workspace::scripts_root(workspace_dir);
    let Ok(canonical_root) = root.canonicalize() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for arg in args {
        let raw = Path::new(arg);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            base_dir.join(raw)
        };
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
            continue;
        }
        let Some(ext) = extension_of(&canonical) else {
            continue;
        };
        if interpreter_for_ext(&ext).is_none() {
            continue;
        }
        if !found.contains(&canonical) {
            found.push(canonical);
        }
    }
    found
}

/// Read and scan one script body.
pub(crate) fn scan(path: &Path) -> Result<ScriptScan, String> {
    let metadata = fs::metadata(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    if metadata.len() > SCRIPT_MAX_BYTES {
        return Err(format!(
            "Script '{}' is {} bytes, above the {} byte execution limit.",
            path.display(),
            metadata.len(),
            SCRIPT_MAX_BYTES
        ));
    }
    let bytes =
        fs::read(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("script")
        .to_string();
    Ok(ScriptScan {
        file_name,
        hash: fnv1a_hex(&bytes),
        hits: scan_script_body(&String::from_utf8_lossy(&bytes)),
    })
}

/// True when the user already approved this exact script content.
pub(crate) fn is_approved(workspace_dir: &Path, file_name: &str, hash: &str) -> bool {
    let root = root(workspace_dir);
    load_index(&root)
        .scripts
        .get(file_name)
        .and_then(|stat| stat.approved_hash.as_deref())
        == Some(hash)
}

/// Remember that the user approved the current content of a script.
pub(crate) fn approve(workspace_dir: &Path, file_name: &str, hash: &str) {
    let root = root(workspace_dir);
    let _guard = lock_index();
    let mut index = load_index(&root);
    let stat = index.scripts.entry(file_name.to_string()).or_default();
    stat.approved_hash = Some(hash.to_string());
    stat.approved_at = Some(now_rfc3339());
    index.version = INDEX_VERSION;
    let _ = save_index(&root, &index);
}

fn exit_status(output: &str) -> Option<i32> {
    if output.starts_with("Error:")
        || output.contains("timed out after")
        || output.contains("cancelled")
    {
        return None;
    }
    output
        .lines()
        .rev()
        .find_map(|line| {
            line.trim()
                .strip_prefix("Exit code: ")
                .and_then(|value| value.trim().parse::<i32>().ok())
        })
        .or(Some(0))
}

/// Record one execution in `.index.json` (uses, timestamp, best-effort exit code).
pub(crate) fn record_use(workspace_dir: &Path, file_name: &str, output: &str) {
    let root = root(workspace_dir);
    let _guard = lock_index();
    let mut index = load_index(&root);
    let stat = index.scripts.entry(file_name.to_string()).or_default();
    stat.uses = stat.uses.saturating_add(1);
    stat.last_used_at = Some(now_rfc3339());
    stat.last_exit = exit_status(output);
    index.version = INDEX_VERSION;
    let _ = save_index(&root, &index);
}

/// Render the `action: "list"` response shown to the model.
pub(crate) fn render_list(workspace_dir: &Path) -> String {
    let entries = list(workspace_dir);
    let label = "./.data/scripts";
    if entries.is_empty() {
        return format!(
            "Script library `{label}` is empty.\n\n\
             Add a script with `file_actions write` (path `./.data/scripts/<name>.ps1`, `.<name>.sh`, \
             `<name>.py`, `<name>.js`, …). Give it a header comment:\n\
             `# @description: what the script does and when to use it`\n\
             `# @usage: <name> [args]`\n\
             (use `//` in JavaScript and `REM` in batch files). Then run it with \
             `run_cmd {{\"action\":\"run\",\"command\":\"<name> [args]\"}}`."
        );
    }

    let mut out = format!(
        "Script library: `{label}` ({} script{}). The directory is prepended to \
         run_cmd's PATH; run a script by bare name (extension optional).\n",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" }
    );

    for entry in &entries {
        out.push_str(&format!(
            "\n- {}  ({} · {})",
            entry.name,
            entry.file_name,
            entry.interpreter.label()
        ));
        match &entry.description {
            Some(description) => out.push_str(&format!("\n  description: {description}")),
            None => out.push_str(
                "\n  description: (missing — add `@description: ...` to the header)",
            ),
        }
        if let Some(usage) = &entry.usage {
            out.push_str(&format!("\n  usage: {usage}"));
        } else {
            out.push_str(&format!("\n  usage: {} [args]", entry.name));
        }
        out.push_str(&format!("\n  uses: {}", entry.uses));
        if let Some(last) = &entry.last_used_at {
            out.push_str(&format!(" · last: {last}"));
        }
        if let Some(code) = entry.last_exit {
            out.push_str(&format!(" · last exit: {code}"));
        }
        if let Ok(scan) = scan(&entry.path) {
            if !scan.hits.is_empty() {
                out.push_str(&format!(
                    "\n  safety: {} dangerous pattern(s) · {}",
                    scan.hits.len(),
                    if entry.approved {
                        "content approved"
                    } else {
                        "first run needs confirmation"
                    }
                ));
            }
        }
        if let Some(warning) = &entry.warning {
            out.push_str(&format!("\n  warning: {warning}"));
        }
        out.push_str(&format!("\n  size: {} bytes", entry.size));
    }

    out.push_str(
        "\n\nRun one with `run_cmd` `{\"action\":\"run\",\"command\":\"<name> [args]\"}`.",
    );
    out
}

/// Compact block for the system prompt. Includes the self-evolution save
/// policy when the mode is on, plus the current library listing when non-empty.
pub(crate) fn prompt_block(workspace_dir: &Path, self_evolution: bool) -> String {
    let entries = list(workspace_dir);
    let mut out = String::new();

    if self_evolution {
        out.push_str(
            "SCRIPT LIBRARY: `./.data/scripts` is prepended to `run_cmd`'s PATH, so a script runs by \
             bare name (extension optional); `run_cmd {\"action\":\"list\"}` shows interpreter, \
             description, usage and use count.\n\
             When a multi-step command has succeeded at least twice, or the user asks to keep/reuse \
             it, save it there with `file_actions`: prefer `.ps1` on Windows and `.sh` elsewhere \
             (`.py`/`.js` only when the logic really needs it); add `@description:` and \
             `@usage: name [args]` header comments in that language's comment prefix; keep it \
             self-contained and argument-driven, with no hardcoded machine paths or secrets \
             (`set -euo pipefail` in bash); run it once by name to verify it. Scripts are shared by \
             the whole workspace and never hold credentials.",
        );
    }

    if !entries.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(
            "AVAILABLE SCRIPTS (`./.data/scripts`, bare name via `run_cmd`; \
             `{\"action\":\"list\"}` for full details):\n",
        );
        let mut shown = 0;
        for entry in &entries {
            let description = entry
                .description
                .as_deref()
                .unwrap_or("(no description)");
            let line = format!("- {} — {}\n", entry.name, description);
            if shown >= PROMPT_MAX_SCRIPTS
                || out.chars().count() + line.chars().count() > PROMPT_MAX_CHARS
            {
                break;
            }
            out.push_str(&line);
            shown += 1;
        }
        if shown < entries.len() {
            out.push_str(&format!(
                "- … and {} more (call `run_cmd` action `list`)\n",
                entries.len() - shown
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_workspace(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-scripts-{label}-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(workspace::scripts_root(&dir)).unwrap();
        dir
    }

    #[test]
    fn metadata_uses_hash_comments_and_falls_back_to_the_first_comment() {
        let (description, usage) = parse_metadata(
            "#!/usr/bin/env bash\n\
             # Build the frontend and verify the bundle\n\
             # @description: Build the frontend\n\
             # @usage: build-frontend --release\n",
        );
        assert_eq!(description.as_deref(), Some("Build the frontend"));
        assert_eq!(usage.as_deref(), Some("build-frontend --release"));

        let (description, usage) = parse_metadata("// do the thing\n// @usage: thing\n");
        assert_eq!(description.as_deref(), Some("do the thing"));
        assert_eq!(usage.as_deref(), Some("thing"));
    }

    #[test]
    fn metadata_accepts_rem_and_double_colon_comments() {
        let (description, usage) =
            parse_metadata("REM @description: Batch job\r\n:: @usage: job.cmd all\r\n");
        assert_eq!(description.as_deref(), Some("Batch job"));
        assert_eq!(usage.as_deref(), Some("job.cmd all"));
    }

    #[test]
    fn metadata_is_case_insensitive_for_keys() {
        let (description, usage) = parse_metadata(
            "# @DESCRIPTION: Upper case key\n# @Usage: upper --x\n",
        );
        assert_eq!(description.as_deref(), Some("Upper case key"));
        assert_eq!(usage.as_deref(), Some("upper --x"));
    }

    #[test]
    fn missing_metadata_yields_no_description_or_usage() {
        let (description, usage) = parse_metadata("print('hello')\n");
        assert!(description.is_none());
        assert!(usage.is_none());
    }

    #[test]
    fn list_skips_hidden_unsupported_and_backup_files() {
        let workspace = temp_workspace("list");
        let root = workspace::scripts_root(&workspace);
        fs::write(root.join("build.ps1"), "# @description: Build\n").unwrap();
        fs::write(root.join("deploy.sh"), "#!/bin/sh\necho hi\n").unwrap();
        fs::write(root.join("run.txt"), "not a script\n").unwrap();
        fs::write(root.join(".index.json"), "{}").unwrap();
        fs::write(root.join("old.sh.bak.1"), "backup\n").unwrap();

        let names: Vec<String> = list(&workspace)
            .into_iter()
            .map(|entry| entry.file_name)
            .collect();
        assert_eq!(names, vec!["build.ps1".to_string(), "deploy.sh".to_string()]);
    }

    #[test]
    fn resolve_accepts_exact_names_and_extensionless_stems() {
        let workspace = temp_workspace("resolve");
        let root = workspace::scripts_root(&workspace);
        fs::write(
            root.join("build.ps1"),
            "# @description: Build\n# @usage: build --release\n",
        )
        .unwrap();

        let by_stem = resolve(&workspace, "build").unwrap().expect("stem resolves");
        assert_eq!(by_stem.entry.file_name, "build.ps1");
        assert_eq!(by_stem.entry.description.as_deref(), Some("Build"));

        let by_name = resolve(&workspace, "build.ps1")
            .unwrap()
            .expect("exact name resolves");
        assert_eq!(by_name.entry.file_name, "build.ps1");

        assert!(resolve(&workspace, "missing").unwrap().is_none());
        assert!(resolve(&workspace, "run.txt").unwrap().is_none());
    }

    #[test]
    fn command_parts_puts_the_path_in_its_own_argument() {
        let workspace = temp_workspace("parts");
        let root = workspace::scripts_root(&workspace);
        fs::write(root.join("deploy.sh"), "#!/bin/sh\necho hi\n").unwrap();

        let script = resolve(&workspace, "deploy").unwrap().unwrap();
        let (program, args) = script.command_parts(&["staging".to_string()]);
        assert_eq!(program, "bash");
        assert_eq!(args.last().map(String::as_str), Some("staging"));
        assert!(args.iter().any(|arg| arg.ends_with("deploy.sh")));
    }

    #[test]
    fn approved_content_is_tracked_by_hash() {
        let workspace = temp_workspace("approve");
        let root = workspace::scripts_root(&workspace);
        let path = root.join("risky.sh");
        fs::write(&path, "#!/bin/sh\nrm -rf ~\n").unwrap();

        let first = scan(&path).unwrap();
        assert!(!first.hits.is_empty());
        assert!(!is_approved(&workspace, "risky.sh", &first.hash));

        approve(&workspace, "risky.sh", &first.hash);
        assert!(is_approved(&workspace, "risky.sh", &first.hash));

        fs::write(&path, "#!/bin/sh\nrm -rf ~ # changed\n").unwrap();
        let changed = scan(&path).unwrap();
        assert_ne!(changed.hash, first.hash);
        assert!(!is_approved(&workspace, "risky.sh", &changed.hash));
    }

    #[test]
    fn use_count_and_exit_status_are_recorded() {
        let workspace = temp_workspace("uses");
        record_use(&workspace, "build.sh", "ok");
        record_use(&workspace, "build.sh", "boom\nExit code: 3");
        let entry = list(&workspace)
            .into_iter()
            .find(|entry| entry.file_name == "build.sh");
        // `list` only returns real files; create one so the stats can be read.
        assert!(entry.is_none());

        let root = workspace::scripts_root(&workspace);
        fs::write(root.join("build.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        let entry = list(&workspace)
            .into_iter()
            .find(|entry| entry.file_name == "build.sh")
            .expect("script listed");
        assert_eq!(entry.uses, 2);
        assert_eq!(entry.last_exit, Some(3));
    }

    #[test]
    fn prompt_block_includes_policy_and_available_scripts() {
        let workspace = temp_workspace("prompt");
        let root = workspace::scripts_root(&workspace);
        fs::write(
            root.join("build.ps1"),
            "# @description: Build the app\n# @usage: build --release\n",
        )
        .unwrap();

        let block = prompt_block(&workspace, true);
        assert!(block.contains("SCRIPT LIBRARY"));
        assert!(block.contains("at least twice"));
        assert!(block.contains("AVAILABLE SCRIPTS"));
        assert!(block.contains("build — Build the app"));

        let empty = prompt_block(&temp_workspace("prompt-empty"), true);
        assert!(empty.contains("SCRIPT LIBRARY"));
        assert!(!empty.contains("AVAILABLE SCRIPTS"));
    }
}
