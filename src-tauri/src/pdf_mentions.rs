//! PDF `@` mentions: one page range of a workspace PDF → Markdown for the
//! composer.
//!
//! The composer's `@file` popup offers every workspace file, and a text file is
//! sliced by line right there (`read_workspace_file_range`). A PDF cannot be
//! sliced by line — and shipping the binary to the provider needs a model with
//! `supports_files` while the prompt would still be unreadable to most of
//! them — so `@paper.pdf#2` takes a different road:
//!
//! 1. the page range is parsed from the mention (`#2`, `#2-5`, `#2-`);
//! 2. the pages are rendered to Markdown by the in-process `pdf2md-rs` library;
//! 3. the Markdown is cached under `<workspace>/.data/tmp/pdf/` (the user can
//!    later read or open it, and the model is told where it is);
//! 4. the command returns it, and the frontend appends it to the LLM request as
//!    a *text* attachment — no `supports_files` requirement, same path as
//!    `@notes.md`.
//!
//! Only one page range is converted per command; a message with several PDF
//! mentions issues one call each, which keeps the IPC surface tiny and lets the
//! caller report per-mention failures.
//!
//! Why a cache file at all instead of returning the string only: the Markdown is
//! larger than what the prompt keeps (the char budget truncates it), so the full
//! conversion stays available at a stable path after the turn is sent. The name
//! is `<stem>-<digest>.md`, where the digest covers path, range and file size —
//! the same mention of the same file lands on the same file again.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

use crate::workspace;
use crate::workspace_files::resolve_in_workspace;
use crate::util::truncate_chars;
use crate::AppState;

/// Largest PDF a mention may convert (48 MiB). Bigger files are a deliberate
/// drop-the-whole-book case, not a page quote.
const MAX_PDF_BYTES: u64 = 48 * 1024 * 1024;

/// Pages converted when the mention carries no `#` range. A bare `@paper.pdf`
/// must not paste a 400-page book into the prompt; `#1-` still means "all of
/// it".
const DEFAULT_PAGE_LIMIT: usize = 60;

/// Characters of Markdown handed to the prompt per mention (≈ 15k tokens).
/// The full text stays in the cache file.
const MAX_MARKDOWN_CHARS: usize = 60_000;

/// Cached conversions older than this are dropped when a new one is written.
/// A 60-page conversion is not small, and the scratch root is shared with
/// mission batches and composer pastes. The cache survives the paste sweep
/// because it lives in a directory (`pdf/`) while that sweep only ever deletes
/// plain files at the top level — so a week here covers "read the rest of it"
/// without keeping every page ever quoted.
const CACHE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// What the frontend appends to the LLM request for one PDF mention.
#[derive(Debug, Clone, Serialize)]
pub struct PdfMention {
    /// Markdown to inline into the request, prefixed with a provenance comment.
    pub markdown: String,
    /// Workspace-relative path of the cache file (`".data/tmp/pdf/…md"`).
    pub path: String,
    /// 1-based first page that was converted.
    pub start_page: usize,
    /// 1-based last page that was converted (clamped to the document).
    pub end_page: usize,
    /// Pages the PDF actually has.
    pub total_pages: usize,
    /// The range or the character budget dropped content.
    pub truncated: bool,
}

/// One progress tick of a mention conversion, streamed to the composer.
///
/// `page`/`total` are what the pipeline is working on right now; both are 0
/// outside the page loops (reading, rendering), which is why the composer falls
/// back to a plain "converting…" label for those stages.
#[derive(Debug, Clone, Serialize)]
pub struct PdfMentionProgress {
    /// Pipeline stage code: `reading`, `extracting`, `analysing`, `parsing`,
    /// `cleaning`, `rendering` or `done`.
    pub stage: String,
    pub page: usize,
    pub total: usize,
    /// Overall completion in percent, as the pipeline computes it.
    pub percent: u8,
}

/// Resolved page window of one mention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PagePlan {
    from: usize,
    to: usize,
    /// Content was dropped: by [`DEFAULT_PAGE_LIMIT`] or by the document end.
    truncated: bool,
}

/// Turn the mention's (optional) page bounds into a concrete window.
///
/// `start = None` means "no range in the mention"; `end = None` with a start is
/// the open-ended `#2-`.
fn plan_range(total: usize, start: Option<usize>, end: Option<usize>) -> Result<PagePlan, String> {
    if total == 0 {
        return Err("the PDF has no pages".to_string());
    }
    let from = start.unwrap_or(1).max(1);
    if from > total {
        return Err(format!(
            "page {from} is past the end of the document ({total} pages)"
        ));
    }
    match end {
        // `#2` and `#2-5`: an explicit end, clamped to the document.
        Some(end) => {
            let to = end.max(from);
            Ok(PagePlan { from, to: to.min(total), truncated: to > total })
        }
        // `#2-`: the user asked for the rest of the document.
        None if start.is_some() => Ok(PagePlan { from, to: total, truncated: false }),
        // No range at all: a prompt-sized preview, not the whole book.
        None => {
            let to = total.min(DEFAULT_PAGE_LIMIT);
            Ok(PagePlan { from: 1, to, truncated: total > to })
        }
    }
}

/// FNV-1a over the cache key. Hand-rolled (not `DefaultHasher`) so the file
/// name is identical across Rust versions — a rebuild must not orphan a cache
/// the prompt already points at.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// File name without the extension of a workspace-relative path.
fn file_stem(relative: &str) -> &str {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    name.rsplit_once('.')
        .map(|(stem, _)| stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or(name)
}

/// Keep the cache names readable and portable; the digest carries uniqueness.
fn sanitize_stem(stem: &str) -> String {
    let mut out: String = stem
        .chars()
        .map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
        .collect();
    out = out.trim_matches('_').to_string();
    if out.is_empty() {
        out.push_str("document");
    }
    out.chars().take(60).collect()
}

/// Where the Markdown of this mention is cached.
fn cache_path(workspace_dir: &Path, relative: &str, plan: PagePlan, source_len: u64) -> PathBuf {
    // The converter version is part of the key: a shipped upgrade that renders
    // tables or equations differently must not silently serve an old cache.
    let key = format!(
        "{relative}|{}|{}|{source_len}|{}",
        plan.from,
        plan.to,
        pdf2md_rs::VERSION
    );
    let digest = fnv1a(key.as_bytes());
    workspace::tmp_root(workspace_dir)
        .join("pdf")
        .join(format!("{}-{digest:016x}.md", sanitize_stem(file_stem(relative))))
}

/// Relative path with `/` separators, for display and for the prompt.
fn slash_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The part of `full` the prompt gets, plus whether anything was dropped.
///
/// The cache file always keeps everything; only the request is budgeted, and
/// the cut names the cache so a later tool call can read the rest.
fn prompt_markdown(full: &str, cached_relative: &str) -> (String, bool) {
    if full.chars().count() <= MAX_MARKDOWN_CHARS {
        return (full.to_string(), false);
    }
    // The note is part of the budget, not an overshoot on top of it.
    let note = format!(
        "\n\n<!-- truncated: this mention was cut to fit the prompt — the full Markdown \
         stays at {cached_relative} (read it with file_actions), or quote fewer pages with \
         #N or a range with #N-M -->"
    );
    let budget = MAX_MARKDOWN_CHARS
        .saturating_sub(note.chars().count())
        .max(1);
    let mut cut = truncate_chars(full, budget);
    cut.push_str(&note);
    (cut, true)
}

/// Delete cached conversions older than [`CACHE_MAX_AGE`].
///
/// Only our own `*.md` files in the cache directory are touched and every
/// failure is ignored: a stale file is better than a failed mention.
fn prune_cache(dir: &Path, now: std::time::SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|ext| ext != "md").unwrap_or(true) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        // `duration_since` errs when the mtime is in the future; keep the file.
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age >= CACHE_MAX_AGE && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// User-facing message for a conversion failure.
fn describe_error(name: &str, error: &pdf2md_rs::Error) -> String {
    use pdf2md_rs::Error;
    match error {
        Error::Encrypted => format!("{name} is encrypted; the mention cannot open it"),
        Error::NoTextLayer => {
            format!("{name} has no text layer (scanned PDF) — OCR is not built in")
        }
        Error::NoPages => format!("{name} has no pages"),
        other => format!("{name}: {other}"),
    }
}

/// The whole conversion, on a blocking thread.
fn convert_blocking(
    workspace_dir: &Path,
    relative: &str,
    start: Option<usize>,
    end: Option<usize>,
    progress: &mut dyn FnMut(PdfMentionProgress),
) -> Result<PdfMention, String> {
    let file = resolve_in_workspace(workspace_dir, relative)?;
    if !file
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
    {
        return Err(format!("{relative} is not a PDF"));
    }
    let meta = std::fs::metadata(&file).map_err(|e| format!("{relative}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("not a file: {relative}"));
    }
    if meta.len() > MAX_PDF_BYTES {
        return Err(format!(
            "{relative} is too large to convert ({} bytes, limit {MAX_PDF_BYTES})",
            meta.len()
        ));
    }

    let bytes = std::fs::read(&file).map_err(|e| format!("{relative}: {e}"))?;
    let total_pages =
        pdf2md_rs::pipeline::page_count(&bytes).map_err(|e| describe_error(relative, &e))?;
    let plan = plan_range(total_pages, start, end)?;

    let mut options = pdf2md_rs::Options {
        page_range: Some((plan.from, plan.to)),
        ..Default::default()
    };
    // A paper page without its equations is much less useful than an odd
    // `$$…$$` around a centred line, so mention conversion opts in.
    options.formula.detect = true;

    // The library reports per-page and per-stage progress; forward it as-is so
    // the composer can say "page 12 of 30" while the UI is otherwise idle.
    let mut on_page = |update: pdf2md_rs::pipeline::Progress| {
        progress(PdfMentionProgress {
            stage: update.stage.as_str().to_string(),
            page: update.page,
            total: update.total,
            percent: update.percent(),
        });
    };
    let conversion = pdf2md_rs::pipeline::convert_bytes(&bytes, &options, &mut on_page)
        .map_err(|e| describe_error(relative, &e))?;
    if conversion.is_empty() {
        return Err(format!(
            "{relative}: pages {}-{} produced no text",
            plan.from, plan.to
        ));
    }

    let cached = cache_path(workspace_dir, relative, plan, meta.len());
    if let Some(parent) = cached.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let cached_relative = slash_path(workspace_dir, &cached);

    // The provenance line is part of the prompt on purpose: it names the source
    // and the cache file, so a later tool call can read the full Markdown.
    let full = format!(
        "<!-- pdf2md: {relative} · pages {from}-{to} of {total_pages} · cached at {cached_relative} -->\n\n{body}",
        from = plan.from,
        to = plan.to,
        body = conversion.markdown.trim(),
    );
    // The cache is the full conversion — that is what makes it worth keeping
    // once the prompt has been cut. Only the returned text is budgeted.
    std::fs::write(&cached, &full).map_err(|e| format!("{}: {e}", cached.display()))?;
    // Sweep stale conversions while we are writing into that directory anyway.
    if let Some(dir) = cached.parent() {
        prune_cache(dir, std::time::SystemTime::now());
    }
    let (markdown, budget_cut) = prompt_markdown(&full, &cached_relative);
    let truncated = plan.truncated || budget_cut;

    Ok(PdfMention {
        markdown,
        path: cached_relative,
        start_page: plan.from,
        end_page: plan.to,
        total_pages,
        truncated,
    })
}

/// Log one mention conversion (`INFO` on success, `WARN` on failure).
fn log_conversion(
    app: &AppHandle,
    name: &str,
    outcome: &Result<PdfMention, String>,
    elapsed: std::time::Duration,
) {
    let state = app.state::<AppState>();
    let Ok(logger) = state.logger.lock() else {
        return;
    };
    match outcome {
        Ok(mention) => logger.log(
            "INFO",
            &format!(
                "pdf2md {name}: pages {}-{} of {} → {} ({} chars{}) in {} ms",
                mention.start_page,
                mention.end_page,
                mention.total_pages,
                mention.path,
                mention.markdown.chars().count(),
                if mention.truncated { ", truncated" } else { "" },
                elapsed.as_millis()
            ),
        ),
        Err(error) => logger.log("WARN", &format!("pdf2md {name} failed: {error}")),
    }
}

/// Convert the page range of a workspace PDF into Markdown for the composer.
///
/// Arguments come from the `@path#N` mention: `start_page`/`end_page` are
/// 1-based and inclusive, `end_page = None` with a start means "to the end".
/// The Markdown is cached under `<workspace>/.data/tmp/pdf/` and also returned,
/// so the caller can append it to the LLM request without touching the disk
/// twice.
///
/// `on_progress` streams one message per visible step (`page 12 of 30`, then
/// `parsing`, `rendering`…): a 60-page mention takes long enough that a silent
/// composer looks broken.
#[tauri::command]
pub async fn convert_pdf_mention(
    app: AppHandle,
    path: String,
    start_page: Option<usize>,
    end_page: Option<usize>,
    on_progress: Channel<PdfMentionProgress>,
) -> Result<PdfMention, String> {
    let workspace_dir = app
        .state::<AppState>()
        .workspace_dir
        .lock()
        .map(|dir| dir.clone())
        .map_err(|_| "workspace directory lock poisoned".to_string())?;

    // PDF extraction is CPU-bound (text decoding, layout, rendering): keep it
    // off the IPC thread, which is also what makes an accidental 100-page
    // mention unable to freeze the UI.
    tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        // The pipeline re-reports a stage it has already announced (once per
        // page, then again on the stage change): one IPC message per visible
        // change is enough and keeps a fast conversion at a few frames.
        let mut last: Option<(String, usize, usize)> = None;
        let mut progress = move |update: PdfMentionProgress| {
            let key = (update.stage.clone(), update.page, update.total);
            if last.as_ref() == Some(&key) {
                return;
            }
            last = Some(key);
            let _ = on_progress.send(update);
        };
        let outcome = convert_blocking(&workspace_dir, &path, start_page, end_page, &mut progress);
        log_conversion(&app, &path, &outcome, started.elapsed());
        outcome
    })
    .await
    .map_err(|error| format!("PDF conversion task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    /// A PDF with three lines of prose per page (`Page N body text …`).
    ///
    /// Dense enough to pass the router's "scanned" check (40 characters per
    /// page), which a single 13-character line does not.
    fn sample_pdf(pages: usize) -> Vec<u8> {
        let mut pdf = Document::with_version("1.5");
        let pages_id = pdf.new_object_id();
        let font = pdf.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let mut kids: Vec<Object> = Vec::new();
        for page in 1..=pages {
            let lines = [
                format!("Page {page} body text starts here and keeps going"),
                String::from("The second line makes the text layer dense enough."),
                String::from("A third line closes the paragraph of this page."),
            ];
            let mut operations = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                let y = 720.0 - index as f32 * 16.0;
                operations.push(Operation::new("BT", vec![]));
                operations.push(Operation::new(
                    "Tf",
                    vec![Object::Name(b"F1".to_vec()), 12.0.into()],
                ));
                operations.push(Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 72.into(), y.into()],
                ));
                operations.push(Operation::new("Tj", vec![Object::string_literal(line.as_str())]));
                operations.push(Operation::new("ET", vec![]));
            }
            let content = Content { operations };
            let stream = pdf.add_object(Stream::new(
                dictionary! {},
                content.encode().expect("content stream"),
            ));
            let id = pdf.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Contents" => stream,
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
            });
            kids.push(id.into());
        }
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => pages as i64,
            }),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).expect("save");
        bytes
    }

    /// A throwaway workspace directory, removed on drop.
    struct TempWorkspace {
        dir: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("pdf-mention-{}-{stamp}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp workspace");
            TempWorkspace { dir }
        }

        fn put(&self, name: &str, bytes: &[u8]) {
            let path = self.dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("fixture directory");
            }
            std::fs::write(path, bytes).expect("fixture file");
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// `convert_blocking` with the progress stream dropped on the floor.
    fn convert(
        workspace_dir: &Path,
        relative: &str,
        start: Option<usize>,
        end: Option<usize>,
    ) -> Result<PdfMention, String> {
        convert_blocking(workspace_dir, relative, start, end, &mut |_| {})
    }

    #[test]
    fn progress_reports_pages_then_the_finishing_stages() {
        let ws = TempWorkspace::new();
        ws.put("kk.pdf", &sample_pdf(3));

        let mut ticks: Vec<(String, usize, usize, u8)> = Vec::new();
        convert_blocking(&ws.dir, "kk.pdf", None, None, &mut |update| {
            ticks.push((update.stage, update.page, update.total, update.percent));
        })
        .expect("convert");

        // One tick per extracted page, in order, with the running page count.
        let pages: Vec<usize> = ticks
            .iter()
            .filter(|(stage, page, total, _)| stage == "extracting" && *page > 0 && *total > 0)
            .map(|(_, page, _, _)| *page)
            .collect();
        assert_eq!(pages, vec![1, 2, 3], "{ticks:?}");

        // The stage stream ends with the finishing steps, and the percentage
        // never walks backwards (the composer draws it as a bar).
        let stages: Vec<&str> = ticks.iter().map(|(stage, _, _, _)| stage.as_str()).collect();
        assert!(stages.contains(&"rendering"), "{stages:?}");
        assert_eq!(stages.last().copied(), Some("done"), "{stages:?}");
        let percentages: Vec<u8> = ticks.iter().map(|(_, _, _, percent)| *percent).collect();
        assert!(
            percentages.windows(2).all(|pair| pair[0] <= pair[1]),
            "{percentages:?}"
        );
    }

    #[test]
    fn a_single_page_mention_converts_only_that_page() {
        let ws = TempWorkspace::new();
        ws.put("kk.pdf", &sample_pdf(3));
        let mention = convert(&ws.dir, "kk.pdf", Some(2), Some(2)).expect("convert");

        assert_eq!((mention.start_page, mention.end_page, mention.total_pages), (2, 2, 3));
        assert!(mention.markdown.contains("Page 2 body text"), "{}", mention.markdown);
        assert!(!mention.markdown.contains("Page 1 body text"), "{}", mention.markdown);
        assert!(!mention.markdown.contains("Page 3 body text"), "{}", mention.markdown);
        assert!(!mention.truncated);
    }

    #[test]
    fn an_open_ended_range_runs_to_the_last_page() {
        let ws = TempWorkspace::new();
        ws.put("kk.pdf", &sample_pdf(3));
        let mention = convert(&ws.dir, "kk.pdf", Some(2), None).expect("convert");

        assert_eq!((mention.start_page, mention.end_page), (2, 3));
        assert!(mention.markdown.contains("Page 2 body text"));
        assert!(mention.markdown.contains("Page 3 body text"));
    }

    #[test]
    fn a_range_past_the_end_is_clamped() {
        let ws = TempWorkspace::new();
        ws.put("kk.pdf", &sample_pdf(2));
        let mention = convert(&ws.dir, "kk.pdf", Some(1), Some(99)).expect("convert");

        assert_eq!(mention.end_page, 2);
        assert!(mention.truncated, "clamping must be reported");
    }

    #[test]
    fn a_page_past_the_end_is_rejected() {
        let ws = TempWorkspace::new();
        ws.put("kk.pdf", &sample_pdf(2));
        let error = convert(&ws.dir, "kk.pdf", Some(9), None).expect_err("out of range");
        assert!(error.contains("past the end"), "{error}");
    }

    #[test]
    fn a_non_pdf_mention_is_rejected() {
        let ws = TempWorkspace::new();
        ws.put("notes.md", b"# notes\n");
        let error = convert(&ws.dir, "notes.md", None, None).expect_err("not a pdf");
        assert!(error.contains("not a PDF"), "{error}");
    }

    #[test]
    fn the_markdown_is_cached_under_dot_data_tmp() {
        let ws = TempWorkspace::new();
        ws.put("papers/kk.pdf", &sample_pdf(3));
        let mention = convert(&ws.dir, "papers/kk.pdf", Some(1), Some(1)).expect("convert");

        assert!(mention.path.starts_with(".data/tmp/pdf/"), "{}", mention.path);
        assert!(mention.path.ends_with(".md"), "{}", mention.path);
        // Readable back from the workspace root with the relative path.
        let cached = std::fs::read_to_string(ws.dir.join(&mention.path)).expect("cache file");
        assert_eq!(cached, mention.markdown);
        // The provenance comment names the source, the range and the cache.
        assert!(cached.contains("pdf2md: papers/kk.pdf · pages 1-1 of 3"), "{cached}");
    }

    #[test]
    fn the_same_mention_reuses_one_cache_file() {
        let ws = TempWorkspace::new();
        ws.put("kk.pdf", &sample_pdf(3));
        let first = convert(&ws.dir, "kk.pdf", Some(2), Some(2)).expect("first");
        let second = convert(&ws.dir, "kk.pdf", Some(2), Some(2)).expect("second");
        let other = convert(&ws.dir, "kk.pdf", Some(3), Some(3)).expect("other");

        assert_eq!(first.path, second.path);
        assert_ne!(first.path, other.path, "a different range is a different file");
    }

    #[test]
    fn stale_cached_conversions_are_pruned() {
        let ws = TempWorkspace::new();
        let cache_dir = ws.dir.join(".data/tmp/pdf");
        std::fs::create_dir_all(&cache_dir).expect("cache dir");
        let stale = cache_dir.join("old-conversion-0.md");
        let unrelated = cache_dir.join("notes.txt");
        std::fs::write(&stale, "a week-old conversion").expect("stale");
        std::fs::write(&unrelated, "not ours").expect("unrelated");
        let week_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 60 * 60);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .expect("open stale")
            .set_times(std::fs::FileTimes::new().set_modified(week_ago))
            .expect("backdate stale");

        ws.put("kk.pdf", &sample_pdf(3));
        let mention = convert(&ws.dir, "kk.pdf", Some(1), Some(1)).expect("convert");

        assert!(!stale.exists(), "a week-old conversion should be gone");
        assert!(unrelated.exists(), "only our own .md files are pruned");
        assert!(ws.dir.join(&mention.path).exists(), "the fresh cache stays");
    }

    #[test]
    fn the_default_range_is_capped() {
        let plan = plan_range(400, None, None).expect("plan");
        assert_eq!((plan.from, plan.to), (1, DEFAULT_PAGE_LIMIT));
        assert!(plan.truncated);

        let short = plan_range(3, None, None).expect("plan");
        assert_eq!((short.from, short.to), (1, 3));
        assert!(!short.truncated);
    }

    #[test]
    fn an_explicit_range_is_not_capped() {
        let plan = plan_range(400, Some(1), None).expect("plan");
        assert_eq!((plan.from, plan.to), (1, 400));
        assert!(!plan.truncated);
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let text = "一二三四五";
        assert_eq!(truncate_chars(text, 2), "一二");
        assert_eq!(truncate_chars(text, 99), text);
    }

    #[test]
    fn a_budget_cut_points_at_the_cache() {
        let full = "x".repeat(MAX_MARKDOWN_CHARS + 10);
        let (cut, truncated) = prompt_markdown(&full, ".data/tmp/pdf/kk-0f.md");

        assert!(truncated);
        assert!(cut.chars().count() <= MAX_MARKDOWN_CHARS);
        // The model must learn where the rest of the conversion lives.
        assert!(cut.contains(".data/tmp/pdf/kk-0f.md"), "{}", &cut[cut.len() - 300..]);
    }

    #[test]
    fn a_short_conversion_is_returned_untouched() {
        let (text, truncated) = prompt_markdown("hello", ".data/tmp/pdf/kk.md");
        assert_eq!(text, "hello");
        assert!(!truncated);
    }

    #[test]
    fn cache_names_stay_readable() {
        assert_eq!(sanitize_stem("my paper (final)"), "my_paper__final");
        assert_eq!(sanitize_stem(""), "document");
        assert_eq!(file_stem("papers/kk.pdf"), "kk");
        assert_eq!(file_stem("no-extension"), "no-extension");
    }
}
