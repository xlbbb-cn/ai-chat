//! Model capability metadata, sourced from the public
//! [`basellm/llm-metadata`](https://github.com/basellm/llm-metadata) catalogue
//! (the same feed new-api / one-api ships).
//!
//! An OpenAI-compatible `/models` listing only carries ids (and sometimes a
//! context window), so the Settings panel enriches each remote model with this
//! feed: `tags` are expanded into capability flags (`Tools`, `Reasoning`,
//! `Vision`, `Files`, `Audio`, …) and a token window, and the frontend matches
//! each remote id against the closest catalogue name (see
//! `src/utils/modelMetadata.ts`). Matched entries are persisted per model in
//! `AppConfig::model_metadata`.
//!
//! The payload (~370 entries) barely changes, so the parsed result is cached in
//! memory for a few hours; pass `force` to bypass the cache.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::llm_complete;

/// Catalogue with English descriptions (an i18n/zh variant exists but pulls the
/// same tags — descriptions are only shown as-is in the UI).
const METADATA_URL: &str = "https://basellm.github.io/llm-metadata/api/newapi/models.json";

/// How long a fetched catalogue is served from memory before re-downloading.
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// One catalogue entry with `tags` expanded into capability flags.
///
/// The same shape is stored per remote model in `AppConfig::model_metadata`,
/// where `match_score` records how close the remote id was to `model_name`
/// (1.0 = identical after normalisation). Mirrored by `ModelMetadata` in
/// `src/types.ts` — keep both in sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    /// Catalogue name this entry (or the matched remote model) refers to.
    pub model_name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub description: String,
    /// Raw tags, e.g. `["Tools", "Reasoning", "Vision", "1M"]`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Context window in tokens parsed from the size tag (`128K`, `131.1K`, `1M`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub supports_tools: bool,
    /// "Thinking" / chain-of-thought models (`Reasoning` tag).
    #[serde(default)]
    pub supports_reasoning: bool,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_files: bool,
    #[serde(default)]
    pub supports_audio: bool,
    #[serde(default)]
    pub open_weights: bool,
    #[serde(default)]
    pub deprecated: bool,
    /// Similarity (0–1) between the stored model id and `model_name`. Only set
    /// on entries persisted in the config; absent in the raw catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_score: Option<f64>,
}

struct CacheEntry {
    fetched_at: Instant,
    entries: Vec<ModelMetadata>,
}

static CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn cached_entries() -> Option<Vec<ModelMetadata>> {
    let guard = cache().lock().ok()?;
    let entry = guard.as_ref()?;
    (entry.fetched_at.elapsed() < CACHE_TTL).then(|| entry.entries.clone())
}

/// Return the model-metadata catalogue.
///
/// Served from an in-memory cache (6 h) unless `force` is set. A failed
/// download is reported as an error so the caller can degrade gracefully —
/// the model list itself never depends on this feed.
#[tauri::command]
pub async fn fetch_model_metadata(force: Option<bool>) -> Result<Vec<ModelMetadata>, String> {
    if !force.unwrap_or(false) {
        if let Some(entries) = cached_entries() {
            return Ok(entries);
        }
    }

    let entries = download().await?;
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CacheEntry {
            fetched_at: Instant::now(),
            entries: entries.clone(),
        });
    }
    Ok(entries)
}

async fn download() -> Result<Vec<ModelMetadata>, String> {
    let client = llm_complete::build_http_client()?;
    let res = client
        .get(METADATA_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch model metadata: {e}"))?;

    if !res.status().is_success() {
        return Err(format!(
            "Model metadata request failed with HTTP {}",
            res.status()
        ));
    }

    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Invalid model metadata response: {e}"))?;

    // The feed wraps the array in `{ data: [...], message, success }`; accept a
    // bare array too so a format tweak upstream does not break the feature.
    let data = body
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| body.as_array())
        .ok_or_else(|| "Unexpected model metadata payload: missing `data` array".to_string())?;

    Ok(data.iter().filter_map(parse_entry).collect())
}

fn parse_entry(raw: &serde_json::Value) -> Option<ModelMetadata> {
    let model_name = raw.get("model_name").and_then(|v| v.as_str())?.trim();
    if model_name.is_empty() {
        return None;
    }

    let tags: Vec<String> = raw
        .get("tags")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let has = |label: &str| tags.iter().any(|t| t.eq_ignore_ascii_case(label));

    Some(ModelMetadata {
        model_name: model_name.to_string(),
        vendor: string_field(raw, "vendor_name"),
        description: string_field(raw, "description"),
        context_length: tags.iter().filter_map(|t| parse_size_tag(t)).max(),
        supports_tools: has("Tools"),
        supports_reasoning: has("Reasoning"),
        supports_vision: has("Vision"),
        supports_files: has("Files"),
        supports_audio: has("Audio"),
        open_weights: has("Open Weights"),
        deprecated: has("Deprecated"),
        tags,
        match_score: None,
    })
}

fn string_field(raw: &serde_json::Value, key: &str) -> String {
    raw.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Parse a context-window tag such as `128K`, `131.1K` or `1M` into tokens.
///
/// The catalogue uses decimal multipliers (`131.1K` describes the usual
/// 131 072-token window), which is close enough for context budgeting. Tags
/// without a unit (`480`, `Open Weights`, …) are ignored.
fn parse_size_tag(tag: &str) -> Option<u32> {
    let upper = tag.trim().to_ascii_uppercase();
    let (digits, multiplier) = if let Some(rest) = upper.strip_suffix('K') {
        (rest, 1_000f64)
    } else if let Some(rest) = upper.strip_suffix('M') {
        (rest, 1_000_000f64)
    } else {
        return None;
    };

    let value = digits.trim().parse::<f64>().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let tokens = (value * multiplier).round();
    (tokens <= u32::MAX as f64).then_some(tokens as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_size_tags() {
        assert_eq!(parse_size_tag("128K"), Some(128_000));
        assert_eq!(parse_size_tag("131.1K"), Some(131_100));
        assert_eq!(parse_size_tag("1M"), Some(1_000_000));
        assert_eq!(parse_size_tag("480"), None);
        assert_eq!(parse_size_tag("Open Weights"), None);
        assert_eq!(parse_size_tag(""), None);
    }

    #[test]
    fn expands_tags_into_capabilities() {
        let raw = serde_json::json!({
            "model_name": "gpt-4o",
            "vendor_name": "OpenAI",
            "description": "Flagship multimodal model",
            "tags": "Tools,Vision,Files,128K",
        });
        let entry = parse_entry(&raw).expect("entry parsed");
        assert_eq!(entry.model_name, "gpt-4o");
        assert_eq!(entry.vendor, "OpenAI");
        assert_eq!(entry.context_length, Some(128_000));
        assert!(entry.supports_tools && entry.supports_vision && entry.supports_files);
        assert!(!entry.supports_reasoning && !entry.deprecated);
        assert_eq!(entry.match_score, None);
    }

    #[test]
    fn skips_entries_without_a_name() {
        assert!(parse_entry(&serde_json::json!({ "tags": "Tools" })).is_none());
        assert!(parse_entry(&serde_json::json!({ "model_name": "  " })).is_none());
    }
}
