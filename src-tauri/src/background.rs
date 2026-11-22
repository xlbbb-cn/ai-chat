//! Chat background photo — `<app_data>/backgrounds/chat-background-<id>.<ext>`.
//!
//! The photo is copied into the application data directory when the user picks
//! it, and `AppConfig::chat_background` stores only the app-data-relative path.
//! That is what makes the setting portable: a backup carries the folder next to
//! `config.json`, MSIX redirects the whole app-data tree as one unit, and moving
//! or deleting the original file later never breaks the background.
//!
//! The webview is the only place the image is decoded, so this module only
//! copies bytes, sniffs the format and hands the file back as a `data:` URL.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::AppState;

/// Folder under the app data directory.
pub(crate) const BACKGROUND_DIR: &str = "backgrounds";

/// Largest accepted photo (20 MiB). A background is a photograph, not a
/// document: this leaves room for a full-resolution phone picture while keeping
/// the `data:` URL the webview receives bounded.
const MAX_BACKGROUND_BYTES: u64 = 20 * 1024 * 1024;

/// Formats every supported webview can decode. HEIC is deliberately absent (the
/// WebView2 build does not decode it everywhere); SVG is absent because a photo
/// picker has no business accepting executable markup.
const ALLOWED_FORMATS: [&str; 5] = ["png", "jpeg", "webp", "gif", "bmp"];

/// The wallpaper shipped with the app, embedded so "use default" works on a
/// fresh install with no picker round trip. `docs/wallpaper.png` stays the one
/// source of truth: a build fails loudly if it is moved or removed.
const DEFAULT_WALLPAPER: &[u8] = include_bytes!("../../docs/wallpaper.png");
/// Stable file name — clicking "use default" again replaces the same file
/// instead of leaving a new copy behind (the previous custom photo is retired).
const DEFAULT_WALLPAPER_FILE: &str = "chat-background-default.png";

#[derive(Debug, Clone, Serialize)]
pub struct ChatBackground {
    /// App-data-relative path with `/` separators (`backgrounds/<name>.<ext>`).
    pub path: String,
    /// `data:<mime>;base64,…` for an `<img>` / CSS `url()`.
    pub data_url: String,
}

fn backgrounds_dir(app_data: &Path) -> PathBuf {
    app_data.join(BACKGROUND_DIR)
}

/// Resolve a stored relative path, refusing anything that is not exactly
/// `backgrounds/<file>` (absolute paths, `..`, nested folders).
fn resolve(app_data: &Path, relative: &str) -> Option<PathBuf> {
    let mut components = Path::new(relative).components();
    let Component::Normal(dir) = components.next()? else {
        return None;
    };
    let Component::Normal(file) = components.next()? else {
        return None;
    };
    if components.next().is_some() || dir != OsStr::new(BACKGROUND_DIR) {
        return None;
    }
    Some(backgrounds_dir(app_data).join(file))
}

/// Identify the image from its magic bytes — the extension a phone or a
/// screenshot tool wrote is a hint, not a guarantee.
fn sniff_format(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        Some("gif")
    } else if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        Some("webp")
    } else if header.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

fn mime_for(format: &str) -> &'static str {
    match format {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
}

/// Read a stored background back as a `data:` URL. `None` when the config path
/// is malformed, the file is gone, or it is no longer a supported image.
fn read_data_url(app_data: &Path, relative: &str) -> Option<String> {
    let path = resolve(app_data, relative)?;
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BACKGROUND_BYTES {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let format = sniff_format(&bytes)?;
    Some(format!(
        "data:{};base64,{}",
        mime_for(format),
        STANDARD.encode(bytes)
    ))
}

/// Validate image bytes before they are written. `subject` keeps the error
/// wording aligned with where the image came from (picker or built-in).
fn validate(bytes: &[u8], subject: &str) -> Result<(), String> {
    if bytes.is_empty() {
        return Err(format!("{subject} is empty."));
    }
    if bytes.len() as u64 > MAX_BACKGROUND_BYTES {
        return Err(format!(
            "{subject} is too large ({} MB, limit {} MB).",
            bytes.len() / (1024 * 1024),
            MAX_BACKGROUND_BYTES / (1024 * 1024)
        ));
    }
    if sniff_format(bytes).is_none() {
        return Err(format!(
            "Unsupported image format. Choose a {} file.",
            ALLOWED_FORMATS.join(" / ")
        ));
    }
    Ok(())
}

/// Write image bytes under `backgrounds/`, persist the path and retire the old
/// photo. The config write happens before the previous file is deleted: a
/// failed write must leave the old setting and its file intact. A failed write
/// after the copy removes the copy again, so no orphan is left behind.
fn install(
    app: &AppHandle,
    state: &AppState,
    bytes: &[u8],
    file_name: &str,
    subject: &str,
) -> Result<ChatBackground, String> {
    validate(bytes, subject)?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve the app data directory: {e}"))?;
    let dir = backgrounds_dir(&app_data);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let target = dir.join(file_name);
    fs::write(&target, bytes).map_err(|e| format!("Cannot save the background: {e}"))?;
    let relative = format!("{BACKGROUND_DIR}/{file_name}");

    let previous = state.config.lock().unwrap().chat_background.clone();
    let mut live = state.config.lock().unwrap().clone();
    live.chat_background = Some(relative.clone());
    if let Err(err) = crate::apply_config(app, state, live) {
        // The config still points at the old photo; this copy has no owner.
        let _ = fs::remove_file(&target);
        return Err(err);
    }

    if let Some(previous) = previous {
        if previous != relative {
            if let Some(old_path) = resolve(&app_data, &previous) {
                let _ = fs::remove_file(old_path);
            }
        }
    }

    let data_url = read_data_url(&app_data, &relative)
        .ok_or_else(|| "The saved background could not be read back.".to_string())?;
    Ok(ChatBackground {
        path: relative,
        data_url,
    })
}

/// Copy the picked photo into the app data directory and persist its path.
pub(crate) fn set(
    app: &AppHandle,
    state: &AppState,
    source_path: &str,
) -> Result<ChatBackground, String> {
    let source = Path::new(source_path);
    let metadata =
        fs::metadata(source).map_err(|e| format!("Cannot read the selected photo: {e}"))?;
    if !metadata.is_file() {
        return Err("The selected path is not a file.".to_string());
    }
    if metadata.len() == 0 {
        return Err("The selected photo is empty.".to_string());
    }
    // Check the size before reading: a huge pick must not be pulled into memory.
    if metadata.len() > MAX_BACKGROUND_BYTES {
        return Err(format!(
            "The selected photo is too large ({} MB, limit {} MB).",
            metadata.len() / (1024 * 1024),
            MAX_BACKGROUND_BYTES / (1024 * 1024)
        ));
    }
    let bytes = fs::read(source).map_err(|e| format!("Cannot read the selected photo: {e}"))?;
    let format = sniff_format(&bytes).ok_or_else(|| {
        format!(
            "Unsupported image format. Choose a {} file.",
            ALLOWED_FORMATS.join(" / ")
        )
    })?;
    let file_name = format!("chat-background-{}.{}", uuid::Uuid::new_v4().simple(), format);
    install(app, state, &bytes, &file_name, "The selected photo")
}

/// Install the wallpaper bundled with the app as the chat background.
///
/// The bytes come from `include_bytes!`, so there is no file-picker round trip
/// and the image is available in a fresh install. It lands under the same
/// stable name every time, so repeated clicks replace one file instead of
/// piling up copies, and a backup carries it like any picked photo.
pub(crate) fn use_default(app: &AppHandle, state: &AppState) -> Result<ChatBackground, String> {
    install(
        app,
        state,
        DEFAULT_WALLPAPER,
        DEFAULT_WALLPAPER_FILE,
        "The default wallpaper",
    )
}

/// Drop the stored photo: clear the config field first, then delete the file.
pub(crate) fn clear(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let previous = state.config.lock().unwrap().chat_background.clone();
    if previous.is_none() {
        return Ok(());
    }
    let mut live = state.config.lock().unwrap().clone();
    live.chat_background = None;
    crate::apply_config(app, state, live)?;

    if let (Some(previous), Ok(app_data)) = (previous, app.path().app_data_dir()) {
        if let Some(path) = resolve(&app_data, &previous) {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

/// The stored background as a `data:` URL, or `None` when there is none / the
/// file has gone missing (an old backup, a cleaned app-data folder).
pub(crate) fn read(app: &AppHandle, state: &AppState) -> Option<String> {
    let relative = state.config.lock().unwrap().chat_background.clone()?;
    let app_data = app.path().app_data_dir().ok()?;
    let data_url = read_data_url(&app_data, &relative);
    if data_url.is_none() {
        state.logger.lock().unwrap().log(
            "WARN",
            &format!("Chat background is missing or unreadable: {relative}"),
        );
    }
    data_url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_accepts_only_the_backgrounds_folder() {
        let root = Path::new("/app-data");
        assert_eq!(
            resolve(root, "backgrounds/chat-background-1.png"),
            Some(root.join("backgrounds").join("chat-background-1.png"))
        );
        assert_eq!(resolve(root, "../config.json"), None);
        assert_eq!(resolve(root, "backgrounds/../config.json"), None);
        assert_eq!(resolve(root, "other/photo.png"), None);
        assert_eq!(resolve(root, "backgrounds/nested/photo.png"), None);
        assert_eq!(resolve(root, "C:/photo.png"), None);
    }

    #[test]
    fn default_wallpaper_is_a_valid_png() {
        assert_eq!(sniff_format(DEFAULT_WALLPAPER), Some("png"));
        assert!(DEFAULT_WALLPAPER.len() as u64 <= MAX_BACKGROUND_BYTES);
        assert!(DEFAULT_WALLPAPER_FILE.ends_with(".png"));
    }

    #[test]
    fn sniff_format_reads_magic_bytes() {
        assert_eq!(
            sniff_format(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            Some("png")
        );
        assert_eq!(sniff_format(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpeg"));
        assert_eq!(sniff_format(b"GIF89a...."), Some("gif"));
        assert_eq!(sniff_format(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff_format(b"BM...."), Some("bmp"));
        assert_eq!(sniff_format(b"not an image"), None);
    }
}
