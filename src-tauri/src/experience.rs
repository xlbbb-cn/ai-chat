//! Global experience packages (`<app_data>/experiences/<slug>/`).
//!
//! An *experience* is a self-contained capability that outlives a workspace: a
//! `experience.json` manifest (identity, deterministic matchers, process
//! contract, tool list), a `guide.md` the model reads on `load`, and whatever
//! executable files the tools need (`bin/*`).
//!
//! Scope rules — these are the whole point of the module:
//!
//! * The store lives at the **app-data top level** and nowhere else: there is
//!   no workspace copy, no shadowing, no merge with `skills` / `mcp.conf`.
//! * It is **not** a skill. Nothing here is reachable through `use_skill`, the
//!   workspace skills root, the `run_cmd` script library or the MCP roster. The
//!   only entry point is the `experience` tool.
//! * Matching is **program-side** (extension / MIME / magic bytes), not a model
//!   judgement: [`resolve_for_path`] is a pure lookup with a fixed precedence.
//!
//! The module itself is Tauri-free so it can be unit-tested with plain paths.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Manifest file inside every package folder.
pub const MANIFEST_FILE: &str = "experience.json";

/// Default cap on the guide text injected by `load` / auto-resolution.
pub const DEFAULT_GUIDE_CAP: usize = 8192;

fn default_schema() -> u32 {
    1
}

fn default_contract() -> String {
    "argv".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuideSpec {
    pub file: String,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Program + arguments. Relative entries resolve against the package dir.
    /// Never a shell string: the tool layer spawns the array verbatim.
    #[serde(default)]
    pub command: Vec<String>,
    /// `{ "<arg>": { "type": "string", "required": true } }`
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerifySpec {
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub expect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub slug: String,
    #[serde(default)]
    pub title: String,
    /// Deterministic matchers (`ext:docx`, `mime:…`, `magic:504b0304:word/`).
    #[serde(default)]
    pub when: Vec<String>,
    /// `draft` / `verified` / `global` / `archived`.
    #[serde(default)]
    pub state: String,
    #[serde(default, rename = "source_workspace")]
    pub source_workspace: String,
    #[serde(default, rename = "source_hash")]
    pub source_hash: String,
    #[serde(default)]
    pub distilled_from: Vec<String>,
    #[serde(default)]
    pub guide: Option<GuideSpec>,
    #[serde(default = "default_contract")]
    pub contract: String,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub verify: Option<VerifySpec>,
}

#[derive(Debug, Clone)]
pub struct Experience {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Experience {
    pub fn slug(&self) -> &str {
        &self.manifest.slug
    }

    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.manifest.tools.iter().find(|tool| tool.name == name)
    }

    pub fn guide_path(&self) -> Option<PathBuf> {
        // `guide.md` is the conventional name: a hand-written package may omit
        // the `guide` block entirely and still be readable.
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

    /// Guide text, capped (char-safe) at the declared `max_bytes`.
    pub fn guide_text(&self) -> String {
        let Some(path) = self.guide_path() else {
            return String::new();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return String::new();
        };
        let cap = self
            .manifest
            .guide
            .as_ref()
            .and_then(|guide| guide.max_bytes)
            .unwrap_or(DEFAULT_GUIDE_CAP);
        if raw.chars().count() <= cap {
            return raw;
        }
        let mut capped: String = raw.chars().take(cap).collect();
        capped.push_str("\n…[guide truncated]");
        capped
    }

    /// Content fingerprint used to bind an approval to this exact package
    /// build (a modified package loses its trust entry). Not a security hash.
    pub fn fingerprint(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.manifest.slug.hash(&mut hasher);
        // The manifest is hashed with `source_hash` cleared: `pack` writes the
        // fingerprint back into that field, and a self-referential hash would
        // change on every write.
        if let Ok(raw) = fs::read_to_string(self.dir.join(MANIFEST_FILE)) {
            if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(map) = value.as_object_mut() {
                    map.remove("source_hash");
                }
                serde_json::to_string(&value)
                    .unwrap_or(raw)
                    .hash(&mut hasher);
            } else {
                raw.hash(&mut hasher);
            }
        }
        let mut entries: Vec<(String, u64)> = walk_files(&self.dir)
            .into_iter()
            .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some(MANIFEST_FILE))
            .filter_map(|path| {
                let name = path
                    .strip_prefix(&self.dir)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                let len = fs::metadata(&path).ok()?.len();
                Some((name, len))
            })
            .collect();
        entries.sort();
        entries.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else if path.is_file() {
            out.push(path);
        }
    }
    out
}

// ─── Matchers ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matcher {
    /// `ext:docx`
    Ext(String),
    /// `mime:application/pdf`
    Mime(String),
    /// `magic:504b0304` or `magic:504b0304:word/`
    Magic { prefix: Vec<u8>, needle: Option<String> },
    /// `name:*.pdf` / `name:invoice-*`
    Name(String),
    /// `tag:pdf` — free-form label matched by `search`, never by a path.
    Tag(String),
}

/// Fixed precedence: magic > mime > ext > name > tag. Ties are broken by slug
/// order and, if still tied, reported as ambiguous rather than guessed.
pub fn matcher_specificity(matcher: &Matcher) -> u8 {
    match matcher {
        Matcher::Magic { .. } => 3,
        Matcher::Mime(_) => 2,
        Matcher::Ext(_) => 1,
        Matcher::Name(_) => 0,
        Matcher::Tag(_) => 0,
    }
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(raw.len() / 2);
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let pair = std::str::from_utf8(&bytes[index..index + 2]).ok()?;
        out.push(u8::from_str_radix(pair, 16).ok()?);
        index += 2;
    }
    Some(out)
}

pub fn parse_matcher(raw: &str) -> Option<Matcher> {
    let raw = raw.trim();
    let (kind, rest) = raw.split_once(':')?;
    let kind = kind.trim().to_ascii_lowercase();
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    match kind.as_str() {
        "ext" => Some(Matcher::Ext(rest.trim_start_matches('.').to_ascii_lowercase())),
        "mime" => Some(Matcher::Mime(rest.to_ascii_lowercase())),
        "magic" => {
            let (prefix_raw, needle) = match rest.split_once(':') {
                Some((prefix, needle)) => (prefix, Some(needle.to_string())),
                None => (rest, None),
            };
            Some(Matcher::Magic {
                prefix: decode_hex(prefix_raw)?,
                needle: needle.filter(|value| !value.is_empty()),
            })
        }
        "name" => Some(Matcher::Name(rest.to_ascii_lowercase())),
        "tag" => Some(Matcher::Tag(rest.to_ascii_lowercase())),
        _ => None,
    }
}

fn read_head(path: &Path, limit: usize) -> Vec<u8> {
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut buffer = Vec::with_capacity(limit);
    let mut chunk = [0u8; 1024];
    while buffer.len() < limit {
        let take = (limit - buffer.len()).min(chunk.len());
        match file.read(&mut chunk[..take]) {
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(_) => break,
        }
    }
    buffer
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn name_matches(pattern: &str, name: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    name == pattern
}

/// Path facts the matcher needs, gathered once per resolution.
pub struct PathFacts {
    pub extension: String,
    pub file_name: String,
    pub head: Vec<u8>,
}

impl PathFacts {
    pub fn read(path: &Path) -> Self {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        Self {
            extension,
            file_name,
            head: read_head(path, 4096),
        }
    }
}

/// Best specificity this package claims for the file, `None` when it does not
/// match at all. Pure function of the manifest + the file's bytes.
pub fn score_path(manifest: &Manifest, facts: &PathFacts) -> Option<u8> {
    let mut best: Option<u8> = None;
    for raw in &manifest.when {
        let Some(matcher) = parse_matcher(raw) else {
            continue;
        };
        let hit = match &matcher {
            Matcher::Ext(ext) => !ext.is_empty() && facts.extension == *ext,
            Matcher::Mime(mime) => !mime.is_empty() && facts.extension == *mime,
            Matcher::Magic { prefix, needle } => {
                if facts.head.len() < prefix.len() || !facts.head.starts_with(prefix) {
                    false
                } else {
                    match needle {
                        Some(needle) => contains_bytes(&facts.head, needle.as_bytes()),
                        None => true,
                    }
                }
            }
            Matcher::Name(pattern) => name_matches(pattern, &facts.file_name),
            Matcher::Tag(_) => false,
        };
        if hit {
            let score = matcher_specificity(&matcher);
            best = Some(best.map_or(score, |current| current.max(score)));
        }
    }
    best
}

// ─── Registry ────────────────────────────────────────────────────────────────

/// `<app_data>/experiences`
pub fn root_for(app_data: &Path) -> PathBuf {
    app_data.join("experiences")
}

fn is_hidden_entry(name: &str) -> bool {
    name.starts_with('.') || name.contains(".bak.")
}

pub fn validate_slug(slug: &str) -> Result<(), String> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err("slug is empty".to_string());
    }
    if slug.len() > 64 {
        return Err("slug is longer than 64 characters".to_string());
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "slug '{slug}' may only contain letters, digits, '-' and '_'"
        ));
    }
    Ok(())
}

pub fn parse_manifest(raw: &str) -> Result<Manifest, String> {
    let manifest: Manifest =
        serde_json::from_str(raw).map_err(|err| format!("invalid experience.json: {err}"))?;
    if manifest.schema != 1 {
        return Err(format!(
            "unsupported experience schema {} (only 1 is understood)",
            manifest.schema
        ));
    }
    validate_slug(&manifest.slug)?;
    Ok(manifest)
}

/// Load one package folder (no scanning of siblings).
pub fn load_dir(dir: &Path) -> Result<Experience, String> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let raw = fs::read_to_string(&manifest_path)
        .map_err(|err| format!("cannot read {}: {err}", manifest_path.display()))?;
    let manifest = parse_manifest(&raw)?;
    Ok(Experience {
        dir: dir.to_path_buf(),
        manifest,
    })
}

/// Every package under the store. Invalid manifests are skipped (the panel
/// reports them separately) so one broken package cannot blind the registry.
pub fn scan(app_data: &Path) -> Vec<Experience> {
    let root = root_for(app_data);
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_hidden_entry(&name) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(experience) = load_dir(&path) {
            if experience.manifest.state.eq_ignore_ascii_case("archived") {
                continue;
            }
            out.push(experience);
        }
    }
    out.sort_by(|left, right| left.slug().cmp(right.slug()));
    out
}

pub fn load(app_data: &Path, slug: &str) -> Option<Experience> {
    let slug = slug.trim();
    validate_slug(slug).ok()?;
    load_dir(&root_for(app_data).join(slug)).ok()
}

/// One resolution result: the package plus the specificity it claimed.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub specificity: u8,
    pub experience: Experience,
}

/// Deterministic path → packages resolution.
///
/// Order: specificity descending, then slug ascending. Two entries with the
/// same specificity mean the registry itself is ambiguous — callers must show
/// both instead of picking one.
pub fn resolve_for_path(app_data: &Path, path: &Path) -> Vec<Resolved> {
    let facts = PathFacts::read(path);
    let mut hits: Vec<Resolved> = scan(app_data)
        .into_iter()
        .filter_map(|experience| {
            score_path(&experience.manifest, &facts).map(|specificity| Resolved {
                specificity,
                experience,
            })
        })
        .collect();
    hits.sort_by(|left, right| {
        right
            .specificity
            .cmp(&left.specificity)
            .then_with(|| left.experience.slug().cmp(right.experience.slug()))
    });
    hits
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchRow {
    pub slug: String,
    pub title: String,
    pub state: String,
    pub contract: String,
    pub when: Vec<String>,
    pub tools: Vec<String>,
    pub has_guide: bool,
}

pub fn row_of(experience: &Experience) -> SearchRow {
    SearchRow {
        slug: experience.slug().to_string(),
        title: experience.manifest.title.clone(),
        state: experience.manifest.state.clone(),
        contract: experience.manifest.contract.clone(),
        when: experience.manifest.when.clone(),
        tools: experience
            .manifest
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect(),
        has_guide: experience.guide_path().is_some(),
    }
}

/// Lexical search over the registry. Deterministic: matcher hits rank above
/// tag hits, which rank above title/guide substring hits; ties by slug.
pub fn search(app_data: &Path, query: &str, ext: &str, tag: &str) -> Vec<SearchRow> {
    let query = query.trim().to_ascii_lowercase();
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let tag = tag.trim().to_ascii_lowercase();

    let mut scored: Vec<(u8, String, SearchRow)> = Vec::new();
    for experience in scan(app_data) {
        let manifest = &experience.manifest;
        let mut score: u8 = 0;

        if !ext.is_empty() {
            let facts = PathFacts {
                extension: ext.clone(),
                file_name: String::new(),
                head: Vec::new(),
            };
            if manifest
                .when
                .iter()
                .filter_map(|raw| parse_matcher(raw))
                .any(|matcher| matches!(&matcher, Matcher::Ext(value) if *value == ext))
            {
                score = score.max(3);
            }
            // `score_path` also covers `name:` patterns, but without bytes only
            // extension-based matchers can hit — the explicit check above is
            // what the model means by `ext`.
            let _ = facts;
        }

        if !tag.is_empty()
            && manifest.when.iter().filter_map(|raw| parse_matcher(raw)).any(
                |matcher| matches!(&matcher, Matcher::Tag(value) if *value == tag),
            )
        {
            score = score.max(2);
        }

        if !query.is_empty() {
            let haystack_title = manifest.title.to_ascii_lowercase();
            let haystack_slug = manifest.slug.to_ascii_lowercase();
            if haystack_title.contains(&query) || haystack_slug.contains(&query) {
                score = score.max(2);
            } else if experience
                .guide_text()
                .to_ascii_lowercase()
                .contains(&query)
            {
                score = score.max(1);
            }
        }

        if score > 0 || (query.is_empty() && ext.is_empty() && tag.is_empty()) {
            scored.push((score, manifest.slug.clone(), row_of(&experience)));
        }
    }

    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored.into_iter().map(|(_, _, row)| row).collect()
}

// ─── Rendering ───────────────────────────────────────────────────────────────

fn render_tools(experience: &Experience, out: &mut String) {
    if experience.manifest.tools.is_empty() {
        out.push_str("\n[Tools] (none — this package carries knowledge only)\n");
        return;
    }
    out.push_str("\n[Tools]\n");
    for tool in &experience.manifest.tools {
        out.push_str(&format!(
            "- {} — {}\n",
            tool.name,
            tool.description.trim().trim_end_matches('.')
        ));
        if !tool.command.is_empty() {
            out.push_str(&format!("  command: {}\n", tool.command.join(" ")));
        }
        if !tool.args.is_null() {
            out.push_str(&format!(
                "  args: {}\n",
                serde_json::to_string(&tool.args).unwrap_or_default()
            ));
        }
    }
}

/// The block injected on `load` and on automatic resolution. Deliberately
/// different framing from `--- Skill: … ---` so the two never blur together.
pub fn render_block(experience: &Experience) -> String {
    let manifest = &experience.manifest;
    let mut out = String::new();
    out.push_str(&format!("\n--- Experience: {} ---\n", manifest.slug));
    if !manifest.title.trim().is_empty() {
        out.push_str(&format!("[Title] {}\n", manifest.title.trim()));
    }
    out.push_str(&format!("[Experience root: {}]\n", experience.dir.display()));
    out.push_str(&format!("[Contract: {}]\n", manifest.contract));
    if !manifest.state.trim().is_empty() {
        out.push_str(&format!("[State: {}]\n", manifest.state));
    }
    if !manifest.when.is_empty() {
        out.push_str(&format!("[Matches] {}\n", manifest.when.join(", ")));
    }
    out.push_str("\n");
    let guide = experience.guide_text();
    if guide.trim().is_empty() {
        out.push_str("(no guide.md)\n");
    } else {
        out.push_str(guide.trim_end());
        out.push('\n');
    }
    render_tools(experience, &mut out);
    out.push_str(&format!(
        "\n[How to run] experience {{\"action\":\"invoke\",\"slug\":\"{}\",\"tool\":\"<tool>\",\"args\":{{…}}}}\n",
        manifest.slug
    ));
    out
}

/// Text handed back when a file arrives that the app cannot read as text.
pub fn render_unsupported(path: &Path, hits: &[Resolved]) -> String {
    let file = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let mut out = format!(
        "Unsupported file format: `{}` cannot be read as text.\n\
         The experience registry matched {} package(s) for this input — \
         these are program-side matches (extension/magic), not a guess.\n",
        file,
        hits.len()
    );
    for hit in hits.iter().take(2) {
        out.push_str(&render_block(&hit.experience));
    }
    if hits.len() > 2 {
        out.push_str(&format!(
            "\n…{} more package(s): use experience {{\"action\":\"search\",\"ext\":\"{}\"}}.\n",
            hits.len() - 2,
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
        ));
    }
    if hits.len() > 1 && hits[0].specificity == hits[1].specificity {
        out.push_str(
            "\n[Ambiguous] two packages claim this input at the same specificity — \
             show both options to the user instead of picking one.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rabbit-exp-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("experiences")).unwrap();
        dir
    }

    fn write_package(app_data: &Path, slug: &str, manifest: &str, guide: &str) {
        let dir = root_for(app_data).join(slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MANIFEST_FILE), manifest).unwrap();
        fs::write(dir.join("guide.md"), guide).unwrap();
    }

    #[test]
    fn parses_matchers_and_ranks_specificity() {
        assert_eq!(parse_matcher("ext:docx"), Some(Matcher::Ext("docx".into())));
        assert_eq!(parse_matcher("ext:.PDF"), Some(Matcher::Ext("pdf".into())));
        assert!(matches!(
            parse_matcher("magic:504b0304:word/"),
            Some(Matcher::Magic { .. })
        ));
        assert_eq!(parse_matcher("nonsense"), None);
        assert_eq!(
            matcher_specificity(&parse_matcher("magic:504b").unwrap()),
            3
        );
        assert_eq!(matcher_specificity(&parse_matcher("ext:docx").unwrap()), 1);
    }

    #[test]
    fn magic_beats_extension_and_ambiguity_is_reported() {
        let app_data = temp_root("resolve");
        write_package(
            &app_data,
            "docx-ext",
            r#"{"schema":1,"slug":"docx-ext","title":"ext","when":["ext:docx"],"contract":"argv","tools":[]}"#,
            "by extension",
        );
        write_package(
            &app_data,
            "docx-magic",
            r#"{"schema":1,"slug":"docx-magic","title":"magic","when":["magic:504b0304:word/"],"contract":"argv","tools":[]}"#,
            "by magic",
        );
        let file = app_data.join("sample.docx");
        // A real zip header + the Word part name, so the magic matcher hits.
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.extend_from_slice(b"....word/document.xml....");
        fs::write(&file, &bytes).unwrap();

        let hits = resolve_for_path(&app_data, &file);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].experience.slug(), "docx-magic");
        assert_eq!(hits[0].specificity, 3);
        assert_eq!(hits[1].experience.slug(), "docx-ext");

        fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn scan_skips_backups_archives_and_broken_manifests() {
        let app_data = temp_root("scan");
        write_package(
            &app_data,
            "good",
            r#"{"schema":1,"slug":"good","title":"g","when":["ext:txt"],"contract":"argv","tools":[]}"#,
            "ok",
        );
        write_package(
            &app_data,
            "gone",
            r#"{"schema":1,"slug":"gone","title":"g","when":[],"state":"archived","contract":"argv","tools":[]}"#,
            "ok",
        );
        let broken = root_for(&app_data).join("broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join(MANIFEST_FILE), "{not json").unwrap();
        fs::create_dir_all(root_for(&app_data).join(".archive")).unwrap();

        let slugs: Vec<String> = scan(&app_data).into_iter().map(|e| e.slug().to_string()).collect();
        assert_eq!(slugs, vec!["good".to_string()]);

        fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn rejects_path_traversal_in_slug() {
        assert!(validate_slug("../etc").is_err());
        assert!(validate_slug("a/b").is_err());
        assert!(validate_slug("").is_err());
        assert!(validate_slug("docx-text_2").is_ok());
    }

    #[test]
    fn guide_text_is_capped_and_block_names_the_root() {
        let app_data = temp_root("guide");
        write_package(
            &app_data,
            "cap",
            r#"{"schema":1,"slug":"cap","title":"t","when":["ext:txt"],"contract":"json",
                "guide":{"file":"guide.md","max_bytes":8},"tools":[]}"#,
            "0123456789abcdef",
        );
        let experience = load(&app_data, "cap").unwrap();
        assert!(experience.guide_text().starts_with("01234567"));
        assert!(experience.guide_text().contains("truncated"));

        write_package(
            &app_data,
            "full",
            r#"{"schema":1,"slug":"full","title":"t","when":[],"contract":"argv",
                "tools":[{"name":"extract","command":["python","bin/x.py"],"args":{"path":{"type":"string"}}}]}"#,
            "body",
        );
        let block = render_block(&load(&app_data, "full").unwrap());
        assert!(block.contains("--- Experience: full ---"));
        assert!(block.contains("[Contract: argv]"));
        assert!(block.contains("body"));
        assert!(block.contains("python bin/x.py"));

        fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn search_ranks_extension_hits_above_title_hits() {
        let app_data = temp_root("search");
        write_package(
            &app_data,
            "alpha",
            r#"{"schema":1,"slug":"alpha","title":"docx helper","when":["ext:docx"],"contract":"argv","tools":[]}"#,
            "a",
        );
        write_package(
            &app_data,
            "beta",
            r#"{"schema":1,"slug":"beta","title":"docx other","when":["ext:pdf"],"contract":"argv","tools":[]}"#,
            "b",
        );
        let rows = search(&app_data, "docx", "docx", "");
        assert_eq!(rows[0].slug, "alpha");
        assert!(rows.iter().any(|row| row.slug == "beta"));

        fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn fingerprint_changes_when_the_package_changes() {
        let app_data = temp_root("fp");
        write_package(
            &app_data,
            "fp",
            r#"{"schema":1,"slug":"fp","title":"t","when":[],"contract":"argv","tools":[]}"#,
            "one",
        );
        let first = load(&app_data, "fp").unwrap().fingerprint();
        fs::write(root_for(&app_data).join("fp").join("guide.md"), "two-two").unwrap();
        let second = load(&app_data, "fp").unwrap().fingerprint();
        assert_ne!(first, second);

        fs::remove_dir_all(&app_data).ok();
    }
}
