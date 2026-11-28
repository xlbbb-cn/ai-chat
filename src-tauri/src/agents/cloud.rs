// Execution backend for `cloud`-type sub-agents: sends the task to a remote
// HTTP agent endpoint and returns its result. Supports both a single
// request/response endpoint and an asynchronous job that is polled until it
// reports completion.
//
// Everything protocol-specific is configurable on the agent (request body
// template, result/status dot paths, polling URL), so this works with an
// OpenAI-compatible chat endpoint as well as custom agent services — and with
// generation APIs (text-to-image, image-to-image, text/image-to-video) whose
// answer is the generated media itself: when the extracted result is a base64
// payload or a media URL, it is saved into the task's deliverable directory
// and only the saved paths travel back to the main agent.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::exchange::{format_size, DeliverTarget};
use super::{resolve_agent_timeout, KeyValue, SubAgent};
use crate::http::APP_USER_AGENT;

/// Timeout for the panel's connectivity probe.
const PROBE_TIMEOUT_SECS: u64 = 30;
/// Longest a single HTTP call may take. Synchronous image APIs need more than
/// 300 s at high quality / 4K (the apiyi gpt-image-2 docs recommend ≥360 s and
/// warn that a 120 s client timeout misses a lot), and the same client also
/// serves poll GETs and deliverable downloads. Configurable per agent via
/// `request_timeout_seconds` (0 = this default).
const REQUEST_TIMEOUT_SECS: u64 = 600;
/// A deliverable larger than this is refused: generated media stays far below
/// it, and the cap keeps a runaway endpoint from filling the workspace.
const MAX_DELIVERABLE_BYTES: u64 = 256 * 1024 * 1024;
/// The shortest string still considered a base64 media payload. Anything
/// shorter stays text: reports and answers are routinely longer than this.
const MIN_BASE64_CHARS: usize = 4096;
/// Guard for a result that is not media but still huge (a result_path pointed
/// at a wrapper object): the payload must not reach the mission report whole.
const MAX_RESULT_CHARS: usize = 200_000;

#[derive(Debug, Clone, Serialize)]
pub struct CloudProbe {
    pub endpoint: String,
    pub reachable: bool,
    pub status: Option<u16>,
    pub detail: String,
}

pub struct CloudRunOutcome {
    pub result: String,
    pub duration_ms: u64,
    pub job_id: Option<String>,
    pub summary: String,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Escapes a value for inclusion *inside* a JSON string literal (the template
/// supplies the surrounding quotes).
fn json_escape_fragment(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn expand_body(
    template: &str,
    prompt: &str,
    agent_name: &str,
    cwd: &Path,
    images: &[ImageInput],
    params: &BTreeMap<String, String>,
) -> Result<(String, bool), String> {
    let mut out = template
        .replace("{prompt}", &json_escape_fragment(prompt))
        .replace("{agent_name}", &json_escape_fragment(agent_name))
        .replace("{cwd}", &json_escape_fragment(&cwd.to_string_lossy()));

    let uses_images = out.contains("{image_b64}")
        || out.contains("{image_data_url}")
        || out.contains("{images_data_urls}");
    if uses_images {
        let Some(first) = images.first() else {
            return Err(
                "the request body template uses an image placeholder, but the delegating call \
                 supplied no input images (pass workspace-relative paths in `images`)"
                    .to_string(),
            );
        };
        out = out
            .replace("{image_b64}", &json_escape_fragment(&first.b64))
            .replace("{image_data_url}", &json_escape_fragment(&first.data_url));
        if out.contains("{images_data_urls}") {
            let array: Vec<&str> = images.iter().map(|image| image.data_url.as_str()).collect();
            let array = serde_json::to_string(&array).unwrap_or_else(|_| "[]".to_string());
            out = out.replace("{images_data_urls}", &array);
        }
    }

    // Params go last so a value can never shadow a built-in placeholder: the
    // built-in tokens are already gone and `replace` does not re-scan what it
    // inserted.
    for (key, value) in params {
        out = out.replace(&format!("{{{key}}}"), &json_escape_fragment(value));
    }
    Ok((out, uses_images))
}

/// One input image loaded from the workspace, ready for the body template.
pub struct ImageInput {
    /// Base64 payload without the data-URL prefix.
    pub b64: String,
    /// `data:<mime>;base64,<payload>`.
    pub data_url: String,
}

/// Per-file cap for an input image: apiyi documents a ~20 MiB field-length
/// limit for base64 data URLs (base64 inflates ~4/3) and recommends ≤15 MB
/// originals.
const MAX_IMAGE_INPUT_BYTES: u64 = 20 * 1024 * 1024;

fn mime_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        "avi" => Some("video/x-msvideo"),
        "mkv" => Some("video/x-matroska"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "m4a" => Some("audio/mp4"),
        "ogg" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        _ => None,
    }
}

/// Loads and sandbox-checks the task's input images. Paths are
/// workspace-relative; `resolve_safe_path` enforces the workspace sandbox
/// (a traversal attempt fails the run with the reason, it is never silently
/// ignored). The mime type comes from the extension, falling back to the
/// byte-sniff for extension-less or unusual files.
pub fn load_image_inputs(
    workspace_dir: &Path,
    images: &[String],
) -> Result<Vec<ImageInput>, String> {
    let mut out = Vec::new();
    for raw in images {
        let path = crate::tools::resolve_safe_path(workspace_dir, raw.trim())
            .map_err(|e| format!("input image '{}': {e}", raw.trim()))?;
        let bytes = std::fs::read(&path).map_err(|e| {
            format!(
                "could not read the input image '{}': {e}",
                path.display()
            )
        })?;
        if bytes.len() as u64 > MAX_IMAGE_INPUT_BYTES {
            return Err(format!(
                "input image '{}' exceeds the {} MB size cap",
                raw.trim(),
                MAX_IMAGE_INPUT_BYTES / 1024 / 1024
            ));
        }
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();
        let mime = mime_for_extension(&ext)
            .or_else(|| sniff_media(&bytes).map(|(_, kind)| kind))
            .unwrap_or("application/octet-stream");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        out.push(ImageInput {
            data_url: format!("data:{mime};base64,{b64}"),
            b64,
        });
    }
    Ok(out)
}

/// Reads a dot path (`choices.0.message.content`) out of a JSON value.
fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for part in trimmed.split('.') {
        if part.is_empty() {
            continue;
        }
        current = match current {
            Value::Object(map) => map.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn extract_text(value: &Value, path: &str) -> Result<String, String> {
    json_path(value, path)
        .map(value_to_text)
        .ok_or_else(|| format!("the response has no '{}' field", path.trim()))
}

fn emit_progress(app: &AppHandle, task_id: &str, session_id: &str, text: String) {
    let _ = app.emit(
        crate::events::AGENT_TASK_TOKEN,
        crate::events::AgentTaskToken {
            task_id: task_id.to_string(),
            session_id: Some(session_id.to_string()),
            token: text,
        },
    );
}

fn apply_headers(
    mut request: reqwest::RequestBuilder,
    token: &str,
    headers: &[KeyValue],
) -> reqwest::RequestBuilder {
    if !token.trim().is_empty() {
        request = request.bearer_auth(token.trim());
    }
    for KeyValue { key, value } in headers {
        if !key.trim().is_empty() {
            request = request.header(key.trim(), value.trim());
        }
    }
    request
}

fn status_matches(status: &str, values: &[String]) -> bool {
    values
        .iter()
        .any(|value| value.trim().eq_ignore_ascii_case(status.trim()))
}

// ─── Deliverable media (image / video / audio) ──────────────────────────────

/// One media deliverable saved into the task's exchange directory.
struct SavedMedia {
    /// Workspace-relative path of the file — the form `file_actions` and
    /// `@`-mentions use, and what is reported back to the main agent.
    relative: String,
    size: u64,
    /// Human kind, e.g. `image/png` or `video/mp4`.
    kind: String,
}

/// Byte-signature sniffing: `(extension, kind)` for a known container, `None`
/// otherwise. Generation APIs return `b64_json` without any mime hint, so the
/// decoded bytes are the only source for the extension.
fn sniff_media(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image/png"))
    } else if starts(b"\xFF\xD8\xFF") {
        Some(("jpg", "image/jpeg"))
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some(("gif", "image/gif"))
    } else if bytes.len() >= 12 && starts(b"RIFF") {
        match &bytes[8..12] {
            b"WEBP" => Some(("webp", "image/webp")),
            b"WAVE" => Some(("wav", "audio/wav")),
            _ => None,
        }
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        Some(("mp4", "video/mp4"))
    } else if starts(b"\x1A\x45\xDF\xA3") {
        Some(("webm", "video/webm"))
    } else if starts(b"OggS") {
        Some(("ogg", "audio/ogg"))
    } else if starts(b"fLaC") {
        Some(("flac", "audio/flac"))
    } else if starts(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0) {
        Some(("mp3", "audio/mpeg"))
    } else if starts(b"BM") && bytes.len() > 54 {
        Some(("bmp", "image/bmp"))
    } else if starts(b"%PDF") {
        Some(("pdf", "application/pdf"))
    } else if starts(b"PK\x03\x04") {
        Some(("zip", "application/zip"))
    } else if starts(b"\x1F\x8B") {
        Some(("gz", "application/gzip"))
    } else {
        None
    }
}

/// `Content-Type` header → file extension, for the deliverable download path.
fn ext_for_content_type(content_type: &str) -> Option<&'static str> {
    let base = content_type.split(';').next().unwrap_or("").trim();
    match base {
        "image/png" | "image/x-png" => Some("png"),
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/bmp" | "image/x-ms-bmp" => Some("bmp"),
        "image/svg+xml" => Some("svg"),
        "video/mp4" | "application/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "video/quicktime" => Some("mov"),
        "video/x-msvideo" => Some("avi"),
        "video/x-matroska" => Some("mkv"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" | "audio/x-wav" | "audio/vnd.wave" => Some("wav"),
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some("m4a"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        _ => None,
    }
}

/// Whether a `Content-Type` names a binary media stream worth saving. Video
/// CDNs frequently answer `application/octet-stream`, so that counts too —
/// but an error page (`text/html`, `application/json`) never does.
fn content_type_is_media(content_type: &str) -> bool {
    let base = content_type.split(';').next().unwrap_or("").trim();
    base.starts_with("image/")
        || base.starts_with("video/")
        || base.starts_with("audio/")
        || base == "application/octet-stream"
}

/// The URL-path extension, lowercase, when it names a media file. Used as the
/// fallback extension when the `Content-Type` says nothing more specific.
fn url_media_extension(url: &str) -> Option<&'static str> {
    const MEDIA_EXTS: [&str; 18] = [
        "png", "jpg", "jpeg", "webp", "gif", "bmp", "svg", "mp4", "webm", "mov", "m4v", "avi",
        "mkv", "mp3", "wav", "m4a", "ogg", "flac",
    ];
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit(['/', '\\']).next()?;
    let dot = name.rfind('.')?;
    let ext = name[dot + 1..].to_ascii_lowercase();
    if MEDIA_EXTS.contains(&ext.as_str()) {
        MEDIA_EXTS.iter().find(|known| **known == ext).copied()
    } else {
        None
    }
}

fn kind_word(ext: &str) -> &'static str {
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" => "image",
        "mp4" | "webm" | "mov" | "m4v" | "avi" | "mkv" => "video",
        "mp3" | "wav" | "m4a" | "ogg" | "flac" => "audio",
        _ => "file",
    }
}

/// A string that carries binary media on its own: a `data:` URL, a long
/// base64 payload, or an http(s) URL ending in a media extension.
fn is_media_candidate(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.starts_with("data:") && trimmed.contains(";base64,") {
        return true;
    }
    if looks_like_base64(trimmed) {
        return true;
    }
    trimmed.starts_with("http://") || trimmed.starts_with("https://") &&
        url_media_extension(trimmed).is_some()
}

/// Walks a JSON result looking for the first string that carries media. Used
/// when `result_path` points at a wrapper (`data`, the root object) instead of
/// the media field itself, so a whole `{"data":[{"b64_json":…}]}` response is
/// still saved instead of being pretty-printed into the mission report.
fn media_string_in_json(raw: &str) -> Option<String> {
    fn walk(value: &Value, depth: usize, out: &mut Option<String>) {
        if out.is_some() || depth > 8 {
            return;
        }
        match value {
            Value::String(text) => {
                if is_media_candidate(text) {
                    *out = Some(text.clone());
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, depth + 1, out);
                    if out.is_some() {
                        break;
                    }
                }
            }
            Value::Object(map) => {
                for (_key, item) in map {
                    walk(item, depth + 1, out);
                    if out.is_some() {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let value: Value = serde_json::from_str(raw).ok()?;
    let mut found = None;
    walk(&value, 0, &mut found);
    found
}

/// Whether a string is *long enough and shaped enough* to be a base64 media
/// payload. Strict enough that prose (spaces, punctuation) never matches.
fn looks_like_base64(value: &str) -> bool {
    value.len() >= MIN_BASE64_CHARS
        && value
            .chars()
            .filter(|c| !c.is_whitespace())
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'))
}

/// Decodes a base64 payload, tolerating whitespace and every common alphabet
/// (standard / URL-safe, padded / unpadded) — relays differ in all four.
fn decode_base64(payload: &str) -> Result<Vec<u8>, String> {
    use base64::engine::general_purpose as gp;
    let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    gp::STANDARD
        .decode(&cleaned)
        .or_else(|_| gp::STANDARD_NO_PAD.decode(&cleaned))
        .or_else(|_| gp::URL_SAFE.decode(&cleaned))
        .or_else(|_| gp::URL_SAFE_NO_PAD.decode(&cleaned))
        .map_err(|e| format!("not valid base64: {e}"))
}

/// A collision-free file name inside the deliverable directory: the URL's own
/// stem when one exists, otherwise `<kind>-<millis>`; a number is appended
/// before the extension on collision so a second image never overwrites the
/// first.
fn unique_name(dir: &Path, ext: &str, url: Option<&str>) -> String {
    let from_url = url.and_then(|value| {
        let path = value.split(['?', '#']).next()?;
        let name = path.rsplit(['/', '\\']).next()?;
        let stem = match name.rfind('.') {
            Some(dot) if dot > 0 => &name[..dot],
            _ => name,
        };
        let sanitized: String = stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .take(64)
            .collect();
        let trimmed = sanitized.trim_matches(|c: char| c == '.' || c == '_');
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    let stem = from_url.unwrap_or_else(|| {
        format!(
            "{}-{}",
            kind_word(ext),
            chrono::Utc::now().timestamp_millis()
        )
    });
    let mut candidate = format!("{stem}.{ext}");
    let mut n = 2;
    while dir.join(&candidate).exists() {
        candidate = format!("{stem}-{n}.{ext}");
        n += 1;
    }
    candidate
}

fn save_bytes(
    bytes: &[u8],
    ext: &str,
    kind: &str,
    url: Option<&str>,
    target: &DeliverTarget,
) -> Result<SavedMedia, String> {
    if bytes.len() as u64 > MAX_DELIVERABLE_BYTES {
        return Err(format!(
            "the deliverable exceeds the {} MB size cap",
            MAX_DELIVERABLE_BYTES / 1024 / 1024
        ));
    }
    let name = unique_name(&target.dir, ext, url);
    let path = target.dir.join(&name);
    std::fs::write(&path, bytes).map_err(|e| {
        format!(
            "could not write the deliverable '{}': {e}",
            path.display()
        )
    })?;
    Ok(SavedMedia {
        relative: format!("{}/{}", target.relative.trim_end_matches('/'), name),
        size: bytes.len() as u64,
        kind: kind.to_string(),
    })
}

/// Saves the result value when it carries media itself: a `data:` URL, a bare
/// base64 payload (generation APIs answer `b64_json` without a mime hint) or
/// anything else that decodes to a recognized container. Unrecognized binary
/// is still written (`.bin`) — the point is to keep multi-MB payloads out of
/// the mission report, not to guess formats.
fn save_decoded_payload(
    value: &str,
    target: &DeliverTarget,
) -> Result<Option<SavedMedia>, String> {
    let trimmed = value.trim();
    if let Some((mime, data)) = trimmed.strip_prefix("data:").and_then(|rest| {
        let (mime, data) = rest.split_once(";base64,")?;
        Some((mime.trim().to_ascii_lowercase(), data))
    }) {
        let bytes = decode_base64(data)?;
        let sniff = sniff_media(&bytes);
        let ext = sniff
            .map(|(ext, _)| ext)
            .or_else(|| ext_for_content_type(&mime))
            .unwrap_or("bin");
        let kind = sniff
            .map(|(_, kind)| kind)
            .unwrap_or("application/octet-stream");
        return save_bytes(&bytes, ext, kind, None, target).map(Some);
    }
    if looks_like_base64(trimmed) {
        let bytes = decode_base64(trimmed)?;
        let Some((ext, kind)) = sniff_media(&bytes) else {
            return save_bytes(&bytes, "bin", "application/octet-stream", None, target)
                .map(Some);
        };
        return save_bytes(&bytes, ext, kind, None, target).map(Some);
    }
    Ok(None)
}

/// Downloads a result URL into the deliverable directory when it serves
/// binary media. The `Content-Type` decides — a URL whose extension lies
/// (a `.mp4` link answering an HTML error page) is not saved, while an
/// extension-less CDN link that answers `video/mp4` is. A non-media URL is
/// kept as text (`Ok(None)`): the caller may legitimately want the link.
async fn download_media(
    client: &reqwest::Client,
    token: &str,
    headers: &[KeyValue],
    value: &str,
    target: &DeliverTarget,
) -> Result<Option<SavedMedia>, String> {
    let url = value.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Ok(None);
    }
    let request = apply_headers(client.get(url), token, headers);
    let response = request
        .send()
        .await
        .map_err(|e| format!("deliverable download from {url} failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("deliverable download from {url} returned HTTP {status}"));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type_is_media(&content_type) {
        return Ok(None);
    }
    let base_kind = content_type
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_string();
    let ext = ext_for_content_type(&content_type)
        .or_else(|| url_media_extension(url))
        .unwrap_or("bin");
    let name = unique_name(&target.dir, ext, Some(url));
    let path = target.dir.join(&name);
    let mut file = std::fs::File::create(&path)
        .map_err(|e| format!("could not create the deliverable '{}': {e}", path.display()))?;
    let mut response = response;
    let mut total: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("deliverable download from {url} failed: {e}"))?
    {
        total += chunk.len() as u64;
        if total > MAX_DELIVERABLE_BYTES {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!(
                "the deliverable from {url} exceeds the {} MB download cap",
                MAX_DELIVERABLE_BYTES / 1024 / 1024
            ));
        }
        file.write_all(&chunk)
            .map_err(|e| format!("could not write the deliverable '{}': {e}", path.display()))?;
    }
    file.flush()
        .map_err(|e| format!("could not write the deliverable '{}': {e}", path.display()))?;
    if total == 0 {
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }
    Ok(Some(SavedMedia {
        relative: format!("{}/{}", target.relative.trim_end_matches('/'), name),
        size: total,
        kind: base_kind,
    }))
}

/// Tries every way the result could carry media: the value itself, then —
/// when it is a JSON wrapper — the first media string found inside it.
async fn save_result_media(
    client: &reqwest::Client,
    token: &str,
    headers: &[KeyValue],
    raw: &str,
    target: &DeliverTarget,
) -> Result<Option<SavedMedia>, String> {
    if let Some(saved) = save_decoded_payload(raw, target)? {
        return Ok(Some(saved));
    }
    if let Some(candidate) = media_string_in_json(raw) {
        if let Some(saved) = save_decoded_payload(&candidate, target)? {
            return Ok(Some(saved));
        }
        if let Some(saved) = download_media(client, token, headers, &candidate, target).await? {
            return Ok(Some(saved));
        }
    }
    download_media(client, token, headers, raw, target).await
}

/// Rewrites the result text for the mission report: a media result becomes
/// the saved paths (the multi-MB payload never travels back), a short answer
/// keeps its original text, and a misconfigured-but-huge result is capped.
async fn finalize_result(
    client: &reqwest::Client,
    token: &str,
    headers: &[KeyValue],
    raw: String,
    deliver: Option<&DeliverTarget>,
) -> Result<(String, Option<SavedMedia>), String> {
    let Some(target) = deliver else {
        return Ok((cap_result_text(raw), None));
    };
    let saved = save_result_media(client, token, headers, &raw, target).await?;
    match saved {
        Some(saved) => {
            let mut text = format!(
                "Deliverable saved to the exchange area (workspace-relative path; read it with \
                 file_actions or attach it with an @ mention):\n- {} ({}, {})\n",
                saved.relative,
                saved.kind,
                format_size(saved.size)
            );
            let trimmed = raw.trim();
            if !trimmed.is_empty() && trimmed.len() <= 2000 {
                text.push_str(&format!("Upstream response: {trimmed}"));
            }
            Ok((text, Some(saved)))
        }
        None => Ok((cap_result_text(raw), None)),
    }
}

/// Caps a non-media result so a misconfigured `result_path` cannot dump a
/// multi-megabyte payload into the mission report.
fn cap_result_text(raw: String) -> String {
    let count = raw.chars().count();
    if count <= MAX_RESULT_CHARS {
        return raw;
    }
    let mut out: String = raw.chars().take(MAX_RESULT_CHARS).collect();
    out.push_str(&format!(
        "\n\n… result truncated ({count} chars total; point result_path at the media field so \
         the payload is saved to the exchange area instead)"
    ));
    out
}

// ─── Probe ───────────────────────────────────────────────────────────────────

/// Best-effort reachability check: GET the endpoint with a short timeout. Any
/// HTTP status counts as reachable — the point is to catch typos, DNS failures
/// and TLS/connection problems before a run.
pub async fn probe(
    endpoint: &str,
    token: &str,
    headers: &[KeyValue],
) -> Result<CloudProbe, String> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err("The endpoint URL is empty.".to_string());
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("The endpoint must start with http:// or https://".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .user_agent(APP_USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;

    let request = apply_headers(client.get(trimmed), token, headers);
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(300).collect();
            Ok(CloudProbe {
                endpoint: trimmed.to_string(),
                reachable: true,
                status: Some(status.as_u16()),
                detail: if snippet.trim().is_empty() {
                    format!("HTTP {status}")
                } else {
                    format!("HTTP {status}\n{snippet}")
                },
            })
        }
        Err(err) => Ok(CloudProbe {
            endpoint: trimmed.to_string(),
            reachable: false,
            status: None,
            detail: err.to_string(),
        }),
    }
}

// ─── Execution ───────────────────────────────────────────────────────────────

/// Runs a `cloud`-type agent: POSTs the task, then either reads the result
/// straight from the response or polls `poll_url` until it is done. When the
/// result carries generated media (a base64 payload or a media URL) it is
/// saved into `deliver` — the task's deliverable directory — and only the
/// saved workspace-relative paths travel back to the main agent.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &AppHandle,
    agent: &SubAgent,
    task_id: &str,
    prompt: &str,
    cwd: &Path,
    session_id: &str,
    workspace_dir: &Path,
    images: &[String],
    params: &BTreeMap<String, String>,
    deliver: Option<&DeliverTarget>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<CloudRunOutcome, String> {
    let settings = &agent.cloud;
    let endpoint = settings.endpoint.trim();
    if endpoint.is_empty() {
        return Err("this cloud agent has no endpoint configured".to_string());
    }
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err("the endpoint must start with http:// or https://".to_string());
    }

    let overall_timeout = resolve_agent_timeout(settings.timeout_seconds);
    let started = std::time::Instant::now();
    let deadline = overall_timeout.map(|timeout| tokio::time::Instant::now() + timeout);

    let request_timeout = if settings.request_timeout_seconds > 0 {
        settings.request_timeout_seconds as u64
    } else {
        REQUEST_TIMEOUT_SECS
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(request_timeout))
        .user_agent(APP_USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;

    // Input images are loaded (and sandbox-checked) before anything is sent;
    // a bad path fails the run with a reason instead of shipping a template
    // with an empty placeholder.
    let inputs = load_image_inputs(workspace_dir, images)?;
    let (body_text, uses_image_placeholders) =
        expand_body(&settings.body_template, prompt, &agent.name, cwd, &inputs, params)?;
    if !inputs.is_empty() && !uses_image_placeholders {
        emit_progress(
            app,
            task_id,
            session_id,
            format!(
                "note: {} input image(s) supplied but the body template has no image placeholder\n",
                inputs.len()
            ),
        );
    }
    // Fail fast on a broken template instead of shipping garbage upstream.
    let body: Value = serde_json::from_str(&body_text)
        .map_err(|e| format!("the request body template is not valid JSON: {e}"))?;

    emit_progress(app, task_id, session_id, format!("POST {endpoint}\n"));
    let request = apply_headers(client.post(endpoint), &settings.token, &settings.headers)
        .header("Content-Type", "application/json")
        .json(&body);

    let response = request
        .send()
        .await
        .map_err(|e| format!("request to {endpoint} failed: {e}"))?;
    let status = response.status();
    let payload_text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = payload_text.chars().take(600).collect();
        return Err(format!("{endpoint} returned HTTP {status}: {snippet}"));
    }

    let payload: Value = serde_json::from_str(&payload_text)
        .map_err(|e| format!("the response is not valid JSON: {e}"))?;

    // ── Single-shot mode ────────────────────────────────────────────────────
    if settings.poll_url.trim().is_empty() {
        let raw = extract_text(&payload, &settings.result_path)?;
        let (result, saved) = finalize_result(
            &client,
            &settings.token,
            &settings.headers,
            raw,
            deliver,
        )
        .await?;
        emit_progress(
            app,
            task_id,
            session_id,
            format!("HTTP {status} (no polling configured)\n"),
        );
        if let Some(saved) = &saved {
            emit_progress(
                app,
                task_id,
                session_id,
                format!("saved {} ({}, {})\n", saved.relative, saved.kind, format_size(saved.size)),
            );
        }
        let mut summary = format!(
            "cloud agent \"{}\" answered directly (HTTP {status})",
            agent.name
        );
        if let Some(saved) = &saved {
            summary.push_str(&format!(
                " · saved {} ({})",
                saved.relative.rsplit('/').next().unwrap_or(""),
                format_size(saved.size)
            ));
        }
        return Ok(CloudRunOutcome {
            result,
            duration_ms: started.elapsed().as_millis() as u64,
            job_id: None,
            summary,
        });
    }

    // ── Async mode: poll until done ─────────────────────────────────────────
    let job_id = json_path(&payload, &settings.poll_id_path)
        .map(value_to_text)
        .unwrap_or_default();
    if job_id.trim().is_empty() {
        return Err(format!(
            "the response has no job id at '{}'",
            settings.poll_id_path.trim()
        ));
    }
    let poll_url = settings.poll_url.replace("{id}", job_id.trim());
    emit_progress(
        app,
        task_id,
        session_id,
        format!("HTTP {status} accepted, job {}\nPolling {poll_url}\n", job_id.trim()),
    );

    let interval = Duration::from_secs((settings.poll_interval_seconds.max(1)) as u64);
    // Some providers spend the first tens of seconds queueing (the Seedance
    // docs recommend querying no earlier than 20–30 s after submission); an
    // optional first delay keeps that from burning requests.
    let first_delay = Duration::from_secs(settings.poll_first_delay_seconds as u64);
    let poll_is_post = settings.poll_method.trim().eq_ignore_ascii_case("post");
    let mut attempt: u32 = 0;
    let mut last_status = String::new();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled by the user".to_string());
        }
        if let Some(deadline) = deadline {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "the job did not finish within {}s (last status: {})",
                    overall_timeout.map(|timeout| timeout.as_secs()).unwrap_or(0),
                    if last_status.trim().is_empty() {
                        "unknown"
                    } else {
                        last_status.trim()
                    }
                ));
            }
        }
        let wait = if attempt == 0 { first_delay } else { interval };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        attempt += 1;

        let poll_request = if poll_is_post {
            apply_headers(client.post(&poll_url), &settings.token, &settings.headers)
                .header("Content-Type", "application/json")
                .json(&json!({}))
        } else {
            apply_headers(client.get(&poll_url), &settings.token, &settings.headers)
        };
        let poll_response = poll_request
            .send()
            .await
            .map_err(|e| format!("poll #{attempt} failed: {e}"))?;
        let poll_status_code = poll_response.status();
        let poll_text = poll_response.text().await.unwrap_or_default();
        if !poll_status_code.is_success() {
            let snippet: String = poll_text.chars().take(300).collect();
            return Err(format!("poll #{attempt} returned HTTP {poll_status_code}: {snippet}"));
        }
        let poll_payload: Value = serde_json::from_str(&poll_text)
            .map_err(|e| format!("poll #{attempt} response is not valid JSON: {e}"))?;

        let job_status = json_path(&poll_payload, &settings.poll_status_path)
            .map(value_to_text)
            .unwrap_or_default();
        last_status = job_status.clone();
        emit_progress(
            app,
            task_id,
            session_id,
            format!("poll #{attempt}: status={}\n", job_status.trim()),
        );

        if status_matches(&job_status, &settings.poll_done_values) {
            let raw = extract_text(&poll_payload, &settings.result_path)?;
            let (result, saved) = finalize_result(
                &client,
                &settings.token,
                &settings.headers,
                raw,
                deliver,
            )
            .await?;
            if let Some(saved) = &saved {
                emit_progress(
                    app,
                    task_id,
                    session_id,
                    format!(
                        "saved {} ({}, {})\n",
                        saved.relative,
                        saved.kind,
                        format_size(saved.size)
                    ),
                );
            }
            let mut summary = format!(
                "cloud agent \"{}\" finished in {:.1}s (job {})",
                agent.name,
                started.elapsed().as_secs_f32(),
                job_id.trim()
            );
            if let Some(saved) = &saved {
                summary.push_str(&format!(
                    " · saved {} ({})",
                    saved.relative.rsplit('/').next().unwrap_or(""),
                    format_size(saved.size)
                ));
            }
            return Ok(CloudRunOutcome {
                result,
                duration_ms: started.elapsed().as_millis() as u64,
                job_id: Some(job_id.trim().to_string()),
                summary,
            });
        }
        if status_matches(&job_status, &settings.poll_failed_values) {
            let detail = json_path(&poll_payload, &settings.result_path)
                .map(value_to_text)
                .unwrap_or_else(|| poll_text.chars().take(400).collect());
            return Err(format!("the job reported status '{}': {detail}", job_status.trim()));
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{sanitize_task_images, sanitize_task_params};

    #[test]
    fn dot_paths_read_objects_and_arrays() {
        let value: Value = serde_json::json!({
            "choices": [{ "message": { "content": "hi" } }],
            "result": { "output": [1, 2] }
        });
        assert_eq!(
            json_path(&value, "choices.0.message.content").map(value_to_text),
            Some("hi".to_string())
        );
        assert_eq!(
            json_path(&value, "result.output.1").map(value_to_text),
            Some("2".to_string())
        );
        assert!(json_path(&value, "missing.path").is_none());
    }

    #[test]
    fn body_template_escapes_placeholders() {
        let (expanded, _) = expand_body(
            "{\"input\":\"{prompt}\"}",
            "line1\nline2 \"quoted\"",
            "agent",
            Path::new("."),
            &[],
            &BTreeMap::new(),
        )
        .expect("expansion works");
        let parsed: Value = serde_json::from_str(&expanded).expect("valid json");
        assert_eq!(parsed["input"], "line1\nline2 \"quoted\"");
    }

    #[test]
    fn image_placeholders_inject_loaded_images_or_fail_loudly() {
        let template = r#"{"prompt":"{prompt}","image":"{image_data_url}"}"#;
        // No images supplied: the run must fail with a reason, not ship an
        // empty placeholder upstream.
        let err = expand_body(template, "p", "a", Path::new("."), &[], &BTreeMap::new())
            .expect_err("missing images are an error");
        assert!(err.contains("no input images"));

        let input = ImageInput {
            b64: "QUJD+".to_string(),
            data_url: "data:image/png;base64,QUJD+".to_string(),
        };
        let (expanded, used) = expand_body(
            template,
            "p",
            "a",
            Path::new("."),
            std::slice::from_ref(&input),
            &BTreeMap::new(),
        )
        .expect("expansion works");
        assert!(used);
        let parsed: Value = serde_json::from_str(&expanded).expect("valid json");
        assert_eq!(parsed["image"], "data:image/png;base64,QUJD+");
    }

    #[test]
    fn images_array_placeholder_becomes_a_json_array() {
        let input = ImageInput {
            b64: "QQ==".to_string(),
            data_url: "data:image/jpeg;base64,QQ==".to_string(),
        };
        let (expanded, _) = expand_body(
            r#"{"images":{images_data_urls}}"#,
            "p",
            "a",
            Path::new("."),
            std::slice::from_ref(&input),
            &BTreeMap::new(),
        )
        .expect("expansion works");
        let parsed: Value = serde_json::from_str(&expanded).expect("valid json");
        assert_eq!(parsed["images"][0], "data:image/jpeg;base64,QQ==");
    }

    #[test]
    fn params_replace_template_variables_but_never_builtins() {
        let mut params = BTreeMap::new();
        params.insert("size".to_string(), "2048x1152".to_string());
        // Reserved keys are dropped by `sanitize_task_params` before they get
        // here; a stray value cannot resurrect a built-in placeholder either.
        params.insert("evil".to_string(), "{prompt}".to_string());
        let (expanded, _) = expand_body(
            r#"{"prompt":"{prompt}","size":"{size}"}"#,
            "original",
            "a",
            Path::new("."),
            &[],
            &params,
        )
        .expect("expansion works");
        let parsed: Value = serde_json::from_str(&expanded).expect("valid json");
        assert_eq!(parsed["prompt"], "original");
        assert_eq!(parsed["size"], "2048x1152");
    }

    #[test]
    fn task_params_are_sanitized_and_reserved_keys_dropped() {
        let raw = serde_json::json!({
            "size": 2048,
            "quality": "high",
            "ok-flag": true,
            "prompt": "injected",
            "bad key!": "x",
            "drop": null
        });
        let params = sanitize_task_params(raw.as_object().unwrap());
        assert_eq!(params.get("size").map(String::as_str), Some("2048"));
        assert_eq!(params.get("quality").map(String::as_str), Some("high"));
        assert_eq!(params.get("ok-flag").map(String::as_str), Some("true"));
        assert!(!params.contains_key("prompt"));
        assert!(!params.contains_key("bad key!"));
        assert!(!params.contains_key("drop"));
    }

    #[test]
    fn task_images_are_trimmed_deduped_and_capped() {
        let raw: Vec<String> = std::iter::repeat(" a.png ".to_string())
            .take(20)
            .collect();
        let images = sanitize_task_images(&raw);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0], "a.png");
        let many: Vec<String> = (0..20).map(|i| format!("img{i}.png")).collect();
        assert_eq!(sanitize_task_images(&many).len(), 16);
    }

    #[test]
    fn status_matching_ignores_case_and_space() {
        let values = vec!["completed".to_string(), "done".to_string()];
        assert!(status_matches(" Completed ", &values));
        assert!(!status_matches("running", &values));
    }

    #[test]
    fn sniffing_recognizes_common_media_containers() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 1, 2];
        assert_eq!(sniff_media(&png), Some(("png", "image/png")));
        let mut webp = b"RIFFxxxxWEBP".to_vec();
        assert_eq!(sniff_media(&webp), Some(("webp", "image/webp")));
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend_from_slice(b"ftypisom");
        assert_eq!(sniff_media(&mp4), Some(("mp4", "video/mp4")));
        assert_eq!(sniff_media(b"%PDF-1.7"), Some(("pdf", "application/pdf")));
        assert_eq!(sniff_media(b"hello world"), None);
        let _ = &mut png;
        let _ = &mut webp;
    }

    #[test]
    fn base64_detection_ignores_prose_and_short_strings() {
        assert!(!looks_like_base64("a short answer"));
        assert!(!looks_like_base64(&"there is a lot of prose here. ".repeat(300)));
        let payload = "QUJDREVGRw==".repeat(600);
        assert!(looks_like_base64(&payload));
    }

    #[test]
    fn data_urls_and_bare_base64_are_decoded_and_saved() {
        let dir = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-cloud-save-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let target = DeliverTarget {
            dir: dir.clone(),
            relative: ".data/deliver/test/task".to_string(),
        };
        // A tiny PNG header wrapped in a data URL.
        let png_bytes: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x01\x02";
        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png_bytes)
        );
        let saved = save_decoded_payload(&data_url, &target).unwrap().unwrap();
        assert!(saved.relative.starts_with(".data/deliver/test/task/image-"));
        assert!(saved.relative.ends_with(".png"));
        assert_eq!(saved.kind, "image/png");
        let file_name = saved.relative.rsplit('/').next().unwrap().to_string();
        assert!(dir.join(&file_name).is_file());
        // The `4096` minimum keeps short base64 strings (ids, tokens) as text.
        assert!(save_decoded_payload("QUJDREVGRw==", &target).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn url_extension_and_content_type_drive_saving() {
        assert_eq!(url_media_extension("https://cdn/x/clip.mp4?sig=1"), Some("mp4"));
        assert_eq!(url_media_extension("https://cdn/x/page.html"), None);
        assert_eq!(ext_for_content_type("video/mp4; charset=binary"), Some("mp4"));
        assert_eq!(ext_for_content_type("text/html"), None);
        assert!(content_type_is_media("video/mp4"));
        assert!(content_type_is_media("application/octet-stream"));
        assert!(!content_type_is_media("text/html"));
    }

    #[test]
    fn file_names_come_from_the_url_and_stay_unique() {
        let dir = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-cloud-name-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_name(&dir, "mp4", Some("https://cdn/out/a video.mp4?sig=1"));
        assert_eq!(first, "a_video.mp4");
        std::fs::write(dir.join(&first), b"x").unwrap();
        let second = unique_name(&dir, "mp4", Some("https://cdn/out/a video.mp4?sig=1"));
        assert_eq!(second, "a_video-2.mp4");
        let synthetic = unique_name(&dir, "png", None);
        assert!(synthetic.starts_with("image-") && synthetic.ends_with(".png"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_wrappers_are_searched_for_the_media_string() {
        let wrapper = r#"{"created":1,"data":[{"b64_json":"iVBORw0KGgo="}],"usage":{}}"#;
        assert!(media_string_in_json(wrapper).is_none()); // too short to count
        let long = "QUJDREVGRw==".repeat(600);
        let wrapper = format!(r#"{{"data":[{{"b64_json":"{long}"}}]}}"#);
        assert_eq!(media_string_in_json(&wrapper).as_deref(), Some(long.as_str()));
        assert_eq!(media_string_in_json("not json"), None);
    }

    #[test]
    fn huge_non_media_results_are_capped() {
        let raw = "x".repeat(MAX_RESULT_CHARS + 1);
        let capped = cap_result_text(raw);
        assert!(capped.chars().count() < MAX_RESULT_CHARS + 200);
        assert!(capped.contains("result truncated"));
        assert_eq!(cap_result_text("short".to_string()), "short");
    }
}
