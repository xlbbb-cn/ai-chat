// Memory tool: session (in-memory), user/repo/evolution (file-backed) and the
// read-only `all` fan-out across every scope.
//
// The `evolution` scope is special: it is the self-evolution memory area, and
// it is the only scope with a structured on-disk format (YAML front matter),
// a ranked read path the model queries on demand, and a "consolidate into a
// skill" archive path.

use crate::util::{slugify, truncate_with_ellipsis};
use crate::db;
use jieba_rs::{Jieba, TokenizeMode};
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager, State};

use super::emit_tool_progress;
use super::search::{run_integrated_search, SearchOptions};
use super::truncate_tool_output;

/// The self-evolution memory area.
///
/// A dedicated scope for verified facts that must survive across sessions:
/// the default interpreter / runtime environment, the correct way to operate a
/// tool, the file locations that are used often. It only exists while
/// self-evolution mode is on.
pub const EVOLUTION_SCOPE: &str = "evolution";

/// Stable key of the evolution-maintenance resume note.
///
/// The silent maintenance turn writes this entry before it stops (user
/// double-click, budget exhaustion, upstream error). The next pass reads it
/// from its inventory, verifies what is already saved and continues from
/// there. It is deliberately excluded from consolidation clusters and cannot
/// be archived or deleted by the `memory` tool; only the maintenance turn
/// clears it after a completed pass.
pub const MAINTENANCE_PROGRESS_KEY: &str = "evolution-maintenance-progress";

/// Budget (in characters) of one rendered block of the self-evolution memory
/// area — what the panel shows as the top tier and what a `plan` consolidation
/// measures its clusters against. A UI/analysis budget: the request only carries
/// the much smaller `MEMORY_HINT_BUDGET_CHARS` prefix built from keyword hits.
pub const EVOLUTION_CONTEXT_BUDGET_CHARS: usize = 4_000;

/// On-disk format version of an evolution memory entry.
pub const EVOLUTION_FORMAT_VERSION: u32 = 2;

/// Character caps that keep one entry from crowding out the rest.
const EVOLUTION_TITLE_MAX: usize = 100;
const EVOLUTION_SUMMARY_MAX: usize = 300;
/// Cap of the one-line conflict reason kept in `last_conflict`.
const EVOLUTION_CONFLICT_NOTE_MAX: usize = 200;
/// Maximum number of automatic (`source: auto`) entries kept on disk.
const EVOLUTION_AUTO_LIMIT: usize = 40;
/// A memory cluster is only a consolidation candidate above these thresholds —
/// "a block that occupies a large share of memory", not every pair of facts.
const CONSOLIDATE_MIN_ENTRIES: usize = 4;
const CONSOLIDATE_MIN_SHARE_PCT: usize = 20;
/// Space held back for the "+ N lower-ranked facts" tail line. The renderer
/// measures the final pointer line as well: when a very small budget cannot
/// spare this reserve, the lowest-ranked placed lines are popped until the
/// render fits.
const EVOLUTION_TAIL_RESERVE: usize = 140;

/// Every scope the `memory` tool accepts, in fan-out order.
fn concrete_scopes(self_evolution: bool) -> Vec<&'static str> {
    if self_evolution {
        vec!["session", "user", "repo", EVOLUTION_SCOPE]
    } else {
        vec!["session", "user", "repo"]
    }
}

/// Sanitize a memory key so it can be used as a file name.
fn safe_memory_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ─── Paths ───────────────────────────────────────────────────────────────────

/// The hot evolution memory directory (`<workspace>/.data/memory/evolution`).
pub fn evolution_dir(workspace_dir: &Path) -> PathBuf {
    crate::workspace::memory_root(workspace_dir).join(EVOLUTION_SCOPE)
}

/// Where consolidated entries are parked after being folded into a skill.
///
/// Kept inside the evolution tree so a workspace backup still carries it, but
/// never scanned by the injector, `list` or `search` — the skill is the live
/// copy and owns the archive.
pub fn evolution_archive_dir(workspace_dir: &Path) -> PathBuf {
    evolution_dir(workspace_dir).join("archive")
}

// ─── Entry model ─────────────────────────────────────────────────────────────

/// Structured metadata of one evolution entry (the YAML front matter).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EvolutionMeta {
    pub format_version: u32,
    pub key: String,
    pub title: String,
    /// `fact` | `recipe` | `location` | `pitfall` — used to group a skill body.
    pub kind: String,
    pub tags: Vec<String>,
    /// 1–5, set by the model. 5 (or `pinned`) outranks every other entry and
    /// its line is the last one dropped when the budget fills.
    pub importance: u32,
    /// 1–5, how strong the evidence behind the fact is.
    pub confidence: u32,
    /// Times a real result disagreed with this fact. A conflict forces
    /// `confidence` down to the floor and is never cleared automatically.
    pub contradictions: u32,
    /// One-line reason of the most recent conflict (`get` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_conflict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_conflict_at: Option<String>,
    /// Independent confirmations (a repeated `add` bumps this).
    pub evidence: u32,
    /// Times the entry was explicitly retrieved/searched.
    pub uses: u32,
    pub pinned: bool,
    /// `verified` | `auto` | `user`.
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// One parsed evolution entry.
#[derive(Debug, Clone)]
pub struct EvolutionEntry {
    pub meta: EvolutionMeta,
    /// Distilled single-line form used for prompt injection.
    pub summary: String,
    /// The full fact (never truncated on disk; `get` returns it verbatim).
    pub body: String,
    /// `None` for an entry synthesized in memory (never written).
    pub path: Option<PathBuf>,
}

impl EvolutionEntry {
    fn display_title(&self) -> String {
        let title = self.meta.title.trim();
        if !title.is_empty() {
            truncate_with_ellipsis(title, EVOLUTION_TITLE_MAX)
        } else {
            truncate_with_ellipsis(&self.summary, EVOLUTION_TITLE_MAX)
        }
    }

    fn chars(&self) -> usize {
        self.body.chars().count()
    }

    fn needs_migration(&self) -> bool {
        self.meta.format_version != EVOLUTION_FORMAT_VERSION
    }
}

/// An entry plus its ranking score and the text/token views derived from it.
///
/// `views` is private on purpose: it is shared with the parse cache, so
/// `search` matches the same tokens the hint saw instead of re-deriving them
/// from the entry (three tokenization passes per entry were the bulk of a
/// search over a few thousand entries).
#[derive(Debug, Clone)]
pub struct ScoredEntry {
    pub entry: EvolutionEntry,
    pub score: f64,
    views: Arc<EntryViews>,
}

// ─── Small text helpers ──────────────────────────────────────────────────────

/// Collapse markdown lines into one dense line, preserving order and content.
fn one_line(text: &str, max: usize) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(|c: char| matches!(c, '-' | '*' | '#' | '>' | '+' | '·'))
            .trim();
        if trimmed.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(trimmed);
        if out.chars().count() >= max {
            break;
        }
    }
    truncate_with_ellipsis(&out, max)
}

/// First sentence (EN or CJK punctuation), used as a fallback title.
fn first_sentence(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return String::new();
    }
    for (index, ch) in flat.char_indices() {
        if matches!(ch, '。' | '！' | '？' | '.' | '!' | '?') {
            let candidate = &flat[..index + ch.len_utf8()];
            if candidate.chars().count() >= 12 {
                return truncate_with_ellipsis(candidate.trim(), EVOLUTION_TITLE_MAX);
            }
        }
    }
    truncate_with_ellipsis(flat.trim(), EVOLUTION_TITLE_MAX)
}

/// Lower-cased alphanumeric + CJK text, used for identity comparisons.
fn normalize_text(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Words that do not discriminate. Chinese stopwords used to be dead entries —
/// the old word-cutter only pruned them off English runs; the jieba path below
/// checks every token, so they are honoured now.
const TOKEN_STOPWORDS: &[&str] = &[
    // English
    "the", "and", "for", "with", "that", "this", "from", "into", "use", "using", "used",
    "you", "your", "have", "has", "can", "will", "would", "should", "what", "which",
    "when", "where", "who", "how", "not", "are", "was", "were", "been", "our", "they",
    // 中文
    "默认", "使用", "可以", "如果", "需要", "一个", "进行", "通过", "这个", "那个", "我们",
    "他们", "什么", "怎么", "为什么", "如何", "就是", "还是", "但是", "因为", "所以", "没有",
    "然后", "已经", "以及", "或者", "这些", "那些", "一些", "相关", "关于", "时候",
];

/// One tokenizer for every matcher: lowercased, cut at real word boundaries by
/// jieba (so CJK stops being overlapping bigrams), English stemmed (Snowball,
/// so `building`/`built` meet `build`), stopwords dropped, sorted and deduped.
///
/// The hint, `search` and the cluster/dedupe paths all go through this one
/// function, so they cannot disagree on what a "word" is again.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = tokenize_multi(text);
    tokens.sort();
    tokens.dedup();
    tokens
}

/// `tokenize` but keeping repeats, only for the relevance scorer which must
/// count how often a word occurs (`tf`). Everything else wants the deduped set.
fn tokenize_multi(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in jieba().tokenize(text, TokenizeMode::Default, true) {
        let word = token.word.trim();
        if word.is_empty() || !word.chars().any(|ch| ch.is_alphanumeric()) {
            continue;
        }
        let lower = word.to_lowercase();
        if TOKEN_STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        if lower.chars().all(|ch| ch.is_ascii()) {
            if lower.chars().count() < 3 {
                continue;
            }
            tokens.push(stemmer().stem(&lower).to_string());
        } else {
            tokens.push(lower);
        }
    }
    tokens
}

/// The embedded jieba dictionary costs a small cold-start load, so it lives
/// behind a `OnceLock` and is built on first use — not per request.
fn jieba() -> &'static Jieba {
    static JIEBA: std::sync::OnceLock<Jieba> = std::sync::OnceLock::new();
    JIEBA.get_or_init(Jieba::new)
}

fn stemmer() -> &'static Stemmer {
    static STEMMER: std::sync::OnceLock<Stemmer> = std::sync::OnceLock::new();
    STEMMER.get_or_init(|| Stemmer::create(Algorithm::English))
}

/// Jaccard similarity of two sets; both empty means "not similar" (0.0).
fn jaccard(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(right).count() as f64;
    let union = left.union(right).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

// ─── Parsing / serialization ─────────────────────────────────────────────────

fn split_front_matter(content: &str) -> Option<(&str, &str)> {
    let content = content.trim_start_matches('\u{feff}');
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let body = rest[end + 4..]
        .trim_start_matches('\n')
        .trim_start_matches('\r')
        .trim();
    Some((frontmatter, body))
}

/// Strip the legacy `# key` heading and the trailing `tags: a, b` line.
fn legacy_body(raw: &str) -> (String, Vec<String>) {
    let mut lines: Vec<&str> = raw.lines().collect();
    let mut tags: Vec<String> = Vec::new();

    while let Some(first) = lines.first() {
        let trimmed = first.trim();
        if trimmed.is_empty() || trimmed.starts_with("# ") {
            lines.remove(0);
        } else {
            break;
        }
    }
    while let Some(last) = lines.last() {
        let trimmed = last.trim();
        if trimmed.is_empty() {
            lines.pop();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("tags:") {
            tags = rest
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
            lines.pop();
        }
        break;
    }

    (lines.join("\n").trim().to_string(), tags)
}

fn system_time_to_rfc3339(time: std::time::SystemTime) -> String {
    let stamp: chrono::DateTime<chrono::Utc> = time.into();
    stamp.to_rfc3339()
}

/// Parse one evolution file, tolerating both the structured and the legacy
/// (`# key` + free text) format.
pub fn parse_evolution_entry(path: &Path) -> Option<EvolutionEntry> {
    let raw = fs::read_to_string(path).ok()?;
    let mtime = fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .map(system_time_to_rfc3339)
        .unwrap_or_default();
    let file_key = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("fact")
        .to_string();

    if let Some((frontmatter, body)) = split_front_matter(&raw) {
        if let Ok(meta) = serde_yaml::from_str::<EvolutionMeta>(frontmatter) {
            if !meta.key.trim().is_empty() {
                let body = body.trim().to_string();
                let summary = one_line(&body, EVOLUTION_SUMMARY_MAX);
                let title = if meta.title.trim().is_empty() {
                    first_sentence(&body)
                } else {
                    meta.title.clone()
                };
                let mut meta = meta;
                if meta.title.trim().is_empty() {
                    meta.title = title;
                }
                if meta.created_at.is_empty() {
                    meta.created_at = mtime.clone();
                }
                if meta.updated_at.is_empty() {
                    meta.updated_at = mtime;
                }
                if meta.evidence == 0 {
                    meta.evidence = 1;
                }
                if meta.importance == 0 {
                    meta.importance = 3;
                }
                if meta.confidence == 0 {
                    meta.confidence = 3;
                }
                if meta.source.is_empty() {
                    meta.source = "verified".to_string();
                }
                return Some(EvolutionEntry {
                    meta,
                    summary,
                    body,
                    path: Some(path.to_path_buf()),
                });
            }
        }
    }

    // Legacy format: `# <key>\n\n<content>\ntags: a, b`.
    let (body, tags) = legacy_body(&raw);
    if body.is_empty() {
        return None;
    }
    let is_auto = file_key.starts_with("conclusion-");
    let title = first_sentence(&body);
    let summary = one_line(&body, EVOLUTION_SUMMARY_MAX);
    Some(EvolutionEntry {
        meta: EvolutionMeta {
            format_version: 0,
            key: file_key,
            title,
            kind: "fact".to_string(),
            tags,
            importance: if is_auto { 1 } else { 3 },
            confidence: 3,
            contradictions: 0,
            last_conflict: None,
            last_conflict_at: None,
            evidence: 1,
            uses: 0,
            pinned: false,
            source: if is_auto { "auto" } else { "verified" }.to_string(),
            created_at: mtime.clone(),
            updated_at: mtime,
            last_used_at: None,
            aliases: Vec::new(),
        },
        summary,
        body,
        path: Some(path.to_path_buf()),
    })
}

/// Serialize one entry to the structured format.
pub fn evolution_entry_to_md(entry: &EvolutionEntry) -> String {
    let mut meta = entry.meta.clone();
    meta.format_version = EVOLUTION_FORMAT_VERSION;
    if meta.title.trim().is_empty() {
        meta.title = first_sentence(&entry.body);
    }
    let yaml = serde_yaml::to_string(&meta).unwrap_or_default();
    format!("---\n{}---\n\n{}\n", yaml, entry.body)
}

/// Write a file through a temp file so a crash never leaves half a document.
///
/// Unix `rename` replaces the target atomically; Windows refuses, so the
/// fallback removes the target first and retries. The temp file is kept if the
/// final rename fails — losing the new content would be worse than a stray
/// `.tmp`.
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, content)?;
    if fs::rename(&tmp, path).is_ok() {
        return Ok(());
    }
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&tmp, path)
}

/// Rewrite every legacy file in the evolution directory once.
///
/// Called from the `memory` tool and from the panel command — never from the
/// prompt builder, which must stay free of filesystem writes so the cached
/// prompt prefix is byte-stable.
pub fn persist_evolution_migration(workspace_dir: &Path) -> usize {
    let dir = evolution_dir(workspace_dir);
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };
    let mut migrated = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") || name.contains(".bak.") {
            continue;
        }
        let Some(parsed) = parse_evolution_entry(&path) else {
            continue;
        };
        if !parsed.needs_migration() {
            continue;
        }
        if write_atomic(&path, &evolution_entry_to_md(&parsed)).is_ok() {
            migrated += 1;
        }
    }
    migrated
}

// ─── Loading / ranking ───────────────────────────────────────────────────────

fn parse_ts(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|stamp| stamp.with_timezone(&chrono::Utc))
}

/// Age in days of the last proven usefulness (or the last update).
fn entry_age_days(entry: &EvolutionEntry) -> f64 {
    let raw = entry
        .meta
        .last_used_at
        .as_deref()
        .filter(|stamp| !stamp.is_empty())
        .unwrap_or(&entry.meta.updated_at);
    parse_ts(raw)
        .map(|stamp| (chrono::Utc::now() - stamp).num_seconds() as f64 / 86_400.0)
        .unwrap_or(30.0)
        .max(0.0)
}

/// Ranking score: importance, confidence, how often it was confirmed and used,
/// and how recently it proved useful. Pinned entries float above everything.
///
/// The recency term is bucketed to **whole days**: the ranked render lands in
/// the panel and in on-demand `list` output, so a fractional-day wall-clock
/// drift would silently reorder entries between two looks at the same data.
/// With whole-day buckets the order only changes at day boundaries or on
/// explicit `uses`/`evidence` bumps — never click by click of the clock.
pub fn score_evolution_entry(entry: &EvolutionEntry) -> f64 {
    let meta = &entry.meta;
    let age_days = entry_age_days(entry).floor();
    let mut score = 2.0 * meta.importance as f64
        + meta.confidence as f64
        + 1.5 * ((1 + meta.evidence) as f64).ln()
        + 1.2 * ((1 + meta.uses) as f64).ln()
        + 3.0 * (-age_days / 30.0).exp();
    if meta.pinned || meta.importance >= 5 {
        score += 100.0;
    }
    score
}

/// Merge entries whose normalized body is identical.
///
/// The survivor keeps its identity (key/path) and accumulates the evidence and
/// usage of its duplicates, so repeated auto-recorded conclusions stop
/// competing for prompt space without deleting anything from disk.
fn dedupe_entries(entries: Vec<EvolutionEntry>) -> Vec<EvolutionEntry> {
    let mut by_body: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<EvolutionEntry> = Vec::new();

    for entry in entries {
        let signature = normalize_text(&entry.body);
        if !signature.is_empty() {
            if let Some(&index) = by_body.get(&signature) {
                let keep_incoming =
                    score_evolution_entry(&entry) > score_evolution_entry(&out[index]);
                let mut merged = if keep_incoming {
                    entry.clone()
                } else {
                    out[index].clone()
                };
                let loser = if keep_incoming { &out[index] } else { &entry };
                merge_evidence(&mut merged, loser);
                out[index] = merged;
                continue;
            }
            by_body.insert(signature, out.len());
        }
        out.push(entry);
    }

    out
}

/// Fold a duplicate's evidence into the survivor.
///
/// Shared by the owned and the cached loader so the two cannot disagree about
/// what merging an entry means.
fn merge_evidence(merged: &mut EvolutionEntry, loser: &EvolutionEntry) {
    merged.meta.evidence = merged.meta.evidence.max(1) + loser.meta.evidence.max(1);
    merged.meta.uses = merged.meta.uses.saturating_add(loser.meta.uses);
    merged.meta.pinned |= loser.meta.pinned;
    merged.meta.importance = merged.meta.importance.max(loser.meta.importance);
    for tag in &loser.meta.tags {
        if !merged.meta.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            merged.meta.tags.push(tag.clone());
        }
    }
    if loser.meta.key != merged.meta.key && !merged.meta.aliases.contains(&loser.meta.key) {
        merged.meta.aliases.push(loser.meta.key.clone());
    }
}

/// Everything one entry derives from a pass over its text: the raw field texts
/// and their token bags.
///
/// The hint builds these for every entry on every user message, and `search`
/// used to derive them up to three more times per entry. They are computed once
/// when the file is parsed and shared by both paths.
#[derive(Debug)]
struct EntryViews {
    /// `[key, title, tags, summary + body]`.
    texts: [String; 4],
    /// `tokenize` of each text: deduped, order preserved.
    fields: [Vec<String>; 4],
}

impl EntryViews {
    fn of(entry: &EvolutionEntry) -> Self {
        let texts = entry_field_texts(entry);
        let fields = [
            tokenize(&texts[0]),
            tokenize(&texts[1]),
            tokenize(&texts[2]),
            tokenize(&texts[3]),
        ];
        Self { texts, fields }
    }
}

/// One parsed entry plus everything derived from it by a pass over its text:
/// the views above and the body signature the dedupe groups on.
struct CachedEntry {
    /// File identity the cache validates against. A rewrite changes at least one
    /// of the two — a `confidence`/`uses` bump changes the length, an edited body
    /// changes both — so an entry on disk can never be served stale.
    length: u64,
    modified: Option<std::time::SystemTime>,
    entry: EvolutionEntry,
    views: Arc<EntryViews>,
    signature: String,
}

fn entry_cache() -> &'static Mutex<HashMap<PathBuf, Arc<CachedEntry>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<CachedEntry>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Every hot entry, reusing the cached parse for any file whose `(mtime, len)`
/// did not change.
///
/// The hint runs once per user message and used to re-read, re-parse and
/// re-tokenize the whole scope every time; at a few thousand entries that is
/// seconds of work for a 200-character block. A warm call now costs one
/// `metadata` per file plus a hash lookup, and the `Arc`s it returns are shared
/// rather than cloned.
fn scan_entries(workspace_dir: &Path) -> Vec<Arc<CachedEntry>> {
    let dir = evolution_dir(workspace_dir);
    let Ok(read_dir) = fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut cache = entry_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut out: Vec<Arc<CachedEntry>> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for dirent in read_dir.flatten() {
        let name = dirent.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") || name.contains(".bak.") {
            continue;
        }
        // `DirEntry::metadata` reuses the handle `read_dir` already opened on
        // Windows, unlike `path.metadata()`, which resolves the path again.
        let Ok(metadata) = dirent.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let path = dirent.path();
        let length = metadata.len();
        let modified = metadata.modified().ok();
        seen.insert(path.clone());

        if let Some(cached) = cache.get(&path) {
            if cached.length == length && cached.modified == modified {
                out.push(Arc::clone(cached));
                continue;
            }
        }

        let Some(entry) = parse_evolution_entry(&path) else {
            continue;
        };
        let views = Arc::new(EntryViews::of(&entry));
        let signature = normalize_text(&entry.body);
        let cached = Arc::new(CachedEntry {
            length,
            modified,
            entry,
            views,
            signature,
        });
        cache.insert(path, Arc::clone(&cached));
        out.push(cached);
    }

    // A deleted entry must stop answering: drop everything this directory no
    // longer lists, while keeping other workspaces' warmed entries.
    cache.retain(|path, _| !path.starts_with(&dir) || seen.contains(path));

    out
}

/// The entries after body-dedupe, kept as shared handles so the hint does not
/// clone a few thousand bodies per turn.
fn dedupe_cached(entries: Vec<Arc<CachedEntry>>) -> Vec<Arc<CachedEntry>> {
    let mut by_body: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Arc<CachedEntry>> = Vec::new();

    for cached in entries {
        if !cached.signature.is_empty() {
            if let Some(&index) = by_body.get(cached.signature.as_str()) {
                let keep_incoming = score_evolution_entry(&cached.entry)
                    > score_evolution_entry(&out[index].entry);
                let (winner, loser) = if keep_incoming {
                    (&cached, &out[index])
                } else {
                    (&out[index], &cached)
                };
                let mut merged = winner.entry.clone();
                merge_evidence(&mut merged, &loser.entry);
                out[index] = Arc::new(CachedEntry {
                    // A merged entry is synthesized in memory and never written,
                    // so it carries no file identity to validate.
                    length: 0,
                    modified: None,
                    views: Arc::new(EntryViews::of(&merged)),
                    signature: cached.signature.clone(),
                    entry: merged,
                });
                continue;
            }
            by_body.insert(cached.signature.clone(), out.len());
        }
        out.push(cached);
    }

    out
}

/// Load every hot entry (migration is **not** performed here).
pub fn load_evolution_entries(workspace_dir: &Path) -> Vec<EvolutionEntry> {
    dedupe_entries(
        scan_entries(workspace_dir)
            .into_iter()
            .map(|cached| cached.entry.clone())
            .collect(),
    )
}

/// Read, parse and tokenize the whole hot scope, discarding the result.
///
/// Called on a background thread at app start. The first turn of a session
/// builds the hint from every entry, so on a large scope (a few thousand
/// entries) the cold pass is seconds of latency the user would otherwise feel
/// before the model even starts. Warming moves that work off the request path;
/// the cache still validates by `(mtime, len)`, so a warmed scan can never
/// serve stale text.
pub fn warm_memory_cache(workspace_dir: &Path) {
    let _ = scan_entries(workspace_dir);
}

/// Load + rank. The single read entry point shared by `memory list`, the
/// consolidation planner and the panel.
pub fn ranked_evolution_entries(workspace_dir: &Path) -> Vec<ScoredEntry> {
    let mut scored: Vec<ScoredEntry> = dedupe_cached(scan_entries(workspace_dir))
        .into_iter()
        .map(|cached| ScoredEntry {
            score: score_evolution_entry(&cached.entry),
            entry: cached.entry.clone(),
            views: Arc::clone(&cached.views),
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.entry.meta.key.cmp(&right.entry.meta.key))
    });
    scored
}

// ─── Prompt rendering ────────────────────────────────────────────────────────

/// Result of the tiered renderer: the rendered text plus the keys that made it
/// into the top tier (the panel marks those rows).
pub struct RenderedEvolution {
    /// Rendered block. Nothing injects it into a request any more (the model
    /// searches the scope on demand and only a ≤200-char hint is prefixed), so
    /// only the reference tests read it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub text: String,
    pub injected: Vec<String>,
}

/// Three tiers: full distilled entries, title-only entries, then a pointer line.
///
/// Deterministic: it never writes to disk, and ties break on `key`, so two
/// consecutive builds produce byte-identical text (prompt-cache friendly).
///
/// Every measurement is in **characters**: `EVOLUTION_CONTEXT_BUDGET_CHARS` is
/// a character budget, and a CJK line costs ~3 bytes per character, so byte
/// accounting would silently fill only a third of it.
///
/// Pinned / importance-5 entries are protected: their +100 score always ranks
/// them above every other entry, so they fill the tiers first and their lines
/// are the last ones dropped when a budget tightens.
pub fn render_ranked_entries(ranked: &[ScoredEntry], budget_chars: usize) -> RenderedEvolution {
    if ranked.is_empty() {
        return RenderedEvolution {
            text: String::new(),
            injected: Vec::new(),
        };
    }

    let tail_reserve = EVOLUTION_TAIL_RESERVE.min(budget_chars / 10);
    let hard_budget = budget_chars.saturating_sub(tail_reserve);
    let tier1_budget = hard_budget * 85 / 100;

    // One rendered line per entry, kept in rank order. `tier1` is the full
    // summary line, `tier2` the title-only fallback (the two sections are
    // concatenated below, as before).
    struct Placed {
        key: String,
        line: String,
        chars: usize,
        tier1: bool,
    }

    let char_len = |text: &str| text.chars().count();
    let mut placed: Vec<Placed> = Vec::new();
    let mut tier1_chars = 0usize;
    let mut tier2_chars = 0usize;
    let mut pending: Vec<&ScoredEntry> = Vec::new();

    // Tier 1: full distilled summaries in rank order.
    for scored in ranked {
        let key = scored.entry.meta.key.clone();
        let summary = one_line(&scored.entry.summary, EVOLUTION_SUMMARY_MAX);
        if summary.is_empty() {
            pending.push(scored);
            continue;
        }
        let line = format!("- {key}: {summary}\n");
        let line_chars = char_len(&line);
        if tier1_chars + line_chars <= tier1_budget {
            tier1_chars += line_chars;
            placed.push(Placed {
                key,
                line,
                chars: line_chars,
                tier1: true,
            });
        } else {
            pending.push(scored);
        }
    }

    // Tier 2: title-only fallback lines, still in rank order — protected
    // entries (pinned / importance 5) come first because only they carry the
    // +100 score bonus.
    let mut omitted = 0usize;
    for scored in pending {
        let key = scored.entry.meta.key.clone();
        let title = scored.entry.display_title();
        if title.is_empty() {
            omitted += 1;
            continue;
        }
        let line = format!("- {key}: {title}\n");
        let line_chars = char_len(&line);
        if tier1_chars + tier2_chars + line_chars <= hard_budget {
            tier2_chars += line_chars;
            placed.push(Placed {
                key,
                line,
                chars: line_chars,
                tier1: false,
            });
        } else {
            omitted += 1;
        }
    }

    // The pointer line must also fit the budget. The reserve usually absorbs
    // it; with very small budgets it may not, so drop the lowest-ranked placed
    // line (the end of the rank-ordered list) until the render fits exactly.
    // Every drop frees at least a few characters while the omitted digit count
    // can grow by one, so the loop terminates.
    let tail_line = |omitted: usize| -> String {
        if omitted > 0 {
            format!("+ {omitted} lower-ranked facts — memory search scope=evolution")
        } else {
            String::new()
        }
    };
    loop {
        let tail_chars = char_len(&tail_line(omitted));
        if tier1_chars + tier2_chars + tail_chars <= budget_chars {
            break;
        }
        let Some(last) = placed.pop() else {
            // Nothing left to drop; a tail-only render is as small as this
            // gets (only reachable with a degenerate budget).
            break;
        };
        if last.tier1 {
            tier1_chars -= last.chars;
        } else {
            tier2_chars -= last.chars;
        }
        omitted += 1;
    }

    let mut text = String::new();
    for entry in placed.iter().filter(|entry| entry.tier1) {
        text.push_str(&entry.line);
    }
    for entry in placed.iter().filter(|entry| !entry.tier1) {
        text.push_str(&entry.line);
    }
    let tail = tail_line(omitted);
    if !tail.is_empty() {
        text.push_str(&tail);
    }
    text = text.trim_end().to_string();

    RenderedEvolution {
        text,
        injected: placed
            .iter()
            .filter(|entry| entry.tier1)
            .map(|entry| entry.key.clone())
            .chain(
                placed
                    .iter()
                    .filter(|entry| !entry.tier1)
                    .map(|entry| entry.key.clone()),
            )
            .collect(),
    }
}

/// Render the self-evolution memory area as one text block.
///
/// This is **not** part of any request: preloading every verified fact made the
/// first request's prefill dominate the prompt, so the model now gets a ≤200
/// character hint of the entries matching the current message (see
/// [`build_memory_hint`]) plus a pointer to the `evolution` scope it searches on
/// demand. The renderer is kept as the reference implementation its tests pin
/// down (deterministic tiering and a character budget).
#[cfg(test)]
pub fn render_evolution_context(workspace_dir: &Path, budget_chars: usize) -> String {
    let ranked = ranked_evolution_entries(workspace_dir);
    render_ranked_entries(&ranked, budget_chars).text
}

// ─── Request hint (confidence-ranked, ≤ MEMORY_HINT_BUDGET_CHARS) ────────────

/// Character budget of the memory hint prepended to the newest user message.
///
/// The whole rendered block — header and titles — never exceeds it. It is
/// deliberately tiny: the full memory area is searched on demand, and this only
/// points the model at the memories whose *name* matches the message it just
/// received (a title is a pointer, the body is the payload it fetches).
pub const MEMORY_HINT_BUDGET_CHARS: usize = 200;

/// Cap of one rendered title, so a verbose title cannot eat the budget.
const MEMORY_HINT_ITEM_CHARS: usize = 80;

/// Upper bound of query keywords considered, longest (most specific) first.
const MEMORY_HINT_MAX_KEYWORDS: usize = 32;

/// File-backed `user` / `repo` notes carry no trust metadata of their own; they
/// enter the ranking as a neutral fact, below a confirmed (`c4`/`c5`) entry.
const FILE_MEMORY_CONFIDENCE: u32 = 3;
const FILE_MEMORY_IMPORTANCE: u32 = 3;

/// The two file-backed scopes whose `.md` notes are eligible for the hint.
const MEMORY_HINT_SCOPES: [&str; 2] = ["user", "repo"];

/// Hit priority of the three tiers, highest first: **title** (a curated name —
/// the entry's key ranks here, it is the same words) beats **tag**, which beats
/// a word buried in **body**.
const HINT_TITLE_WEIGHT: u32 = 3;
const HINT_TAG_WEIGHT: u32 = 2;
const HINT_CONTENT_WEIGHT: u32 = 1;

/// Opening of the block; `hint_text` closes the parenthesis once it knows how
/// many matching entries were left out.
const MEMORY_HINT_HEADER: &str = "MEMORY (confidence-ranked; verify before relying";

/// A rendered hint plus the keys it placed (the keys are logged, never the
/// titles — the request log is already noisy enough).
pub struct MemoryHint {
    pub text: String,
    pub keys: Vec<String>,
}

struct HintCandidate {
    key: String,
    /// The only thing the block renders: the entry's curated, collapsed and
    /// truncated name. The body is matched against, never printed.
    title: String,
    confidence: u32,
    importance: u32,
    /// Best keyword hit weight (0 = no match, filtered out).
    weight: u32,
    /// Key / title / tags / body views — matching only, never rendered. Shared
    /// with the entry cache, so a warm turn clones nothing.
    views: Arc<EntryViews>,
}

/// Build the confidence-ranked hint for one user message.
///
/// Read-only on purpose: this runs on the request path, so it never migrates an
/// entry, never bumps `uses` and never touches `confidence` (a hint that
/// counted as usage would rank itself up just by being relevant).
///
/// `None` means "inject nothing": no usable keyword, no candidate, no match, or
/// a budget too small for even one line.
pub fn build_memory_hint(
    workspace_dir: &Path,
    query: &str,
    include_evolution: bool,
) -> Option<MemoryHint> {
    // Cheap bail-out first: an empty message must not touch the disk.
    let keywords = hint_keywords(query);
    if keywords.is_empty() {
        return None;
    }

    let mut ranked: Vec<HintCandidate> = hint_candidates(workspace_dir, include_evolution)
        .into_iter()
        .filter_map(|mut candidate| {
            candidate.weight = hint_weight(&candidate.views.fields, &keywords);
            (candidate.weight > 0).then_some(candidate)
        })
        .collect();
    if ranked.is_empty() {
        return None;
    }

    // "Ranked by confidence" — the higher the evidence, the earlier the line.
    // The rest only makes two differently-evidenced entries deterministic:
    // stronger hit (title > tag > body), higher importance, denser line, key.
    ranked.sort_by(|left, right| {
        right
            .confidence
            .cmp(&left.confidence)
            .then_with(|| right.weight.cmp(&left.weight))
            .then_with(|| right.importance.cmp(&left.importance))
            .then_with(|| {
                left.title
                    .chars()
                    .count()
                    .cmp(&right.title.chars().count())
            })
            .then_with(|| left.key.cmp(&right.key))
    });

    // Two entries may share a name (an evolution entry and a `repo` note about
    // the same topic): the block renders titles, so a repeat would be a wasted
    // line. The sort put the better-evidenced one first, so keep that.
    let mut seen_titles: HashSet<String> = HashSet::new();
    ranked.retain(|candidate| seen_titles.insert(normalize_text(&candidate.title)));

    let (text, placed) = render_hint(&ranked, MEMORY_HINT_BUDGET_CHARS)?;
    Some(MemoryHint {
        text,
        keys: ranked[..placed]
            .iter()
            .map(|candidate| candidate.key.clone())
            .collect(),
    })
}

/// Keywords of one user message: dictionary-segmented words plus stemmed
/// English, minus stopwords, keeping the longest `MEMORY_HINT_MAX_KEYWORDS`
/// (a lone CJK character matches everything, so single-character tokens are
/// dropped).
fn hint_keywords(query: &str) -> HashSet<String> {
    let mut tokens: Vec<String> = tokenize(query)
        .into_iter()
        .filter(|token| token.chars().count() >= 2)
        .collect();
    // Deterministic on equal length, and the truncation keeps the specific ones.
    tokens.sort_by(|left, right| {
        right
            .chars()
            .count()
            .cmp(&left.chars().count())
            .then_with(|| left.cmp(right))
    });
    tokens.dedup();
    tokens.truncate(MEMORY_HINT_MAX_KEYWORDS);
    tokens.into_iter().collect()
}

/// Every fact the hint may draw from: the hot evolution entries (when the mode
/// is on) plus the file-backed `user` / `repo` notes.
///
/// Low-trust entries (`confidence <= LOW_TRUST_CONFIDENCE`) and the maintenance
/// resume note are skipped: a fact the model must re-verify anyway is not worth
/// a fifth of the budget, and the resume note is an internal bookmark.
fn hint_candidates(workspace_dir: &Path, include_evolution: bool) -> Vec<HintCandidate> {
    let mut candidates: Vec<HintCandidate> = Vec::new();

    if include_evolution {
        for cached in dedupe_cached(scan_entries(workspace_dir)) {
            let entry = &cached.entry;
            if entry.meta.key == MAINTENANCE_PROGRESS_KEY {
                continue;
            }
            if entry.meta.confidence <= LOW_TRUST_CONFIDENCE {
                continue;
            }
            candidates.push(HintCandidate {
                key: entry.meta.key.clone(),
                title: one_line(
                    if entry.meta.title.is_empty() {
                        &entry.meta.key
                    } else {
                        &entry.meta.title
                    },
                    MEMORY_HINT_ITEM_CHARS,
                ),
                confidence: entry.meta.confidence,
                importance: entry.meta.importance,
                weight: 0,
                views: Arc::clone(&cached.views),
            });
        }
    }

    for scope in MEMORY_HINT_SCOPES {
        for path in scope_note_paths(workspace_dir, scope) {
            let Some((key, body, tags)) = read_scope_note(&path) else {
                continue;
            };
            candidates.push(HintCandidate {
                // A note's name is its `# key` heading (or the file stem).
                title: one_line(&key, MEMORY_HINT_ITEM_CHARS),
                // The `# key` heading carries the readable key; the file stem
                // is the sanitized version of it.
                views: Arc::new(EntryViews {
                    fields: [
                        tokenize(&key),
                        tokenize(&key),
                        tokenize(&tags.join(" ")),
                        tokenize(&body),
                    ],
                    texts: [key.clone(), key.clone(), tags.join(" "), body.clone()],
                }),
                key,
                confidence: FILE_MEMORY_CONFIDENCE,
                importance: FILE_MEMORY_IMPORTANCE,
                weight: 0,
            });
        }
    }

    candidates.sort_by(|left, right| left.key.cmp(&right.key));
    candidates
}

/// Every `user` / `repo` note file of one scope, sorted.
///
/// `read_dir` order is filesystem-dependent; sorting keeps the rendered output
/// byte-identical between two builds of the same memory set.
fn scope_note_paths(workspace_dir: &Path, scope: &str) -> Vec<PathBuf> {
    let dir = crate::workspace::memory_root(workspace_dir).join(scope);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.ends_with(".md") && !name.contains(".bak."))
                    .unwrap_or(false)
        })
        .collect();
    paths.sort();
    paths
}

/// Read one `user` / `repo` note: `# key`, free text, optional `tags: a, b`.
fn read_scope_note(path: &Path) -> Option<(String, String, Vec<String>)> {
    let raw = fs::read_to_string(path).ok()?;
    let (body, tags) = legacy_body(&raw);
    let body = body.trim().to_string();
    if body.is_empty() {
        return None;
    }
    let heading = raw
        .lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .filter(|heading| !heading.is_empty())
        .map(str::to_string);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    Some((heading.unwrap_or(stem), body, tags))
}

/// Result of matching one memory's fields against a query's keywords.
struct KeywordMatch {
    /// 3 = the curated name (key or title), 2 = tags, 1 = body only.
    tier: u32,
    /// Distinct query keywords found anywhere in the entry (the coverage).
    hits: usize,
    /// TF-IDF relevance, only filled by `search`; the hint build ignores it.
    score: f32,
    /// True when two or more consecutive query words appear, in order, in the
    /// entry's name — a strong "this memory is about the question" signal.
    phrase: bool,
}

impl KeywordMatch {
    /// Tier name printed by the `search` output.
    fn tier_label(&self) -> &'static str {
        match self.tier {
            HINT_TITLE_WEIGHT => "title",
            HINT_TAG_WEIGHT => "tags",
            _ => "body",
        }
    }

    /// A hit strong enough to prove the entry is *about* the query: a name or
    /// tag hit, or a body hit on two different keywords.
    fn is_credible(&self) -> bool {
        self.tier > HINT_CONTENT_WEIGHT || self.hits >= 2
    }
}

/// Match `keywords` against `[key, title, tags, body]` and report the best tier
/// plus how many distinct keywords matched.
fn keyword_match(fields: &[Vec<String>; 4], keywords: &HashSet<String>) -> Option<KeywordMatch> {
    let mut tiers = [false; 4];
    let mut matched: HashSet<&String> = HashSet::new();
    for (index, field) in fields.iter().enumerate() {
        for token in field {
            if keywords.contains(token.as_str()) {
                tiers[index] = true;
                matched.insert(token);
            }
        }
    }
    if matched.is_empty() {
        return None;
    }
    let tier = if tiers[0] || tiers[1] {
        HINT_TITLE_WEIGHT
    } else if tiers[2] {
        HINT_TAG_WEIGHT
    } else {
        HINT_CONTENT_WEIGHT
    };
    Some(KeywordMatch {
        tier,
        hits: matched.len(),
        score: 0.0,
        phrase: false,
    })
}

/// The raw `[key, title, tags, body]` texts of one entry.
fn entry_field_texts(entry: &EvolutionEntry) -> [String; 4] {
    [
        entry.meta.key.clone(),
        entry.meta.title.clone(),
        entry.meta.tags.join(" "),
        format!("{} {}", entry.summary, entry.body),
    ]
}

/// Hit weight the hint uses: the tier, with the stricter rule that a
/// **body-only** match needs two distinct keywords — one common word is not
/// evidence that the entry is *about* the question.
fn hint_weight(fields: &[Vec<String>; 4], keywords: &HashSet<String>) -> u32 {
    match keyword_match(fields, keywords) {
        Some(hit) if hit.is_credible() => hit.tier,
        _ => 0,
    }
}

/// Upper bound of one `search` result list; the rest is a pointer line.
///
/// A line carries a full distilled summary (`EVOLUTION_SUMMARY_MAX`), so a
/// dozen keeps the whole result under the tool-output head budget instead of
/// being cut in the middle by `truncate_tool_output`.
const EVOLUTION_SEARCH_MAX_MATCHES: usize = 12;

/// Added to the score when two consecutive query words appear, in order, in the
/// entry's name (`key`/`title`) — the phrasing matches, not just the words.
const PHRASE_BONUS: f32 = 1.5;

/// In-memory statistics of the searched corpus: how many entries, in how many
/// of them each word occurs, and the mean entry length. Rebuilt per `search`
/// because entries are a couple of dozen small files — not worth caching.
struct CorpusStats {
    n: usize,
    df: HashMap<String, u32>,
    avg_len: f32,
}

fn corpus_stats(entries: &[ScoredEntry]) -> CorpusStats {
    let mut df: HashMap<String, u32> = HashMap::new();
    let mut total_len = 0usize;
    for scored in entries {
        let fields = &scored.views.fields;
        for token in fields.iter().flatten() {
            *df.entry(token.clone()).or_insert(0) += 1;
        }
        total_len += fields.iter().map(Vec::len).sum::<usize>();
    }
    CorpusStats {
        n: entries.len(),
        df,
        avg_len: if entries.is_empty() {
            1.0
        } else {
            total_len as f32 / entries.len() as f32
        },
    }
}

/// Field weights of `relevance`, mirroring the hit tiers: the curated name
/// carries most, tags next, the prose least.
fn field_weights() -> [f32; 4] {
    [
        HINT_TITLE_WEIGHT as f32,
        HINT_TITLE_WEIGHT as f32,
        HINT_TAG_WEIGHT as f32,
        HINT_CONTENT_WEIGHT as f32,
    ]
}

/// TF-IDF relevance of one entry to the query: smoothed idf, field-weighted
/// tf, lightly length-normalised, plus a phrase bonus. Comparable only within
/// one `search` — the corpus stats are per-query.
fn relevance(
    scored: &ScoredEntry,
    keywords: &HashSet<String>,
    query_ordered: &[String],
    corpus: &CorpusStats,
) -> (f32, bool) {
    let texts = &scored.views.texts;
    let weights = field_weights();
    let mut fields: [Vec<String>; 4] = Default::default();
    let mut name_tokens: Vec<String> = Vec::new();
    let mut doc_len = 0usize;
    for index in 0..4 {
        let tokens = tokenize_multi(&texts[index]);
        doc_len += tokens.len();
        if index < 2 {
            name_tokens.extend(tokens.iter().cloned());
        }
        fields[index] = tokens;
    }

    let mut score = 0.0f32;
    for keyword in keywords {
        let df = corpus.df.get(keyword).copied().unwrap_or(0) as f32;
        let idf = ((1.0 + corpus.n as f32) / (1.0 + df)).ln();
        let mut tfw = 0.0f32;
        for index in 0..4 {
            let count = fields[index]
                .iter()
                .filter(|token| token.as_str() == keyword)
                .count() as f32;
            tfw += weights[index] * count;
        }
        score += idf * tfw;
    }
    score /= 1.0 + (doc_len as f32 / corpus.avg_len);

    let phrase = phrase_match(&name_tokens, query_ordered);
    if phrase {
        score += PHRASE_BONUS;
    }
    (score, phrase)
}

/// Two or more consecutive query words in order inside the entry's name.
fn phrase_match(name_tokens: &[String], query_ordered: &[String]) -> bool {
    query_ordered.len() >= 2
        && name_tokens.len() >= 2
        && query_ordered
            .windows(2)
            .any(|pair| name_tokens.windows(2).any(|window| window == pair))
}

/// Match every entry against `query`, ordered by relevance: TF-IDF score first,
/// then phrase, tier, coverage, the experience score, then `key`. (The hint
/// still orders by confidence — this richer order is for `search`, where
/// relevance beats evidence.)
fn rank_keyword_matches(ranked: Vec<ScoredEntry>, query: &str) -> Vec<(KeywordMatch, ScoredEntry)> {
    let keywords = hint_keywords(query);
    let query_ordered = tokenize_multi(query);
    if keywords.is_empty() {
        return Vec::new();
    }
    let corpus = corpus_stats(&ranked);
    let mut matches: Vec<(KeywordMatch, ScoredEntry)> = ranked
        .into_iter()
        .filter_map(|scored| {
            let mut hit = keyword_match(&scored.views.fields, &keywords)?;
            let (score, phrase) = relevance(&scored, &keywords, &query_ordered, &corpus);
            hit.score = score;
            hit.phrase = phrase;
            Some((hit, scored))
        })
        .collect();
    matches.sort_by(|(left_hit, left), (right_hit, right)| {
        right_hit
            .score
            .partial_cmp(&left_hit.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right_hit.phrase.cmp(&left_hit.phrase))
            .then_with(|| right_hit.tier.cmp(&left_hit.tier))
            .then_with(|| right_hit.hits.cmp(&left_hit.hits))
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.entry.meta.key.cmp(&right.entry.meta.key))
    });
    matches
}
///
/// The tier and the coverage are printed because they *are* the order: a name
/// hit means the memory is about the question, a body hit only that the words
/// appear somewhere in it.
fn render_search_matches(matches: &[(KeywordMatch, ScoredEntry)]) -> String {
    let mut text = String::new();
    for (hit, scored) in matches.iter().take(EVOLUTION_SEARCH_MAX_MATCHES) {
        let summary = one_line(&scored.entry.summary, EVOLUTION_SUMMARY_MAX);
        let mut label = hit.tier_label().to_string();
        if hit.phrase {
            label.push_str(", phrase");
        }
        text.push_str(&format!(
            "- {} ({}, c{}, {}, {}{}): {}\n",
            scored.entry.meta.key,
            scored.entry.meta.kind,
            scored.entry.meta.confidence,
            label,
            hit.hits,
            if hit.hits == 1 { " hit" } else { " hits" },
            summary,
        ));
    }
    if matches.len() > EVOLUTION_SEARCH_MAX_MATCHES {
        text.push_str(&format!(
            "+ {} lower-ranked matches — narrow the query\n",
            matches.len() - EVOLUTION_SEARCH_MAX_MATCHES
        ));
    }
    text.trim_end().to_string()
}

/// Render the largest prefix of `ranked` that fits `budget_chars`; `None` when
/// not even one line fits (a header-only block is never injected).
fn render_hint(ranked: &[HintCandidate], budget_chars: usize) -> Option<(String, usize)> {
    let mut placed = 0usize;
    for index in 1..=ranked.len() {
        let omitted = ranked.len() - index;
        if hint_text(&ranked[..index], omitted).chars().count() <= budget_chars {
            placed = index;
        } else {
            break;
        }
    }
    if placed == 0 {
        return None;
    }
    let text = hint_text(&ranked[..placed], ranked.len() - placed);
    Some((text, placed))
}

/// One single-line block: header, then the matched titles joined by ` · `.
///
/// Single line on purpose — it is prefixed to the user's own message, so a
/// multi-line block would read like something the user typed.
fn hint_text(placed: &[HintCandidate], omitted: usize) -> String {
    if placed.is_empty() {
        return String::new();
    }
    let mut header = String::from(MEMORY_HINT_HEADER);
    if omitted > 0 {
        // Tells the model the hint is a sample, not the whole memory.
        header.push_str(&format!(", +{omitted} more"));
    }
    header.push_str("): ");
    let titles: Vec<&str> = placed
        .iter()
        .map(|candidate| candidate.title.as_str())
        .collect();
    format!("{header}{}", titles.join(" · "))
}


// ─── Writes: upsert / merge ──────────────────────────────────────────────────

#[derive(Debug)]
pub struct UpsertOutcome {
    pub entry: EvolutionEntry,
    pub merged_into: Option<String>,
    pub created: bool,
    /// Trust before this call, so the caller can report the delta.
    pub confidence_before: u32,
}

fn find_similar(entries: &[EvolutionEntry], title: &str, tags: &[String]) -> Option<usize> {
    let title_norm = normalize_text(title);
    if title_norm.is_empty() {
        return None;
    }
    let incoming_tokens: HashSet<String> = tokenize(title).into_iter().collect();
    let incoming_tags: HashSet<String> = tags.iter().map(|tag| tag.to_lowercase()).collect();

    let mut best: Option<(f64, usize)> = None;
    for (index, entry) in entries.iter().enumerate() {
        let candidate_title = normalize_text(&entry.meta.title);
        let candidate_tokens: HashSet<String> = tokenize(&entry.meta.title)
            .into_iter()
            .chain(tokenize(&entry.meta.key))
            .collect();
        let candidate_tags: HashSet<String> = entry
            .meta
            .tags
            .iter()
            .map(|tag| tag.to_lowercase())
            .collect();

        let exact_title = !candidate_title.is_empty() && candidate_title == title_norm;
        let token_similarity = jaccard(&incoming_tokens, &candidate_tokens);
        let tag_similarity = jaccard(&incoming_tags, &candidate_tags);
        let similarity = if exact_title {
            1.0
        } else if token_similarity >= 0.6 && tag_similarity >= 0.5 {
            token_similarity + tag_similarity / 2.0
        } else {
            continue;
        };

        if best.map(|(score, _)| similarity > score).unwrap_or(true) {
            best = Some((similarity, index));
        }
    }
    best.map(|(_, index)| index)
}

fn resolve_entry_index(entries: &[EvolutionEntry], key: &str) -> Option<usize> {
    let safe = safe_memory_key(key);
    entries.iter().position(|entry| {
        entry.meta.key == key
            || entry.meta.key == safe
            || entry.meta.aliases.iter().any(|alias| alias == key || alias == &safe)
            || entry
                .path
                .as_ref()
                .and_then(|path| path.file_stem())
                .and_then(|stem| stem.to_str())
                .map(|stem| stem == safe)
                .unwrap_or(false)
    })
}

/// Insert or update an evolution entry.
///
/// The same key is an upsert (evidence += 1). A *similar* title/tag pair with a
/// different key is merged into the existing entry instead of creating a
/// second file — that is what keeps auto-recorded conclusions from piling up.
///
/// A plain re-add also counts as a **confirmation**: it raises `confidence` by
/// one. Use [`upsert_evolution_entry_trusted`] to refute a fact instead.
#[allow(clippy::too_many_arguments)]
pub fn upsert_evolution_entry(
    workspace_dir: &Path,
    key: &str,
    content: &str,
    title: &str,
    kind: &str,
    importance: Option<u32>,
    pinned: bool,
    tags: &[String],
    merge: bool,
) -> Result<UpsertOutcome, String> {
    upsert_evolution_entry_trusted(
        workspace_dir,
        key,
        content,
        title,
        kind,
        importance,
        pinned,
        tags,
        merge,
        TrustUpdate::confirming(),
    )
}

/// How one `add` treats the stored fact: a plain re-add confirms it, a
/// `conflict` refutes it.
#[derive(Debug, Clone, Default)]
pub struct TrustUpdate {
    /// Explicit trust (1–5). It **overwrites** the stored value, which is the
    /// only way trust may fall — `importance` keeps its `max` semantics.
    pub confidence: Option<u32>,
    /// The real result disagreed with the stored fact.
    pub conflict: bool,
    /// One-line reason of the conflict, stored as `last_conflict`.
    pub note: String,
    /// Count a plain re-add as a confirmation (+1 confidence). Off by default:
    /// an automatic recording is not evidence of anything.
    pub confirm: bool,
}

impl TrustUpdate {
    /// A plain re-add: one confirmation, no refutation.
    pub fn confirming() -> Self {
        Self {
            confirm: true,
            ..Self::default()
        }
    }
}

/// Insert or update an entry and calibrate its trust (see [`TrustUpdate`]).
///
/// Why the asymmetry: a fact costs one conflict to lose its credibility but one
/// confirmation to earn a single point of it back. Trust is cheap to lose and
/// expensive to regain — that is what keeps a stale fact from staying loud.
#[allow(clippy::too_many_arguments)]
pub fn upsert_evolution_entry_trusted(
    workspace_dir: &Path,
    key: &str,
    content: &str,
    title: &str,
    kind: &str,
    importance: Option<u32>,
    pinned: bool,
    tags: &[String],
    merge: bool,
    trust: TrustUpdate,
) -> Result<UpsertOutcome, String> {
    upsert_evolution_entry_with_source(
        workspace_dir,
        key,
        content,
        title,
        kind,
        importance,
        pinned,
        tags,
        merge,
        "verified",
        trust,
    )
}

/// Remove one evolution entry by key. Returns `false` when it did not exist.
///
/// Used by the maintenance turn to clear its own resume note after a completed
/// pass; the `memory` tool's `delete` action refuses the same key.
pub fn clear_evolution_entry(workspace_dir: &Path, key: &str) -> bool {
    let dir = evolution_dir(workspace_dir);
    let path = dir.join(format!("{}.md", safe_memory_key(key)));
    fs::remove_file(path).is_ok()
}

#[allow(clippy::too_many_arguments)]
fn upsert_evolution_entry_with_source(
    workspace_dir: &Path,
    key: &str,
    content: &str,
    title: &str,
    kind: &str,
    importance: Option<u32>,
    pinned: bool,
    tags: &[String],
    merge: bool,
    source: &str,
    trust: TrustUpdate,
) -> Result<UpsertOutcome, String> {
    let dir = evolution_dir(workspace_dir);
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

    let existing_entries = load_evolution_entries(workspace_dir);
    let mut merged_into: Option<String> = None;

    let mut index = resolve_entry_index(&existing_entries, key);
    if index.is_none() && merge && !title.trim().is_empty() {
        if let Some(found) = find_similar(&existing_entries, title, tags) {
            // Never fold another fact into the maintenance resume note; it is
            // a task note, not knowledge. The maintenance turn writes it by
            // key, so the normal upsert path still works.
            if existing_entries[found].meta.key != MAINTENANCE_PROGRESS_KEY {
                if existing_entries[found].meta.key != key {
                    merged_into = Some(existing_entries[found].meta.key.clone());
                }
                index = Some(found);
            }
        }
    }

    // A conflict must name an entry that exists: refuting something never
    // retrieved would invent a fact at the lowest trust. The maintenance note
    // is a task note, not knowledge, so it has no trust to calibrate.
    if trust.conflict {
        if key == MAINTENANCE_PROGRESS_KEY
            || index
                .map(|found| existing_entries[found].meta.key == MAINTENANCE_PROGRESS_KEY)
                .unwrap_or(false)
        {
            return Err(format!(
                "The '{MAINTENANCE_PROGRESS_KEY}' entry is the maintenance resume note and has no trust to calibrate."
            ));
        }
        if index.is_none() {
            return Err(format!(
                "No evolution memory found for key '{key}': a conflict must target an entry you \
                 retrieved. Record a new fact with a plain add instead."
            ));
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    let (mut entry, created, previous_path) = match index {
        Some(found) => {
            let existing = &existing_entries[found];
            (existing.clone(), false, existing.path.clone())
        }
        None => (
            EvolutionEntry {
                meta: EvolutionMeta {
                    format_version: EVOLUTION_FORMAT_VERSION,
                    key: key.to_string(),
                    title: String::new(),
                    kind: "fact".to_string(),
                    tags: Vec::new(),
                    importance: 3,
                    confidence: 3,
                    contradictions: 0,
                    last_conflict: None,
                    last_conflict_at: None,
                    evidence: 0,
                    uses: 0,
                    pinned: false,
                    source: source.to_string(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    last_used_at: None,
                    aliases: Vec::new(),
                },
                summary: String::new(),
                body: String::new(),
                path: None,
            },
            true,
            None,
        ),
    };

    let confidence_before = entry.meta.confidence.clamp(1, 5);
    if trust.conflict {
        // A conflict is not a confirmation: evidence stays where it was.
        entry.meta.evidence = entry.meta.evidence.max(1);
    } else if !created {
        entry.meta.evidence = entry.meta.evidence.max(1) + 1;
    } else {
        entry.meta.evidence = 1;
    }
    if trust.conflict {
        // Fast to distrust: one contradiction drops the fact to the floor.
        entry.meta.contradictions = entry.meta.contradictions.saturating_add(1);
        let note = trust.note.trim();
        if !note.is_empty() {
            entry.meta.last_conflict =
                Some(truncate_with_ellipsis(note, EVOLUTION_CONFLICT_NOTE_MAX));
        }
        entry.meta.last_conflict_at = Some(now.clone());
        entry.meta.confidence = 1;
    } else if let Some(explicit) = trust.confidence.map(|value| value.clamp(1, 5)) {
        entry.meta.confidence = explicit;
    } else if !created && trust.confirm {
        // Slow to trust: a plain re-add is one confirmation, +1 capped at 5.
        entry.meta.confidence = (confidence_before + 1).clamp(1, 5);
    }
    if let Some(importance) = importance {
        let clamped = importance.clamp(1, 5);
        entry.meta.importance = if created {
            clamped
        } else {
            entry.meta.importance.max(clamped)
        };
    }
    if !title.trim().is_empty() {
        entry.meta.title = title.trim().to_string();
    }
    if !kind.trim().is_empty() {
        entry.meta.kind = kind.trim().to_lowercase();
    }
    entry.meta.pinned |= pinned;
    for tag in tags {
        let tag = tag.trim();
        if !tag.is_empty() && !entry.meta.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            entry.meta.tags.push(tag.to_string());
        }
    }
    if merged_into.is_some()
        && key != entry.meta.key
        && !entry.meta.aliases.iter().any(|alias| alias == key)
    {
        entry.meta.aliases.push(key.to_string());
    }
    if !content.trim().is_empty() {
        entry.body = content.trim().to_string();
    }
    if entry.body.is_empty() {
        entry.body = entry.meta.title.clone();
    }
    if entry.meta.title.trim().is_empty() {
        entry.meta.title = first_sentence(&entry.body);
    }
    entry.summary = one_line(&entry.body, EVOLUTION_SUMMARY_MAX);
    entry.meta.format_version = EVOLUTION_FORMAT_VERSION;
    entry.meta.updated_at = now.clone();
    if entry.meta.created_at.is_empty() {
        entry.meta.created_at = now;
    }
    if entry.meta.confidence == 0 {
        entry.meta.confidence = 3;
    }

    let target = dir.join(format!("{}.md", safe_memory_key(&entry.meta.key)));
    write_atomic(&target, &evolution_entry_to_md(&entry)).map_err(|error| error.to_string())?;
    entry.path = Some(target.clone());

    // An entry merged into a different key leaves its old file behind; remove
    // it so the same fact does not appear twice.
    if let Some(previous) = previous_path {
        if previous != target && previous.exists() {
            let _ = fs::remove_file(previous);
        }
    }

    Ok(UpsertOutcome {
        entry,
        merged_into,
        created,
        confidence_before,
    })
}

/// Bump the usage counters of the entries a `get` / `search` actually reached.
fn bump_uses(workspace_dir: &Path, keys: &[String]) {
    if keys.is_empty() {
        return;
    }
    let wanted: HashSet<&str> = keys.iter().map(|key| key.as_str()).collect();
    let now = chrono::Utc::now().to_rfc3339();
    for entry in load_evolution_entries(workspace_dir) {
        if !wanted.contains(entry.meta.key.as_str()) {
            continue;
        }
        let Some(path) = entry.path.clone() else {
            continue;
        };
        let mut updated = entry;
        updated.meta.uses = updated.meta.uses.max(1) + 1;
        updated.meta.last_used_at = Some(now.clone());
        let _ = write_atomic(&path, &evolution_entry_to_md(&updated));
    }
}

// ─── Archive lookup ──────────────────────────────────────────────────────────

/// Find an archived entry: returns `(skill name, file path)`.
pub fn find_archived_entry(workspace_dir: &Path, key: &str) -> Option<(String, PathBuf)> {
    let safe = safe_memory_key(key);
    let root = evolution_archive_dir(workspace_dir);
    let entries = fs::read_dir(&root).ok()?;
    for entry in entries.flatten() {
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }
        let candidate = skill_dir.join(format!("{safe}.md"));
        if candidate.is_file() {
            let skill = skill_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("archive")
                .to_string();
            return Some((skill, candidate));
        }
        // Also match by the entry's internal key (the file may be named after
        // an alias added during a merge).
        if let Ok(files) = fs::read_dir(&skill_dir) {
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }
                if let Some(parsed) = parse_evolution_entry(&path) {
                    if parsed.meta.key == key || parsed.meta.aliases.iter().any(|a| a == key) {
                        let skill = skill_dir
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("archive")
                            .to_string();
                        return Some((skill, path));
                    }
                }
            }
        }
    }
    None
}

// ─── Consolidation (memory → skill) ──────────────────────────────────────────

/// One candidate group of memory entries that could become a skill.
#[derive(Debug, Clone)]
pub struct EvolutionCluster {
    pub id: String,
    pub suggested_skill: String,
    pub keys: Vec<String>,
    pub chars: usize,
    /// `(key, title)` in ranking order.
    pub titles: Vec<(String, String)>,
}

fn cluster_similarity(left: &EvolutionEntry, right: &EvolutionEntry) -> bool {
    let left_tags: HashSet<String> = left.meta.tags.iter().map(|t| t.to_lowercase()).collect();
    let right_tags: HashSet<String> = right.meta.tags.iter().map(|t| t.to_lowercase()).collect();
    let tag_similarity = jaccard(&left_tags, &right_tags);
    if tag_similarity >= 0.5 {
        return true;
    }

    let left_tokens: HashSet<String> = tokenize(&format!("{} {}", left.meta.key, left.meta.title))
        .into_iter()
        .collect();
    let right_tokens: HashSet<String> =
        tokenize(&format!("{} {}", right.meta.key, right.meta.title))
            .into_iter()
            .collect();
    if jaccard(&left_tokens, &right_tokens) >= 0.4 {
        return true;
    }

    // Same category plus any shared tag is enough: two "recipe" facts about the
    // same machine belong together even when their wording differs.
    !left_tags.is_disjoint(&right_tags)
        && !left.meta.kind.is_empty()
        && left.meta.kind.eq_ignore_ascii_case(&right.meta.kind)
}

/// Group entries into candidate skill clusters (connected components above the
/// size threshold). Deterministic and LLM-free.
pub fn plan_clusters(ranked: &[ScoredEntry], budget_chars: usize) -> Vec<EvolutionCluster> {
    // The maintenance resume note is not knowledge to consolidate; keep it out
    // of every cluster so `plan`/`apply` can never archive it.
    let ranked: Vec<&ScoredEntry> = ranked
        .iter()
        .filter(|scored| scored.entry.meta.key != MAINTENANCE_PROGRESS_KEY)
        .collect();
    let count = ranked.len();
    if count == 0 {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..count).collect();

    fn find(parent: &mut Vec<usize>, index: usize) -> usize {
        let mut root = index;
        while parent[root] != root {
            root = parent[root];
        }
        let mut current = index;
        while parent[current] != current {
            let next = parent[current];
            parent[current] = root;
            current = next;
        }
        root
    }

    for left in 0..count {
        for right in (left + 1)..count {
            if cluster_similarity(&ranked[left].entry, &ranked[right].entry) {
                let left_root = find(&mut parent, left);
                let right_root = find(&mut parent, right);
                if left_root != right_root {
                    parent[right_root] = left_root;
                }
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..count {
        let root = find(&mut parent, index);
        groups.entry(root).or_default().push(index);
    }
    // Deterministic order: group ids must not depend on hash-map iteration.
    let mut members_list: Vec<Vec<usize>> = groups.into_values().collect();
    members_list.sort_by(|left, right| left[0].cmp(&right[0]));

    let minimum_chars = budget_chars * CONSOLIDATE_MIN_SHARE_PCT / 100;
    let mut clusters: Vec<EvolutionCluster> = Vec::new();
    for members in members_list {
        if members.len() < CONSOLIDATE_MIN_ENTRIES {
            continue;
        }
        let chars: usize = members.iter().map(|&i| ranked[i].entry.chars()).sum();
        if chars < minimum_chars {
            continue;
        }
        let mut tag_counts: HashMap<String, usize> = HashMap::new();
        for &index in &members {
            for tag in &ranked[index].entry.meta.tags {
                *tag_counts.entry(tag.to_lowercase()).or_insert(0) += 1;
            }
        }
        let dominant_tag = tag_counts
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
            .map(|(tag, _)| tag)
            .unwrap_or_default();
        let suggested_skill = if slugify(&dominant_tag, 0).len() >= 3 {
            format!("{}-archive", slugify(&dominant_tag, 0))
        } else {
            "memory-archive".to_string()
        };
        let titles = members
            .iter()
            .map(|&index| {
                (
                    ranked[index].entry.meta.key.clone(),
                    ranked[index].entry.display_title(),
                )
            })
            .collect();
        let keys = members
            .iter()
            .map(|&index| ranked[index].entry.meta.key.clone())
            .collect();
        clusters.push(EvolutionCluster {
            id: String::new(),
            suggested_skill,
            keys,
            chars,
            titles,
        });
    }

    clusters.sort_by(|left, right| right.chars.cmp(&left.chars));
    for (index, cluster) in clusters.iter_mut().enumerate() {
        cluster.id = format!("c{}", index + 1);
    }
    clusters
}

fn plan_report(workspace_dir: &Path) -> String {
    let ranked = ranked_evolution_entries(workspace_dir);
    if ranked.is_empty() {
        return "No evolution memories to consolidate.".to_string();
    }
    let total_chars: usize = ranked.iter().map(|scored| scored.entry.chars()).sum();
    let clusters = plan_clusters(&ranked, EVOLUTION_CONTEXT_BUDGET_CHARS);

    let mut out = format!(
        "Memory consolidation plan — {} entries, {} chars.\n",
        ranked.len(),
        total_chars
    );
    if clusters.is_empty() {
        out.push_str(&format!(
            "No cluster is large enough yet (needs >= {} entries and >= {}% of the {} char prompt budget).\n",
            CONSOLIDATE_MIN_ENTRIES, CONSOLIDATE_MIN_SHARE_PCT, EVOLUTION_CONTEXT_BUDGET_CHARS
        ));
        return out;
    }

    for cluster in &clusters {
        let share = cluster.chars * 100 / EVOLUTION_CONTEXT_BUDGET_CHARS.max(1);
        out.push_str(&format!(
            "\n### Cluster {} — {} entries, {} chars ({}% of budget)\nSuggested skill name: {}\n",
            cluster.id,
            cluster.keys.len(),
            cluster.chars,
            share,
            cluster.suggested_skill
        ));
        let listed = cluster.titles.iter().take(12);
        for (key, title) in listed {
            out.push_str(&format!("- [{}] {}\n", key, title));
        }
        if cluster.titles.len() > 12 {
            out.push_str(&format!(
                "- (+{} more entries)\n",
                cluster.titles.len() - 12
            ));
        }
        out.push_str(&format!(
            "Keys: {}\n",
            cluster.keys.join(", ")
        ));
    }

    out.push_str(
        "\nNext: write a skill that covers the cluster (front matter `source-memory` must list \
         every key), then call `memory` with action `consolidate`, phase `apply`, `keys` and the \
         full `skill_md`. Nothing is removed from memory until that call succeeds.\n",
    );
    truncate_tool_output(out)
}

fn string_array(args: &Value, field: &str) -> Vec<String> {
    args[field]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::trim).filter(|s| !s.is_empty()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Fold memory entries into a skill and archive them.
///
/// Order is deliberate and never changes: parse + validate the skill, write it,
/// enable it, and only then move the memory files. A failure at any earlier
/// step leaves memory untouched.
pub fn apply_consolidation(
    workspace_dir: &Path,
    skill_md: &str,
    keys: &[String],
) -> Result<String, String> {
    if keys.is_empty() {
        return Err("'keys' is required for phase 'apply' — copy them from the plan.".to_string());
    }
    if keys.iter().any(|key| key == MAINTENANCE_PROGRESS_KEY) {
        return Err(format!(
            "The '{MAINTENANCE_PROGRESS_KEY}' entry is the maintenance resume note and cannot be archived. Finish the maintenance pass first."
        ));
    }

    let skill = crate::skills::parse_skill_md(skill_md)?;
    crate::skills::validate_skill_name(&skill.name)?;

    // Never clobber a skill the user authored in the Skills panel: pick another
    // name for the archive (a self-evolution skill is safe to extend).
    if crate::skills::skill_source(workspace_dir, &skill.name).as_deref()
        == Some(crate::skills::SKILL_SOURCE_USER)
    {
        return Err(format!(
            "skill '{}' was authored through the Skills panel; choose a different name for the memory archive.",
            skill.name
        ));
    }

    let missing: Vec<&String> = keys
        .iter()
        .filter(|key| !skill.source_memory.iter().any(|item| item == *key))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "The skill's `source-memory` does not list these keys: {}. Add every key you archive.",
            missing
                .iter()
                .map(|key| key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let ranked = ranked_evolution_entries(workspace_dir);
    let mut to_archive: Vec<EvolutionEntry> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let all_entries: Vec<EvolutionEntry> = ranked
        .iter()
        .map(|scored| scored.entry.clone())
        .collect();
    for key in keys {
        match resolve_entry_index(&all_entries, key) {
            Some(index) => {
                let entry = &ranked[index].entry;
                // A deduplicated entry is one file behind several keys; archive
                // it once.
                if seen.insert(entry.meta.key.clone()) {
                    to_archive.push(entry.clone());
                }
            }
            None => unknown.push(key.clone()),
        }
    }
    if !unknown.is_empty() {
        return Err(format!(
            "These memory keys do not exist (already archived?): {}",
            unknown.join(", ")
        ));
    }

    // 1) The skill must exist before anything is removed from memory.
    crate::skills::write_skill_with_backup(workspace_dir, &skill)?;
    crate::skills::enable_skill_for_workspace(workspace_dir, &skill.name)?;

    // 2) Park the source entries in the archive (still on disk, never injected).
    let archive_dir = evolution_archive_dir(workspace_dir).join(&skill.name);
    fs::create_dir_all(&archive_dir).map_err(|error| error.to_string())?;
    let mut moved = 0usize;
    let mut freed = 0usize;
    for entry in &to_archive {
        let Some(path) = entry.path.clone() else {
            continue;
        };
        let file_name = path
            .file_name()
            .map(|name| name.to_owned())
            .ok_or_else(|| format!("invalid memory path: {}", path.display()))?;
        let target = archive_dir.join(file_name);
        if target.exists() {
            let _ = fs::remove_file(&target);
        }
        fs::rename(&path, &target)
            .map_err(|error| format!("failed to archive {}: {error}", path.display()))?;
        moved += 1;
        freed += entry.chars();
    }

    Ok(format!(
        "Archived {moved} entries ({freed} chars) into skill '{}' — the description is now the only \
         permanent prompt cost; load the skill with `use_skill`, or read its `skill.md` with \
         `file_actions` where `use_skill` is not offered (source-memory lists {} keys).",
        skill.name,
        skill.source_memory.len()
    ))
}

/// Move archived entries back into the hot area.
pub fn restore_consolidation(workspace_dir: &Path, skill_name: &str) -> Result<String, String> {
    crate::skills::validate_skill_name(skill_name)?;
    let skills_root = crate::workspace::skills_root(workspace_dir);
    let resolved = crate::skills::resolve_skill_by_name(&skills_root, skill_name)
        .map_err(|error| format!("skill '{skill_name}' not found: {error}"))?;

    let mut skill = resolved.skill;
    if skill.source_memory.is_empty() {
        return Err(format!(
            "skill '{skill_name}' has no `source-memory` entries to restore."
        ));
    }

    let archive_dir = evolution_archive_dir(workspace_dir).join(&skill.name);
    let hot_dir = evolution_dir(workspace_dir);
    fs::create_dir_all(&hot_dir).map_err(|error| error.to_string())?;

    let mut restored = 0usize;
    let mut still_archived: Vec<String> = Vec::new();
    for key in skill.source_memory.clone() {
        let safe = safe_memory_key(&key);
        let source = archive_dir.join(format!("{safe}.md"));
        let target = hot_dir.join(format!("{safe}.md"));
        if !source.is_file() {
            still_archived.push(key);
            continue;
        }
        if target.exists() {
            // A newer copy was recorded while the archive was parked: keep it
            // and drop the archive duplicate.
            let _ = fs::remove_file(&source);
            restored += 1;
            continue;
        }
        fs::rename(&source, &target)
            .map_err(|error| format!("failed to restore {key}: {error}"))?;
        restored += 1;
    }

    skill.source_memory = still_archived;
    crate::skills::write_skill_with_backup(workspace_dir, &skill)?;

    Ok(format!(
        "Restored {restored} entries from skill '{}' into evolution memory ({} still archived).",
        skill.name,
        skill.source_memory.len()
    ))
}

/// Summary of one archive directory, for the panel and `phase: list`.
#[derive(Debug, Clone, Serialize)]
pub struct ArchivedSkill {
    pub name: String,
    pub entries: usize,
    pub chars: usize,
    pub skill_exists: bool,
}

pub fn collect_archived_skills(workspace_dir: &Path) -> Vec<ArchivedSkill> {
    let root = evolution_archive_dir(workspace_dir);
    let Ok(dirs) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let skills_root = crate::workspace::skills_root(workspace_dir);
    let mut out: Vec<ArchivedSkill> = Vec::new();
    for dir in dirs.flatten() {
        let path = dir.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let mut entries = 0usize;
        let mut chars = 0usize;
        if let Ok(files) = fs::read_dir(&path) {
            for file in files.flatten() {
                let file_path = file.path();
                if file_path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }
                entries += 1;
                if let Some(parsed) = parse_evolution_entry(&file_path) {
                    chars += parsed.chars();
                }
            }
        }
        if entries == 0 {
            continue;
        }
        out.push(ArchivedSkill {
            skill_exists: crate::skills::resolve_skill_by_name(&skills_root, &name).is_ok(),
            name,
            entries,
            chars,
        });
    }
    out.sort_by(|left, right| right.chars.cmp(&left.chars));
    out
}

fn consolidate_report(args: &Value, workspace_dir: &Path) -> String {
    let phase = args["phase"].as_str().unwrap_or("plan");
    match phase {
        "plan" => plan_report(workspace_dir),
        "apply" => {
            let skill_md = args["skill_md"].as_str().unwrap_or("");
            if skill_md.trim().is_empty() {
                return "Error: 'skill_md' is required for phase 'apply'.".to_string();
            }
            let mut keys = string_array(args, "keys");
            if keys.is_empty() {
                if let Some(cluster_id) = args["cluster_id"].as_str() {
                    let ranked = ranked_evolution_entries(workspace_dir);
                    let clusters = plan_clusters(&ranked, EVOLUTION_CONTEXT_BUDGET_CHARS);
                    if let Some(cluster) = clusters.into_iter().find(|c| c.id == cluster_id) {
                        keys = cluster.keys;
                    }
                }
            }
            match apply_consolidation(workspace_dir, skill_md, &keys) {
                Ok(message) => message,
                Err(error) => format!("Error: {error}"),
            }
        }
        "restore" => match args["skill"].as_str() {
            Some(skill) if !skill.trim().is_empty() => {
                match restore_consolidation(workspace_dir, skill.trim()) {
                    Ok(message) => message,
                    Err(error) => format!("Error: {error}"),
                }
            }
            _ => "Error: 'skill' is required for phase 'restore'.".to_string(),
        },
        "list" => {
            let archives = collect_archived_skills(workspace_dir);
            if archives.is_empty() {
                return "No archived memory skills.".to_string();
            }
            let mut out = String::from("Archived memory skills:\n");
            for archive in archives {
                out.push_str(&format!(
                    "- {} ({} entries, {} chars){}\n",
                    archive.name,
                    archive.entries,
                    archive.chars,
                    if archive.skill_exists {
                        ""
                    } else {
                        " — skill file missing"
                    }
                ));
            }
            out
        }
        other => format!(
            "Unknown consolidate phase '{other}'. Supported: plan, apply, restore, list."
        ),
    }
}

// ─── Auto conclusion distillation ────────────────────────────────────────────

/// Distill a final answer into a dense, single-fact entry.
///
/// The fallback recorder used to dump the whole answer, which is what made the
/// evolution area lose density. This keeps the first paragraph plus up to three
/// bullets, capped — a summary, never a transcript.
pub fn distill_conclusion(answer: &str) -> (String, String) {
    let mut paragraph: Vec<&str> = Vec::new();
    for line in answer.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            continue;
        }
        paragraph.push(trimmed);
        if paragraph.len() >= 4 {
            break;
        }
    }

    let mut bullets: Vec<&str> = Vec::new();
    for line in answer.lines() {
        let trimmed = line.trim();
        let is_bullet = trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("• ");
        if is_bullet {
            bullets.push(
                trimmed
                    .trim_start_matches(|c: char| matches!(c, '-' | '*' | '•'))
                    .trim(),
            );
            if bullets.len() >= 3 {
                break;
            }
        }
    }

    let mut body = paragraph.join(" ");
    if !bullets.is_empty() {
        let mut bullet_text = String::new();
        for bullet in bullets {
            if bullet.is_empty() {
                continue;
            }
            if !bullet_text.is_empty() {
                bullet_text.push_str("; ");
            }
            bullet_text.push_str(bullet);
        }
        if !bullet_text.is_empty() && !body.contains(&bullet_text) {
            if !body.is_empty() {
                body.push_str(" — ");
            }
            body.push_str(&bullet_text);
        }
    }

    let body = truncate_with_ellipsis(body.trim(), 320);
    let title = first_sentence(&body);
    (title, body)
}

/// Persist one automatically distilled conclusion into the evolution area.
///
/// Returns the entry key when something was written. Duplicates only bump the
/// evidence count, and the lowest-scoring automatic entries are pruned once the
/// cap is exceeded — automatic notes can never crowd out verified facts.
pub fn record_auto_conclusion(workspace_dir: &Path, _session_id: &str, answer: &str) -> Option<String> {
    let (title, body) = distill_conclusion(answer);
    if body.chars().count() < 40 {
        return None;
    }
    let slug = slugify(&title, 0);
    let key = if slug.chars().count() >= 4 {
        format!("auto-{}", truncate_with_ellipsis(&slug, 60))
    } else {
        format!("auto-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    };

    let outcome = upsert_evolution_entry_with_source(
        workspace_dir,
        &key,
        &body,
        &title,
        "fact",
        Some(2),
        false,
        &[],
        true,
        "auto",
        TrustUpdate::default(),
    )
    .ok()?;

    prune_auto_entries(workspace_dir);
    Some(outcome.entry.meta.key)
}

/// Keep only the highest-ranked `EVOLUTION_AUTO_LIMIT` automatic entries.
fn prune_auto_entries(workspace_dir: &Path) {
    let ranked = ranked_evolution_entries(workspace_dir);
    let auto: Vec<&ScoredEntry> = ranked
        .iter()
        .filter(|scored| scored.entry.meta.source == "auto")
        .collect();
    if auto.len() <= EVOLUTION_AUTO_LIMIT {
        return;
    }
    for scored in auto.iter().skip(EVOLUTION_AUTO_LIMIT) {
        if let Some(path) = &scored.entry.path {
            let _ = fs::remove_file(path);
        }
    }
}

/// Does the user explicitly ask for a memory tidy-up?
///
/// Multilingual on purpose: the instruction that drives consolidation is a
/// user sentence, and the model should not have to guess the trigger.
pub fn wants_memory_consolidation(text: &str) -> bool {
    let lower = text.to_lowercase();
    const PHRASES: &[&str] = &[
        "整理记忆",
        "整理記憶",
        "归档记忆",
        "歸檔記憶",
        "沉淀记忆",
        "consolidate memory",
        "consolidate the memory",
        "tidy memory",
        "tidy up memory",
        "organize memory",
        "organise memory",
        "archive memory",
        "memory cleanup",
    ];
    PHRASES.iter().any(|phrase| lower.contains(phrase))
}

// ─── Nap sweep: forget noise, merge duplicates ───────────────────────────────

/// How many noise candidates the maintenance inventory lists. The sweep judges
/// every entry itself; these caps only bound the prompt.
pub const MAX_NOISE_CANDIDATES: usize = 40;
/// How many duplicate groups the maintenance inventory lists per pass.
pub const MAX_DUPLICATE_GROUPS: usize = 12;
/// A group larger than this is a consolidation cluster, not a merge.
const MAX_DUPLICATE_GROUP_SIZE: usize = 4;
const SWEEP_TITLE_CLIP: usize = 90;
const SWEEP_BODY_CLIP: usize = 120;

/// Name fragments that make a note a status report instead of a fact.
///
/// A fact names its subject (`python-default`, `cargo-test-exe-locked`); a
/// progress note names what just happened (`报告已生成`, `完成task1`). Chinese
/// matches by substring, Latin by whole word (see `latin_words`), so
/// `autocomplete` never matches `complete`.
const NOISE_PHRASES_CJK: &[&str] = &[
    "已生成",
    "已写好",
    "已写完",
    "已改好",
    "已改完",
    "已完成",
    "已提交",
    "已推送",
    "已发布",
    "已修复",
    "已合并",
    "改完了",
    "修好了",
    "跑通了",
    "测试全绿",
    "验证全绿",
    "全部通过",
    "都通过了",
    "看完了",
    "看过了",
    "查过了",
    "查清了",
    "定位到了",
    "读完了",
    "核对完毕",
    "当前进度",
    "当前状态",
    "报告已",
    "完成task",
    "完成 task",
    "任务完成",
    "收尾完成",
    "下一步",
    "待办事项",
];
const NOISE_PHRASES_LATIN: &[&str] = &[
    "next step",
    "next up",
    "all green",
    "task complete",
    "task done",
    "tests pass",
    "report written",
];
const NOISE_WORDS_LATIN: &[&str] = &["done", "finished", "completed", "todo", "wip"];

fn latin_words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && word.is_ascii())
        .map(str::to_string)
        .collect()
}

/// Why a note looks like a progress report (`None` = it may be a fact).
///
/// `name` is the note's title (the `# key` heading for a `user` / `repo` note);
/// `alias` is the sanitized file key, so a note whose heading was lost still
/// matches. A caller that only has a body (a session note) passes it as
/// `alias`.
pub fn noise_reason(name: &str, alias: &str) -> Option<&'static str> {
    let haystack = format!("{name} {alias}").to_lowercase();
    for phrase in NOISE_PHRASES_CJK {
        if haystack.contains(phrase) {
            return Some(phrase);
        }
    }
    for phrase in NOISE_PHRASES_LATIN {
        if haystack.contains(phrase) {
            return Some(phrase);
        }
    }
    let words = latin_words(&haystack);
    NOISE_WORDS_LATIN
        .iter()
        .copied()
        .find(|word| words.contains(*word))
}

/// One memory entry the nap may forget, with the reason it was flagged.
#[derive(Debug, Clone)]
pub struct NoiseCandidate {
    /// `evolution` | `user` | `repo`.
    pub scope: &'static str,
    pub key: String,
    pub title: String,
    pub body: String,
    /// The progress phrase that made it a candidate.
    pub reason: String,
}

/// One set of entries that state the same thing under different keys.
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub id: String,
    /// `(scope, key, title)` in listing order.
    pub entries: Vec<(String, String, String)>,
    /// The subject words every member shares.
    pub subject: String,
}

/// True when deleting this evolution entry cannot destroy reusable knowledge.
///
/// Every guard here is about evidence, not wording: a fact that was confirmed,
/// retrieved, pinned, marked as a recipe/pitfall/location, or written by the
/// user outranks a progress report and is never offered to the sweep.
fn forgettable(entry: &EvolutionEntry) -> bool {
    let meta = &entry.meta;
    meta.key != MAINTENANCE_PROGRESS_KEY
        && !meta.pinned
        && meta.source != "user"
        && meta.importance < 4
        && meta.confidence < 4
        && meta.evidence < 3
        // Three retrievals prove a fact earned its place; one or two are what a
        // search over a noisy scope looks like, so they do not shield an entry.
        && meta.uses < 3
        && (meta.kind.is_empty() || meta.kind.eq_ignore_ascii_case("fact"))
}

/// Evolution and file-backed notes whose name reports progress.
///
/// Strong candidates (the name matches a progress phrase) come first, then the
/// weak ones, each ordered by scope and key and capped at
/// `MAX_NOISE_CANDIDATES`. The list is a hint, not a verdict: the maintenance
/// pass still reads each entry before it deletes anything.
pub fn noise_candidates(workspace_dir: &Path) -> Vec<NoiseCandidate> {
    // `0` = the name matches a progress phrase (strong), `1` = an automatic
    // note the recorder wrote at the lowest importance (weak, judge it
    // yourself). Strong candidates are listed first: when the cap cuts the
    // list short, the obvious noise must be the part that survives.
    let mut out: Vec<(u8, NoiseCandidate)> = Vec::new();

    for scored in ranked_evolution_entries(workspace_dir) {
        let entry = &scored.entry;
        if !forgettable(entry) {
            continue;
        }
        let title = entry.display_title();
        let (rank, reason) = match noise_reason(&title, &entry.meta.key) {
            Some(reason) => (0u8, reason.to_string()),
            // An `auto` note is a distilled conclusion the recorder wrote on
            // its own at importance 2: the pass reads it and keeps it only when
            // it holds something a later turn can reuse.
            None if entry.meta.source == "auto" && entry.meta.importance <= 2 => (
                1,
                "auto note at importance 2 (no phrase match)".to_string(),
            ),
            None => continue,
        };
        out.push((
            rank,
            NoiseCandidate {
                scope: EVOLUTION_SCOPE,
                key: entry.meta.key.clone(),
                title: truncate_with_ellipsis(&title, SWEEP_TITLE_CLIP),
                body: one_line(&entry.body, SWEEP_BODY_CLIP),
                reason,
            },
        ));
    }

    for scope in MEMORY_HINT_SCOPES {
        for path in scope_note_paths(workspace_dir, scope) {
            let Some((name, body, _tags)) = read_scope_note(&path) else {
                continue;
            };
            let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("");
            let Some(reason) = noise_reason(&name, stem) else {
                continue;
            };
            out.push((
                0,
                NoiseCandidate {
                    scope,
                    key: stem.to_string(),
                    title: truncate_with_ellipsis(&name, SWEEP_TITLE_CLIP),
                    body: one_line(&body, SWEEP_BODY_CLIP),
                    reason: reason.to_string(),
                },
            ));
        }
    }

    out.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.scope.cmp(right.1.scope))
            .then_with(|| left.1.key.cmp(&right.1.key))
    });
    out.truncate(MAX_NOISE_CANDIDATES);
    out.into_iter().map(|(_, candidate)| candidate).collect()
}

/// Forget in-memory `session` notes that only report progress.
///
/// The session scope lives in `AppState`, not on disk, and the maintenance turn
/// runs under its own session id — the model can neither see nor delete the
/// user's session notes. This is therefore the one deterministic sweep: the nap
/// drops status lines ("报告已生成", "完成task1") that no later turn can reuse.
/// Returns `<session>:<key>` for every entry removed.
pub fn prune_session_noise(state: &crate::AppState) -> Vec<String> {
    let mut removed: Vec<String> = Vec::new();
    let mut memories = state.session_memories.lock().unwrap();
    for (session_id, entries) in memories.iter_mut() {
        entries.retain(|key, value| {
            let content = value
                .get("content")
                .and_then(|content| content.as_str())
                .unwrap_or("");
            if noise_reason(key, content).is_some() {
                removed.push(format!("{session_id}:{key}"));
                false
            } else {
                true
            }
        });
    }
    removed
}

/// One note as the merge sweep sees it: identity plus the token views that
/// decide whether two notes state the same fact.
struct SweepNote {
    scope: &'static str,
    key: String,
    title: String,
    /// `key` + `title` + tags — what the note is *about*.
    name_tokens: HashSet<String>,
    /// The whole note, body included.
    tokens: HashSet<String>,
}

fn sweep_notes(workspace_dir: &Path) -> Vec<SweepNote> {
    let mut notes: Vec<SweepNote> = Vec::new();

    for scored in ranked_evolution_entries(workspace_dir) {
        let entry = &scored.entry;
        if entry.meta.key == MAINTENANCE_PROGRESS_KEY {
            continue;
        }
        // A deletion target is not a merge target.
        if forgettable(entry) && noise_reason(&entry.display_title(), &entry.meta.key).is_some() {
            continue;
        }
        notes.push(SweepNote {
            scope: EVOLUTION_SCOPE,
            key: entry.meta.key.clone(),
            title: truncate_with_ellipsis(&entry.display_title(), SWEEP_TITLE_CLIP),
            name_tokens: tokenize(&format!(
                "{} {} {}",
                entry.meta.key,
                entry.meta.title,
                entry.meta.tags.join(" ")
            ))
            .into_iter()
            .collect(),
            tokens: tokenize(&format!("{} {}", entry.meta.title, entry.body))
                .into_iter()
                .collect(),
        });
    }

    for scope in MEMORY_HINT_SCOPES {
        for path in scope_note_paths(workspace_dir, scope) {
            let Some((name, body, tags)) = read_scope_note(&path) else {
                continue;
            };
            let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("");
            if noise_reason(&name, stem).is_some() {
                continue;
            }
            notes.push(SweepNote {
                scope,
                key: stem.to_string(),
                title: truncate_with_ellipsis(&name, SWEEP_TITLE_CLIP),
                name_tokens: tokenize(&format!("{name} {}", tags.join(" ")))
                    .into_iter()
                    .collect(),
                tokens: tokenize(&format!("{name} {body}")).into_iter().collect(),
            });
        }
    }

    notes
}

/// Share of the smaller set that both sides contain (1.0 = one is a subset).
fn overlap(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    let smaller = left.len().min(right.len());
    if smaller == 0 {
        return 0.0;
    }
    left.intersection(right).count() as f64 / smaller as f64
}

/// Do two notes state the same fact? Two notes about the same subject with a
/// partly shared body qualify; two notes that merely share one word do not.
fn same_fact(left: &SweepNote, right: &SweepNote) -> bool {
    if left.key == right.key && left.scope == right.scope {
        return false;
    }
    let name = overlap(&left.name_tokens, &right.name_tokens);
    let body = overlap(&left.tokens, &right.tokens);
    name >= 0.45 || body >= 0.5 || (name >= 0.3 && body >= 0.25)
}

fn shared_subject(notes: &[&SweepNote]) -> String {
    let Some(first) = notes.first() else {
        return String::new();
    };
    let mut shared: Vec<String> = first.name_tokens.iter().cloned().collect();
    for note in notes.iter().skip(1) {
        shared.retain(|token| note.name_tokens.contains(token));
    }
    shared.sort();
    if shared.is_empty() {
        return first.title.clone();
    }
    shared.truncate(4);
    shared.join(" ")
}

/// Groups of notes that say the same thing under different keys, largest
/// first. Deterministic and LLM-free: the maintenance pass reads each group and
/// decides whether the members really are one fact.
pub fn duplicate_groups(workspace_dir: &Path) -> Vec<DuplicateGroup> {
    let notes = sweep_notes(workspace_dir);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, note) in notes.iter().enumerate() {
        if note.tokens.is_empty() && note.name_tokens.is_empty() {
            continue;
        }
        let mut placed = false;
        for group in groups.iter_mut() {
            if group.len() >= MAX_DUPLICATE_GROUP_SIZE {
                continue;
            }
            if group.iter().any(|&member| same_fact(&notes[member], note)) {
                group.push(index);
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push(vec![index]);
        }
    }

    let mut out: Vec<DuplicateGroup> = Vec::new();
    for (number, group) in groups.into_iter().enumerate() {
        if group.len() < 2 {
            continue;
        }
        let members: Vec<&SweepNote> = group.iter().map(|&index| &notes[index]).collect();
        out.push(DuplicateGroup {
            id: format!("dup-{}", number + 1),
            subject: shared_subject(&members),
            entries: members
                .iter()
                .map(|note| (note.scope.to_string(), note.key.clone(), note.title.clone()))
                .collect(),
        });
    }
    out.sort_by(|left, right| {
        right
            .entries
            .len()
            .cmp(&left.entries.len())
            .then_with(|| left.entries[0].1.cmp(&right.entries[0].1))
    });
    out.truncate(MAX_DUPLICATE_GROUPS);
    out
}

// ─── Memory tool handler ─────────────────────────────────────────────────────

/// Handle the `memory` tool call.
///
/// `scope: "all"` re-runs this same tool once per concrete scope and merges
/// the results, so `all` behaves exactly like three separate calls. Mutating
/// actions deliberately require one concrete scope.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_memory_tool(
    app: &AppHandle,
    args: &Value,
    session_id: Option<&str>,
    workspace_dir: &Path,
    config: &crate::AppConfig,
    allowed_commands: &[String],
    protected_skill_root: Option<&Path>,
    mission_id: Option<&str>,
    active_skill_dirs: &[(String, PathBuf)],
    // `tool_call.id` of the call being executed (see `emit_tool_progress`).
    call_id: Option<&str>,
) -> String {
    let action = args["action"].as_str().unwrap_or("");
    let scope = args["scope"].as_str().unwrap_or("session");
    let key = args["key"].as_str().unwrap_or("").to_string();
    let content = args["content"].as_str().unwrap_or("").to_string();
    let query = args["query"].as_str().unwrap_or("").to_string();
    let tags: Vec<String> = args["tags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // The evolution scope only exists while self-evolution mode is on.
    if scope == EVOLUTION_SCOPE && !config.self_evolution_mode {
        return format!(
            "Error: the '{}' memory scope is only available while self-evolution mode is on.",
            EVOLUTION_SCOPE
        );
    }
    if action == "consolidate" && scope != EVOLUTION_SCOPE {
        return "Error: memory consolidate requires scope 'evolution'.".to_string();
    }

    // ── All scopes (read-only fan-out) ──────────────────────────────
    // `scope: "all"` re-runs this same tool once per concrete scope and
    // merges the results, so `all` behaves exactly like three separate
    // calls. Mutating actions deliberately require one concrete scope.
    if scope == "all" {
        if !matches!(action, "get" | "list" | "search") {
            if matches!(action, "add" | "delete") {
                return format!(
                    "Error: memory scope 'all' is read-only (get, list, search). Call the memory tool again with scope 'session', 'user', 'repo'{} to {}.",
                    if config.self_evolution_mode {
                        ", or 'evolution'"
                    } else {
                        ""
                    },
                    if action == "add" {
                        "add a memory"
                    } else {
                        "delete a memory"
                    }
                );
            }
            return format!(
                "Unknown memory action '{}'. Supported: add, get, list, search, delete, consolidate.",
                action
            );
        }
        if action == "search" && query.is_empty() {
            return "Error: 'query' is required for memory search.".to_string();
        }
        if action == "get" && key.is_empty() {
            return "Error: 'key' is required for memory get.".to_string();
        }

        let _ = emit_tool_progress(
            app,
            mission_id,
            session_id,
            call_id,
            format!("*Memory {} across all scopes*\n", action),
        );

        let mut sections: Vec<String> = Vec::new();
        for sub_scope in concrete_scopes(config.self_evolution_mode) {
            let mut sub_args = args.clone();
            sub_args["scope"] = json!(sub_scope);
            let sub_args_str = sub_args.to_string();
            let result = Box::pin(super::execute_tool(
                app,
                "memory",
                &sub_args_str,
                workspace_dir.to_path_buf(),
                config,
                allowed_commands,
                protected_skill_root,
                mission_id,
                session_id,
                call_id,
                active_skill_dirs,
                None,
                None,
            ))
            .await;
            sections.push(format!("### {}\n{}", sub_scope, result));
        }

        let merged = format!(
            "Memory {} across all scopes:\n\n{}",
            action,
            sections.join("\n\n")
        );
        return truncate_tool_output(merged);
    }

    // ── Session-scoped (in-memory) ──────────────────────────────────
    if scope == "session" {
        let session_id = session_id.unwrap_or("default");
        let state = app.state::<crate::AppState>();
        let mut memories = state.session_memories.lock().unwrap();
        let session_mem = memories.entry(session_id.to_string()).or_default();

        match action {
            "add" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory add.".to_string();
                }
                let entry = serde_json::json!({
                    "key": key,
                    "content": content,
                    "tags": tags,
                    "scope": "session",
                });
                session_mem.insert(key.clone(), entry.clone());
                let _ = emit_tool_progress(
                    app,
                    mission_id,
                    session_id,
                    call_id,
                    format!("*Session memory stored: {}*\n", key),
                );
                serde_json::to_string_pretty(&entry).unwrap_or_else(|_| entry.to_string())
            }
            "get" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory get.".to_string();
                }
                match session_mem.get(&key) {
                    Some(entry) => {
                        let _ = emit_tool_progress(
                            app,
                            mission_id,
                            session_id,
                            call_id,
                            format!("*Session memory retrieved: {}*\n", key),
                        );
                        serde_json::to_string_pretty(entry).unwrap_or_else(|_| entry.to_string())
                    }
                    None => format!("No session memory found for key '{}'.", key),
                }
            }
            "list" => {
                if session_mem.is_empty() {
                    return "(no session memories)".to_string();
                }
                let keys: Vec<&String> = session_mem.keys().collect();
                let _ = emit_tool_progress(
                    app,
                    mission_id,
                    session_id,
                    call_id,
                    format!("*Listing {} session memories*\n", keys.len()),
                );
                serde_json::to_string_pretty(&json!({
                    "scope": "session",
                    "keys": keys,
                    "count": keys.len(),
                }))
                .unwrap_or_else(|_| "[]".to_string())
            }
            "search" => {
                if query.is_empty() {
                    return "Error: 'query' is required for memory search.".to_string();
                }

                // ── In-memory search ──
                let query_lower = query.to_lowercase();
                let in_memory_matches: Vec<&serde_json::Value> = session_mem
                    .values()
                    .filter(|entry| {
                        let key_match = entry["key"]
                            .as_str()
                            .map(|k| k.to_lowercase().contains(&query_lower))
                            .unwrap_or(false);
                        let content_match = entry["content"]
                            .as_str()
                            .map(|c| c.to_lowercase().contains(&query_lower))
                            .unwrap_or(false);
                        key_match || content_match
                    })
                    .collect();

                // ── DB history search across past sessions ──
                let db_guard = state.db.lock().unwrap();
                let history_matches =
                    db::search_history_messages(&db_guard, session_id, &query, 20);
                let summary_matches =
                    db::search_session_summaries(&db_guard, session_id, &query, 5);
                drop(db_guard);

                let _ = emit_tool_progress(
                    app,
                    mission_id,
                    session_id,
                    call_id,
                    format!("*Searching session memories for '{}'*\n", query),
                );

                let has_in_memory = !in_memory_matches.is_empty();
                let has_history = history_matches
                    .as_ref()
                    .map(|h| !h.is_empty())
                    .unwrap_or(false);
                let has_summaries = summary_matches
                    .as_ref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);

                if !has_in_memory && !has_history && !has_summaries {
                    format!(
                        "No session memories or past conversations match '{}'.",
                        query
                    )
                } else {
                    let mut result = json!({
                        "scope": "session",
                        "query": query,
                        "in_memory_matches": in_memory_matches,
                        "in_memory_count": in_memory_matches.len(),
                    });
                    if let Ok(hist) = history_matches {
                        result["session_history_matches"] =
                            serde_json::to_value(&hist).unwrap_or_default();
                        result["session_history_count"] = json!(hist.len());
                    }
                    if let Ok(sum) = summary_matches {
                        result["session_summaries"] =
                            serde_json::to_value(&sum).unwrap_or_default();
                        result["session_summaries_count"] = json!(sum.len());
                    }
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
                }
            }
            "delete" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory delete.".to_string();
                }
                match session_mem.remove(&key) {
                    Some(_) => {
                        let _ = emit_tool_progress(
                            app,
                            mission_id,
                            session_id,
                            call_id,
                            format!("*Session memory deleted: {}*\n", key),
                        );
                        format!("Session memory '{}' deleted.", key)
                    }
                    None => format!("No session memory found for key '{}'.", key),
                }
            }
            _ => format!(
                "Unknown memory action '{}'. Supported: add, get, list, search, delete.",
                action
            ),
        }
    // ── Evolution scoped (structured, ranked, archive-aware) ────────
    } else if scope == EVOLUTION_SCOPE {
        handle_evolution_scope(
            app,
            args,
            action,
            &key,
            &content,
            &query,
            &tags,
            workspace_dir,
            mission_id,
            session_id,
            call_id,
        )
    // ── User / Repo scoped (file-backed) ─────────────────────────────
    } else if scope == "user" || scope == "repo" {
        let memory_root = crate::workspace::memory_root(workspace_dir).join(scope);
        let _ = fs::create_dir_all(&memory_root);

        match action {
            "add" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory add.".to_string();
                }
                // Sanitize key for use as filename
                let safe_key = safe_memory_key(&key);
                let file_path = memory_root.join(format!("{}.md", safe_key));

                let tags_line = if tags.is_empty() {
                    String::new()
                } else {
                    format!("\ntags: {}\n", tags.join(", "))
                };

                let file_content = format!("# {key}\n\n{content}{tags_line}\n");

                match fs::write(&file_path, &file_content) {
                    Ok(_) => {
                        let _ = emit_tool_progress(
                            app,
                            mission_id,
                            session_id,
                            call_id,
                            format!("*{} memory stored: {}*\n", scope, key),
                        );
                        let entry = json!({
                            "key": key,
                            "content": content,
                            "tags": tags,
                            "scope": scope,
                            "file": file_path.to_string_lossy(),
                        });
                        serde_json::to_string_pretty(&entry).unwrap_or_else(|_| entry.to_string())
                    }
                    Err(e) => format!("Error writing memory file: {}", e),
                }
            }
            "get" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory get.".to_string();
                }
                let safe_key = safe_memory_key(&key);
                let file_path = memory_root.join(format!("{}.md", safe_key));

                match fs::read_to_string(&file_path) {
                    Ok(content) => {
                        let _ = emit_tool_progress(
                            app,
                            mission_id,
                            session_id,
                            call_id,
                            format!("*{} memory retrieved: {}*\n", scope, key),
                        );
                        content
                    }
                    Err(_) => format!("No {} memory found for key '{}'.", scope, key),
                }
            }
            "list" => match fs::read_dir(&memory_root) {
                Ok(entries) => {
                    let mut items: Vec<serde_json::Value> = Vec::new();
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".md") {
                            let display_key = name.trim_end_matches(".md").to_string();
                            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                            items.push(json!({
                                "key": display_key,
                                "file": name,
                                "size_bytes": size,
                            }));
                        }
                    }
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        session_id,
                        call_id,
                        format!("*Listing {} {} memories*\n", items.len(), scope),
                    );
                    if items.is_empty() {
                        format!("(no {} memories)", scope)
                    } else {
                        serde_json::to_string_pretty(&json!({
                            "scope": scope,
                            "entries": items,
                            "count": items.len(),
                        }))
                        .unwrap_or_else(|_| "[]".to_string())
                    }
                }
                Err(e) => format!("Error listing memory directory: {}", e),
            },
            "search" => {
                if query.is_empty() {
                    return "Error: 'query' is required for memory search.".to_string();
                }
                let _ = emit_tool_progress(
                    app,
                    mission_id,
                    session_id,
                    call_id,
                    format!("*Searching {} memories for '{}'*\n", scope, query),
                );
                match run_integrated_search(
                    &query,
                    &memory_root,
                    workspace_dir,
                    &SearchOptions {
                        include_hidden: false,
                        glob: Some("**/*.md".to_string()),
                        ..SearchOptions::default()
                    },
                ) {
                    Ok(output) => {
                        if output.is_empty() || output == "(no matches)" {
                            format!("No {} memories match '{}'.", scope, query)
                        } else {
                            output
                        }
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "delete" => {
                if key.is_empty() {
                    return "Error: 'key' is required for memory delete.".to_string();
                }
                let safe_key = safe_memory_key(&key);
                let file_path = memory_root.join(format!("{}.md", safe_key));

                match fs::remove_file(&file_path) {
                    Ok(_) => {
                        let _ = emit_tool_progress(
                            app,
                            mission_id,
                            session_id,
                            call_id,
                            format!("*{} memory deleted: {}*\n", scope, key),
                        );
                        format!("{} memory '{}' deleted.", scope, key)
                    }
                    Err(e) => format!("Error deleting memory '{}': {}", key, e),
                }
            }
            _ => format!(
                "Unknown memory action '{}'. Supported: add, get, list, search, delete.",
                action
            ),
        }
    } else {
        format!(
            "Unknown memory scope '{}'. Supported scopes: session, user, repo, all{}.",
            scope,
            if config.self_evolution_mode {
                ", evolution"
            } else {
                ""
            }
        )
    }
}

/// Confidence at or below which a fact is flagged as low trust, so the model
/// re-verifies it before relying on it again.
const LOW_TRUST_CONFIDENCE: u32 = 2;

/// Warning line for a fact whose trust was calibrated down. `None` while the
/// fact is still trusted.
fn low_trust_hint(entry: &EvolutionEntry) -> Option<String> {
    if entry.meta.confidence > LOW_TRUST_CONFIDENCE {
        return None;
    }
    let mut hint = format!("Low trust: confidence {}/5", entry.meta.confidence.max(1));
    if entry.meta.contradictions > 0 {
        hint.push_str(&format!(", contradicted {}×", entry.meta.contradictions));
    }
    if let Some(reason) = entry.meta.last_conflict.as_deref().filter(|r| !r.is_empty()) {
        hint.push_str(&format!(" — last conflict: {reason}"));
    }
    hint.push_str(
        ". Verify it against the real environment before relying on it, then re-add the fact \
         (`conflict: true` if it is still wrong).",
    );
    Some(hint)
}

#[allow(clippy::too_many_arguments)]
fn handle_evolution_scope(
    app: &AppHandle,
    args: &Value,
    action: &str,
    key: &str,
    content: &str,
    query: &str,
    tags: &[String],
    workspace_dir: &Path,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    call_id: Option<&str>,
) -> String {
    let dir = evolution_dir(workspace_dir);
    let _ = fs::create_dir_all(&dir);

    // One-time upgrade of legacy entries. Idempotent, and never part of the
    // prompt-building path (which must not write).
    let _ = persist_evolution_migration(workspace_dir);

    match action {
        "add" => {
            if key.is_empty() {
                return "Error: 'key' is required for memory add.".to_string();
            }
            let title = args["title"].as_str().unwrap_or("");
            let kind = args["kind"].as_str().unwrap_or("");
            let importance = args["importance"]
                .as_u64()
                .map(|value| value.clamp(1, 5) as u32);
            let pinned = args["pinned"].as_bool().unwrap_or(false);
            let merge = args["merge"].as_bool().unwrap_or(true);
            let trust = TrustUpdate {
                confidence: args["confidence"]
                    .as_u64()
                    .map(|value| value.clamp(1, 5) as u32),
                conflict: args["conflict"].as_bool().unwrap_or(false),
                note: args["note"].as_str().unwrap_or("").to_string(),
                confirm: true,
            };
            let conflicted = trust.conflict;

            match upsert_evolution_entry_trusted(
                workspace_dir,
                key,
                content,
                title,
                kind,
                importance,
                pinned,
                tags,
                merge,
                trust,
            ) {
                Ok(outcome) => {
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        session_id,
                        call_id,
                        format!(
                            "*Evolution memory {}: {}*\n",
                            if conflicted {
                                "contradicted"
                            } else if outcome.created {
                                "stored"
                            } else {
                                "updated"
                            },
                            outcome.entry.meta.key
                        ),
                    );
                    let mut payload = json!({
                        "key": outcome.entry.meta.key,
                        "title": outcome.entry.display_title(),
                        "kind": outcome.entry.meta.kind,
                        "tags": outcome.entry.meta.tags,
                        "importance": outcome.entry.meta.importance,
                        "confidence": outcome.entry.meta.confidence,
                        "confidence_before": outcome.confidence_before,
                        "contradictions": outcome.entry.meta.contradictions,
                        "evidence": outcome.entry.meta.evidence,
                        "scope": EVOLUTION_SCOPE,
                        "created": outcome.created,
                    });
                    if let Some(merged_into) = outcome.merged_into {
                        payload["merged_into"] = json!(merged_into);
                    }
                    if let Some(reason) = outcome
                        .entry
                        .meta
                        .last_conflict
                        .as_deref()
                        .filter(|reason| !reason.is_empty())
                    {
                        payload["last_conflict"] = json!(reason);
                    }
                    let mut text = serde_json::to_string_pretty(&payload)
                        .unwrap_or_else(|_| payload.to_string());
                    if let Some(hint) = low_trust_hint(&outcome.entry) {
                        text.push_str(&format!("\n\n{hint}"));
                    }
                    text
                }
                Err(error) => format!("Error: {error}"),
            }
        }
        "get" => {
            if key.is_empty() {
                return "Error: 'key' is required for memory get.".to_string();
            }
            let ranked = ranked_evolution_entries(workspace_dir);
            if let Some(index) = resolve_entry_index(
                &ranked.iter().map(|scored| scored.entry.clone()).collect::<Vec<_>>(),
                key,
            ) {
                let entry = &ranked[index].entry;
                bump_uses(workspace_dir, &[entry.meta.key.clone()]);
                let _ = emit_tool_progress(
                    app,
                    mission_id,
                    session_id,
                    call_id,
                    format!("*Evolution memory retrieved: {}*\n", entry.meta.key),
                );
                let mut text = evolution_entry_to_md(entry);
                if let Some(hint) = low_trust_hint(entry) {
                    text.push_str(&format!("\n{hint}\n"));
                }
                return text;
            }
            if let Some((skill, path)) = find_archived_entry(workspace_dir, key) {
                return format!(
                    "Memory '{key}' was archived into skill '{skill}'. Load the skill with \
                     `use_skill`, or read its `skill.md` with `file_actions` where `use_skill` is \
                     not offered; the archived file is {}.",
                    path.display()
                );
            }
            format!("No evolution memory found for key '{}'.", key)
        }
        "list" => {
            let ranked = ranked_evolution_entries(workspace_dir);
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!("*Listing {} evolution memories*\n", ranked.len()),
            );
            if ranked.is_empty() {
                return "(no evolution memories)".to_string();
            }
            let entries: Vec<Value> = ranked
                .iter()
                .enumerate()
                .map(|(index, scored)| {
                    json!({
                        "rank": index + 1,
                        "key": scored.entry.meta.key,
                        "title": scored.entry.display_title(),
                        "kind": scored.entry.meta.kind,
                        "tags": scored.entry.meta.tags,
                        "importance": scored.entry.meta.importance,
                        "confidence": scored.entry.meta.confidence,
                        "contradictions": scored.entry.meta.contradictions,
                        "last_conflict": scored.entry.meta.last_conflict,
                        "evidence": scored.entry.meta.evidence,
                        "uses": scored.entry.meta.uses,
                        "pinned": scored.entry.meta.pinned,
                        "source": scored.entry.meta.source,
                        "score": (scored.score * 100.0).round() / 100.0,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&json!({
                "scope": EVOLUTION_SCOPE,
                "count": entries.len(),
                "entries": entries,
                "archived_skills": collect_archived_skills(workspace_dir).len(),
            }))
            .unwrap_or_else(|_| "[]".to_string())
        }
        "search" => {
            if query.is_empty() {
                return "Error: 'query' is required for memory search.".to_string();
            }
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!("*Searching evolution memories for '{}'*\n", query),
            );
            let matches = rank_keyword_matches(ranked_evolution_entries(workspace_dir), query);
            if matches.is_empty() {
                // Nothing matched a keyword (a one-character query, or a phrase
                // that does not tokenize): fall back to a literal sweep so an
                // archived file that `load_evolution_entries` skips is still
                // findable.
                return match run_integrated_search(
                    query,
                    &dir,
                    workspace_dir,
                    &SearchOptions {
                        include_hidden: false,
                        glob: Some("**/*.md".to_string()),
                        ..SearchOptions::default()
                    },
                ) {
                    Ok(output) if output.is_empty() || output == "(no matches)" => {
                        format!("No evolution memories match '{}'.", query)
                    }
                    Ok(output) => output,
                    Err(error) => format!("Error: {error}"),
                };
            }

            // Only a credible hit counts as usage: a stray common word that
            // merely appears somewhere in the prose must not rank the entry up
            // over time (`uses` feeds `score_evolution_entry`).
            let used: Vec<String> = matches
                .iter()
                .filter(|(hit, _)| hit.is_credible())
                .map(|(_, scored)| scored.entry.meta.key.clone())
                .collect();
            bump_uses(workspace_dir, &used);

            let mut text = format!(
                "{} evolution {} for '{}', ordered by hit tier then coverage:\n{}",
                matches.len(),
                if matches.len() == 1 { "match" } else { "matches" },
                query,
                render_search_matches(&matches)
            );
            let low_trust: Vec<String> = matches
                .iter()
                .filter_map(|(_, scored)| {
                    low_trust_hint(&scored.entry)
                        .map(|hint| format!("- {}: {hint}", scored.entry.meta.key))
                })
                .collect();
            if !low_trust.is_empty() {
                text.push_str("\n\nLow-trust matches — re-verify before use:\n");
                text.push_str(&low_trust.join("\n"));
            }
            text
        }
        "delete" => {
            if key.is_empty() {
                return "Error: 'key' is required for memory delete.".to_string();
            }
            if key == MAINTENANCE_PROGRESS_KEY {
                return "The evolution-maintenance progress entry is the resume note and cannot be deleted manually. Run the maintenance again to complete it.".to_string();
            }
            let safe_key = safe_memory_key(key);
            let path = dir.join(format!("{safe_key}.md"));
            match fs::remove_file(&path) {
                Ok(_) => {
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        session_id,
                        call_id,
                        format!("*Evolution memory deleted: {}*\n", key),
                    );
                    format!("Evolution memory '{key}' deleted.")
                }
                Err(_) => {
                    if let Some((skill, _)) = find_archived_entry(workspace_dir, key) {
                        format!(
                            "Evolution memory '{key}' is archived into skill '{skill}'; delete or \
                             edit that skill instead."
                        )
                    } else {
                        format!("No evolution memory found for key '{key}'.")
                    }
                }
            }
        }
        "consolidate" => {
            let report = consolidate_report(args, workspace_dir);
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                "*Consolidating evolution memory*\n".to_string(),
            );
            report
        }
        _ => format!(
            "Unknown memory action '{}'. Supported: add, get, list, search, delete, consolidate.",
            action
        ),
    }
}

// ─── Panel commands ──────────────────────────────────────────────────────────

/// One row of the read-only memory panel.
#[derive(Debug, Clone, Serialize)]
pub struct EvolutionMemoryView {
    pub rank: usize,
    pub key: String,
    pub title: String,
    pub kind: String,
    pub tags: Vec<String>,
    pub importance: u32,
    pub confidence: u32,
    /// Times a real result contradicted this fact (never reset).
    pub contradictions: u32,
    /// One-line reason of the most recent conflict.
    pub last_conflict: Option<String>,
    pub evidence: u32,
    pub uses: u32,
    pub pinned: bool,
    pub source: String,
    pub updated_at: String,
    pub chars: usize,
    pub score: f64,
    pub injected: bool,
}

/// Build the ranked panel view. Uses the same ranking and the same tiered
/// renderer as the prompt, so the panel literally shows what the model sees.
pub fn evolution_memory_view(workspace_dir: &Path) -> Vec<EvolutionMemoryView> {
    let ranked = ranked_evolution_entries(workspace_dir);
    let rendered = render_ranked_entries(&ranked, EVOLUTION_CONTEXT_BUDGET_CHARS);
    let injected: HashSet<&str> = rendered.injected.iter().map(|key| key.as_str()).collect();
    ranked
        .iter()
        .enumerate()
        .map(|(index, scored)| EvolutionMemoryView {
            rank: index + 1,
            key: scored.entry.meta.key.clone(),
            title: scored.entry.display_title(),
            kind: scored.entry.meta.kind.clone(),
            tags: scored.entry.meta.tags.clone(),
            importance: scored.entry.meta.importance,
            confidence: scored.entry.meta.confidence,
            contradictions: scored.entry.meta.contradictions,
            last_conflict: scored.entry.meta.last_conflict.clone(),
            evidence: scored.entry.meta.evidence,
            uses: scored.entry.meta.uses,
            pinned: scored.entry.meta.pinned,
            source: scored.entry.meta.source.clone(),
            updated_at: scored.entry.meta.updated_at.clone(),
            chars: scored.entry.chars(),
            score: (scored.score * 100.0).round() / 100.0,
            injected: injected.contains(scored.entry.meta.key.as_str()),
        })
        .collect()
}

pub(crate) fn list_evolution_memory(state: State<'_, crate::AppState>) -> Vec<EvolutionMemoryView> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let _ = persist_evolution_migration(&workspace_dir);
    evolution_memory_view(&workspace_dir)
}

pub(crate) fn read_evolution_entry(
    state: State<'_, crate::AppState>,
    key: String,
) -> Result<String, String> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let ranked = ranked_evolution_entries(&workspace_dir);
    if let Some(index) = resolve_entry_index(
        &ranked.iter().map(|scored| scored.entry.clone()).collect::<Vec<_>>(),
        &key,
    ) {
        return Ok(evolution_entry_to_md(&ranked[index].entry));
    }
    if let Some((_, path)) = find_archived_entry(&workspace_dir, &key) {
        return fs::read_to_string(&path).map_err(|error| error.to_string());
    }
    Err(format!("No evolution memory found for key '{key}'."))
}

pub(crate) fn list_archived_skills(state: State<'_, crate::AppState>) -> Vec<ArchivedSkill> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    collect_archived_skills(&workspace_dir)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct TempWorkspace {
        dir: PathBuf,
    }

    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "rabbit-chat-studio-memory-test-{tag}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn evolution(&self) -> PathBuf {
            let dir = evolution_dir(&self.dir);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn write(&self, key: &str, content: &str) {
            fs::write(self.evolution().join(format!("{key}.md")), content).unwrap();
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// The parse cache is keyed by `(mtime, len)`: a rewrite that keeps the
    /// length the same is still re-read, so only the mtime can save it.
    #[test]
    fn a_rewritten_entry_is_not_served_from_the_cache() {
        let workspace = TempWorkspace::new("cache-rewrite");
        let first = "---\nformat_version: 2\nkey: python\ntitle: Python\nkind: fact\ntags: []\n---\n\nAlfa1 is the interpreter.\n";
        let second = "---\nformat_version: 2\nkey: python\ntitle: Python\nkind: fact\ntags: []\n---\n\nBeta2 is the interpreter.\n";
        assert_eq!(first.len(), second.len(), "the test needs equal lengths");

        workspace.write("python", first);
        let loaded = load_evolution_entries(&workspace.dir);
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].body.contains("Alfa1"), "{}", loaded[0].body);

        workspace.write("python", second);
        let reloaded = load_evolution_entries(&workspace.dir);
        assert_eq!(reloaded.len(), 1);
        assert!(reloaded[0].body.contains("Beta2"), "{}", reloaded[0].body);
        assert!(!reloaded[0].body.contains("Alfa1"));
    }

    /// A deleted file disappears on the next read instead of being resurrected
    /// from the cache.
    #[test]
    fn a_deleted_entry_leaves_the_cache() {
        let workspace = TempWorkspace::new("cache-delete");
        workspace.write("alpha", "# alpha\n\nAlpha fact.\n");
        workspace.write("beta", "# beta\n\nBeta fact.\n");
        assert_eq!(load_evolution_entries(&workspace.dir).len(), 2);

        fs::remove_file(workspace.evolution().join("alpha.md")).unwrap();
        let remaining = load_evolution_entries(&workspace.dir);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].meta.key, "beta");
    }

    /// The evolution scope only exists while self-evolution mode is on.
    #[test]
    fn the_evolution_scope_follows_the_self_evolution_switch() {
        assert!(!concrete_scopes(false).contains(&EVOLUTION_SCOPE));
        assert!(concrete_scopes(true).contains(&EVOLUTION_SCOPE));
    }

    #[test]
    fn the_fan_out_includes_the_evolution_scope() {
        let scopes = concrete_scopes(true);
        assert_eq!(scopes, vec!["session", "user", "repo", "evolution"]);
        assert_eq!(concrete_scopes(false), vec!["session", "user", "repo"]);
    }

    /// Legacy files (no front matter) keep working and are upgraded once.
    #[test]
    fn legacy_entries_are_parsed_and_migrated() {
        let workspace = TempWorkspace::new("migrate");
        workspace.write(
            "python",
            "# python\n\nUse `python`, not `python3`.\n\ntags: runtime, python\n",
        );

        let entry = parse_evolution_entry(&workspace.evolution().join("python.md")).unwrap();
        assert_eq!(entry.meta.key, "python");
        assert_eq!(entry.meta.tags, vec!["runtime", "python"]);
        assert_eq!(entry.body, "Use `python`, not `python3`.");
        assert_eq!(entry.meta.format_version, 0);

        assert_eq!(persist_evolution_migration(&workspace.dir), 1);
        let migrated = parse_evolution_entry(&workspace.evolution().join("python.md")).unwrap();
        assert_eq!(migrated.meta.format_version, EVOLUTION_FORMAT_VERSION);
        assert_eq!(migrated.body, "Use `python`, not `python3`.");
        // Migration is idempotent.
        assert_eq!(persist_evolution_migration(&workspace.dir), 0);
    }

    /// `conclusion-*` dumps are demoted to cheap automatic notes.
    #[test]
    fn conclusion_dumps_are_demoted_to_auto_notes() {
        let workspace = TempWorkspace::new("conclusion");
        workspace.write(
            "conclusion-session-20260101-000000",
            "# conclusion-session-20260101-000000\n\nA very long final answer that was stored verbatim.\n",
        );
        let entry =
            parse_evolution_entry(&workspace.evolution().join("conclusion-session-20260101-000000.md"))
                .unwrap();
        assert_eq!(entry.meta.source, "auto");
        assert_eq!(entry.meta.importance, 1);
    }

    /// Re-adding a key updates the same file and increments its evidence.
    #[test]
    fn re_adding_a_key_bumps_evidence_without_creating_a_file() {
        let workspace = TempWorkspace::new("upsert");
        let first = upsert_evolution_entry(
            &workspace.dir,
            "python",
            "Use `python`.",
            "Default interpreter",
            "fact",
            Some(3),
            false,
            &["runtime".to_string()],
            true,
        )
        .unwrap();
        assert!(first.created);

        let second = upsert_evolution_entry(
            &workspace.dir,
            "python",
            "Use `python`, not `python3`.",
            "Default interpreter",
            "fact",
            Some(4),
            false,
            &["python".to_string()],
            true,
        )
        .unwrap();
        assert!(!second.created);
        assert_eq!(second.entry.meta.evidence, 2);
        assert_eq!(second.entry.meta.importance, 4);
        assert!(second.entry.meta.tags.contains(&"runtime".to_string()));
        assert!(second.entry.meta.tags.contains(&"python".to_string()));

        let files: Vec<_> = fs::read_dir(workspace.evolution())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        assert_eq!(files.len(), 1);
    }

    /// A near-duplicate title merges into the existing entry instead of
    /// creating a second file (the anti-pile-up rule).
    #[test]
    fn similar_titles_merge_into_the_existing_entry() {
        let workspace = TempWorkspace::new("merge");
        upsert_evolution_entry(
            &workspace.dir,
            "idc-shanghai",
            "跳板机 10.0.0.1",
            "上海机房跳板机",
            "location",
            Some(3),
            false,
            &["idc".to_string()],
            true,
        )
        .unwrap();

        let outcome = upsert_evolution_entry(
            &workspace.dir,
            "idc-shanghai-bastion",
            "跳板机 10.0.0.1，端口 22",
            "上海机房跳板机",
            "location",
            Some(3),
            false,
            &["idc".to_string()],
            true,
        )
        .unwrap();
        assert_eq!(outcome.merged_into.as_deref(), Some("idc-shanghai"));
        assert_eq!(load_evolution_entries(&workspace.dir).len(), 1);
        // The incoming key stays addressable as an alias of the survivor.
        assert!(resolve_entry_index(&load_evolution_entries(&workspace.dir), "idc-shanghai-bastion").is_some());
    }

    /// Pinned entries outrank high-evidence ones, and evidence raises a score.
    #[test]
    fn ranking_prefers_pinned_importance_and_evidence() {
        let workspace = TempWorkspace::new("rank");
        upsert_evolution_entry(
            &workspace.dir,
            "a-plain",
            "plain fact",
            "plain",
            "fact",
            Some(2),
            false,
            &[],
            true,
        )
        .unwrap();
        upsert_evolution_entry(
            &workspace.dir,
            "b-evidence",
            "confirmed fact",
            "confirmed",
            "fact",
            Some(2),
            false,
            &[],
            true,
        )
        .unwrap();
        // Second confirmation raises evidence.
        upsert_evolution_entry(
            &workspace.dir,
            "b-evidence",
            "confirmed fact",
            "confirmed",
            "fact",
            None,
            false,
            &[],
            true,
        )
        .unwrap();
        upsert_evolution_entry(
            &workspace.dir,
            "c-pinned",
            "critical fact",
            "critical",
            "fact",
            Some(1),
            true,
            &[],
            true,
        )
        .unwrap();

        let ranked = ranked_evolution_entries(&workspace.dir);
        assert_eq!(ranked.first().unwrap().entry.meta.key, "c-pinned");
        let evidence_score = ranked
            .iter()
            .find(|scored| scored.entry.meta.key == "b-evidence")
            .unwrap()
            .score;
        let plain_score = ranked
            .iter()
            .find(|scored| scored.entry.meta.key == "a-plain")
            .unwrap()
            .score;
        assert!(evidence_score > plain_score);
    }

    /// Build an in-memory entry whose `updated_at` sits exactly `days_ago`
    /// behind the wall clock (no file involved).
    fn entry_with_age(days_ago: f64) -> EvolutionEntry {
        let stamp =
            (chrono::Utc::now() - chrono::Duration::nanoseconds((days_ago * 86_400.0 * 1e9) as i64))
                .to_rfc3339();
        EvolutionEntry {
            meta: EvolutionMeta {
                key: "age-test".to_string(),
                title: "age test".to_string(),
                kind: "fact".to_string(),
                importance: 2,
                confidence: 3,
                evidence: 2,
                uses: 4,
                pinned: false,
                source: "verified".to_string(),
                created_at: stamp.clone(),
                updated_at: stamp,
                ..Default::default()
            },
            summary: "age test summary".to_string(),
            body: "age test body".to_string(),
            path: None,
        }
    }

    /// The recency term is bucketed to whole days: two entries whose exact
    /// ages fall into the same day bucket score identically, so the injected
    /// system-prompt render cannot drift within a day (prompt-cache
    /// stability). Crossing a bucket boundary still drops the term.
    #[test]
    fn recency_score_is_bucketed_to_whole_days() {
        // A quarter of a day apart, both floor into "day 0" — identical score.
        let early_today = entry_with_age(0.05);
        let late_today = entry_with_age(0.90);
        assert_eq!(
            score_evolution_entry(&early_today),
            score_evolution_entry(&late_today)
        );
        // One bucket older: the exp term falls, the score drops.
        let yesterday = entry_with_age(1.10);
        assert!(
            score_evolution_entry(&yesterday) < score_evolution_entry(&early_today),
            "an older day bucket must score lower"
        );
    }

    /// The budget is a hard cap, the tail line accounts for what was dropped,
    /// and two renders are byte-identical (prompt-cache stability).
    #[test]
    fn rendering_respects_the_budget_and_is_deterministic() {
        let workspace = TempWorkspace::new("render");
        for index in 0..12 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("fact-{index:02}"),
                &format!("This is a sufficiently long fact number {index} with details that fill the line."),
                &format!("Fact {index}"),
                "fact",
                Some(3),
                false,
                &[],
                true,
            )
            .unwrap();
        }

        let first = render_evolution_context(&workspace.dir, 600);
        let second = render_evolution_context(&workspace.dir, 600);
        assert_eq!(first, second);
        assert!(first.chars().count() <= 600);
        assert!(first.contains("lower-ranked facts"));
    }

    /// The budget is counted in characters, not bytes: a CJK workspace fills
    /// the injection instead of silently getting a third of it.
    #[test]
    fn budget_is_counted_in_characters_not_bytes() {
        let workspace = TempWorkspace::new("cjk-budget");
        let fact =
            "这是一条足够长的中文事实，用来证明预算必须按字符而不是按字节统计才能够把整个注入区域用满。";
        for index in 0..12 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("fact-{index:02}"),
                &format!("{fact}编号 {index}。"),
                &format!("中文事实 {index}"),
                "fact",
                Some(3),
                false,
                &[],
                true,
            )
            .unwrap();
        }

        let budget = 600;
        let first = render_evolution_context(&workspace.dir, budget);
        let second = render_evolution_context(&workspace.dir, budget);
        assert_eq!(first, second);
        assert!(first.chars().count() <= budget);
        // Each CJK summary line is ~60 characters (~180 bytes). Byte
        // accounting renders the same lines as roughly a third of the budget,
        // leaving most of it unused.
        assert!(
            first.chars().count() > budget * 2 / 5,
            "the CJK render must fill the character budget: {} of {budget} chars",
            first.chars().count()
        );
    }

    /// Even a very small budget keeps the final render within the cap: the
    /// tail line is measured, and placed lines are popped when the reserve
    /// cannot cover the pointer.
    #[test]
    fn a_tiny_budget_still_respects_the_hard_cap() {
        let workspace = TempWorkspace::new("tiny-cap");
        for index in 0..12 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("fact-{index:02}"),
                &format!("This is a sufficiently long fact number {index} with details that fill the line."),
                &format!("Fact {index}"),
                "fact",
                Some(3),
                false,
                &[],
                true,
            )
            .unwrap();
        }

        let budget = 300;
        let rendered = render_evolution_context(&workspace.dir, budget);
        assert!(
            rendered.chars().count() <= budget,
            "the render must respect the budget: {} chars for budget {budget}",
            rendered.chars().count()
        );
        assert!(rendered.contains("lower-ranked facts"));
    }

    /// Pinned entries are protected: a flood of pins ranked first still
    /// contributes every one of its lines, while the lower-ranked plain
    /// entries give way under the same cap.
    #[test]
    fn pinned_entries_survive_the_budget_cut() {
        let workspace = TempWorkspace::new("pinned-flood");
        for index in 0..8 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("critical-{index:02}"),
                &format!("Critical fact {index}: a body long enough to overflow the tier one budget."),
                &format!("Critical {index}"),
                "fact",
                Some(1),
                true,
                &[],
                true,
            )
            .unwrap();
            upsert_evolution_entry(
                &workspace.dir,
                &format!("plain-{index:02}"),
                &format!("Plain fact {index}: fillers that will be cut when the pins flood the budget."),
                &format!("Plain {index}"),
                "fact",
                Some(3),
                false,
                &[],
                true,
            )
            .unwrap();
        }

        let rendered = render_evolution_context(&workspace.dir, 600);
        assert!(rendered.chars().count() <= 600);
        // Every pinned entry lands, whichever tier fits it.
        for index in 0..8 {
            assert!(
                rendered.contains(&format!("critical-{index:02}")),
                "pinned entry {index} was dropped from the render"
            );
        }
        // At most two of the lower-ranked plain entries fit behind the pins.
        assert!(
            rendered.matches("plain-0").count() <= 2,
            "plain entries must give way to pinned entries"
        );
        assert!(rendered.contains("plain-00"));
    }

    /// CJK truncation is by characters, never by bytes.
    #[test]
    fn truncation_is_character_safe() {
        let text = "上海机房跳板机";
        assert_eq!(truncate_with_ellipsis(text, 3), "上海…");
        assert_eq!(truncate_with_ellipsis(text, 100), text);
    }

    /// Identical bodies collapse into one ranked row with summed evidence.
    #[test]
    fn duplicate_bodies_collapse_in_the_ranked_view() {
        let workspace = TempWorkspace::new("dedupe");
        workspace.write(
            "a",
            "---\nformat_version: 2\nkey: a\ntitle: same\nimportance: 3\nconfidence: 3\nevidence: 1\n---\n\n同一件事\n",
        );
        workspace.write(
            "b",
            "---\nformat_version: 2\nkey: b\ntitle: same\nimportance: 3\nconfidence: 3\nevidence: 4\n---\n\n同一件事\n",
        );

        let ranked = ranked_evolution_entries(&workspace.dir);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].entry.meta.evidence, 5);
    }

    /// Clustering only groups related entries and ignores blocks below the
    /// size threshold.
    #[test]
    fn clustering_isolates_a_large_related_block() {
        let workspace = TempWorkspace::new("cluster");
        for index in 0..5 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("idc-{index}"),
                &format!("上海机房条目 {index}: 10.0.0.{index}"),
                &format!("上海机房 {index}"),
                "location",
                Some(3),
                false,
                &["idc".to_string(), "shanghai".to_string()],
                false,
            )
            .unwrap();
        }
        upsert_evolution_entry(
            &workspace.dir,
            "python",
            "Use python",
            "默认 python",
            "fact",
            Some(3),
            false,
            &["runtime".to_string()],
            true,
        )
        .unwrap();

        let ranked = ranked_evolution_entries(&workspace.dir);
        let clusters = plan_clusters(&ranked, 200);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].keys.len(), 5);
        assert!(clusters[0].keys.iter().all(|key| key.starts_with("idc-")));
    }

    fn archive_skill_md(name: &str, keys: &[&str]) -> String {
        let keys = keys
            .iter()
            .map(|key| format!("\"{key}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "---\nname: {name}\ndescription: 上海机房运维归档\nsource-memory: [{keys}]\n---\n\n# 上海机房\n\n## 环境\n10.0.0.1\n"
        )
    }

    /// The skill must cover every key before memory is touched.
    #[test]
    fn apply_refuses_when_the_skill_does_not_cover_all_keys() {
        let workspace = TempWorkspace::new("apply-refuse");
        for index in 0..2 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("idc-{index}"),
                &format!("10.0.0.{index}"),
                "上海机房",
                "location",
                Some(3),
                false,
                &["idc".to_string()],
                false,
            )
            .unwrap();
        }
        let keys = vec!["idc-0".to_string(), "idc-1".to_string()];
        let error = apply_consolidation(&workspace.dir, &archive_skill_md("idc-archive", &["idc-0"]), &keys)
            .unwrap_err();
        assert!(error.contains("idc-1"));
        assert_eq!(load_evolution_entries(&workspace.dir).len(), 2);
        assert!(!crate::workspace::skills_root(&workspace.dir).join("idc-archive").exists());
    }

    /// A panel-authored skill is never overwritten by a memory archive.
    #[test]
    fn apply_refuses_to_overwrite_a_panel_authored_skill() {
        let workspace = TempWorkspace::new("apply-user-skill");
        upsert_evolution_entry(
            &workspace.dir,
            "idc-0",
            "10.0.0.1",
            "上海机房",
            "location",
            Some(3),
            false,
            &["idc".to_string()],
            false,
        )
        .unwrap();

        // A skill created in the Skills panel, recorded in `skills.status`.
        let mut status = crate::skills::SkillsStatus::default();
        status.skills.insert(
            "idc-archive".to_string(),
            crate::skills::SkillStatusEntry {
                enabled: true,
                source: crate::skills::SKILL_SOURCE_USER.to_string(),
                ..Default::default()
            },
        );
        let status_path = crate::workspace::skills_status_path(&workspace.dir);
        fs::create_dir_all(status_path.parent().unwrap()).unwrap();
        fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let skill_dir = crate::workspace::skills_root(&workspace.dir).join("idc-archive");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("skill.md"),
            "---\nname: idc-archive\ndescription: user skill\n---\n\nbody\n",
        )
        .unwrap();

        let keys = vec!["idc-0".to_string()];
        let error = apply_consolidation(&workspace.dir, &archive_skill_md("idc-archive", &["idc-0"]), &keys)
            .unwrap_err();
        assert!(error.contains("Skills panel"));
        assert_eq!(load_evolution_entries(&workspace.dir).len(), 1);
    }

    /// A successful apply writes + enables the skill, then archives the entries.
    #[test]
    fn apply_archives_entries_into_the_skill_and_restore_round_trips() {
        let workspace = TempWorkspace::new("apply-ok");
        for index in 0..2 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("idc-{index}"),
                &format!("10.0.0.{index}"),
                "上海机房",
                "location",
                Some(3),
                false,
                &["idc".to_string()],
                false,
            )
            .unwrap();
        }
        let keys = vec!["idc-0".to_string(), "idc-1".to_string()];
        let report = apply_consolidation(
            &workspace.dir,
            &archive_skill_md("idc-archive", &["idc-0", "idc-1"]),
            &keys,
        )
        .unwrap();
        assert!(report.contains("Archived 2 entries"));

        // Hot memory is empty, the archive and the skill exist.
        assert!(load_evolution_entries(&workspace.dir).is_empty());
        assert!(crate::workspace::skills_root(&workspace.dir)
            .join("idc-archive")
            .join("skill.md")
            .is_file());
        assert_eq!(collect_archived_skills(&workspace.dir).len(), 1);
        assert!(find_archived_entry(&workspace.dir, "idc-0").is_some());

        // The skill was enabled automatically, otherwise its description would
        // never reach the prompt.
        let status: crate::skills::SkillsStatus = serde_json::from_str(
            &fs::read_to_string(crate::workspace::skills_status_path(&workspace.dir)).unwrap(),
        )
        .unwrap();
        assert!(status.skills.get("idc-archive").map(|e| e.enabled).unwrap_or(false));

        // Restore moves the entries back and clears `source-memory`.
        let restored = restore_consolidation(&workspace.dir, "idc-archive").unwrap();
        assert!(restored.contains("Restored 2 entries"));
        assert_eq!(load_evolution_entries(&workspace.dir).len(), 2);
        assert!(collect_archived_skills(&workspace.dir).is_empty());
        let skill = crate::skills::resolve_skill_by_name(
            &crate::workspace::skills_root(&workspace.dir),
            "idc-archive",
        )
        .unwrap();
        assert!(skill.skill.source_memory.is_empty());
    }

    /// End-to-end smoke test for the whole lifecycle: legacy migration →
    /// ranked injection → consolidation plan → skill archive → restore.
    #[test]
    fn end_to_end_memory_lifecycle_smoke() {
        let workspace = TempWorkspace::new("e2e");

        // 1. A legacy fact and a structured one must coexist and be ranked.
        workspace.write("legacy-python", "# legacy-python\n\nUse `python`, not `python3`.\n");
        for index in 0..4 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("idc-{index}"),
                &format!("上海机房节点 {index}: 10.0.0.{index}"),
                &format!("上海机房节点 {index}"),
                "location",
                Some(3),
                false,
                &["idc".to_string()],
                false,
            )
            .unwrap();
        }

        let rendered = render_evolution_context(&workspace.dir, 4_000);
        assert!(rendered.contains("legacy-python"));
        assert!(rendered.contains("idc-0"));

        // 2. The plan finds the idc block and nothing else. A 200-char budget
        //    keeps the 20%-of-budget threshold reachable for the tiny fixtures.
        let ranked = ranked_evolution_entries(&workspace.dir);
        let clusters = plan_clusters(&ranked, 200);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].keys.len(), 4);

        // 3. Apply writes + enables the skill and empties the hot area.
        let keys: Vec<String> = clusters[0].keys.clone();
        let skill_md = archive_skill_md(
            "idc-archive",
            &keys.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        apply_consolidation(&workspace.dir, &skill_md, &keys).unwrap();

        let remaining: Vec<String> = ranked_evolution_entries(&workspace.dir)
            .into_iter()
            .map(|scored| scored.entry.meta.key)
            .collect();
        assert_eq!(remaining, vec!["legacy-python"]);
        assert_eq!(collect_archived_skills(&workspace.dir)[0].entries, 4);

        // 4. The panel view only shows the hot entry.
        let view = evolution_memory_view(&workspace.dir);
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].key, "legacy-python");

        // 5. The archive is still reachable, then restorable.
        assert!(find_archived_entry(&workspace.dir, "idc-2").is_some());
        restore_consolidation(&workspace.dir, "idc-archive").unwrap();
        assert_eq!(load_evolution_entries(&workspace.dir).len(), 5);
        assert!(collect_archived_skills(&workspace.dir).is_empty());
    }

    /// Auto-recorded conclusions are distilled, not dumped.
    #[test]
    fn conclusions_are_distilled_into_a_dense_fact() {
        let answer = "结论：默认解释器是 python。\n\n- 不要用 python3\n- 用 python -m pip\n- 虚拟环境在 .venv\n\n\
                      这是一段很长的补充说明，不应该整段进入记忆，因为它只是解释。";
        let (title, body) = distill_conclusion(answer);
        assert!(body.chars().count() <= 320);
        assert!(body.contains("python"));
        assert!(body.contains(".venv"));
        assert!(!title.is_empty());

        let workspace = TempWorkspace::new("distill");
        let key = record_auto_conclusion(&workspace.dir, "s1", answer).unwrap();
        let entry = parse_evolution_entry(&workspace.evolution().join(format!("{key}.md"))).unwrap();
        assert_eq!(entry.meta.source, "auto");
        assert!(entry.body.chars().count() <= 320);

        // The same conclusion twice only raises evidence.
        record_auto_conclusion(&workspace.dir, "s2", answer).unwrap();
        assert_eq!(load_evolution_entries(&workspace.dir).len(), 1);
        let files = fs::read_dir(workspace.evolution())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
            .count();
        assert_eq!(files, 1, "a repeated conclusion must not create a second file");
    }

    /// The explicit "tidy my memory" trigger is recognised in EN and ZH.
    #[test]
    fn consolidation_trigger_is_multilingual() {
        assert!(wants_memory_consolidation("帮我整理记忆"));
        assert!(wants_memory_consolidation("Please consolidate memory now"));
        assert!(!wants_memory_consolidation("explain how memory works"));
    }

    /// The sweep judge names the event, a reusable fact names its subject.
    #[test]
    fn noise_patterns_flag_progress_reports_only() {
        assert_eq!(noise_reason("报告已生成", "report-generated"), Some("已生成"));
        assert_eq!(noise_reason("完成task1", "task1"), Some("完成task"));
        assert_eq!(noise_reason("Current progress", "wip"), Some("wip"));
        // A fact names a subject, so it is never offered to the sweep.
        assert_eq!(noise_reason("python-default", "python-default"), None);
        assert_eq!(noise_reason("cargo-test-exe-locked", "cargo-test"), None);
        // Latin words match whole, so a longer word cannot trip them.
        assert_eq!(noise_reason("autocomplete", "autocomplete"), None);
        assert_eq!(noise_reason("todos-tool", "todos-tool"), None);
    }

    fn write_sweep_entry(workspace: &TempWorkspace, key: &str, title: &str, body: &str) {
        workspace.write(
            key,
            &format!(
                "---\nformat_version: 2\nkey: {key}\ntitle: {title}\nkind: fact\ntags: []\n---\n\n{body}\n"
            ),
        );
    }

    #[test]
    fn the_noise_sweep_skips_trusted_and_reusable_entries() {
        let workspace = TempWorkspace::new("noise-guards");
        // Importance 5: a fact worth keeping, even though its title reports
        // progress — the guard is evidence, never wording.
        workspace.write(
            "kept-high",
            "---\nformat_version: 2\nkey: kept-high\ntitle: 报告已生成\nkind: fact\ntags: []\nimportance: 5\n---\n\nkeep me\n",
        );
        // A recipe is reusable by definition.
        workspace.write(
            "kept-recipe",
            "---\nformat_version: 2\nkey: kept-recipe\ntitle: 已完成\nkind: recipe\ntags: []\n---\n\nrun it\n",
        );
        write_sweep_entry(&workspace, "noise", "报告已生成", "报告已生成，放在临时目录。");
        // The resume note is never a candidate.
        write_sweep_entry(
            &workspace,
            MAINTENANCE_PROGRESS_KEY,
            "已完成",
            "Reason: stopped by user",
        );

        let keys: Vec<String> = noise_candidates(&workspace.dir)
            .into_iter()
            .map(|candidate| candidate.key)
            .collect();
        assert_eq!(keys, vec!["noise".to_string()]);
    }

    #[test]
    fn duplicate_groups_pair_two_notes_about_the_same_subject() {
        let workspace = TempWorkspace::new("duplicates");
        write_sweep_entry(
            &workspace,
            "python-default",
            "python 默认解释器",
            "默认解释器是 python，python3 是 Store 桩，store 里没有可执行文件。",
        );
        write_sweep_entry(
            &workspace,
            "python3-stub",
            "python3 不可用",
            "python3 是 Store 桩，store 里没有可执行文件，改用 node。",
        );
        write_sweep_entry(
            &workspace,
            "mermaid-api",
            "mermaid 渲染接口",
            "mermaid.render 返回 promise，必须等它 resolve 再插入 SVG。",
        );

        let groups = duplicate_groups(&workspace.dir);
        assert_eq!(groups.len(), 1, "{groups:?}");
        let keys: Vec<String> = groups[0]
            .entries
            .iter()
            .map(|(_, key, _)| key.clone())
            .collect();
        assert!(keys.contains(&"python-default".to_string()), "{keys:?}");
        assert!(keys.contains(&"python3-stub".to_string()), "{keys:?}");
    }

    #[test]
    fn a_progress_note_is_never_a_merge_target() {
        let workspace = TempWorkspace::new("duplicates-noise");
        write_sweep_entry(
            &workspace,
            "python-default",
            "python 默认解释器",
            "默认解释器是 python，store 里没有可执行文件。",
        );
        write_sweep_entry(
            &workspace,
            "python-report",
            "报告已生成",
            "报告已生成：默认解释器是 python，store 里没有可执行文件。",
        );

        assert!(duplicate_groups(&workspace.dir).iter().all(|group| group
            .entries
            .iter()
            .all(|(_, key, _)| key != "python-report")));
    }

    #[test]
    fn an_auto_note_at_low_importance_is_a_weak_candidate() {
        let workspace = TempWorkspace::new("noise-auto");
        workspace.write(
            "looked-at-it",
            "---\nformat_version: 2\nkey: looked-at-it\ntitle: I read the whole path\nkind: fact\ntags: []\nimportance: 2\nsource: auto\n---\n\nI read the whole path and split it into three parts.\n",
        );
        write_sweep_entry(&workspace, "noise", "报告已生成", "报告已生成，放在临时目录。");

        let candidates = noise_candidates(&workspace.dir);
        // The phrase match is strong, so it is listed before the auto note.
        assert_eq!(candidates[0].key, "noise", "{candidates:?}");
        assert_eq!(candidates.len(), 2, "{candidates:?}");
        assert!(candidates[1].reason.contains("auto note"), "{candidates:?}");
    }

    /// Trust calibration: an explicit `confidence` overwrites (so trust can
    /// fall) while `importance` keeps its `max` semantics.
    #[test]
    fn explicit_confidence_can_lower_trust_while_importance_only_grows() {
        let workspace = TempWorkspace::new("trust-explicit");
        upsert_evolution_entry(
            &workspace.dir,
            "python-default",
            "Use `python`.",
            "默认解释器",
            "fact",
            Some(3),
            false,
            &[],
            false,
        )
        .unwrap();

        let lowered = upsert_evolution_entry_trusted(
            &workspace.dir,
            "python-default",
            "",
            "",
            "",
            None,
            false,
            &[],
            false,
            TrustUpdate {
                confidence: Some(2),
                ..TrustUpdate::confirming()
            },
        )
        .unwrap();
        assert_eq!(lowered.confidence_before, 3);
        assert_eq!(lowered.entry.meta.confidence, 2);
        assert_eq!(
            lowered.entry.body, "Use `python`.",
            "an empty content keeps the stored body"
        );

        let raised = upsert_evolution_entry_trusted(
            &workspace.dir,
            "python-default",
            "",
            "",
            "",
            Some(1),
            false,
            &[],
            false,
            TrustUpdate {
                confidence: Some(5),
                ..TrustUpdate::confirming()
            },
        )
        .unwrap();
        assert_eq!(raised.entry.meta.confidence, 5);
        assert_eq!(raised.entry.meta.importance, 3, "importance never drops");
    }

    /// A plain re-add is one confirmation: +1 confidence, capped at 5.
    #[test]
    fn a_plain_re_add_raises_confidence_by_one_capped_at_five() {
        let workspace = TempWorkspace::new("trust-confirm");
        let mut confidences: Vec<u32> = Vec::new();
        for _ in 0..5 {
            let outcome = upsert_evolution_entry(
                &workspace.dir,
                "build-tag-first",
                "Run the tagged build before the tests.",
                "build tag first",
                "recipe",
                None,
                false,
                &[],
                false,
            )
            .unwrap();
            confidences.push(outcome.entry.meta.confidence);
        }
        assert_eq!(confidences, vec![3, 4, 5, 5, 5]);
    }

    /// A conflict is fast to lose: confidence drops straight to 1, the body is
    /// rewritten when the corrected fact is supplied, and evidence stays put.
    #[test]
    fn a_conflict_drops_confidence_to_one_and_rewrites_the_body() {
        let workspace = TempWorkspace::new("trust-conflict");
        upsert_evolution_entry(
            &workspace.dir,
            "python-default",
            "Use `python3` for everything.",
            "默认解释器",
            "fact",
            Some(4),
            false,
            &[],
            false,
        )
        .unwrap();

        let outcome = upsert_evolution_entry_trusted(
            &workspace.dir,
            "python-default",
            "Use `python`, not `python3` — python3 is a Store stub here.",
            "默认解释器",
            "fact",
            None,
            false,
            &[],
            true,
            TrustUpdate {
                conflict: true,
                note: "python3 打开应用商店，实测失败".to_string(),
                ..TrustUpdate::default()
            },
        )
        .unwrap();

        assert_eq!(outcome.confidence_before, 3);
        assert_eq!(outcome.entry.meta.confidence, 1);
        assert_eq!(outcome.entry.meta.contradictions, 1);
        assert_eq!(
            outcome.entry.meta.evidence, 1,
            "a conflict is not a confirmation"
        );
        assert!(outcome.entry.body.contains("Store stub"));
        assert!(outcome
            .entry
            .meta
            .last_conflict
            .as_deref()
            .unwrap_or_default()
            .contains("python3"));
        assert!(outcome.entry.meta.last_conflict_at.is_some());

        // Persisted, not only returned in memory.
        let stored =
            parse_evolution_entry(&workspace.evolution().join("python-default.md")).unwrap();
        assert_eq!(stored.meta.confidence, 1);
        assert_eq!(stored.meta.contradictions, 1);
        assert!(stored.body.contains("Store stub"));
        assert!(!stored.body.contains("for everything"));
    }

    /// Without a corrected fact the entry keeps its body — only trust drops and
    /// the reason is recorded.
    #[test]
    fn a_conflict_without_content_keeps_the_body_but_records_the_reason() {
        let workspace = TempWorkspace::new("trust-no-content");
        upsert_evolution_entry(
            &workspace.dir,
            "idc-sh",
            "跳板机 10.0.0.1",
            "上海机房跳板机",
            "location",
            Some(3),
            false,
            &[],
            false,
        )
        .unwrap();

        let outcome = upsert_evolution_entry_trusted(
            &workspace.dir,
            "idc-sh",
            "",
            "",
            "",
            None,
            false,
            &[],
            false,
            TrustUpdate {
                conflict: true,
                note: "该跳板机已下线".to_string(),
                ..TrustUpdate::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.entry.body, "跳板机 10.0.0.1");
        assert_eq!(outcome.entry.meta.confidence, 1);
        assert_eq!(
            outcome.entry.meta.last_conflict.as_deref(),
            Some("该跳板机已下线")
        );
    }

    /// A conflict must target an entry that exists, and the maintenance resume
    /// note has no trust to calibrate.
    #[test]
    fn a_conflict_never_invents_an_entry_or_touches_the_resume_note() {
        let workspace = TempWorkspace::new("trust-guards");
        let unknown = upsert_evolution_entry_trusted(
            &workspace.dir,
            "never-stored",
            "",
            "",
            "",
            None,
            false,
            &[],
            true,
            TrustUpdate {
                conflict: true,
                ..TrustUpdate::default()
            },
        )
        .unwrap_err();
        assert!(unknown.contains("must target"), "{unknown}");
        assert!(load_evolution_entries(&workspace.dir).is_empty());

        let note = upsert_evolution_entry_trusted(
            &workspace.dir,
            MAINTENANCE_PROGRESS_KEY,
            "progress",
            "resume note",
            "fact",
            Some(5),
            false,
            &[],
            true,
            TrustUpdate {
                conflict: true,
                ..TrustUpdate::default()
            },
        )
        .unwrap_err();
        assert!(note.contains(MAINTENANCE_PROGRESS_KEY), "{note}");
    }

    /// The panel row and the tool hints surface the calibration, so a
    /// contradicted entry cannot be mistaken for a trusted one.
    #[test]
    fn the_panel_and_the_hints_surface_a_contradicted_entry() {
        let workspace = TempWorkspace::new("trust-panel");
        upsert_evolution_entry(
            &workspace.dir,
            "python-default",
            "Use python3",
            "默认解释器",
            "fact",
            Some(4),
            false,
            &[],
            false,
        )
        .unwrap();
        assert!(low_trust_hint(&load_evolution_entries(&workspace.dir)[0]).is_none());

        upsert_evolution_entry_trusted(
            &workspace.dir,
            "python-default",
            "Use python",
            "默认解释器",
            "fact",
            None,
            false,
            &[],
            false,
            TrustUpdate {
                conflict: true,
                note: "python3 是 Store 桩".to_string(),
                ..TrustUpdate::default()
            },
        )
        .unwrap();

        let entries = load_evolution_entries(&workspace.dir);
        let hint = low_trust_hint(&entries[0]).expect("a confidence-1 entry is flagged");
        assert!(hint.contains("confidence 1/5"), "{hint}");
        assert!(hint.contains("contradicted 1×"), "{hint}");
        assert!(hint.contains("Store"), "{hint}");

        let view = evolution_memory_view(&workspace.dir);
        assert_eq!(view[0].confidence, 1);
        assert_eq!(view[0].contradictions, 1);
        assert_eq!(view[0].last_conflict.as_deref(), Some("python3 是 Store 桩"));
    }

    /// The panel view uses the same ranking and marks injected rows.
    #[test]
    fn panel_view_reports_rank_and_injection() {
        let workspace = TempWorkspace::new("view");
        for index in 0..4 {
            upsert_evolution_entry(
                &workspace.dir,
                &format!("fact-{index}"),
                &format!("fact {index}"),
                &format!("Fact {index}"),
                "fact",
                Some(3),
                false,
                &[],
                true,
            )
            .unwrap();
        }
        let view = evolution_memory_view(&workspace.dir);
        assert_eq!(view.len(), 4);
        assert_eq!(view[0].rank, 1);
        assert!(view.iter().any(|row| row.injected));
    }

    /// A legacy workspace without an evolution area renders nothing.
    #[test]
    fn a_missing_evolution_area_renders_nothing() {
        let workspace = TempWorkspace::new("missing");
        assert!(render_evolution_context(&workspace.dir, 4_000).is_empty());
    }

    /// Write one structured entry with an explicit confidence / importance.
    fn write_entry(
        workspace: &TempWorkspace,
        key: &str,
        title: &str,
        confidence: u32,
        importance: u32,
        body: &str,
    ) {
        workspace.write(
            key,
            &format!(
                "---\nformat_version: 2\nkey: {key}\ntitle: {title}\nimportance: {importance}\n\
                 confidence: {confidence}\nevidence: 1\n---\n\n{body}\n"
            ),
        );
    }

    /// A keyword hit is injected, unrelated entries are not, the higher
    /// confidence comes first, and only the title is rendered.
    #[test]
    fn a_keyword_hit_is_injected_and_ranked_by_confidence() {
        let workspace = TempWorkspace::new("hint-rank");
        write_entry(
            &workspace,
            "build-tag",
            "Tauri build needs a tag",
            5,
            3,
            "`tauri build` needs a git tag first.",
        );
        write_entry(
            &workspace,
            "python-default",
            "Default interpreter",
            4,
            3,
            "Use `python`, not `python3`.",
        );
        write_entry(
            &workspace,
            "menu-colours",
            "Menu colours",
            5,
            3,
            "The menu uses the accent token.",
        );

        let hint = build_memory_hint(
            &workspace.dir,
            "tauri build 需要先打 tag 吗？python 解释器用哪个",
            true,
        )
        .unwrap();
        assert!(hint.text.starts_with(MEMORY_HINT_HEADER), "{}", hint.text);
        let tag = hint.text.find("Tauri build needs a tag").expect(&hint.text);
        let python = hint.text.find("Default interpreter").expect(&hint.text);
        assert!(tag < python, "{}", hint.text);
        assert!(!hint.text.contains("Menu colours"), "{}", hint.text);
        // Titles only: the bodies are matched against, never printed.
        assert!(!hint.text.contains("git tag first"), "{}", hint.text);
        assert!(!hint.text.contains("build-tag"), "{}", hint.text);
        assert_eq!(hint.keys, vec!["build-tag", "python-default"]);
    }

    /// Nothing matches → nothing is injected (an unrelated message pays zero).
    #[test]
    fn an_unrelated_or_empty_message_gets_no_hint() {
        let workspace = TempWorkspace::new("hint-none");
        write_entry(
            &workspace,
            "runtime",
            "Interpreter",
            4,
            3,
            "Use `python`, not `python3`.",
        );

        assert!(build_memory_hint(&workspace.dir, "", true).is_none());
        assert!(build_memory_hint(&workspace.dir, "   \n  ", true).is_none());
        assert!(build_memory_hint(&workspace.dir, "颜色面板的间距", true).is_none());
    }

    /// The block never exceeds the budget, and dropped entries leave a counter.
    #[test]
    fn the_hint_stays_within_the_budget() {
        let workspace = TempWorkspace::new("hint-budget");
        for index in 0..6 {
            write_entry(
                &workspace,
                &format!("note-{index}"),
                // Long titles: one of them already fills the block on its own.
                &format!(
                    "Runtime interpreter note {index} that keeps going on and on about the \
                     environment it runs in"
                ),
                4,
                3,
                &format!(
                    "Very long interpreter note number {index} that keeps talking about the \
                     runtime environment and everything around it for far longer than one line."
                ),
            );
        }

        let hint = build_memory_hint(&workspace.dir, "the interpreter note", true).unwrap();
        let chars = hint.text.chars().count();
        assert!(chars <= MEMORY_HINT_BUDGET_CHARS, "{chars}: {}", hint.text);
        assert!(!hint.text.contains('\n'), "{}", hint.text);
        assert!(hint.text.contains(" more)"), "{}", hint.text);
        assert_eq!(hint.keys.len(), 1);
    }

    /// A fact the model has to re-verify anyway never takes budget.
    #[test]
    fn low_trust_entries_are_never_injected() {
        let workspace = TempWorkspace::new("hint-low-trust");
        write_entry(
            &workspace,
            "weak",
            "Interpreter",
            LOW_TRUST_CONFIDENCE,
            3,
            "Interpreter note that was contradicted.",
        );
        assert!(build_memory_hint(&workspace.dir, "interpreter note", true).is_none());
    }

    /// The maintenance resume note is an internal bookmark, not a fact.
    #[test]
    fn the_maintenance_note_is_never_injected() {
        let workspace = TempWorkspace::new("hint-maintenance");
        write_entry(
            &workspace,
            MAINTENANCE_PROGRESS_KEY,
            "Maintenance",
            5,
            5,
            "Interpreter maintenance pass in progress.",
        );
        assert!(build_memory_hint(&workspace.dir, "interpreter maintenance", true).is_none());
    }

    /// `user` / `repo` notes are eligible; they rank below a confirmed fact.
    #[test]
    fn user_and_repo_notes_are_eligible_with_a_neutral_confidence() {
        let workspace = TempWorkspace::new("hint-notes");
        write_entry(
            &workspace,
            "runtime",
            "Default interpreter",
            4,
            3,
            "Use `python`, not `python3`.",
        );
        let repo = crate::workspace::memory_root(&workspace.dir).join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            repo.join("interpreter.md"),
            "# repo interpreter note\n\nPrefer the workspace python.\n\ntags: runtime\n",
        )
        .unwrap();

        let hint = build_memory_hint(&workspace.dir, "which python interpreter", true).unwrap();
        let runtime = hint.text.find("Default interpreter").expect(&hint.text);
        let note = hint
            .text
            .find("repo interpreter note")
            .expect(&hint.text);
        assert!(runtime < note, "{}", hint.text);
        assert_eq!(hint.keys, vec!["runtime", "repo interpreter note"]);
    }

    /// Without the self-evolution mode only file notes are considered.
    #[test]
    fn without_the_evolution_mode_only_file_notes_are_used() {
        let workspace = TempWorkspace::new("hint-mode-off");
        write_entry(
            &workspace,
            "runtime",
            "Interpreter",
            5,
            5,
            "Use `python`, not `python3`.",
        );
        assert!(build_memory_hint(&workspace.dir, "which python interpreter", false).is_none());

        let repo = crate::workspace::memory_root(&workspace.dir).join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("interpreter.md"), "# interpreter\n\nPrefer python3.\n").unwrap();
        let hint = build_memory_hint(&workspace.dir, "which python interpreter", false).unwrap();
        assert_eq!(hint.keys, vec!["interpreter"]);
    }

    /// Two builds of the same memory set render byte-identical text.
    #[test]
    fn the_render_is_deterministic() {
        let workspace = TempWorkspace::new("hint-determinism");
        for (key, confidence) in [("alpha", 3), ("beta", 4), ("gamma", 5)] {
            write_entry(
                &workspace,
                key,
                key,
                confidence,
                3,
                // Distinct bodies: `load_evolution_entries` folds identical
                // facts together, which would leave a single candidate.
                &format!("The {key} interpreter note for this workspace."),
            );
        }
        let first = build_memory_hint(&workspace.dir, "interpreter note", true).unwrap();
        let second = build_memory_hint(&workspace.dir, "interpreter note", true).unwrap();
        assert_eq!(first.text, second.text);
        assert_eq!(first.keys, second.keys);
    }

    /// One body keyword is not evidence; two are. (English words are used here
    /// because jieba's CJK segmentation is dictionary-driven, not a fixed
    /// bigram — the assertion is about the strength rule, not the cutter.)
    #[test]
    fn a_body_only_match_needs_two_keywords() {
        let workspace = TempWorkspace::new("hint-body");
        write_entry(
            &workspace,
            "noise",
            "Noise",
            3,
            3,
            "A remark about the interpreter only.",
        );
        assert!(build_memory_hint(&workspace.dir, "python interpreter", true).is_none());

        write_entry(
            &workspace,
            "signal",
            "Signal",
            3,
            3,
            "The python interpreter is explained.",
        );
        let hint = build_memory_hint(&workspace.dir, "python interpreter", true).unwrap();
        assert_eq!(hint.keys, vec!["signal"]);
    }

    /// Two entries sharing a name render one line — the block shows titles, so a
    /// repeat would be a wasted line.
    #[test]
    fn a_repeated_title_is_rendered_once() {
        let workspace = TempWorkspace::new("hint-same-title");
        write_entry(
            &workspace,
            "interpreter",
            "Interpreter",
            4,
            3,
            "Use `python`, not `python3`.",
        );
        let repo = crate::workspace::memory_root(&workspace.dir).join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            repo.join("interpreter.md"),
            "# Interpreter\n\nPrefer python3.\n",
        )
        .unwrap();

        let hint = build_memory_hint(&workspace.dir, "which python interpreter", true).unwrap();
        assert_eq!(hint.text.matches("Interpreter").count(), 1, "{}", hint.text);
        // The better-evidenced (`c4`) entry is the one that survives.
        assert_eq!(hint.keys, vec!["interpreter"]);
    }

    /// `search` matches tokens, not the whole query as one substring.
    #[test]
    fn search_matches_tokens_not_the_whole_query() {
        let workspace = TempWorkspace::new("search-tokens");
        write_entry(
            &workspace,
            "python-default",
            "Default interpreter",
            4,
            3,
            "Use `python`, not `python3`.",
        );

        // The old matcher needed "python解释器" verbatim and found nothing.
        let matches = rank_keyword_matches(ranked_evolution_entries(&workspace.dir), "python 解释器");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1.entry.meta.key, "python-default");
        assert_eq!(matches[0].0.tier, HINT_TITLE_WEIGHT);
        assert_eq!(matches[0].0.hits, 1);
    }

    /// CJK tokens are dictionary words (`用户输入` → `用户` + `输入`), not the
    /// old overlapping bigrams (`户输`).
    #[test]
    fn cjk_segmentation_yields_words_not_bigrams() {
        let keywords = hint_keywords("用户输入的时候");
        assert!(keywords.contains("用户"), "{keywords:?}");
        assert!(keywords.contains("输入"), "{keywords:?}");
        assert!(!keywords.contains("户输"), "{keywords:?}");
    }

    /// English inflections fold onto one stem, so `building` finds `build`.
    #[test]
    fn search_stems_english_inflections() {
        let workspace = TempWorkspace::new("search-stem");
        write_entry(
            &workspace,
            "tauri-build",
            "Tauri build",
            4,
            3,
            "A git tag is required before the tauri build.",
        );

        let matches = rank_keyword_matches(
            ranked_evolution_entries(&workspace.dir),
            "building the tauri app",
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1.entry.meta.key, "tauri-build");
        // `building` stemmed to `build`, so the title token matched.
        assert_eq!(matches[0].0.tier, HINT_TITLE_WEIGHT);
    }

    /// Relevance first: a name hit carrying the query words *in order* (phrase)
    /// outranks everything; after that, more coverage beats less.
    #[test]
    fn search_orders_by_relevance_then_tier_then_coverage() {
        let workspace = TempWorkspace::new("search-order");
        write_entry(
            &workspace,
            "name-hit",
            "Python interpreter",
            3,
            3,
            "No prose match here.",
        );
        workspace.write(
            "tag-hit",
            "---\nformat_version: 2\nkey: tag-hit\ntitle: Shell notes\nimportance: 3\nconfidence: 3\ntags: [python]\n---\n\nUnrelated prose.\n",
        );
        write_entry(
            &workspace,
            "body-two",
            "Misc one",
            3,
            3,
            "The python interpreter lives in the toolchain.",
        );
        write_entry(
            &workspace,
            "body-one",
            "Misc two",
            3,
            3,
            "Only python appears.",
        );

        let matches = rank_keyword_matches(
            ranked_evolution_entries(&workspace.dir),
            "python interpreter",
        );
        let order: Vec<&str> = matches
            .iter()
            .map(|(_, scored)| scored.entry.meta.key.as_str())
            .collect();
        assert_eq!(order, vec!["name-hit", "body-two", "tag-hit", "body-one"]);
        // Both words, in order, in the title → the phrase flag is on.
        assert!(matches[0].0.phrase);

        let text = render_search_matches(&matches);
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(text.starts_with("- name-hit ("), "{text}");
        assert!(text.contains(", title, phrase, 2 hits):"), "{text}");
        assert!(text.contains(", tags, 1 hit):"), "{text}");
        assert!(text.contains(", body, 2 hits):"), "{text}");
        assert!(text.contains(", body, 1 hit):"), "{text}");
    }

    /// A body-only single-keyword hit is still found, but it is not evidence
    /// enough to count as usage (`uses` feeds the ranking).
    #[test]
    fn a_weak_hit_is_returned_but_does_not_count_as_usage() {
        let workspace = TempWorkspace::new("search-weak");
        write_entry(&workspace, "prose-only", "Misc", 3, 3, "Only python appears.");

        let matches =
            rank_keyword_matches(ranked_evolution_entries(&workspace.dir), "python interpreter");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0.tier, HINT_CONTENT_WEIGHT);
        assert!(!matches[0].0.is_credible());
    }

}
