//! Model capability metadata, merged from three public catalogues.
//!
//! 1. [`models.dev`](https://models.dev/api.json) — the widest catalogue (one
//!    entry per provider and model) and the only one that reports *exact*
//!    limits, so `limit.context` / `limit.output` and the capability booleans
//!    it carries win over every other source;
//! 2. [`OpenRouter`](https://openrouter.ai/api/v1/models) — `context_length`,
//!    `top_provider.max_completion_tokens` and `supported_parameters`;
//! 3. [`basellm/llm-metadata`](https://github.com/basellm/llm-metadata) — the
//!    feed new-api / one-api ships; it is the only source of *tags*, which
//!    still reach the merge because capability flags are OR-ed, but its
//!    rounded size tags (`128K`, `1M`) only fill a window nobody else reported.
//!
//! An OpenAI-compatible `/models` listing only carries ids (and sometimes a
//! context window), so the Settings panel enriches each remote model with this
//! catalogue: `tags` are expanded into capability flags (`Tools`, `Reasoning`,
//! `Vision`, `Files`, `Audio`, …) plus a token window and a max-output limit,
//! and the frontend matches each remote id against the closest catalogue name
//! (see `src/utils/modelMetadata.ts`). Matched entries are persisted per model
//! in `AppConfig::model_metadata`.
//!
//! The three feeds are downloaded together and merged into one entry per model
//! (see [`merge_catalogs`]: a lower-priority source only fills the fields the
//! higher-priority one left empty, and contributes the models the others miss).
//! The merged result is written to `<app_data>/model_metadata.json` — ~2 900
//! entries, ~1.2 MB — and refreshed at most weekly, so a restart, a restart
//! without network, or a slow mirror all still serve the catalogue. A source
//! that fails (or no longer parses) is skipped and the others still merge; the
//! command only errors when nothing answered *and* no file exists.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::llm_complete;

/// Source label + URL for the models.dev catalogue — the first source in the
/// merge order, because it covers the most models and is the only one that
/// reports exact token limits.
const MODELS_DEV_SOURCE: &str = "models.dev";
const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Source label + URL for OpenRouter's model list.
const OPENROUTER_SOURCE: &str = "openrouter";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";

/// Source label + URL for the llm-metadata catalogue — last in the merge order,
/// kept because it is the only feed that ships `tags` (and therefore the only
/// one that knows about `Audio` / `Files` / `Open Weights` / `Deprecated`
/// entries). English descriptions: an i18n/zh variant exists but pulls the same
/// tags, and descriptions are only shown as-is in the UI.
const LLM_METADATA_SOURCE: &str = "llm-metadata";
const LLM_METADATA_URL: &str = "https://basellm.github.io/llm-metadata/api/newapi/models.json";

/// Merged catalogue inside `<app_data>`, with its `updated_at` timestamp.
const CATALOG_FILE: &str = "model_metadata.json";

/// Bumped when the persisted shape changes in a way an older reader could not
/// cope with. Entries themselves are read leniently (every field has a default),
/// so this is currently informational.
const CATALOG_VERSION: u32 = 1;

/// How long a downloaded catalogue is reused (file *and* memory) before the
/// feeds are fetched again.
const CATALOG_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// Longest description kept in the catalogue; the Settings panel renders it as
/// a one-line hint, and OpenRouter descriptions run to several paragraphs.
const DESCRIPTION_LIMIT: usize = 400;

/// Tokens that carry no model identity, e.g. serving/quantisation markers and
/// release-channel words. Mirrors `DECORATION_TOKENS` in
/// `src/utils/modelMetadata.ts`, so the merge keys agree with the frontend's
/// own normalisation.
const DECORATION_TOKENS: &[&str] = &[
    "latest", "preview", "exp", "experimental", "gguf", "awq", "gptq", "mlx", "fp8", "fp16",
    "int8", "int4",
];

/// Providers whose own catalogue entry should win when the same model is listed
/// by many gateways (models.dev resells most models through dozens of them,
/// each with slightly different limits). Anything outside this list comes after
/// it, ordered by provider id so the merge stays deterministic.
const PREFERRED_PROVIDERS: &[&str] = &[
    "openai",
    "anthropic",
    "google",
    "google-vertex",
    "amazon-bedrock",
    "azure",
    "deepseek",
    "xai",
    "meta",
    "mistral",
    "cohere",
    "alibaba",
    "alibaba-cn",
    "zhipuai",
    "zai",
    "moonshotai",
    "minimax",
    "nvidia",
    "perplexity",
    "groq",
    "cerebras",
    "togetherai",
    "fireworks-ai",
    "openrouter",
    "vercel",
    "github-copilot",
    "huggingface",
    "lmstudio",
    "ollama",
    "upstage",
    "inception",
    "ai21",
    "liquid",
    "morph",
    "synthetic",
    "friendli",
    "modal",
    "xiaomi",
];

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
    /// Raw tags, e.g. `["Tools", "Reasoning", "Vision", "1M"]`. Only the
    /// llm-metadata feed publishes them, so an entry created by either of the
    /// other two sources starts empty and is filled from there.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    /// Largest completion the model accepts, when a source reports one
    /// (`limit.output` on models.dev, `max_completion_tokens` on OpenRouter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
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
    /// Catalogues that contributed to this entry, in priority order:
    /// `models.dev`, `openrouter`, `llm-metadata`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// Similarity (0–1) between the stored model id and `model_name`. Only set
    /// on entries persisted in the config; absent in the raw catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_score: Option<f64>,
}

/// `<app_data>/model_metadata.json`: the merged catalogue plus the Unix
/// timestamp of the download that produced it (the TTL anchor).
#[derive(Debug, Serialize, Deserialize)]
struct CatalogFile {
    version: u32,
    updated_at: u64,
    entries: Vec<ModelMetadata>,
}

/// One feed: the label recorded in [`ModelMetadata::sources`], its URL and the
/// parser for its payload. Declaration order is priority order.
struct Source {
    name: &'static str,
    url: &'static str,
    parse: fn(&serde_json::Value) -> Vec<ModelMetadata>,
}

const SOURCES: &[Source] = &[
    Source {
        name: MODELS_DEV_SOURCE,
        url: MODELS_DEV_URL,
        parse: parse_models_dev,
    },
    Source {
        name: OPENROUTER_SOURCE,
        url: OPENROUTER_URL,
        parse: parse_openrouter,
    },
    Source {
        name: LLM_METADATA_SOURCE,
        url: LLM_METADATA_URL,
        parse: parse_llm_metadata,
    },
];

/// Last catalogue loaded from disk or network, kept so repeated calls (every
/// Settings refresh) do not re-read or re-parse the file.
struct CachedCatalog {
    updated_at: u64,
    entries: Vec<ModelMetadata>,
}

static CACHE: OnceLock<Mutex<Option<CachedCatalog>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CachedCatalog>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Serializes refreshes: two Settings calls at once must not download the three
/// feeds twice.
fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Is a catalogue stamped `updated_at` still inside the weekly window?
///
/// A stamp in the future (clock skew, restored backup) counts as fresh: the
/// alternative is re-downloading on every call.
fn is_fresh(updated_at: u64) -> bool {
    now_secs().saturating_sub(updated_at) < CATALOG_TTL_SECS
}

/// Entries of a catalogue that is still fresh, when one is in memory.
fn cached_fresh() -> Option<Vec<ModelMetadata>> {
    let guard = cache().lock().ok()?;
    let entry = guard.as_ref()?;
    is_fresh(entry.updated_at).then(|| entry.entries.clone())
}

fn remember(catalog: &CatalogFile) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CachedCatalog {
            updated_at: catalog.updated_at,
            entries: catalog.entries.clone(),
        });
    }
}

/// Report a degraded refresh (a failed source, a stale catalogue) in `app.log`.
///
/// The catalogue is a nice-to-have, so nothing here may fail the command.
fn warn(app: &tauri::AppHandle, message: &str) {
    if let Some(state) = app.try_state::<crate::AppState>() {
        let _ = state
            .logger
            .lock()
            .map(|logger| logger.log("WARN", message));
    }
}

/// Return the model-metadata catalogue: llm-metadata, models.dev and OpenRouter
/// merged into one entry per model.
///
/// Served from memory, then from `<app_data>/model_metadata.json` while that
/// file is younger than [`CATALOG_TTL_SECS`]; otherwise the three feeds are
/// downloaded, merged, written back and returned. `force` skips both caches.
///
/// A failed download is reported as an error so the caller can degrade
/// gracefully — the model list itself never depends on this feed — but an
/// existing (stale) catalogue is served instead whenever there is one.
#[tauri::command]
pub async fn fetch_model_metadata(
    app: tauri::AppHandle,
    force: Option<bool>,
) -> Result<Vec<ModelMetadata>, String> {
    let force = force.unwrap_or(false);

    if !force {
        if let Some(entries) = cached_fresh() {
            return Ok(entries);
        }
    }

    let path = catalog_path(&app)?;
    if !force {
        if let Some(catalog) = read_catalog(&path) {
            let fresh = is_fresh(catalog.updated_at);
            remember(&catalog);
            if fresh {
                return Ok(catalog.entries);
            }
        }
    }

    let _guard = refresh_lock().lock().await;
    // Whoever held the lock before us may have just refreshed.
    if !force {
        if let Some(entries) = cached_fresh() {
            return Ok(entries);
        }
    }

    match build_catalog().await {
        Ok(catalog) => {
            remember(&catalog);
            write_catalog(&app, &path, &catalog);
            Ok(catalog.entries)
        }
        Err(err) => match read_catalog(&path) {
            Some(stale) => {
                warn(
                    &app,
                    &format!(
                        "model metadata refresh failed ({err}); serving the cached catalogue \
                         ({} entries, updated {})",
                        stale.entries.len(),
                        stale.updated_at
                    ),
                );
                remember(&stale);
                Ok(stale.entries)
            }
            None => Err(err),
        },
    }
}

/// `<app_data>/model_metadata.json`, with the directory created on demand.
///
/// Tauri's `app_data_dir` (not the workspace): the catalogue is shared by every
/// window and instance — a shadow instance must not download its own copy.
fn catalog_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve the app data directory: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    Ok(dir.join(CATALOG_FILE))
}

fn read_catalog(path: &Path) -> Option<CatalogFile> {
    let raw = fs::read(path).ok()?;
    let catalog: CatalogFile = serde_json::from_slice(&raw).ok()?;
    (!catalog.entries.is_empty()).then_some(catalog)
}

/// Persist the merged catalogue, atomically: a crash (or a full disk) halfway
/// through must not leave a truncated file behind, because that file is the
/// offline fallback and anything unparsable would force a 7 MB re-download.
fn write_catalog(app: &tauri::AppHandle, path: &Path, catalog: &CatalogFile) {
    let serialized = match serde_json::to_vec(catalog) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn(app, &format!("Cannot serialize the model metadata catalogue: {err}"));
            return;
        }
    };

    let tmp = path.with_extension("json.tmp");
    let written = fs::write(&tmp, &serialized).and_then(|_| fs::rename(&tmp, path));
    if let Err(err) = written {
        let _ = fs::remove_file(&tmp);
        warn(
            app,
            &format!("Cannot write {}: {err}", path.display()),
        );
    }
}

/// Download all three feeds and merge them.
///
/// Only an all-source failure is an error: a single unreachable mirror (models.dev
/// is 6 MB and slow, github.io sometimes rate-limits) must not cost the caller the
/// whole catalogue.
async fn build_catalog() -> Result<CatalogFile, String> {
    let client = llm_complete::build_http_client()?;
    let mut parts: Vec<Vec<ModelMetadata>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for source in SOURCES {
        match download(&client, source.url).await {
            Ok(body) => {
                let entries = (source.parse)(&body);
                if entries.is_empty() {
                    failures.push(format!("{}: parsed no entries", source.name));
                } else {
                    parts.push(entries);
                }
            }
            Err(err) => failures.push(format!("{}: {err}", source.name)),
        }
    }

    if parts.is_empty() {
        return Err(format!(
            "no model metadata source answered ({})",
            failures.join("; ")
        ));
    }

    Ok(CatalogFile {
        version: CATALOG_VERSION,
        updated_at: now_secs(),
        entries: merge_catalogs(parts),
    })
}

async fn download(client: &reqwest::Client, url: &str) -> Result<serde_json::Value, String> {
    let res = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status()));
    }

    res.json()
        .await
        .map_err(|e| format!("invalid JSON: {e}"))
}

// ─── Merging ────────────────────────────────────────────────────────────────

/// Merge catalogues, in priority order, into one entry per normalised name.
///
/// Keyed by [`normalize_model_name`], so `openai/gpt-4o`, `gpt-4o` and
/// `gpt-4o-2024-08-06` collapse into a single entry — the first (highest
/// priority) name is kept for display, later sources only fill what is missing
/// and OR the capability flags.
///
/// Order matters: the caller passes the parser output in [`SOURCES`] order
/// (models.dev → OpenRouter → llm-metadata), so a model's limits and vendor
/// come from models.dev whenever it lists the model, and llm-metadata only
/// contributes tags, its own flags and the models the other two do not know.
fn merge_catalogs(parts: Vec<Vec<ModelMetadata>>) -> Vec<ModelMetadata> {
    let mut merged: Vec<ModelMetadata> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for entries in parts {
        for entry in entries {
            let key = normalize_model_name(&entry.model_name);
            if key.is_empty() {
                continue;
            }
            match index.get(&key) {
                Some(&at) => merge_into(&mut merged[at], &entry),
                None => {
                    index.insert(key, merged.len());
                    merged.push(entry);
                }
            }
        }
    }

    merged
}

/// Fold `src` into `dst`: scalars only where `dst` has nothing yet, flags are
/// OR-ed (any source that reports a capability is believed), provenance is
/// unioned.
fn merge_into(dst: &mut ModelMetadata, src: &ModelMetadata) {
    for source in &src.sources {
        if !dst.sources.iter().any(|known| known == source) {
            dst.sources.push(source.clone());
        }
    }
    if dst.vendor.is_empty() {
        dst.vendor = src.vendor.clone();
    }
    if dst.description.is_empty() {
        dst.description = src.description.clone();
    }
    if dst.tags.is_empty() {
        dst.tags = src.tags.clone();
    }
    if dst.context_length.is_none() {
        dst.context_length = src.context_length;
    }
    if dst.max_output_tokens.is_none() {
        dst.max_output_tokens = src.max_output_tokens;
    }
    dst.supports_tools |= src.supports_tools;
    dst.supports_reasoning |= src.supports_reasoning;
    dst.supports_vision |= src.supports_vision;
    dst.supports_files |= src.supports_files;
    dst.supports_audio |= src.supports_audio;
    dst.open_weights |= src.open_weights;
    dst.deprecated |= src.deprecated;
}

/// Reduce a model id to a comparable, hyphen-separated token string.
///
/// Mirrors `normalizeModelName` in `src/utils/modelMetadata.ts`:
/// provider namespaces (`openai/gpt-4o`), Bedrock's bare-vendor dot prefix
/// (`meta.llama3-70b`), variant suffixes (`gpt-4o:free`), API revision stamps
/// (`-2024-07-18`, `-20241022`, `-0613`) and serving decorations (`-latest`,
/// `-fp8`) are all stripped, and dots/underscores/spaces become token
/// boundaries so `gemini-2.5-pro` and `gemini-2-5-pro` agree.
///
/// The two normalisers must not drift: the frontend scores remote ids against
/// the names produced here.
fn normalize_model_name(raw: &str) -> String {
    let mut name = raw.trim().to_lowercase();

    if let Some((_, tail)) = name.rsplit_once('/') {
        name = tail.to_string();
    }

    // `vendor.model`, but only for a bare alphabetic prefix, so versions such
    // as `qwen2.5-72b` stay intact.
    if let Some((head, tail)) = name.split_once('.') {
        if !head.is_empty() && head.chars().all(|c| c.is_ascii_alphabetic()) {
            name = tail.to_string();
        }
    }

    if let Some((head, _)) = name.split_once(':') {
        name = head.to_string();
    }

    let name = strip_revision_stamps(name);

    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty() && !DECORATION_TOKENS.contains(token))
        .collect::<Vec<_>>()
        .join("-")
}

/// Drop a trailing API revision stamp: `-2024-07-18`, `-20241022`, `-0613`.
fn strip_revision_stamps(name: String) -> String {
    let name = strip_trailing_segments(&name, &[4, 2, 2]).unwrap_or(name);
    let name = strip_trailing_segment(&name, |segment| is_digits(segment, 8)).to_string();
    strip_trailing_segment(&name, |segment| is_digits(segment, 4)).to_string()
}

/// Drop the last `-<segment>` when `predicate` accepts it.
fn strip_trailing_segment<'a>(name: &'a str, predicate: impl Fn(&str) -> bool) -> &'a str {
    match name.rsplit_once('-') {
        Some((head, tail)) if predicate(tail) => head,
        _ => name,
    }
}

/// Drop the last `lengths.len()` segments when each is all digits and exactly
/// the given length (`-2024-07-18` → `[4, 2, 2]`).
fn strip_trailing_segments(name: &str, lengths: &[usize]) -> Option<String> {
    let mut head = name;
    for length in lengths.iter().rev() {
        let (shortened, tail) = head.rsplit_once('-')?;
        if !is_digits(tail, *length) {
            return None;
        }
        head = shortened;
    }
    Some(head.to_string())
}

fn is_digits(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_digit())
}

// ─── Parsers ────────────────────────────────────────────────────────────────

/// llm-metadata: `{ data: [{ model_name, vendor_name, description, tags }] }`.
fn parse_llm_metadata(body: &serde_json::Value) -> Vec<ModelMetadata> {
    // The feed wraps the array in `{ data: [...], message, success }`; accept a
    // bare array too so a format tweak upstream does not break the feature.
    let data = body
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| body.as_array());

    let Some(data) = data else {
        return Vec::new();
    };

    data.iter().filter_map(parse_llm_metadata_entry).collect()
}

fn parse_llm_metadata_entry(raw: &serde_json::Value) -> Option<ModelMetadata> {
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
        max_output_tokens: None,
        supports_tools: has("Tools"),
        supports_reasoning: has("Reasoning"),
        supports_vision: has("Vision"),
        supports_files: has("Files"),
        supports_audio: has("Audio"),
        open_weights: has("Open Weights"),
        deprecated: has("Deprecated"),
        tags,
        sources: vec![LLM_METADATA_SOURCE.to_string()],
        match_score: None,
    })
}

/// models.dev: `{ <providerId>: { name, models: { <key>: { id, name, … } } } }`.
fn parse_models_dev(body: &serde_json::Value) -> Vec<ModelMetadata> {
    let Some(providers) = body.as_object() else {
        return Vec::new();
    };

    // Most models are resold by dozens of gateways with slightly different
    // numbers (Claude reports 200 000 tokens at Anthropic and 1 000 000 at some
    // routers). Visit the vendor's own entry first, then the rest by id, so the
    // first value that lands in the merge is the authoritative one and the whole
    // thing stays deterministic across runs.
    let mut provider_ids: Vec<&String> = providers.keys().collect();
    provider_ids.sort_by_key(|id| (provider_rank(id), id.as_str()));

    let mut entries = Vec::new();
    for id in provider_ids {
        let Some(provider) = providers.get(id).and_then(|v| v.as_object()) else {
            continue;
        };
        let vendor = provider
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(id)
            .trim()
            .to_string();
        let Some(models) = provider.get("models").and_then(|v| v.as_object()) else {
            continue;
        };
        for (key, raw) in models {
            if let Some(entry) = parse_models_dev_entry(&vendor, key, raw) {
                entries.push(entry);
            }
        }
    }
    entries
}

fn parse_models_dev_entry(
    provider: &str,
    key: &str,
    raw: &serde_json::Value,
) -> Option<ModelMetadata> {
    let id = string_field(raw, "id");
    let id = if id.is_empty() {
        key.trim().to_string()
    } else {
        id
    };
    if id.is_empty() {
        return None;
    }

    // `name` is the pretty label, except when it carries a separator the name
    // normaliser cuts on (`gemma4:31b`, `kimi/kimi-k2.5`) — then the id is both
    // prettier and closer to what a provider's `/models` reports.
    let name = string_field(raw, "name");
    let model_name = if name.is_empty() || name.contains(':') || name.contains('/') {
        id
    } else {
        name
    };

    let limit = raw.get("limit");
    let input = list_field(raw.get("modalities"), "input");
    let output = list_field(raw.get("modalities"), "output");
    let attachment = bool_field(raw, "attachment");

    Some(ModelMetadata {
        model_name,
        vendor: provider.to_string(),
        description: truncate(&string_field(raw, "description"), DESCRIPTION_LIMIT),
        tags: Vec::new(),
        context_length: u32_field(limit, "context"),
        max_output_tokens: u32_field(limit, "output"),
        supports_tools: bool_field(raw, "tool_call"),
        supports_reasoning: bool_field(raw, "reasoning"),
        supports_vision: contains(&input, "image"),
        supports_files: attachment || contains(&input, "pdf") || contains(&input, "file"),
        supports_audio: contains(&input, "audio") || contains(&output, "audio"),
        open_weights: bool_field(raw, "open_weights"),
        deprecated: string_field(raw, "status").eq_ignore_ascii_case("deprecated"),
        sources: vec![MODELS_DEV_SOURCE.to_string()],
        match_score: None,
    })
}

/// OpenRouter: `{ data: [{ id, name, context_length, architecture, … }] }`.
fn parse_openrouter(body: &serde_json::Value) -> Vec<ModelMetadata> {
    let Some(data) = body.get("data").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    data.iter().filter_map(parse_openrouter_entry).collect()
}

fn parse_openrouter_entry(raw: &serde_json::Value) -> Option<ModelMetadata> {
    let id = string_field(raw, "id");
    if id.is_empty() {
        return None;
    }

    let architecture = raw.get("architecture");
    let input = list_field(architecture, "input_modalities");
    let output = list_field(architecture, "output_modalities");
    let parameters = list_field(Some(raw), "supported_parameters");
    let has_param = |name: &str| parameters.iter().any(|item| item == name);
    let top_provider = raw.get("top_provider");

    // The display name is `Google: Gemini 4 Hydrogen` — a colon the normaliser
    // would cut on — so the id doubles as the catalogue name here.
    let vendor = match id.split_once('/') {
        Some((vendor, _)) => vendor.to_string(),
        None => String::new(),
    };

    Some(ModelMetadata {
        model_name: id,
        vendor,
        description: truncate(&string_field(raw, "description"), DESCRIPTION_LIMIT),
        tags: Vec::new(),
        context_length: u32_field(Some(raw), "context_length")
            .or_else(|| u32_field(top_provider, "context_length")),
        max_output_tokens: u32_field(top_provider, "max_completion_tokens"),
        supports_tools: has_param("tools") || has_param("tool_choice"),
        supports_reasoning: has_param("reasoning")
            || has_param("include_reasoning")
            || has_param("reasoning_effort")
            || raw.get("reasoning").map(|v| v.is_object()).unwrap_or(false),
        supports_vision: contains(&input, "image"),
        supports_files: contains(&input, "file") || contains(&input, "pdf"),
        supports_audio: contains(&input, "audio") || contains(&output, "audio"),
        // OpenRouter links the weights when the model has them; closed models
        // carry `null`, and a few entries ship an empty string instead.
        open_weights: raw
            .get("hugging_face_id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| !id.trim().is_empty()),
        deprecated: false,
        sources: vec![OPENROUTER_SOURCE.to_string()],
        match_score: None,
    })
}

/// Position of a models.dev provider in [`PREFERRED_PROVIDERS`]; providers
/// outside the list share the final rank.
fn provider_rank(id: &str) -> usize {
    PREFERRED_PROVIDERS
        .iter()
        .position(|preferred| preferred.eq_ignore_ascii_case(id))
        .unwrap_or(PREFERRED_PROVIDERS.len())
}

fn string_field(raw: &serde_json::Value, key: &str) -> String {
    raw.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn bool_field(raw: &serde_json::Value, key: &str) -> bool {
    raw.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Positive `u32` at `parent[key]`, tolerating floats (`131072.0`).
fn u32_field(parent: Option<&serde_json::Value>, key: &str) -> Option<u32> {
    let value = parent?.get(key)?;
    let number = value
        .as_u64()
        .or_else(|| value.as_f64().map(|n| n.round().max(0.0) as u64))?;
    (number > 0 && number <= u32::MAX as u64).then_some(number as u32)
}

/// Lowercased strings of the array at `parent[key]`.
fn list_field(parent: Option<&serde_json::Value>, key: &str) -> Vec<String> {
    parent
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(|item| item.trim().to_lowercase())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn contains(values: &[String], wanted: &str) -> bool {
    values.iter().any(|value| value == wanted)
}

/// Clip `text` to `limit` chars (never mid-character) for the one-line hint in
/// the Settings panel.
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let clipped: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", clipped.trim_end())
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

    fn entry(name: &str, source: &str) -> ModelMetadata {
        ModelMetadata {
            model_name: name.to_string(),
            vendor: String::new(),
            description: String::new(),
            tags: Vec::new(),
            context_length: None,
            max_output_tokens: None,
            supports_tools: false,
            supports_reasoning: false,
            supports_vision: false,
            supports_files: false,
            supports_audio: false,
            open_weights: false,
            deprecated: false,
            sources: vec![source.to_string()],
            match_score: None,
        }
    }

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
        let entry = parse_llm_metadata_entry(&raw).expect("entry parsed");
        assert_eq!(entry.model_name, "gpt-4o");
        assert_eq!(entry.vendor, "OpenAI");
        assert_eq!(entry.context_length, Some(128_000));
        assert!(entry.supports_tools && entry.supports_vision && entry.supports_files);
        assert!(!entry.supports_reasoning && !entry.deprecated);
        assert_eq!(entry.sources, vec![LLM_METADATA_SOURCE.to_string()]);
        assert_eq!(entry.match_score, None);
    }

    #[test]
    fn skips_entries_without_a_name() {
        assert!(parse_llm_metadata_entry(&serde_json::json!({ "tags": "Tools" })).is_none());
        assert!(parse_llm_metadata_entry(&serde_json::json!({ "model_name": "  " })).is_none());
    }

    /// The Rust normaliser has to agree with `normalizeModelName` in
    /// `src/utils/modelMetadata.ts`, otherwise the merge groups models the
    /// frontend matches separately.
    #[test]
    fn normalises_ids_like_the_frontend() {
        assert_eq!(normalize_model_name("openai/gpt-4o"), "gpt-4o");
        assert_eq!(normalize_model_name("gpt-4o"), "gpt-4o");
        assert_eq!(normalize_model_name("GPT-4o-Latest"), "gpt-4o");
        assert_eq!(normalize_model_name("gpt-4o-mini-2024-07-18"), "gpt-4o-mini");
        assert_eq!(normalize_model_name("claude-3-5-sonnet-20241022"), "claude-3-5-sonnet");
        assert_eq!(normalize_model_name("gpt-3.5-turbo-0613"), "gpt-3-5-turbo");
        assert_eq!(normalize_model_name("meta.llama3-70b-instruct-v1:0"), "llama3-70b-instruct-v1");
        assert_eq!(normalize_model_name("gemini-2.5-pro"), "gemini-2-5-pro");
        assert_eq!(normalize_model_name("qwen2.5-72b-instruct"), "qwen2-5-72b-instruct");
        assert_eq!(normalize_model_name("llama-3-8b:free"), "llama-3-8b");
        assert_eq!(normalize_model_name("  "), "");
    }

    /// The merge runs in [`SOURCES`] order — models.dev, then OpenRouter, then
    /// llm-metadata — so the widest source's numbers win and the later ones
    /// only fill gaps and add flags.
    #[test]
    fn later_sources_only_fill_gaps() {
        let mut base = entry("GPT-4o", MODELS_DEV_SOURCE);
        base.context_length = Some(200_000);
        base.max_output_tokens = Some(16_384);
        base.supports_vision = true;
        base.vendor = "OpenAI".to_string();

        let mut fallback = entry("openai/gpt-4o", OPENROUTER_SOURCE);
        fallback.context_length = Some(128_000);
        fallback.description = "Omni model".to_string();
        fallback.supports_tools = true;

        let mut third = entry("gpt-4o", LLM_METADATA_SOURCE);
        third.tags = vec!["Tools".to_string(), "Vision".to_string()];
        third.supports_reasoning = true;
        third.vendor = "SomeGateway".to_string();

        let merged = merge_catalogs(vec![vec![base], vec![fallback], vec![third]]);
        assert_eq!(merged.len(), 1);
        let only = &merged[0];
        assert_eq!(only.model_name, "GPT-4o", "the top source's name is kept");
        assert_eq!(only.context_length, Some(200_000), "models.dev wins");
        assert_eq!(only.max_output_tokens, Some(16_384), "models.dev wins");
        assert_eq!(only.description, "Omni model", "gap filled by openrouter");
        assert_eq!(
            only.tags,
            vec!["Tools".to_string(), "Vision".to_string()],
            "tags still arrive from llm-metadata"
        );
        assert_eq!(only.vendor, "OpenAI");
        assert!(only.supports_tools && only.supports_vision && only.supports_reasoning);
        assert_eq!(
            only.sources,
            vec![
                MODELS_DEV_SOURCE.to_string(),
                OPENROUTER_SOURCE.to_string(),
                LLM_METADATA_SOURCE.to_string()
            ]
        );
    }

    /// Pins the priority order the rest of the module assumes.
    #[test]
    fn models_dev_leads_the_merge_order() {
        let order: Vec<&str> = SOURCES.iter().map(|source| source.name).collect();
        assert_eq!(
            order,
            vec![MODELS_DEV_SOURCE, OPENROUTER_SOURCE, LLM_METADATA_SOURCE]
        );
    }

    #[test]
    fn keeps_models_only_a_fallback_knows() {
        let merged = merge_catalogs(vec![
            vec![entry("gpt-4o", MODELS_DEV_SOURCE)],
            vec![entry("brand-new-model", LLM_METADATA_SOURCE)],
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].model_name, "brand-new-model");
    }

    #[test]
    fn reads_models_dev_entries() {
        let body = serde_json::json!({
            "zai": {
                "id": "zai",
                "name": "Z.ai",
                "models": {
                    "glm-5.3": {
                        "id": "zai-org/GLM-5.3",
                        "name": "GLM-5.3",
                        "description": "Flagship GLM model",
                        "tool_call": true,
                        "reasoning": true,
                        "attachment": true,
                        "open_weights": true,
                        "modalities": { "input": ["text", "image"], "output": ["text"] },
                        "limit": { "context": 262144, "output": 131072 },
                        "status": "beta"
                    }
                }
            }
        });

        let entries = parse_models_dev(&body);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.model_name, "GLM-5.3");
        assert_eq!(entry.vendor, "Z.ai");
        assert_eq!(entry.context_length, Some(262_144));
        assert_eq!(entry.max_output_tokens, Some(131_072));
        assert!(entry.supports_tools && entry.supports_reasoning);
        assert!(entry.supports_vision && entry.supports_files);
        assert!(entry.open_weights);
        assert!(!entry.deprecated);
        assert_eq!(entry.sources, vec![MODELS_DEV_SOURCE.to_string()]);
    }

    /// The vendor's own provider entry is read before any reseller, so its
    /// limits are the ones that survive the merge.
    #[test]
    fn vendor_provider_wins_over_resellers() {
        let body = serde_json::json!({
            "aaa-router": {
                "name": "AAA Router",
                "models": {
                    "claude-sonnet-4-5": {
                        "id": "claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5",
                        "limit": { "context": 1000000, "output": 64000 }
                    }
                }
            },
            "anthropic": {
                "name": "Anthropic",
                "models": {
                    "claude-sonnet-4-5": {
                        "id": "claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5",
                        "limit": { "context": 200000, "output": 64000 }
                    }
                }
            }
        });

        let entries = merge_catalogs(vec![parse_models_dev(&body)]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].vendor, "Anthropic");
        assert_eq!(entries[0].context_length, Some(200_000));
    }

    #[test]
    fn reads_openrouter_entries() {
        let body = serde_json::json!({
            "data": [{
                "id": "google/gemini-4-hydrogen",
                "name": "Google: Gemini 4 Hydrogen",
                "description": "High-efficiency workhorse model.",
                "context_length": 1048576,
                "architecture": {
                    "input_modalities": ["text", "image", "file", "audio"],
                    "output_modalities": ["text"]
                },
                "top_provider": { "context_length": 1048576, "max_completion_tokens": 65536 },
                "supported_parameters": ["max_tokens", "tools", "tool_choice", "reasoning"],
                "hugging_face_id": "google/gemini-4-hydrogen"
            }]
        });

        let entries = parse_openrouter(&body);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.model_name, "google/gemini-4-hydrogen");
        assert_eq!(entry.vendor, "google");
        assert_eq!(entry.context_length, Some(1_048_576));
        assert_eq!(entry.max_output_tokens, Some(65_536));
        assert!(entry.supports_tools && entry.supports_reasoning);
        assert!(entry.supports_vision && entry.supports_files && entry.supports_audio);
        assert!(entry.open_weights);
    }

    /// Closed models ship `null`, but some entries carry an empty string where
    /// the weights would go — that is not a link to open weights.
    #[test]
    fn empty_hugging_face_ids_are_not_open_weights() {
        let body = serde_json::json!({
            "data": [
                { "id": "openai/gpt-4o", "hugging_face_id": "" },
                { "id": "openai/gpt-4o-mini", "hugging_face_id": null },
                { "id": "meta/llama-4", "hugging_face_id": "meta-llama/llama-4" }
            ]
        });

        let entries = parse_openrouter(&body);
        assert!(!entries[0].open_weights);
        assert!(!entries[1].open_weights);
        assert!(entries[2].open_weights);
    }

    #[test]
    fn tolerates_empty_and_foreign_payloads() {
        for parse in [parse_llm_metadata, parse_models_dev, parse_openrouter] {
            assert!(parse(&serde_json::json!({})).is_empty());
            assert!(parse(&serde_json::json!([])).is_empty());
            assert!(parse(&serde_json::json!("nope")).is_empty());
        }
    }

    #[test]
    fn weekly_window_decides_freshness() {
        let now = now_secs();
        assert!(is_fresh(now));
        assert!(is_fresh(now - 6 * 24 * 60 * 60));
        assert!(!is_fresh(now - 8 * 24 * 60 * 60));
        // A stamp in the future is treated as fresh, not as "re-download".
        assert!(is_fresh(now + 3600));
    }

    #[test]
    fn catalog_file_round_trips() {
        let mut first = entry("gpt-4o", LLM_METADATA_SOURCE);
        first.context_length = Some(128_000);
        let catalog = CatalogFile {
            version: CATALOG_VERSION,
            updated_at: 1_700_000_000,
            entries: vec![first],
        };

        let encoded = serde_json::to_string(&catalog).expect("serialized");
        let decoded: CatalogFile = serde_json::from_str(&encoded).expect("parsed");
        assert_eq!(decoded.version, CATALOG_VERSION);
        assert_eq!(decoded.updated_at, 1_700_000_000);
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(decoded.entries[0].context_length, Some(128_000));
        // Optional fields stay out of the file when empty.
        assert!(!encoded.contains("match_score"));
        assert!(!encoded.contains("max_output_tokens"));
    }

    #[test]
    fn truncates_long_descriptions() {
        let long = "x".repeat(DESCRIPTION_LIMIT + 50);
        let clipped = truncate(&long, DESCRIPTION_LIMIT);
        assert_eq!(clipped.chars().count(), DESCRIPTION_LIMIT);
        assert!(clipped.ends_with('…'));
        assert_eq!(truncate("short", DESCRIPTION_LIMIT), "short");
    }

    /// Download the three live feeds and check the merge is still usable.
    ///
    /// Ignored by default (it needs the network, like
    /// `update::tests::live_release_lookup`); run it after touching a parser
    /// with `cargo test --manifest-path src-tauri/Cargo.toml live_merge -- --ignored`.
    #[test]
    #[ignore = "hits the public catalogues"]
    fn live_merge_produces_a_usable_catalogue() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let catalog = runtime.block_on(build_catalog()).expect("catalogue");
        let entries = &catalog.entries;

        assert!(entries.len() > 500, "only {} entries merged", entries.len());
        let with_context = entries.iter().filter(|e| e.context_length.is_some()).count();
        assert!(
            with_context * 4 > entries.len() * 3,
            "context window known for only {with_context}/{} entries",
            entries.len()
        );

        let flagship = entries
            .iter()
            .find(|e| normalize_model_name(&e.model_name) == "gpt-4o")
            .expect("gpt-4o in the merged catalogue");
        // models.dev lists gpt-4o, so it supplies the numbers for it…
        // (unless the mirror itself was unreachable — a flaky feed must not
        // fail this smoke test, so the check is gated on models.dev answering)
        if entries
            .iter()
            .any(|entry| entry.sources.iter().any(|s| s == MODELS_DEV_SOURCE))
        {
            assert!(
                flagship.sources.iter().any(|s| s == MODELS_DEV_SOURCE),
                "models.dev answered but does not own gpt-4o"
            );
            assert_eq!(flagship.context_length, Some(128_000));
            assert_eq!(flagship.max_output_tokens, Some(16_384));
        }
        assert!(flagship.supports_tools && flagship.supports_vision);
        assert!(flagship.context_length.is_some_and(|ctx| ctx >= 128_000));
        // …while llm-metadata still contributes the tags of the merge.
        assert!(entries.iter().any(|e| !e.tags.is_empty()));
        assert!(entries.iter().any(|e| e.max_output_tokens.is_some()));
    }
}
