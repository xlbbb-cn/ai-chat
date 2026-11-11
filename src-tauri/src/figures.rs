//! Figures (`<svg>` / `<carve>`) that the assistant draws, saved as PNG.
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

/// `<slug>.png`, or the first free `<slug>-<n>.png`.
///
/// Saving the same figure twice must not overwrite the first file: the user
/// may have kept it, and the two may differ (an orbited `carve` scene, a
/// re-drawn chart).
fn next_free_name(root: &Path, slug: &str) -> (String, bool) {
    let first = format!("{slug}.png");
    if !root.join(&first).exists() {
        return (first, false);
    }
    for n in 2..=99u32 {
        let candidate = format!("{slug}-{n}.png");
        if !root.join(&candidate).exists() {
            return (candidate, true);
        }
    }
    // 99 saves of one figure: overwrite the last slot instead of failing.
    (format!("{slug}-99.png"), true)
}

/// Write one validated PNG into `<workspace>/figures/`.
pub(crate) fn write_figure(
    workspace: &Path,
    slug: &str,
    png: &[u8],
) -> Result<SavedFigure, String> {
    let root = figures_root(workspace);
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let (file_name, renamed) = next_free_name(&root, slug);
    let relative = format!("figures/{file_name}");
    // Last line of defence: the id was whitelisted and the name built here, but
    // the path still goes through the same workspace sandbox every other write
    // uses, so a future change to the naming cannot escape the workspace.
    let target = crate::tools::paths::resolve_safe_path(workspace, &relative)?;
    std::fs::write(&target, png).map_err(|e| format!("{relative}: {e}"))?;
    Ok(SavedFigure {
        path: relative,
        bytes: png.len() as u64,
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
/// sanitizer strips, and a `carve` scene that is prose instead of JSON.
pub fn prompt_block() -> String {
    "Figures — you can draw, and the user can save what you draw:\n\
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
     `plane` (`size`), `line` (`points`). Optional per object: `rot: [x,y,z]`, \
     `color: accent|success|warning|danger|text|muted`, `opacity`, `wireframe`. \
     It renders with three.js in a canvas the user can orbit by dragging.\n\
     - Both accept a title after the language on the fence line (`svg 抛体轨迹`) and a \
     `#name` token used for the PNG file name (`carve #orbit 椭圆轨道`).\n\
     - Every figure gets a save button; PNGs land in `<workspace>/figures/`."
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
    fn prompt_block_names_both_markups() {
        let block = prompt_block();
        assert!(block.contains("svg"));
        assert!(block.contains("carve"));
        assert!(block.contains("figures/"));
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