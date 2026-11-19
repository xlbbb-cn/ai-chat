//! Fetching the model capability catalogue for the app.
//!
//! The catalogue itself is **built in CI** by the `model-catalog` crate (see
//! `src-tauri/src/utils/model-catalog/`): a `pages.yml` step downloads the
//! three public catalogues (models.dev, OpenRouter, llm-metadata), merges them
//! and publishes `model_metadata.json` through GitHub Pages. This module only
//! *fetches* that file — the app never downloads the ~7 MB of raw feeds — and
//! keeps it as the offline fallback in `<app_data>/model_metadata.json`.
//!
//! `force` (the Settings panel's manual refresh) skips the memory cache and
//! the file, otherwise the catalogue is served from memory, then from the
//! local file while it is younger than [`CATALOG_TTL_SECS`]. The file's
//! `updated_at` is the CI download stamp, so the TTL measures the data's age,
//! not when this machine last fetched it. A failed fetch is reported as an
//! error only when there is no existing catalogue at all — the model list
//! itself never depends on this feed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tauri::Manager;

use model_catalog::{CatalogFile, ensure_usable, now_secs, CATALOG_VERSION};
// `ipc/config.rs` and `AppConfig` refer to `model_metadata::ModelMetadata`.
pub use model_catalog::ModelMetadata;

use crate::http::build_http_client;

/// The merged catalogue CI publishes through GitHub Pages. Raw feeds are never
/// fetched here; `model-catalog`'s bin does that in the Pages deploy.
const CATALOG_URL: &str = "https://xlbbb-cn.github.io/ai-chat/model_metadata.json";

/// Merged catalogue inside `<app_data>`, with its `updated_at` timestamp.
const CATALOG_FILE: &str = "model_metadata.json";

/// How long a downloaded catalogue is reused (file *and* memory) before it is
/// fetched from Pages again.
const CATALOG_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// Last catalogue loaded from disk or network, kept so repeated calls (every
/// Settings refresh) do not re-read or re-parse the file. The format version
/// rides along so a stale-format file remembered as the offline fallback
/// cannot come back through [`cached_fresh`] as if it were current.
struct CachedCatalog {
    version: u32,
    updated_at: u64,
    entries: Vec<ModelMetadata>,
}

static CACHE: OnceLock<Mutex<Option<CachedCatalog>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CachedCatalog>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Serializes refreshes: two Settings calls at once must not download the
/// catalogue twice.
fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Is a catalogue stamped `updated_at` still inside the weekly window?
///
/// A stamp in the future (clock skew, restored backup) counts as fresh: the
/// alternative is re-downloading on every call.
fn is_fresh(updated_at: u64) -> bool {
    now_secs().saturating_sub(updated_at) < CATALOG_TTL_SECS
}

/// Is a parsed catalogue usable as the *fresh* answer — written by the current
/// format and inside the TTL window? A version mismatch (a file from another
/// format, or a CI that moved ahead of this build) parses fine and stays the
/// offline fallback, but it must never be served as fresh, or this build would
/// keep matching against names written by a different format until the TTL
/// expires.
fn catalog_is_fresh(catalog: &CatalogFile) -> bool {
    catalog.version == CATALOG_VERSION && is_fresh(catalog.updated_at)
}

/// Entries of a catalogue that is still fresh, when one is in memory.
fn cached_fresh() -> Option<Vec<ModelMetadata>> {
    let guard = cache().lock().ok()?;
    let entry = guard.as_ref()?;
    (entry.version == CATALOG_VERSION && is_fresh(entry.updated_at))
        .then(|| entry.entries.clone())
}

fn remember(catalog: &CatalogFile) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CachedCatalog {
            version: catalog.version,
            updated_at: catalog.updated_at,
            entries: catalog.entries.clone(),
        });
    }
}

/// Report a degraded refresh (a failed fetch, a stale catalogue) in `app.log`.
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

/// Return the model-metadata catalogue: llm-metadata, models.dev and
/// OpenRouter merged into one entry per model, built by CI and published
/// through GitHub Pages.
///
/// Served from memory, then from `<app_data>/model_metadata.json` while that
/// file is younger than [`CATALOG_TTL_SECS`] and written by the current
/// [`CATALOG_VERSION`]; otherwise the catalogue is downloaded from
/// [`CATALOG_URL`], validated, written back and returned. `force` skips both
/// caches.
///
/// A failed download is reported as an error so the caller can degrade
/// gracefully — the model list itself never depends on this feed — but an
/// existing (stale) catalogue is served instead whenever there is one.
pub(crate) async fn fetch_model_metadata(
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
            // An older format is remembered (it still serves as the offline
            // fallback when Pages is unreachable) but not answered as fresh,
            // so this build re-downloads instead of trusting its stored names.
            let fresh = catalog_is_fresh(&catalog);
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

    match fetch_catalog().await {
        Ok(catalog) => {
            if catalog.version != CATALOG_VERSION {
                warn(
                    &app,
                    &format!(
                        "model metadata catalogue from Pages carries version {}, \
                         this build expects {CATALOG_VERSION}",
                        catalog.version
                    ),
                );
            }
            remember(&catalog);
            write_catalog(&app, &path, &catalog);
            Ok(catalog.entries)
        }
        Err(err) => match read_catalog(&path) {
            Some(stale) => {
                warn(
                    &app,
                    &format!(
                        "model metadata fetch failed ({err}); serving the cached catalogue \
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

/// Download the merged catalogue from [`CATALOG_URL`].
///
/// The sanity gate (`ensure_usable`) is the same one the CI builder applies
/// before publishing, so a deployed stub (a broken merge, a deploy of the
/// wrong branch) is refused instead of replacing a good local file.
async fn fetch_catalog() -> Result<CatalogFile, String> {
    let client = build_http_client()?;
    let res = client
        .get(CATALOG_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status()));
    }

    let catalog: CatalogFile = res
        .json()
        .await
        .map_err(|e| format!("invalid JSON: {e}"))?;
    ensure_usable(&catalog)?;
    Ok(catalog)
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

/// Persist the fetched catalogue, atomically: a crash (or a full disk) halfway
/// through must not leave a truncated file behind, because that file is the
/// offline fallback and anything unparsable would force a re-download.
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

#[cfg(test)]
mod tests {
    use super::*;
    use model_catalog::ModelMetadata as Entry;

    fn entry(name: &str) -> Entry {
        Entry {
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
            sources: Vec::new(),
            match_score: None,
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

    /// A catalogue written by another format parses (it stays the offline
    /// fallback) but never counts as fresh, so this build re-downloads
    /// instead of serving stored names from a different format.
    #[test]
    fn version_mismatch_is_not_fresh() {
        let mut catalog = CatalogFile {
            version: CATALOG_VERSION + 1,
            updated_at: now_secs(),
            entries: vec![entry("gpt-4o")],
        };
        assert!(!catalog_is_fresh(&catalog));

        catalog.version = CATALOG_VERSION;
        assert!(catalog_is_fresh(&catalog));

        // The current format outside the TTL window is stale all the same.
        catalog.updated_at = now_secs() - 8 * 24 * 60 * 60;
        assert!(!catalog_is_fresh(&catalog));
    }

    #[test]
    fn catalog_file_round_trips() {
        let mut first = entry("gpt-4o");
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

    /// Download the live Pages catalogue and check it parses and passes the
    /// usability gate the CI builder already applied.
    ///
    /// Ignored by default (it needs the network and a deployed catalogue); run
    /// it with `cargo test --manifest-path src-tauri/Cargo.toml live_pages -- --ignored`
    /// after touching the fetch path.
    #[test]
    #[ignore = "downloads the live Pages catalogue"]
    fn live_pages_catalogue_is_usable() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let catalog = runtime.block_on(fetch_catalog()).expect("catalogue");
        assert_eq!(catalog.version, CATALOG_VERSION);
        assert!(catalog.entries.len() > 500, "only {} entries", catalog.entries.len());
        assert!(catalog.entries.iter().any(|e| e.context_length.is_some()));
    }
}
