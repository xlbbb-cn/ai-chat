//! Figures (`<svg>` / `<carve>` / `flow`) that the assistant draws, saved as
//! PNG (or `.html` for a dynamic figure).
//!
//! The plain drawing mode in [`mermaid_prompt_block`] swaps the hand-written
//! markup for a fenced `mermaid` block, which the transcript renders on its own
//! (no backend involvement) — the toggle only changes the prompt.
//!
//! The webview is the only place these figures exist as pixels: a 2D figure is
//! an SVG rasterised through a canvas, a 3D one is a three.js canvas, and both
//! reach this module as a base64 PNG. Rust therefore never parses or encodes an
//! image (no `image`/`resvg` dependency) — the backend only decides *where* the
//! bytes go and refuses anything that is not a PNG inside
//! `<workspace>/figures/`.
//!
//! `figures/` sits at the workspace root on purpose: the files belong to the
//! user, they show up in the file explorer and in `@file` mentions, and "save
//! this figure" has to produce something the user can act on. Keeping it out of
//! `.data` (which holds the database, skills and scripts) is what makes that
//! true.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::AppState;

/// Largest accepted PNG (12 MiB). The frontend rasterises at 2× device pixels,
/// so a large chart still lands far below this; the cap keeps a malformed or
/// hostile payload from filling the workspace.
const MAX_PNG_BYTES: usize = 12 * 1024 * 1024;

/// base64 inflates by 4/3. Checked before decoding so a bogus megabyte string
/// never reaches the decoder.
const MAX_BASE64_LEN: usize = MAX_PNG_BYTES / 3 * 4 + 8;

/// Largest accepted dynamic figure (2 MiB of UTF-8). A fragment of markup, style
/// and script is far smaller; the cap keeps a runaway payload out of the
/// workspace, the same way [`MAX_PNG_BYTES`] does for an image.
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

const MAX_SLUG_LEN: usize = 64;

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Windows refuses these as a file stem whatever the extension, and a save that
/// silently fails there would look like a UI bug.
const RESERVED_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Folder holding every saved figure, under the workspace root.
pub fn figures_root(workspace: &Path) -> PathBuf {
    workspace.join("figures")
}

#[derive(Debug, Clone, Serialize)]
pub struct SavedFigure {
    /// Workspace-relative path with `/` separators (`figures/<name>.png`).
    pub path: String,
    pub bytes: u64,
    /// That name was taken, so a `-2`, `-3`… suffix was used instead.
    pub renamed: bool,
}

/// Validate the figure id the webview picked for this image.
///
/// The id is the file name, so it is a whitelist and nothing else: letters,
/// digits, `-`, `_`, `.` — no separators, no `..`, no leading dot (that would
/// hide the file), no Windows device name.
pub(crate) fn sanitize_slug(raw: &str) -> Result<String, String> {
    let slug = raw.trim();
    if slug.is_empty() {
        return Err("empty figure id".to_string());
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "figure id may only contain letters, digits, '-', '_' and '.': {slug}"
        ));
    }
    if slug.starts_with('.') {
        return Err(format!("figure id must not start with a dot: {slug}"));
    }
    if slug.len() > MAX_SLUG_LEN {
        return Err(format!(
            "figure id is longer than {MAX_SLUG_LEN} characters: {slug}"
        ));
    }
    let stem = slug.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED_STEMS.contains(&stem.as_str()) {
        return Err(format!("figure id is a reserved file name: {slug}"));
    }
    Ok(slug.to_string())
}

/// Decode the PNG payload, tolerating the `data:` URL prefix a canvas
/// `toDataURL()` leaves on it.
pub(crate) fn decode_png(payload: &str) -> Result<Vec<u8>, String> {
    let payload = payload.trim();
    let payload = payload
        .strip_prefix("data:image/png;base64,")
        .unwrap_or(payload);
    if payload.len() > MAX_BASE64_LEN {
        return Err(format!(
            "PNG payload is too large ({} characters, limit {MAX_BASE64_LEN})",
            payload.len()
        ));
    }
    let bytes = STANDARD
        .decode(payload)
        .map_err(|e| format!("PNG payload is not valid base64: {e}"))?;
    if bytes.len() < PNG_MAGIC.len() || bytes[..PNG_MAGIC.len()] != PNG_MAGIC {
        return Err("payload is not a PNG image".to_string());
    }
    if bytes.len() > MAX_PNG_BYTES {
        return Err(format!(
            "PNG is too large ({} bytes, limit {MAX_PNG_BYTES})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Accept a dynamic figure payload only if it is a standalone document.
///
/// This is a shape check, not a sanitiser: the fragment ran in a sandbox before
/// it got here (see `htmlFigure.ts` on the frontend), and the sandbox is what
/// makes it safe. The check is about the *file*: what lands in `figures/` must be
/// something a browser can open on its own, so the builder's doctype is required
/// and an oversized or empty payload is refused before it touches the disk.
pub(crate) fn check_html_document(html: &str) -> Result<String, String> {
    let trimmed = html.trim_start();
    if trimmed.is_empty() {
        return Err("dynamic figure is empty".to_string());
    }
    if html.len() > MAX_HTML_BYTES {
        return Err(format!(
            "dynamic figure is too large ({} bytes, limit {MAX_HTML_BYTES})",
            html.len()
        ));
    }
    let head: String = trimmed
        .chars()
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase();
    if !head.starts_with("<!doctype") {
        return Err("dynamic figure is not a standalone HTML document".to_string());
    }
    Ok(trimmed.to_string())
}

/// `<slug>.png`, or the first free `<slug>-<n>.png`.
///
/// Saving the same figure twice must not overwrite the first file: the user
/// may have kept it, and the two may differ (an orbited `carve` scene, a
/// re-drawn chart).
fn next_free_name(root: &Path, slug: &str, extension: &str) -> (String, bool) {
    let first = format!("{slug}.{extension}");
    if !root.join(&first).exists() {
        return (first, false);
    }
    for n in 2..=99u32 {
        let candidate = format!("{slug}-{n}.{extension}");
        if !root.join(&candidate).exists() {
            return (candidate, true);
        }
    }
    // 99 saves of one figure: overwrite the last slot instead of failing.
    (format!("{slug}-99.{extension}"), true)
}

/// Write one validated PNG into `<workspace>/figures/`.
pub(crate) fn write_figure(
    workspace: &Path,
    slug: &str,
    png: &[u8],
) -> Result<SavedFigure, String> {
    write_figure_file(workspace, slug, "png", png)
}

/// Write one figure file into `<workspace>/figures/<slug>.<extension>`.
///
/// The extension is picked by the caller, never by the payload: a PNG for a
/// rasterised drawing, `html` for a `<html-fig>` (a dynamic figure has no single
/// frame, so the file *is* the figure).
pub(crate) fn write_figure_file(
    workspace: &Path,
    slug: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<SavedFigure, String> {
    let root = figures_root(workspace);
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let (file_name, renamed) = next_free_name(&root, slug, extension);
    let relative = format!("figures/{file_name}");
    // Last line of defence: the id was whitelisted and the name built here, but
    // the path still goes through the same workspace sandbox every other write
    // uses, so a future change to the naming cannot escape the workspace.
    let target = crate::tools::paths::resolve_safe_path(workspace, &relative)?;
    std::fs::write(&target, bytes).map_err(|e| format!("{relative}: {e}"))?;
    Ok(SavedFigure {
        path: relative,
        bytes: bytes.len() as u64,
        renamed,
    })
}

/// Save one rendered figure as a PNG at `<workspace>/figures/`.
#[tauri::command]
pub fn save_figure(
    state: State<'_, AppState>,
    id: String,
    png_base64: String,
) -> Result<SavedFigure, String> {
    let workspace = state
        .workspace_dir
        .lock()
        .map(|dir| dir.clone())
        .map_err(|_| "workspace directory lock poisoned".to_string())?;
    let slug = sanitize_slug(&id)?;
    let png = decode_png(&png_base64)?;
    write_figure(&workspace, &slug, &png)
}

/// Save one dynamic figure (`<html-fig>`) as a standalone document.
///
/// There is no PNG here on purpose: an animation has no single frame, and a
/// sandboxed frame cannot be rasterised from the app. What the user gets is the
/// figure itself — a complete `.html` that opens in any browser.
#[tauri::command]
pub fn save_figure_html(
    state: State<'_, AppState>,
    id: String,
    html: String,
) -> Result<SavedFigure, String> {
    let workspace = state
        .workspace_dir
        .lock()
        .map(|dir| dir.clone())
        .map_err(|_| "workspace directory lock poisoned".to_string())?;
    let slug = sanitize_slug(&id)?;
    let document = check_html_document(&html)?;
    write_figure_file(&workspace, &slug, "html", document.as_bytes())
}

/// Open `<workspace>/figures` in the OS file manager.
#[tauri::command]
pub fn app_open_figures_dir(app: AppHandle) -> Result<(), String> {
    let workspace = app
        .state::<AppState>()
        .workspace_dir
        .lock()
        .map(|dir| dir.clone())
        .map_err(|_| "workspace directory lock poisoned".to_string())?;
    let dir = figures_root(&workspace);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// System-prompt block telling the model how to draw.
///
/// Static (no workspace interpolation) so it stays a stable prefix for prompt
/// caching, and short: the conventions that matter are the ones a model gets
/// wrong on its own — literal colours instead of tokens, raw HTML that the
/// sanitizer strips, coordinates it cannot compute, and a `carve` scene that
/// is prose instead of JSON.
pub fn prompt_block() -> String {
    "Figures — you can draw, and the user can save what you draw:\n\
     - Flowcharts: a fenced `flow` block holding one JSON object — declare nodes and \
     edges, never coordinates; the renderer measures the text and lays the graph out. \
     Example: `{\"direction\":\"LR\",\"anim\":\"flow\",\"nodes\":[{\"id\":\"a\",\"label\":\
     \"提交代码\",\"shape\":\"start\"},{\"id\":\"b\",\"label\":\"CI 通过?\",\"shape\":\
     \"decision\"},{\"id\":\"c\",\"label\":\"发布\",\"tone\":\"accent\"}],\"edges\":[\
     {\"from\":\"a\",\"to\":\"b\",\"label\":\"push\"},{\"from\":\"b\",\"to\":\"c\",\"label\":\
     \"绿色\"}]}`. Shapes: `process` (default), `start`/`end` (stadium), `decision` \
     (diamond), `io`, `database`. Tones: `default`, `accent`, `success`, `warning`, \
     `danger`. Edges take an optional `label`. `anim`: `flow` (packets travel the \
     edges), `step` (nodes light up in order), or omit for static; optional `speed` \
     0.5–3. No self-loops; keep labels short. Use `flow` for every flowchart — it is \
     saved as a PNG like any drawn figure, and animated ones play on their own.\n\
     - 2D: a fenced `svg` block (or an inline `<svg>` element) holding one complete \
     `<svg viewBox=\"0 0 W H\" …>…</svg>`. Keep every colour on the interface's tokens \
     (`fill=\"var(--c-accent)\"`, `stroke=\"currentColor\"`, `var(--c-text)`, \
     `var(--c-muted)`, `var(--c-border-i)`, and `fill-opacity` steps for shades) instead of \
     literal hex values, so the figure follows light/dark and every palette. Scripts, \
     `<foreignObject>`, `<image>`, `<a>`, event handlers and external `url()` references \
     are stripped before rendering.\n\
     - 3D: a fenced `carve` block holding one JSON object: \
     `{\"camera\":{\"yaw\":35,\"pitch\":22},\"objects\":[{\"type\":\"box\",\"size\":[2,1,1],\"at\":[0,0,0]}]}`. \
     Types: `box` (`size`), `sphere` (`r`), `cylinder`/`cone` (`r`, `h`), `torus` (`r`, `tube`), \
     `plane` (`size`), `line` (`points`), `edge` (`from`/`to` — a line with an arrowhead \
     at `to`, optional `\"dashed\": true`). Optional per object: `rot: [x,y,z]`, \
     `color: accent|success|warning|danger|text|muted`, `opacity`, `wireframe`, `label` \
     (text above the object — name the parts of a scene diagram with it). A soft \
     contact shadow is drawn by default (`\"shadow\": false` opts out); `grid: true` / \
     `axes: true` add reference geometry. It renders with three.js in a canvas the \
     user can orbit by dragging.\n\
     - Dynamic (motion or interaction beyond what `flow` animates): a fenced `html-fig` \
     block (or an inline `<html-fig>…</html-fig>`) holding one self-contained HTML5 \
     fragment — markup, `<style>` and `<script>`, with no external files: the frame's \
     CSP blocks the network and every external `src`/`href`. A flowchart that must be \
     *touched* (click a node to expand it, drag boxes) is an `html-fig`; a moving \
     flowchart is `flow` with `anim`. It runs in a sandboxed frame whose \
     `:root` already carries the app's tokens (`--c-bg`, `--c-surface`, \
     `--c-surface-2`, `--c-text`, `--c-body`, `--c-muted`, `--c-border`, \
     `--c-border-i`, `--c-accent`, `--c-accent-soft`, `--c-on-accent`, \
     `--c-success`, `--c-warning`, `--c-danger`, `--c-code`), a 13px body on \
     `--c-surface`, and `@media (prefers-reduced-motion: reduce)` is honoured. Size \
     the drawing by its content — never `height: 100vh`, the card measures the frame \
     and grows with it. Use it \
     only when the figure must move or be touched; a static chart stays an `svg`, \
     and an `html-fig` is saved as `.html` rather than PNG.\n\
     - All four accept a title after the language on the fence line (`svg 抛体轨迹`) and a \
     `#name` token used for the file name (`carve #orbit 椭圆轨道`).\n\
     - Every figure gets a save button; images land in `<workspace>/figures/` as PNG, \
     dynamic figures as `.html`."
        .to_string()
}

/// System-prompt block for the plain drawing mode: Mermaid instead of a
/// hand-written `<svg>` / `carve` scene.
///
/// The transcript renders a fenced `mermaid` block itself, so this is the cheap
/// path — a few lines of Mermaid source the webview themes, instead of SVG the
/// model has to get right by hand. Static like [`prompt_block`], for the same
/// prompt-caching reason.
pub fn mermaid_prompt_block() -> String {
    "Diagrams — you can draw, and the transcript renders what you draw:\n\
     - Use a fenced `mermaid` block holding Mermaid source: `flowchart`, `sequenceDiagram`, \
     `classDiagram`, `stateDiagram-v2`, `erDiagram`, `gantt`, `pie`, `mindmap`, `timeline`. \
     Example: a fence opened with `mermaid` followed by `flowchart LR` and `  A[request] --> B[gateway]`.\n\
     - Put a title after the language on the fence line (`mermaid 请求链路`); it becomes the \
     diagram caption.\n\
     - Keep labels short, and quote any label holding parentheses, brackets or slashes: \
     `A[\"GET /v1/chat (stream)\"]`.\n\
     - Do not hand-write `<svg>` or `carve` blocks: Mermaid is the diagram format here."
        .to_string()
}

/// The drawing block for the configured mode.
///
/// `enhanced` (on by default) hands the model the `svg` / `carve` markup the
/// transcript can rasterise and save; `false` keeps it on Mermaid.
pub fn prompt_block_for(enhanced: bool) -> String {
    if enhanced {
        prompt_block()
    } else {
        mermaid_prompt_block()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_figure_ids() {
        assert_eq!(sanitize_slug("fig-1").unwrap(), "fig-1");
        assert_eq!(sanitize_slug("  parabola_curve ").unwrap(), "parabola_curve");
        assert_eq!(sanitize_slug("a.b").unwrap(), "a.b");
    }

    #[test]
    fn rejects_separators_and_dot_dot() {
        assert!(sanitize_slug("../escape").is_err());
        assert!(sanitize_slug("a/b").is_err());
        assert!(sanitize_slug("a\\b").is_err());
        assert!(sanitize_slug("..").is_err());
        assert!(sanitize_slug("").is_err());
        assert!(sanitize_slug("   ").is_err());
        assert!(sanitize_slug(".hidden").is_err());
        assert!(sanitize_slug("C:").is_err());
        assert!(sanitize_slug("fig 1").is_err());
        assert!(sanitize_slug(&"x".repeat(MAX_SLUG_LEN + 1)).is_err());
    }

    #[test]
    fn rejects_windows_device_names() {
        assert!(sanitize_slug("CON").is_err());
        assert!(sanitize_slug("com1").is_err());
        assert!(sanitize_slug("NUL.png").is_err());
    }

    #[test]
    fn decode_checks_the_png_signature() {
        let png = [PNG_MAGIC.as_slice(), &[0u8; 16]].concat();
        let encoded = STANDARD.encode(&png);
        assert_eq!(decode_png(&encoded).unwrap(), png);
        // A canvas `toDataURL()` string carries the prefix.
        assert!(decode_png(&format!("data:image/png;base64,{encoded}")).is_ok());
        // JPEG bytes, garbage and truncated payloads are refused.
        assert!(decode_png(&STANDARD.encode([0xff, 0xd8, 0xff, 0xe0])).is_err());
        assert!(decode_png("not base64 !!").is_err());
        assert!(decode_png("").is_err());
    }

    #[test]
    fn second_save_does_not_overwrite_the_first() {
        let root = std::env::temp_dir().join(format!("figures-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let png = [PNG_MAGIC.as_slice(), &[1u8; 8]].concat();
        let first = write_figure(&root, "fig-1", &png).unwrap();
        assert_eq!(first.path, "figures/fig-1.png");
        assert!(!first.renamed);

        let second = write_figure(&root, "fig-1", &png).unwrap();
        assert_eq!(second.path, "figures/fig-1-2.png");
        assert!(second.renamed);

        let third = write_figure(&root, "fig-1", &png).unwrap();
        assert_eq!(third.path, "figures/fig-1-3.png");

        let on_disk = std::fs::read(root.join("figures/fig-1.png")).unwrap();
        assert_eq!(on_disk, png);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_to_leave_the_workspace() {
        let root = std::env::temp_dir().join(format!("figures-escape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // The slug is validated before the write, so a traversal never reaches
        // `write_figure` — this pins that the id, not the path, is the boundary.
        assert!(sanitize_slug("../outside").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prompt_block_names_every_markup() {
        let block = prompt_block();
        assert!(block.contains("flow"));
        assert!(block.contains("svg"));
        assert!(block.contains("carve"));
        assert!(block.contains("html-fig"));
        assert!(block.contains("figures/"));
    }

    /// The example in the prompt must itself be valid JSON: a fence line a
    /// model copies verbatim cannot be the thing that breaks rendering.
    #[test]
    fn prompt_flow_example_is_a_valid_spec() {
        let block = prompt_block();
        let from = block.find("{\"direction\":\"LR\"").expect("flow example present");
        let end = block.find("`. Shapes:").expect("example terminated");
        let json = &block[from..end];
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(json);
        assert!(parsed.is_ok(), "prompt example must be valid JSON: {json}");
    }

    #[test]
    fn a_dynamic_figure_is_saved_as_a_document() {
        let root = std::env::temp_dir().join(format!("figures-html-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let document =
            check_html_document("<!DOCTYPE html>\n<html><body>hi</body></html>").unwrap();
        let saved = write_figure_file(&root, "fig-2", "html", document.as_bytes()).unwrap();
        assert_eq!(saved.path, "figures/fig-2.html");
        let on_disk = std::fs::read_to_string(root.join("figures/fig-2.html")).unwrap();
        assert!(on_disk.starts_with("<!DOCTYPE html>"));

        // The extension is the caller's, never the payload's: the same call
        // cannot be talked into writing a second PNG.
        let again = write_figure_file(&root, "fig-2", "html", document.as_bytes()).unwrap();
        assert_eq!(again.path, "figures/fig-2-2.html");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_a_payload_that_is_not_a_document() {
        assert!(check_html_document("").is_err());
        assert!(check_html_document("   ").is_err());
        assert!(check_html_document("<div>hi</div>").is_err());
        let huge = format!("<!DOCTYPE html>{}", "x".repeat(MAX_HTML_BYTES));
        assert!(check_html_document(&huge).is_err());
    }

    #[test]
    fn mermaid_prompt_block_asks_for_mermaid() {
        let block = mermaid_prompt_block();
        assert!(block.contains("mermaid"));
        assert!(block.contains("flowchart"));
        // The caption convention has to survive: it is what titles the card.
        assert!(block.contains("caption"));
    }

    #[test]
    fn prompt_block_for_switches_on_the_toggle() {
        assert_eq!(prompt_block_for(true), prompt_block());
        assert_eq!(prompt_block_for(false), mermaid_prompt_block());
        assert_ne!(prompt_block(), mermaid_prompt_block());
    }
}
