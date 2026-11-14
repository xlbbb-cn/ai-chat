// Memory tool: session (in-memory), user/repo/evolution (file-backed) and the
// read-only `all` fan-out across every scope.
//
// The `evolution` scope is special: it is the self-evolution memory area, and
// it is the only scope with a structured on-disk format (YAML front matter),
// a ranked read path the model queries on demand, and a "consolidate into a
// skill" archive path.

use crate::util::{slugify, truncate_with_ellipsis};
use crate::db;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
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
/// measures its clusters against. Nothing is preloaded into a request, so this
/// is a UI/analysis budget, not part of the prompt prefix.
pub const EVOLUTION_CONTEXT_BUDGET_CHARS: usize = 4_000;

/// On-disk format version of an evolution memory entry.
pub const EVOLUTION_FORMAT_VERSION: u32 = 2;

/// Character caps that keep one entry from crowding out the rest.
const EVOLUTION_TITLE_MAX: usize = 100;
const EVOLUTION_SUMMARY_MAX: usize = 300;
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

/// An entry plus its ranking score.
#[derive(Debug, Clone)]
pub struct ScoredEntry {
    pub entry: EvolutionEntry,
    pub score: f64,
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

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

const TOKEN_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "use", "using", "used",
    "默认", "使用", "可以", "如果", "需要", "一个", "进行", "通过",
];

/// Tokens for clustering: ASCII words (≥3 chars) plus CJK bigrams.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut ascii = String::new();
    let mut cjk_run: Vec<char> = Vec::new();

    let flush_ascii = |ascii: &mut String, tokens: &mut Vec<String>| {
        if ascii.chars().count() >= 3 {
            let token = ascii.to_lowercase();
            if !TOKEN_STOPWORDS.contains(&token.as_str()) {
                tokens.push(token);
            }
        }
        ascii.clear();
    };
    let flush_cjk = |run: &mut Vec<char>, tokens: &mut Vec<String>| {
        if run.len() == 1 {
            tokens.push(run[0].to_string());
        } else {
            for pair in run.windows(2) {
                tokens.push(pair.iter().collect());
            }
        }
        run.clear();
    };

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            ascii.push(ch);
        } else if is_cjk(ch) {
            flush_ascii(&mut ascii, &mut tokens);
            cjk_run.push(ch);
        } else {
            flush_ascii(&mut ascii, &mut tokens);
            flush_cjk(&mut cjk_run, &mut tokens);
        }
    }
    flush_ascii(&mut ascii, &mut tokens);
    flush_cjk(&mut cjk_run, &mut tokens);

    tokens.sort();
    tokens.dedup();
    tokens
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
                let current_score = score_evolution_entry(&out[index]);
                let incoming_score = score_evolution_entry(&entry);
                let keep_incoming = incoming_score > current_score;
                let mut merged = if keep_incoming {
                    entry.clone()
                } else {
                    out[index].clone()
                };
                let loser = if keep_incoming {
                    out[index].clone()
                } else {
                    entry.clone()
                };
                merged.meta.evidence = merged.meta.evidence.max(1) + loser.meta.evidence.max(1);
                merged.meta.uses = merged.meta.uses.saturating_add(loser.meta.uses);
                merged.meta.pinned |= loser.meta.pinned;
                merged.meta.importance = merged.meta.importance.max(loser.meta.importance);
                for tag in loser.meta.tags {
                    if !merged.meta.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
                        merged.meta.tags.push(tag);
                    }
                }
                if loser.meta.key != merged.meta.key
                    && !merged.meta.aliases.contains(&loser.meta.key)
                {
                    merged.meta.aliases.push(loser.meta.key.clone());
                }
                out[index] = merged;
                continue;
            }
            by_body.insert(signature, out.len());
        }
        out.push(entry);
    }

    out
}

/// Load every hot entry (migration is **not** performed here).
pub fn load_evolution_entries(workspace_dir: &Path) -> Vec<EvolutionEntry> {
    let dir = evolution_dir(workspace_dir);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut loaded: Vec<EvolutionEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") || name.contains(".bak.") {
            continue;
        }
        if let Some(parsed) = parse_evolution_entry(&path) {
            loaded.push(parsed);
        }
    }
    dedupe_entries(loaded)
}

/// Load + rank. The single read entry point shared by `memory list`, the
/// consolidation planner and the panel.
pub fn ranked_evolution_entries(workspace_dir: &Path) -> Vec<ScoredEntry> {
    let mut scored: Vec<ScoredEntry> = load_evolution_entries(workspace_dir)
        .into_iter()
        .map(|entry| ScoredEntry {
            score: score_evolution_entry(&entry),
            entry,
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
    /// searches the scope on demand), so only the reference tests read it.
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
/// first request's prefill dominate the prompt, so the model now gets a pointer
/// to the `evolution` scope and searches it on demand. The renderer is kept as
/// the reference implementation its tests pin down (deterministic tiering and a
/// character budget).
#[cfg(test)]
pub fn render_evolution_context(workspace_dir: &Path, budget_chars: usize) -> String {
    let ranked = ranked_evolution_entries(workspace_dir);
    render_ranked_entries(&ranked, budget_chars).text
}

// ─── Writes: upsert / merge ──────────────────────────────────────────────────

pub struct UpsertOutcome {
    pub entry: EvolutionEntry,
    pub merged_into: Option<String>,
    pub created: bool,
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

    if !created {
        entry.meta.evidence = entry.meta.evidence.max(1) + 1;
    } else {
        entry.meta.evidence = 1;
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
         permanent prompt cost; load the archive with `use_skill` (source-memory lists {} keys).",
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
            format!("🧠 *Memory {} across all scopes*\n", action),
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
                    format!("🧠 *Session memory stored: {}*\n", key),
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
                            format!("🧠 *Session memory retrieved: {}*\n", key),
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
                    format!("🧠 *Listing {} session memories*\n", keys.len()),
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
                    format!("🧠 *Searching session memories for '{}'*\n", query),
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
                            format!("🧠 *Session memory deleted: {}*\n", key),
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
                            format!("🧠 *{} memory stored: {}*\n", scope, key),
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
                            format!("🧠 *{} memory retrieved: {}*\n", scope, key),
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
                        format!("🧠 *Listing {} {} memories*\n", items.len(), scope),
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
                    format!("🧠 *Searching {} memories for '{}'*\n", scope, query),
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
                            format!("🧠 *{} memory deleted: {}*\n", scope, key),
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

            match upsert_evolution_entry(
                workspace_dir,
                key,
                content,
                title,
                kind,
                importance,
                pinned,
                tags,
                merge,
            ) {
                Ok(outcome) => {
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        session_id,
                        format!(
                            "🧠 *Evolution memory {}: {}*\n",
                            if outcome.created { "stored" } else { "updated" },
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
                        "evidence": outcome.entry.meta.evidence,
                        "scope": EVOLUTION_SCOPE,
                        "created": outcome.created,
                    });
                    if let Some(merged_into) = outcome.merged_into {
                        payload["merged_into"] = json!(merged_into);
                    }
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(error) => format!("Error writing memory file: {}", error),
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
                    format!("🧠 *Evolution memory retrieved: {}*\n", entry.meta.key),
                );
                return evolution_entry_to_md(entry);
            }
            if let Some((skill, path)) = find_archived_entry(workspace_dir, key) {
                return format!(
                    "Memory '{key}' was archived into skill '{skill}'. Load it with `use_skill` \
                     (name: {skill}); the archived file is {}.",
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
                format!("🧠 *Listing {} evolution memories*\n", ranked.len()),
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
                format!("🧠 *Searching evolution memories for '{}'*\n", query),
            );
            let needle = normalize_text(query);
            let ranked = ranked_evolution_entries(workspace_dir);
            let mut matched: Vec<String> = Vec::new();
            for scored in &ranked {
                let haystack = normalize_text(&format!(
                    "{} {} {} {}",
                    scored.entry.meta.key, scored.entry.meta.title, scored.entry.summary, scored.entry.body
                ));
                if !needle.is_empty() && haystack.contains(&needle) {
                    matched.push(scored.entry.meta.key.clone());
                }
            }
            bump_uses(workspace_dir, &matched);

            match run_integrated_search(
                query,
                &dir,
                workspace_dir,
                &SearchOptions {
                    include_hidden: false,
                    glob: Some("**/*.md".to_string()),
                    ..SearchOptions::default()
                },
            ) {
                Ok(output) => {
                    if output.is_empty() || output == "(no matches)" {
                        format!("No evolution memories match '{}'.", query)
                    } else if matched.is_empty() {
                        output
                    } else {
                        format!("{}\n\nMatched keys: {}", output, matched.join(", "))
                    }
                }
                Err(error) => format!("Error: {error}"),
            }
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
                        format!("🧠 *Evolution memory deleted: {}*\n", key),
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
                "🧠 *Consolidating evolution memory*\n".to_string(),
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

#[tauri::command]
pub fn list_evolution_memory(state: State<'_, crate::AppState>) -> Vec<EvolutionMemoryView> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let _ = persist_evolution_migration(&workspace_dir);
    evolution_memory_view(&workspace_dir)
}

#[tauri::command]
pub fn read_evolution_entry(
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

#[tauri::command]
pub fn list_archived_skills(state: State<'_, crate::AppState>) -> Vec<ArchivedSkill> {
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
}
