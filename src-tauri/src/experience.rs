//! Experience packages: the global, portable store at `<app_data>/experiences/<slug>/`.
//!
//! A package is **data**, not an execution engine: `experience.json` (metadata), `guide.md`
//! (the method) and the optional payload folders `skill/`, `scripts/`, `mcp/`. The
//! `experience` tool only searches, loads and copies parts into a workspace; the ordinary
//! tools (`use_skill`, `run_shell`, `mcp_self`) then run them. Nothing here spawns a process.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "experience.json";
pub const EXPERIENCES_DIR: &str = "experiences";

/// Files a payload folder may carry; anything hidden or backed up is ignored everywhere.
const MAX_WALK_DEPTH: usize = 4;

fn default_schema() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuideSpec {
    pub file: String,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

/// What the machine that runs the package needs. Written at pack time by scanning the
/// payload; installing those dependencies is the caller's job, never this app's.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Requires {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub packages: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub checked: String,
}

impl Requires {
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.packages.values().all(|list| list.is_empty())
    }

    /// One line for the prompt/panel: `tools: node | python: docx | node: pdf-parse`.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.tools.is_empty() {
            parts.push(format!("tools: {}", self.tools.join(", ")));
        }
        for (ecosystem, modules) in &self.packages {
            if !modules.is_empty() {
                parts.push(format!("{ecosystem}: {}", modules.join(", ")));
            }
        }
        let mut text = parts.join(" | ");
        if !self.notes.trim().is_empty() {
            text.push_str(&format!(" ({})", self.notes.trim()));
        }
        text
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub when: Vec<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub source_workspace: String,
    #[serde(default)]
    pub source_hash: String,
    #[serde(default)]
    pub distilled_from: Vec<String>,
    #[serde(default)]
    pub guide: Option<GuideSpec>,
    #[serde(default)]
    pub requires: Requires,
}

#[derive(Debug, Clone)]
pub struct Experience {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Experience {
    pub fn slug(&self) -> &str {
        self.manifest.slug.as_str()
    }

    pub fn guide_path(&self) -> Option<PathBuf> {
        let name = self
            .manifest
            .guide
            .as_ref()
            .map(|guide| guide.file.clone())
            .unwrap_or_else(|| "guide.md".to_string());
        let path = self.dir.join(name);
        if path.is_file() {
            Some(path)
        } else {
            None
        }
    }

    /// The method, capped so a huge guide cannot flood a context.
    pub fn guide_text(&self) -> String {
        let Some(path) = self.guide_path() else {
            return String::new();
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return String::new();
        };
        let cap = self
            .manifest
            .guide
            .as_ref()
            .and_then(|guide| guide.max_bytes)
            .unwrap_or(8 * 1024);
        if text.len() <= cap {
            return text;
        }
        text.chars().take(cap).collect()
    }

    /// Content fingerprint used by the panel to spot a package that changed on disk.
    pub fn fingerprint(&self) -> String {
        let mut hasher = DefaultHasher::new();
        let mut entries: Vec<(String, u64)> = Vec::new();
        for path in walk_files(&self.dir) {
            if path.file_name().map(|name| name.to_string_lossy().to_string()).as_deref()
                == Some(MANIFEST_FILE)
            {
                continue;
            }
            let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            let relative = path
                .strip_prefix(&self.dir)
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            entries.push((relative, size));
        }
        entries.sort();
        for (relative, size) in &entries {
            relative.hash(&mut hasher);
            size.hash(&mut hasher);
        }
        if let Ok(text) = fs::read_to_string(self.dir.join(MANIFEST_FILE)) {
            let mut manifest: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if let Some(object) = manifest.as_object_mut() {
                object.remove("source_hash");
            }
            manifest.to_string().hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }
}

pub fn root_for(app_data: &Path) -> PathBuf {
    app_data.join(EXPERIENCES_DIR)
}

pub fn validate_slug(slug: &str) -> Result<(), String> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err("a package slug cannot be empty".to_string());
    }
    if slug.len() > 64 {
        return Err("a package slug is limited to 64 characters".to_string());
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "'{slug}' is not a usable slug: letters, digits, '-' and '_' only"
        ));
    }
    Ok(())
}

/// Every package in the store (the folder that holds one directory per slug).
pub fn scan(root: &Path) -> Vec<Experience> {
    let mut items: Vec<Experience> = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return items;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.contains(".bak.") {
            continue;
        }
        if let Some(item) = load_dir(&path) {
            items.push(item);
        }
    }
    items.sort_by(|left, right| left.slug().cmp(right.slug()));
    items
}

pub fn load(root: &Path, slug: &str) -> Option<Experience> {
    let slug = slug.trim();
    if validate_slug(slug).is_err() {
        return None;
    }
    load_dir(&root.join(slug))
}

fn load_dir(dir: &Path) -> Option<Experience> {
    let text = fs::read_to_string(dir.join(MANIFEST_FILE)).ok()?;
    let manifest: Manifest = serde_json::from_str(&text).ok()?;
    if manifest.schema != 1 {
        return None;
    }
    Some(Experience {
        dir: dir.to_path_buf(),
        manifest,
    })
}

// ─── Matchers ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Matcher {
    Ext(String),
    Mime(String),
    Magic { prefix: Vec<u8>, needle: Option<String> },
    Name(String),
    Tag(String),
}

pub fn parse_matcher(raw: &str) -> Option<Matcher> {
    let raw = raw.trim();
    let (kind, rest) = raw.split_once(':')?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    match kind.to_ascii_lowercase().as_str() {
        "ext" => Some(Matcher::Ext(rest.trim_start_matches('.').to_ascii_lowercase())),
        "mime" => Some(Matcher::Mime(rest.to_ascii_lowercase())),
        "magic" => {
            let (hex, needle) = match rest.split_once(':') {
                Some((hex, needle)) => (hex, Some(needle.to_string())),
                None => (rest, None),
            };
            let prefix = decode_hex(hex)?;
            if prefix.is_empty() {
                return None;
            }
            Some(Matcher::Magic { prefix, needle })
        }
        "name" => Some(Matcher::Name(rest.to_ascii_lowercase())),
        "tag" => Some(Matcher::Tag(rest.to_ascii_lowercase())),
        _ => None,
    }
}

pub fn matcher_specificity(matcher: &Matcher) -> u8 {
    match matcher {
        Matcher::Magic { .. } => 3,
        Matcher::Mime(_) => 2,
        Matcher::Ext(_) => 1,
        Matcher::Name(_) | Matcher::Tag(_) => 0,
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() || text.len() % 2 != 0 {
        return None;
    }
    let mut out: Vec<u8> = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let high = (bytes[index] as char).to_digit(16)?;
        let low = (bytes[index + 1] as char).to_digit(16)?;
        out.push(((high << 4) | low) as u8);
        index += 2;
    }
    Some(out)
}

struct PathFacts {
    extension: String,
    head: Vec<u8>,
}

impl PathFacts {
    fn read(path: &Path) -> Self {
        use std::io::Read;
        let extension = path
            .extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mut head = vec![0u8; 64];
        let mut filled = 0usize;
        if let Ok(mut file) = fs::File::open(path) {
            filled = file.read(&mut head).unwrap_or(0);
        }
        head.truncate(filled);
        Self { extension, head }
    }
}

fn score(manifest: &Manifest, path: &Path, facts: &PathFacts) -> Option<u8> {
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut best: Option<u8> = None;
    for raw in &manifest.when {
        let Some(matcher) = parse_matcher(raw) else {
            continue;
        };
        let hit = match &matcher {
            Matcher::Ext(extension) => facts.extension == *extension,
            Matcher::Mime(mime) => facts.extension == *mime,
            Matcher::Magic { prefix, needle } => {
                let prefix_ok = facts.head.len() >= prefix.len()
                    && &facts.head[..prefix.len()] == prefix.as_slice();
                let needle_ok = match needle {
                    Some(text) => {
                        let haystack = String::from_utf8_lossy(&facts.head);
                        haystack.contains(text.as_str())
                    }
                    None => true,
                };
                prefix_ok && needle_ok
            }
            Matcher::Name(wanted) => match wanted.strip_prefix("*.") {
                Some(suffix) => file_name.ends_with(&format!(".{suffix}")),
                None => file_name == *wanted,
            },
            Matcher::Tag(_) => false,
        };
        if hit {
            let specificity = matcher_specificity(&matcher);
            best = Some(best.map_or(specificity, |current| current.max(specificity)));
        }
    }
    best
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub specificity: u8,
    pub experience: Experience,
}

/// Packages whose matchers hit this path, most specific first. Ambiguity is left to the
/// caller: two packages may legitimately claim the same input.
pub fn resolve_for_path(root: &Path, path: &Path) -> Vec<Resolved> {
    let facts = PathFacts::read(path);
    let mut hits: Vec<Resolved> = Vec::new();
    for experience in scan(root) {
        if let Some(specificity) = score(&experience.manifest, path, &facts) {
            hits.push(Resolved {
                specificity,
                experience,
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .specificity
            .cmp(&left.specificity)
            .then_with(|| left.experience.slug().cmp(right.experience.slug()))
    });
    hits
}

#[derive(Debug, Serialize)]
pub struct SearchRow {
    pub slug: String,
    pub title: String,
    pub state: String,
    pub score: u8,
    pub when: Vec<String>,
    pub guide: bool,
    pub requires: String,
}

/// Deterministic search: matcher hits rank above name matches, which rank above guide text.
pub fn search(root: &Path, query: &str, ext: &str, tag: &str) -> Vec<SearchRow> {
    let query = query.trim().to_ascii_lowercase();
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let tag = tag.trim().to_ascii_lowercase();
    let mut scored: Vec<(u8, String, SearchRow)> = Vec::new();
    for experience in scan(root) {
        let manifest = &experience.manifest;
        let mut score_value: u8 = 0;
        for raw in &manifest.when {
            let Some(matcher) = parse_matcher(raw) else {
                continue;
            };
            let wanted = match &matcher {
                Matcher::Ext(value) => {
                    if !ext.is_empty() && *value == ext {
                        score_value = score_value.max(4);
                    }
                    None
                }
                Matcher::Tag(value) => {
                    if !tag.is_empty() && *value == tag {
                        score_value = score_value.max(3);
                    }
                    None
                }
                _ => Some(matcher_specificity(&matcher)),
            };
            let _ = wanted;
        }
        if !query.is_empty() {
            let title = manifest.title.to_ascii_lowercase();
            let slug = manifest.slug.to_ascii_lowercase();
            if title.contains(&query) || slug.contains(&query) {
                score_value = score_value.max(2);
            } else if experience.guide_text().to_ascii_lowercase().contains(&query) {
                score_value = score_value.max(1);
            }
        }
        if score_value > 0 || (query.is_empty() && ext.is_empty() && tag.is_empty()) {
            let row = SearchRow {
                slug: manifest.slug.clone(),
                title: manifest.title.clone(),
                state: manifest.state.clone(),
                score: score_value,
                when: manifest.when.clone(),
                guide: experience.guide_path().is_some(),
                requires: manifest.requires.summary(),
            };
            scored.push((score_value, manifest.slug.clone(), row));
        }
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored.into_iter().map(|(_, _, row)| row).collect()
}

// ─── Payload inspection ──────────────────────────────────────────────────────

/// Files that make up the package payload (hidden entries and backups excluded).
pub fn payload_files(dir: &Path) -> Vec<PathBuf> {
    walk_files(dir)
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > MAX_WALK_DEPTH {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.contains(".bak.") {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth + 1, out);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    walk(dir, 0, &mut out);
    out.sort();
    out
}

const PYTHON_STDLIB: &[&str] = &[
    "os", "sys", "json", "re", "pathlib", "subprocess", "argparse", "csv", "io", "time",
    "math", "hashlib", "base64", "zipfile", "xml", "urllib", "shutil", "tempfile", "typing",
    "dataclasses", "collections", "itertools", "functools", "textwrap", "unicodedata", "glob",
    "struct", "html", "codecs", "random", "string", "traceback", "logging", "datetime",
    "decimal", "enum", "abc", "copy", "queue", "threading", "unittest", "uuid", "platform",
    "statistics", "difflib", "secrets", "importlib",
];

const NODE_BUILTINS: &[&str] = &[
    "fs", "path", "url", "os", "child_process", "readline", "crypto", "http", "https", "stream",
    "util", "events", "zlib", "buffer", "assert", "querystring", "net", "tls", "dns",
    "worker_threads", "process", "timers", "string_decoder", "tty", "v8", "vm", "perf_hooks",
    "inspector",
];

/// Record what the payload needs: the interpreters its commands name, plus the modules its
/// sources import. Deliberately static — a dynamic `import_module(name)` is invisible here, so
/// such a package should say so in `guide.md` or in `requires.notes`.
pub fn scan_requirements(commands: &[Vec<String>], files: &[PathBuf]) -> Requires {
    let mut requires = Requires::default();
    for command in commands {
        let Some(program) = command.first() else {
            continue;
        };
        let name = interpreter_name(program);
        if name.is_empty() {
            continue;
        }
        if !requires
            .tools
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&name))
        {
            requires.tools.push(name);
        }
    }

    let mut python: Vec<String> = Vec::new();
    let mut node: Vec<String> = Vec::new();
    for file in files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        if text.len() > 512 * 1024 {
            continue;
        }
        match file
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "py" => {
                for module in python_modules(&text) {
                    if !python.iter().any(|existing| existing == &module) {
                        python.push(module);
                    }
                }
            }
            "js" | "mjs" | "cjs" | "ts" => {
                for module in node_modules(&text) {
                    if !node.iter().any(|existing| existing == &module) {
                        node.push(module);
                    }
                }
            }
            _ => {}
        }
    }
    if !python.is_empty() {
        requires.packages.insert("python".to_string(), python);
    }
    if !node.is_empty() {
        requires.packages.insert("node".to_string(), node);
    }
    requires.checked = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown host".to_string());
    requires
}

fn interpreter_name(program: &str) -> String {
    let file = Path::new(program)
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut name = file.to_ascii_lowercase();
    if name == "python3" || name == "pythonw" {
        name = "python".to_string();
    }
    name
}

fn python_modules(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let module = if let Some(rest) = line.strip_prefix("import ") {
            rest.split(&[',', ';'][..])
                .next()
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("")
                .split('.')
                .next()
                .unwrap_or("")
                .to_string()
        } else if let Some(rest) = line.strip_prefix("from ") {
            rest.split_whitespace()
                .next()
                .unwrap_or("")
                .split('.')
                .next()
                .unwrap_or("")
                .to_string()
        } else {
            continue;
        };
        let module = module.trim().to_string();
        if module.is_empty() || module.starts_with('.') {
            continue;
        }
        if PYTHON_STDLIB.contains(&module.as_str()) {
            continue;
        }
        if !out.iter().any(|existing| existing == &module) {
            out.push(module);
        }
    }
    out
}

fn node_modules(text: &str) -> Vec<String> {
    let quotes = &['\'', '"'][..];
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        while let Some(index) = rest.find("require(").or_else(|| rest.find("from ")) {
            rest = &rest[index..];
            let Some(start) = rest.find(quotes) else {
                break;
            };
            let after = &rest[start + 1..];
            let Some(end) = after.find(quotes) else {
                break;
            };
            let name = &after[..end];
            rest = &after[end + 1..];
            let name = name.trim();
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('/')
                || name.starts_with("node:")
            {
                continue;
            }
            let base = name.split('/').next().unwrap_or(name);
            if NODE_BUILTINS.contains(&base) || NODE_BUILTINS.contains(&name) {
                continue;
            }
            if !out.iter().any(|existing| existing == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rabbit-exp-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_package(root: &Path, slug: &str, mut manifest: Value, files: &[(&str, &str)]) {
        let dir = root.join(slug);
        fs::create_dir_all(&dir).unwrap();
        if let Some(object) = manifest.as_object_mut() {
            object.insert("schema".to_string(), Value::from(1));
            object.insert("slug".to_string(), Value::from(slug));
        }
        fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        for (name, body) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, body).unwrap();
        }
    }

    #[test]
    fn parses_matchers_and_ranks_specificity() {
        assert_eq!(parse_matcher("ext:.DOCX"), Some(Matcher::Ext("docx".into())));
        assert_eq!(
            parse_matcher("magic:504b0304:word/"),
            Some(Matcher::Magic {
                prefix: vec![0x50, 0x4b, 0x03, 0x04],
                needle: Some("word/".into()),
            })
        );
        assert!(parse_matcher("nonsense").is_none());
        assert!(parse_matcher("ext:").is_none());
        assert!(
            matcher_specificity(&parse_matcher("magic:504b0304").unwrap())
                > matcher_specificity(&parse_matcher("ext:docx").unwrap())
        );
    }

    #[test]
    fn rejects_path_traversal_in_slug() {
        assert!(validate_slug("docx-text").is_ok());
        assert!(validate_slug("../etc").is_err());
        assert!(validate_slug("a/b").is_err());
        assert!(validate_slug("").is_err());
    }

    #[test]
    fn scan_skips_backups_archives_and_broken_manifests() {
        let root = temp_root("scan");
        write_package(&root, "good", serde_json::json!({ "slug": "good" }), &[]);
        fs::create_dir_all(root.join(".archive")).unwrap();
        fs::create_dir_all(root.join("good.bak.1")).unwrap();
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(root.join("broken").join(MANIFEST_FILE), "{not json").unwrap();
        let found = scan(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug(), "good");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn magic_beats_extension_and_ambiguity_is_reported() {
        let root = temp_root("resolve");
        write_package(
            &root,
            "docx-text",
            serde_json::json!({ "slug": "docx-text", "when": ["ext:docx"] }),
            &[],
        );
        write_package(
            &root,
            "zip-magic",
            serde_json::json!({ "slug": "zip-magic", "when": ["magic:504b0304"] }),
            &[],
        );
        let sample = root.join("sample.docx");
        fs::write(&sample, [0x50, 0x4b, 0x03, 0x04, 0x00]).unwrap();
        let hits = resolve_for_path(&root, &sample);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].experience.slug(), "zip-magic");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_ranks_extension_hits_above_title_hits() {
        let root = temp_root("search");
        write_package(
            &root,
            "docx-text",
            serde_json::json!({ "slug": "docx-text", "title": "DOCX text", "when": ["ext:docx"] }),
            &[],
        );
        write_package(
            &root,
            "docx-notes",
            serde_json::json!({ "slug": "docx-notes", "title": "docx notes" }),
            &[],
        );
        let rows = search(&root, "docx", "docx", "");
        assert_eq!(rows[0].slug, "docx-text");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn requirements_capture_imports_and_skip_stdlib_and_builtins() {
        let root = temp_root("requires");
        write_package(
            &root,
            "converter",
            serde_json::json!({ "slug": "converter" }),
            &[
                ("scripts/run.py", "import os\nimport docx\nfrom mylib.sub import x\n"),
                (
                    "scripts/run.js",
                    "const fs = require('fs');\nconst pdf = require('pdf-parse');\n",
                ),
            ],
        );
        let package = load(&root, "converter").unwrap();
        let requires = scan_requirements(
            &[
                vec!["python".to_string(), "scripts/run.py".to_string()],
                vec!["node".to_string(), "scripts/run.js".to_string()],
            ],
            &payload_files(&package.dir),
        );
        assert_eq!(requires.tools, vec!["python".to_string(), "node".to_string()]);
        assert_eq!(requires.packages.get("python").unwrap(), &vec!["docx".to_string(), "mylib".to_string()]);
        assert_eq!(requires.packages.get("node").unwrap(), &vec!["pdf-parse".to_string()]);
        assert!(requires.summary().contains("python: docx"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn guide_text_is_capped_and_fingerprint_changes_with_payload() {
        let root = temp_root("guide");
        write_package(
            &root,
            "capped",
            serde_json::json!({ "slug": "capped", "guide": { "file": "guide.md", "max_bytes": 8 } }),
            &[("guide.md", "0123456789abcdef")],
        );
        let package = load(&root, "capped").unwrap();
        assert_eq!(package.guide_text().chars().count(), 8);
        let before = package.fingerprint();
        fs::write(package.dir.join("extra.txt"), "more").unwrap();
        let reloaded = load(&root, "capped").unwrap();
        assert_ne!(before, reloaded.fingerprint());
        let _ = fs::remove_dir_all(&root);
    }
}
