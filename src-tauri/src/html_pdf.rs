//! HTML → PDF for everything the app exports as a document.
//!
//! What is exported is rendered HTML — the Markdown editor's preview
//! (react-markdown + KaTeX + GFM tables) or a reply with its figure cards — so
//! the only faithful route to a `.pdf` is a browser print engine. The app's
//! own webview cannot do it: wry exposes no `PrintToPdfAsync`, and calling
//! `print()` only opens a dialog. A headless Chromium is therefore started on
//! the document — Edge on Windows (always installed there), Chrome or Chromium
//! elsewhere — and the file it writes is the export.
//!
//! Two things this module has to get right, both learned the hard way:
//!
//! * **A private profile per run.** `--user-data-dir` must point at a fresh
//!   directory. With the default profile a running browser accepts the command
//!   line, prints nothing and exits 0 — a silent empty export.
//! * **Wait for the file, not the process.** `msedge.exe` is a launcher: it
//!   exits in ~50 ms with status 0 while the real browser finishes in the
//!   background, so waiting for the child returns before the PDF exists. The
//!   export polls the target file instead — until it ends with `%%EOF`, which is
//!   the only proof that the writer is done — and gives up after [`TIMEOUT`].

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Returned when the machine has no Chromium engine; the frontend falls back to
/// the system print dialog instead of reporting an error.
pub const NO_ENGINE: &str = "NO_PDF_ENGINE";

const TIMEOUT: Duration = Duration::from_secs(45);
/// Poll interval while the browser lays the document out.
const POLL: Duration = Duration::from_millis(100);
/// How much of the engine's stderr travels back in an error message.
const STDERR_TAIL: usize = 400;
/// A PDF is only complete once its trailer is on disk; Chromium writes `%%EOF`
/// last, so a file without it may still be growing.
const PDF_TRAILER: &[u8] = b"%%EOF";

/// Directories a per-machine or per-user browser install lives under.
#[cfg(target_os = "windows")]
fn install_roots() -> Vec<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"]
        .iter()
        .filter_map(|key| std::env::var_os(key))
        .map(PathBuf::from)
        .collect()
}

#[cfg(not(target_os = "windows"))]
fn install_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// Browser executables relative to an install root (Windows layout).
#[cfg(target_os = "windows")]
const INSTALL_RELATIVE: &[&str] = &[
    "Microsoft\\Edge\\Application\\msedge.exe",
    "Google\\Chrome\\Application\\chrome.exe",
];

#[cfg(not(target_os = "windows"))]
const INSTALL_RELATIVE: &[&str] = &[];

/// Browser executables at a fixed path (macOS bundles).
#[cfg(target_os = "macos")]
const ABSOLUTE_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];

#[cfg(not(target_os = "macos"))]
const ABSOLUTE_CANDIDATES: &[&str] = &[];

/// Executable names on `PATH` — the way a Linux package manager installs them.
#[cfg(target_os = "windows")]
const PATH_NAMES: &[&str] = &["msedge.exe", "chrome.exe"];

#[cfg(not(target_os = "windows"))]
const PATH_NAMES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "microsoft-edge",
    "chromium",
    "chromium-browser",
];

fn engine_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in install_roots() {
        for relative in INSTALL_RELATIVE {
            out.push(root.join(relative));
        }
    }
    for absolute in ABSOLUTE_CANDIDATES {
        out.push(PathBuf::from(absolute));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in PATH_NAMES {
                out.push(dir.join(name));
            }
        }
    }
    out
}

/// The first Chromium engine installed on this machine, if any.
pub fn engine() -> Option<PathBuf> {
    engine_candidates().into_iter().find(|path| path.is_file())
}

/// A `file:` URL — a path with spaces or CJK in it is not one.
pub fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut url = if text.starts_with('/') {
        String::from("file://")
    } else {
        String::from("file:///")
    };
    for byte in text.bytes() {
        let literal = byte.is_ascii_alphanumeric()
            || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~');
        if literal {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// Print `html` to `out`.
///
/// The caller owns `out` (the save dialog already asked about overwriting); the
/// file is removed first so a silent engine failure cannot look like a success
/// that left the previous export in place.
pub async fn export(html: &str, out: &Path) -> Result<(), String> {
    let engine = engine().ok_or_else(|| NO_ENGINE.to_string())?;
    let work = std::env::temp_dir().join(format!("rabbit-chat-studio-pdf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&work)
        .map_err(|err| format!("Failed to create the print workspace: {err}"))?;
    let result = run(&engine, html, out, &work).await;
    // Only now, with the PDF complete on disk, can the document and the private
    // profile go: the browser reads both while it prints.
    let _ = std::fs::remove_dir_all(&work);
    result
}

async fn run(engine: &Path, html: &str, out: &Path, work: &Path) -> Result<(), String> {
    let document = work.join("doc.html");
    std::fs::write(&document, html)
        .map_err(|err| format!("Failed to write the print document: {err}"))?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create the target folder: {err}"))?;
    }
    let _ = std::fs::remove_file(out);

    // stderr goes to a file rather than a pipe: nobody drains a pipe while the
    // export polls, and a full pipe would stall the browser mid-print.
    let log_path = work.join("engine.log");
    let log = File::create(&log_path)
        .map_err(|err| format!("Failed to create the engine log: {err}"))?;

    let mut command = tokio::process::Command::new(engine);
    command
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--disable-extensions")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--user-data-dir={}", work.join("profile").display()))
        // Without it the sheet carries the document URL and the print date.
        .arg("--no-pdf-header-footer")
        .arg(format!("--print-to-pdf={}", out.display()))
        .arg(file_url(&document))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    let mut child = command
        .spawn()
        .map_err(|err| format!("Failed to start {}: {err}", engine.display()))?;

    let deadline = Instant::now() + TIMEOUT;
    let written = loop {
        if pdf_is_complete(out) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(POLL).await;
    };
    // The launcher is long gone on success; this only reaps a live child.
    let _ = child.start_kill();

    if written {
        return Ok(());
    }
    let tail = read_tail(&log_path, STDERR_TAIL);
    Err(if tail.is_empty() {
        "The PDF engine produced no file".to_string()
    } else {
        format!("The PDF engine produced no file: {tail}")
    })
}

/// `true` once `out` is a PDF whose trailer is on disk.
fn pdf_is_complete(out: &Path) -> bool {
    let Ok(mut file) = File::open(out) else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if meta.len() < 1024 {
        return false;
    }
    let mut header = [0u8; 5];
    if file.read_exact(&mut header).is_err() || &header != b"%PDF-" {
        return false;
    }
    let tail_len = (PDF_TRAILER.len() as u64 + 64).min(meta.len());
    if file
        .seek(SeekFrom::End(-(tail_len as i64)))
        .is_err()
    {
        return false;
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return false;
    }
    tail.windows(PDF_TRAILER.len())
        .any(|window| window == PDF_TRAILER)
}

fn read_tail(path: &Path, limit: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= limit {
        return trimmed.to_string();
    }
    trimmed.chars().skip(count - limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chromium_engine_is_located() {
        match engine() {
            Some(path) => eprintln!("using {}", path.display()),
            None => eprintln!("skipped: no Chromium engine on this machine"),
        }
    }

    #[test]
    fn file_urls_survive_spaces_and_cjk() {
        #[cfg(target_os = "windows")]
        assert_eq!(
            file_url(Path::new(r"C:\Users\a b\文档\doc.html")),
            "file:///C:/Users/a%20b/%E6%96%87%E6%A1%A3/doc.html"
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            file_url(Path::new("/tmp/a b/doc.html")),
            "file:///tmp/a%20b/doc.html"
        );
    }

    /// The whole path, for real: an engine on this machine turns the document
    /// the editor builds into a file that starts with `%PDF-` and ends with a
    /// trailer.
    #[tokio::test]
    async fn writes_a_pdf() {
        if engine().is_none() {
            eprintln!("skipped: no Chromium engine on this machine");
            return;
        }
        let dir = std::env::temp_dir().join(format!("rabbit-chat-studio-pdf-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.pdf");
        let html = concat!(
            "<!DOCTYPE html><html lang=\"zh\"><head><meta charset=\"utf-8\">",
            "<title>test</title><style>@page{size:A4;margin:16mm}</style></head>",
            "<body><h1>标题</h1><p>hello</p></body></html>"
        );
        export(html, &out).await.expect("export should write a PDF");
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"%PDF-"), "not a PDF");
        assert!(bytes.len() > 1000, "suspiciously small PDF");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
