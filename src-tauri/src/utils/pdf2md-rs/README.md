# pdf2md-rs

A PDF → Markdown **library**: a document pipeline you embed, not a command-line
tool. It turns plain-text PDFs and academic papers into Markdown, and it stops
half-way at a shared intermediate representation (IR) so that the Markdown
writer can be swapped for an HTML / DOCX / editor backend without touching the
parsing side.

```text
bytes ──▶ extract ──▶ layout ──▶ router ──▶ parse ──▶ clean ──▶ render ──▶ Markdown
          PDFOxide    columns    plain /    IR        headers               + assets
          (GROBID?)   paragraphs paper /              paragraphs
                                 scanned
```

The crate is a workspace member of `ai-chat`; the desktop app links it as a
library. Nothing here reads arguments or writes to stdout — the caller decides
what to do with the `Conversion`.

## Quick start

```rust
use pdf2md_rs::{Options, pipeline};

// In memory (an upload, a drag-and-drop):
let conversion = pipeline::convert_bytes(&bytes, &Options::paper(), &mut |_| {})?;
println!("{}", conversion.markdown);
println!("{} blocks, {} images", conversion.document.content_block_count(), conversion.assets.len());

// A file, writing `paper.md` and `assets/` next to it:
let conversion = pipeline::convert_to_path("paper.pdf", "paper.md", &Options::paper(), &mut |_| {})?;
```

Progress and cancellation are traits, not globals:

```rust
let mut progress = |event: pdf2md_rs::Progress| eprintln!("{:?} {}%", event.stage, event.percent());
pipeline::convert_bytes_with(&bytes, &options, &control, &mut progress)?;  // `control: &dyn Control`
```

Batches, without a CLI:

```rust
use pdf2md_rs::{Options, batch};

let inputs = vec![std::path::PathBuf::from("papers")];
let batch = batch::BatchOptions::new("md/").recursive(true).concurrency(4);
for report in batch::convert(&inputs, &Options::paper(), &batch)? {
    println!("{}", report.summary());
}
```

Background jobs, for a UI that cannot block:

```rust
use pdf2md_rs::{Options, jobs::JobManager};

let manager = JobManager::new(2);                 // at most two at a time
let handle = manager.submit("paper.pdf", "paper.md", &Options::paper())?;
handle.cancel();                                   // stops at the next page
let status = manager.wait(&handle.id, std::time::Duration::from_secs(60));
```

## Modules

| module | responsibility |
|---|---|
| `extract::pdfoxide` | PDF bytes → positioned glyphs, rules, images (own content-stream interpreter, real font widths) |
| `extract::grobid` | optional GROBID TEI backend for academic papers (`grobid` feature) |
| `layout` | glyphs → text area, columns, paragraphs, reading order (column-major, gutter-aware) |
| `router` | decides plain-text / paper / scanned, and says why |
| `parse` | page layout → IR blocks: headings by numbering *and* typography, ruled/aligned tables, captions, lists, formulas |
| `clean` | running headers and footers, hyphenation, paragraphs split by a page break |
| `render` | IR → Markdown, plus the `Renderer` trait |
| `assets` | image naming, deduplication, writing |
| `pipeline` | the orchestrator, progress and cancellation |
| `jobs` | background job manager (submit / poll / cancel / wait) |
| `batch` | directories → one Markdown file per PDF, optionally parallel |

## The IR is the contract

```rust
pub enum Block {
    Heading { level: u8, text: String },
    Paragraph { text: String },
    ListItem { text: String, ordered: bool, level: u8 },
    Table { rows: Vec<Vec<String>> },
    Image { path: String, caption: Option<String> },
    Formula { latex: String, display: bool },
    Code { language: Option<String>, source: String },
    Quote { text: String },
    Rule,
    PageBreak,
}
```

`Document` also carries metadata, the page list and helpers (`to_json` /
`from_json`, `content_block_count`, `text`). A caller can serialise it, edit it
and render again — the parse side never has to run twice.

New block kinds are added here on purpose rather than behind a trait: every
renderer must handle them, so a forgotten backend becomes a compile error
instead of a silently dropped block.

## Extension points

The seams are traits and options, not configuration flags:

| what | how |
|---|---|
| OCR for image-only formulas | implement `formula::FormulaRecognizer`, pass it in `Options::formula.recognizer` |
| another output format | implement `render::Renderer` (`extension` + `render`) and call `render::with(&document, &writer)` |
| GROBID instead of the built-in layout | `Options::layout_backend = LayoutBackend::Grobid` or `ParseMode::ForceGrobid` (`grobid` feature) |
| cancellation | implement `pipeline::Control` |
| different table / formula / page-break syntax | `Options::markdown` |

## Options, briefly

`Options::paper()` and `Options::plain_text()` are the two ready-made profiles;
everything is public and `Default`:

```rust
Options {
    mode, page_range, max_pages, password,
    assets: Assets { dir, .. },
    markdown: Markdown { front_matter, escape, inline_math, heading_offset, table_style, page_breaks, .. },
    clean: Clean { remove_headers_footers, join_hyphenation, merge_paragraphs, .. },
    formula: FormulaOptions { detect, recognizer, .. },
    layout_backend, grobid: GrobidConfig { .. },
    ..
}
```

## What it does — and does not do

* **Text formulas** are found by geometry (centred, short, math symbols in the
  text layer) and transcribed to LaTeX approximately. Superscripts and radicals
  are reconstructed only when the PDF used real glyph runs.
* **Image formulas** need a recognizer plugin; without one they stay images.
* **Headings** come from numbering depth (`1.2.3` → level 3) *and* typography, so
  a document whose headings all share one font size still gets a correct outline.
* **Two-column papers** are read column-major; full-width figures, equations and
  headings split the page into bands instead of being interleaved.
* **Scanned PDFs** are refused with `Error::NoTextLayer` — no silent empty file.

## Features and MSRV

| feature | effect |
|---|---|
| `grobid` | optional GROBID HTTP backend (`ureq`, `quick-xml`) |
| `parallel` | batch conversion on a rayon pool (`batch::convert_files`) |

Default features: none. Rust 1.80.

## Tests

```sh
cargo test -p pdf2md-rs              # unit tests + a generated three-page PDF, end to end
cargo test -p pdf2md-rs --all-features
```

The end-to-end fixture is built with `lopdf` at test time (real text operators,
rules as filled rectangles, a running header, a page break inside a paragraph, a
ruled table, a bulleted list and a centred formula), so the regression it
protects is the whole pipeline, not one heuristic.

## License

GPL-2.0-only, like the rest of the workspace.
