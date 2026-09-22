//! Update checking against the project's GitHub Releases.
//!
//! `check_update` reads the newest **published** release (draft releases are
//! invisible to the public GitHub API) and compares its tag with the version of
//! the running app, reporting the bundles attached to it.
//!
//! `download_update` streams the selected asset into the user's downloads folder
//! while emitting `update-download-progress` events so the UI can show a bar.
//! `open_update_file` / `reveal_update_file` / `open_release_page` hand the result
//! to the OS.

use crate::AppState;
use futures_util::StreamExt;
use serde::Serialize;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::AsyncWriteExt;

const REPO_SLUG: &str = "xlbbb-cn/ai-chat";
const RELEASES_URL: &str = "https://api.github.com/repos/xlbbb-cn/ai-chat/releases?per_page=20";
/// Only these URLs may be handed to the OS by `open_release_page`.
const RELEASE_URL_PREFIX: &str = "https://github.com/xlbbb-cn/ai-chat/";
/// GitHub rejects requests without a User-Agent.
const USER_AGENT: &str = "ai-chat-desktop";
const DOWNLOAD_PROGRESS_EVENT: &str = "update-download-progress";
/// Emit at most one progress event per this many downloaded bytes.
const PROGRESS_STEP: u64 = 256 * 1024;
/// Assets scoring at or above this value are marked as recommended for this machine.
const RECOMMEND_THRESHOLD: i32 = 60;

/// `serde` default for `AppConfig::check_updates_on_startup` (missing field = on).
pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
    /// nsis | msi | exe | dmg | appimage | deb | rpm | archive
    pub kind: String,
    /// True when the asset matches this OS/arch and is a preferred format.
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_name: String,
    pub release_notes: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub assets: Vec<UpdateAsset>,
    /// False when the newest release ships no bundle for this OS/arch.
    pub has_platform_asset: bool,
    /// e.g. "windows-x86_64"
    pub platform: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateDownloadProgress {
    pub asset_name: String,
    pub downloaded: u64,
    pub total: u64,
    pub done: bool,
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

fn platform_tag() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// "v1.2.3" / "1.2.3-beta.1" -> "1.2.3"
fn normalize_version(raw: &str) -> String {
    raw.trim().trim_start_matches(['v', 'V']).trim().to_string()
}

/// Numeric part of a version, e.g. "1.2.3-rc1" -> [1, 2, 3]. Unparsable parts become 0.
fn parse_version(raw: &str) -> Vec<u64> {
    normalize_version(raw)
        .split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|part| part.trim().parse::<u64>().unwrap_or(0))
        .collect()
}

/// Compare dotted versions, treating missing components as 0 ("1.2" == "1.2.0").
fn compare_versions(a: &[u64], b: &[u64]) -> Ordering {
    let len = a.len().max(b.len());
    for i in 0..len {
        let left = a.get(i).copied().unwrap_or(0);
        let right = b.get(i).copied().unwrap_or(0);
        match left.cmp(&right) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// Pick the highest version among the published releases.
fn pick_newest_release<'a>(
    releases: &'a [serde_json::Value],
    include_prerelease: bool,
) -> Option<(&'a serde_json::Value, String)> {
    releases
        .iter()
        .filter(|release| {
            !release
                .get("draft")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .filter(|release| {
            include_prerelease
                || !release
                    .get("prerelease")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
        })
        .filter_map(|release| {
            let tag = release.get("tag_name").and_then(|v| v.as_str())?;
            Some((release, normalize_version(tag)))
        })
        .max_by(|(_, a), (_, b)| compare_versions(&parse_version(a), &parse_version(b)))
}

fn classify_asset(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    // Signatures / updater manifests are not installable bundles.
    if lower.ends_with(".sig")
        || lower.ends_with(".json")
        || lower.ends_with(".txt")
        || lower.ends_with(".sha256")
    {
        return None;
    }
    if lower.ends_with("-setup.exe") {
        return Some("nsis");
    }
    for (suffix, kind) in [
        (".msi", "msi"),
        (".dmg", "dmg"),
        (".appimage", "appimage"),
        (".deb", "deb"),
        (".rpm", "rpm"),
        (".exe", "exe"),
        (".zip", "archive"),
        (".tar.gz", "archive"),
    ] {
        if lower.ends_with(suffix) {
            return Some(kind);
        }
    }
    None
}

/// True when the name carries no architecture tag, or one matching this machine.
fn arch_matches(name: &str) -> bool {
    const TAGS: [&str; 6] = ["aarch64", "arm64", "x64", "x86_64", "amd64", "i686"];
    let lower = name.to_ascii_lowercase();
    let wanted: &[&str] = match std::env::consts::ARCH {
        "aarch64" => &["aarch64", "arm64"],
        "x86_64" => &["x64", "x86_64", "amd64"],
        // Unknown architecture: stay permissive and let the user decide.
        _ => return true,
    };
    if wanted.iter().any(|tag| lower.contains(tag)) {
        return true;
    }
    // No architecture mentioned at all (e.g. universal builds) counts as a match.
    !TAGS.iter().any(|tag| lower.contains(tag))
}

/// Suitability of an asset for the current machine; 0 means "not for this platform".
fn platform_score(kind: &str, name: &str) -> i32 {
    let base = match (std::env::consts::OS, kind) {
        ("windows", "nsis") => 100,
        ("windows", "msi") => 95,
        ("windows", "exe") => 70,
        ("macos", "dmg") => 100,
        ("macos", "archive") => 50,
        ("linux", "appimage") => 100,
        ("linux", "deb") => 90,
        ("linux", "rpm") => 85,
        ("linux", "archive") => 40,
        _ => 0,
    };
    if base > 0 && arch_matches(name) {
        base
    } else {
        0
    }
}

fn collect_assets(release: &serde_json::Value) -> Vec<UpdateAsset> {
    let mut scored: Vec<(i32, UpdateAsset)> = release
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|asset| {
                    let name = asset.get("name").and_then(|v| v.as_str())?;
                    let kind = classify_asset(name)?;
                    let download_url = asset
                        .get("browser_download_url")
                        .and_then(|v| v.as_str())?
                        .to_string();
                    let size = asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                    let score = platform_score(kind, name);
                    Some((
                        score,
                        UpdateAsset {
                            name: name.to_string(),
                            download_url,
                            size,
                            kind: kind.to_string(),
                            recommended: false,
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    // Best match first, then alphabetically for a stable order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

    // Flag only the preferred bundle for this machine (e.g. the NSIS setup on
    // Windows); the remaining platform bundles stay available but unmarked.
    let best = scored.first().map(|(score, _)| *score).unwrap_or(0);
    for (score, asset) in scored.iter_mut() {
        asset.recommended = best >= RECOMMEND_THRESHOLD && *score == best;
    }

    scored.into_iter().map(|(_, asset)| asset).collect()
}

fn api_error_message(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap) -> String {
    if status.as_u16() == 403 || status.as_u16() == 429 {
        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok())
            .map(|epoch| format!(" (rate limit resets at {epoch})"))
            .unwrap_or_default();
        return format!("GitHub API rate limit reached{reset}. Try again later.");
    }
    if status.as_u16() == 404 {
        return "GitHub repository not found (or the release was deleted).".to_string();
    }
    format!("GitHub API returned HTTP {status}.")
}

/// GET the published release list, mapping HTTP failures to readable messages.
async fn fetch_releases() -> Result<Vec<serde_json::Value>, String> {
    let response = build_client()?
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Could not reach the GitHub Releases API: {e}"))?;

    if !response.status().is_success() {
        return Err(api_error_message(response.status(), response.headers()));
    }

    response
        .json()
        .await
        .map_err(|e| format!("Could not parse the GitHub release list: {e}"))
}

/// Fetch the newest published release and compare it with the running version.
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    state: State<'_, AppState>,
    include_prerelease: Option<bool>,
) -> Result<UpdateInfo, String> {
    let include_prerelease = match include_prerelease {
        Some(value) => value,
        None => state.config.lock().unwrap().include_prerelease_updates,
    };
    let current_version = app.package_info().version.to_string();

    let releases = fetch_releases().await?;

    let (release, latest_version) =
        pick_newest_release(&releases, include_prerelease).ok_or_else(|| {
            "No published release found. Releases that are still drafts are not visible here."
                .to_string()
        })?;

    let assets = collect_assets(release);
    let has_update = compare_versions(&parse_version(&latest_version), &parse_version(&current_version))
        == Ordering::Greater;

    state.logger.lock().unwrap().log(
        "info",
        &format!(
            "update check: current={current_version} latest={latest_version} has_update={has_update} assets={}",
            assets.len()
        ),
    );

    Ok(UpdateInfo {
        current_version,
        latest_version: latest_version.clone(),
        has_update,
        release_name: release
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&latest_version)
            .to_string(),
        release_notes: release
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        release_url: release
            .get("html_url")
            .and_then(|v| v.as_str())
            .map(|url| url.to_string())
            .unwrap_or_else(|| format!("{RELEASE_URL_PREFIX}releases/tag/v{latest_version}")),
        published_at: release
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(|value| value.to_string()),
        prerelease: release
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        has_platform_asset: assets.iter().any(|asset| asset.recommended),
        assets,
        platform: platform_tag(),
    })
}

/// Reduce an asset name to a safe plain file name (no separators, no hidden/dot names).
fn sanitize_file_name(name: &str) -> Result<String, String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() || cleaned.len() > 200 {
        return Err("Invalid asset file name.".to_string());
    }
    Ok(cleaned)
}

/// Download one release asset into the downloads folder, streaming progress events.
#[tauri::command]
pub async fn download_update(
    app: AppHandle,
    state: State<'_, AppState>,
    asset_name: String,
    download_url: String,
) -> Result<String, String> {
    if !download_url.starts_with("https://") {
        return Err("Refusing to download from a non-HTTPS URL.".to_string());
    }
    let file_name = sanitize_file_name(&asset_name)?;
    let dir = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let target = dir.join(&file_name);

    state.logger.lock().unwrap().log(
        "info",
        &format!("update download started: {file_name} -> {}", target.display()),
    );

    let response = build_client()?
        .get(&download_url)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Download failed: the release server returned HTTP {}.",
            response.status()
        ));
    }

    let total = response.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(&target)
        .await
        .map_err(|e| format!("Could not write {}: {e}", target.display()))?;

    let mut downloaded: u64 = 0;
    let mut last_emitted: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Could not write {}: {e}", target.display()))?;
        downloaded += chunk.len() as u64;
        if downloaded - last_emitted >= PROGRESS_STEP {
            last_emitted = downloaded;
            let _ = app.emit(
                DOWNLOAD_PROGRESS_EVENT,
                UpdateDownloadProgress {
                    asset_name: file_name.clone(),
                    downloaded,
                    total,
                    done: false,
                },
            );
        }
    }
    file.flush()
        .await
        .map_err(|e| format!("Could not write {}: {e}", target.display()))?;

    let _ = app.emit(
        DOWNLOAD_PROGRESS_EVENT,
        UpdateDownloadProgress {
            asset_name: file_name.clone(),
            downloaded,
            total: if total == 0 { downloaded } else { total },
            done: true,
        },
    );

    state
        .logger
        .lock()
        .unwrap()
        .log("info", &format!("update download finished: {}", target.display()));

    Ok(target.to_string_lossy().into_owned())
}

/// Open a downloaded installer with the OS default handler.
#[tauri::command]
pub fn open_update_file(app: AppHandle, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_file() {
        return Err(format!("{} no longer exists.", path.display()));
    }
    app.opener()
        .open_path(path.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Reveal the downloaded installer in the file manager.
#[tauri::command]
pub fn reveal_update_file(app: AppHandle, path: String) -> Result<(), String> {
    let path = Path::new(&path);
    if !path.exists() {
        return Err(format!("{} no longer exists.", path.display()));
    }
    app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
}

/// Open the release page in the browser (restricted to this repository).
#[tauri::command]
pub fn open_release_page(app: AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with(RELEASE_URL_PREFIX) {
        return Err(format!("Refusing to open a URL outside {REPO_SLUG}."));
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Version reported by the app bundle (used by the About section).
#[tauri::command]
pub fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_compares_versions() {
        assert_eq!(parse_version("v1.2.3"), vec![1, 2, 3]);
        assert_eq!(parse_version("1.2.3-rc1"), vec![1, 2, 3]);
        assert_eq!(parse_version("1.13"), vec![1, 13]);
        assert_eq!(
            compare_versions(&parse_version("1.2"), &parse_version("1.2.0")),
            Ordering::Equal
        );
        assert_eq!(
            compare_versions(&parse_version("1.10.0"), &parse_version("1.9.9")),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions(&parse_version("v1.1.1"), &parse_version("1.13")),
            Ordering::Less
        );
    }

    #[test]
    fn skips_drafts_and_picks_the_highest_version() {
        let releases: Vec<serde_json::Value> = serde_json::from_value(serde_json::json!([
            { "tag_name": "v1.1.1", "draft": false, "prerelease": false },
            { "tag_name": "v1.0.9", "draft": false, "prerelease": false },
            { "tag_name": "v1.2.0", "draft": true, "prerelease": false },
            { "tag_name": "v2.0.0-rc.1", "draft": false, "prerelease": true }
        ]))
        .unwrap();

        let (release, version) = pick_newest_release(&releases, false).unwrap();
        assert_eq!(version, "1.1.1");
        assert_eq!(release["tag_name"], "v1.1.1");

        let (_, with_prerelease) = pick_newest_release(&releases, true).unwrap();
        // The pre-release suffix is kept for display; version math ignores it.
        assert_eq!(with_prerelease, "2.0.0-rc.1");
    }

    #[test]
    fn classifies_bundles_and_ignores_manifests() {
        assert_eq!(classify_asset("AI.Chat_1.1.1_x64-setup.exe"), Some("nsis"));
        assert_eq!(classify_asset("AI.Chat_1.1.1_x64_en-US.msi"), Some("msi"));
        assert_eq!(classify_asset("AI.Chat_1.1.1_aarch64.dmg"), Some("dmg"));
        assert_eq!(classify_asset("AI.Chat_1.1.1_amd64.AppImage"), Some("appimage"));
        assert_eq!(classify_asset("AI.Chat_1.1.1_amd64.deb"), Some("deb"));
        assert_eq!(classify_asset("AI.Chat-1.1.1-1.x86_64.rpm"), Some("rpm"));
        assert_eq!(classify_asset("latest.json"), None);
        assert_eq!(classify_asset("AI.Chat.app.tar.gz.sig"), None);
    }

    /// The CI uploads `-setup.exe`, `.msi`, `.dmg`, `.AppImage`, `.deb` and `.rpm`;
    /// only the ones for this OS/arch may be flagged as recommended.
    #[test]
    fn recommends_only_matching_platform_assets() {
        let release = serde_json::json!({
            "assets": [
                { "name": "AI.Chat_1.1.4_aarch64.dmg", "browser_download_url": "https://github.com/x/a", "size": 1 },
                { "name": "AI.Chat_1.1.4_x64_en-US.msi", "browser_download_url": "https://github.com/x/b", "size": 2 },
                { "name": "AI.Chat_1.1.4_x64-setup.exe", "browser_download_url": "https://github.com/x/c", "size": 3 },
                { "name": "AI.Chat_1.1.4_amd64.AppImage", "browser_download_url": "https://github.com/x/d", "size": 4 }
            ]
        });
        let assets = collect_assets(&release);
        assert_eq!(assets.len(), 4);

        let recommended: Vec<&str> = assets
            .iter()
            .filter(|asset| asset.recommended)
            .map(|asset| asset.name.as_str())
            .collect();

        if cfg!(target_os = "windows") {
            assert_eq!(recommended, vec!["AI.Chat_1.1.4_x64-setup.exe"]);
            assert_eq!(assets[0].kind, "nsis");
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            assert_eq!(recommended, vec!["AI.Chat_1.1.4_aarch64.dmg"]);
        } else if cfg!(target_os = "macos") {
            // Intel macOS: this fixture only ships an arm64 disk image.
            assert!(recommended.is_empty());
        } else if cfg!(target_os = "linux") {
            assert_eq!(recommended, vec!["AI.Chat_1.1.4_amd64.AppImage"]);
        }

        // Foreign-platform bundles stay listed (so users can grab them manually).
        assert!(assets.iter().any(|asset| !asset.recommended));
    }

    #[test]
    fn sanitizes_asset_names() {
        assert_eq!(
            sanitize_file_name("AI.Chat_1.1.4_x64-setup.exe").unwrap(),
            "AI.Chat_1.1.4_x64-setup.exe"
        );
        // Only the last path component survives, so a crafted name cannot escape the folder.
        assert_eq!(sanitize_file_name("../../evil.exe").unwrap(), "evil.exe");
        assert_eq!(sanitize_file_name(r"..\..\evil.exe").unwrap(), "evil.exe");
        assert_eq!(sanitize_file_name("dir/evil.exe").unwrap(), "evil.exe");
        assert_eq!(sanitize_file_name("bad:name?.exe").unwrap(), "bad_name_.exe");
        assert!(sanitize_file_name("   ").is_err());
        assert!(sanitize_file_name("..").is_err());
        // A leading dot is stripped rather than rejected: ".hidden" -> "hidden".
        assert_eq!(sanitize_file_name(".hidden").unwrap(), "hidden");
    }

    /// Live smoke test against the real repository. Opt in with:
    /// `cargo test --manifest-path src-tauri/Cargo.toml live_release_lookup -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "hits the GitHub API"]
    async fn live_release_lookup() {
        let releases = fetch_releases().await.expect("GitHub API should be reachable");
        assert!(!releases.is_empty(), "the repository should have published releases");

        let (release, version) = pick_newest_release(&releases, false).expect("a published release");
        let assets = collect_assets(release);
        println!(
            "live check -> version {version}, {} asset(s): {}",
            assets.len(),
            assets
                .iter()
                .map(|asset| format!("{}{}", asset.name, if asset.recommended { " [recommended]" } else { "" }))
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(!assets.is_empty(), "the newest release should ship bundles");
    }
}

