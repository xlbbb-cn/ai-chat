//! Batch conversion: directories in, one Markdown file per PDF out.
//!
//! A single conversion is [`crate::pipeline`]'s job; this module adds the three
//! things a caller needs when the input is not one file:
//!
//! * **discovery** ([`collect_inputs`]) — files and directories in, a sorted,
//!   de-duplicated list of `*.pdf` out, optionally recursive;
//! * **planning** ([`plan_outputs`]) — where each Markdown file goes, without
//!   overwriting one output with another when two inputs share a stem;
//! * **execution** ([`convert_files`]) — the same conversion for every file,
//!   with an optional worker pool.
//!
//! ```no_run
//! use pdf2md_rs::{Options, batch};
//!
//! let inputs = vec![std::path::PathBuf::from("papers")];
//! let batch = batch::BatchOptions::new("out/").recursive(true).concurrency(4);
//! let reports = batch::convert(&inputs, &Options::paper(), &batch)?;
//! for report in &reports {
//!     println!("{} → {} ({})", report.input.display(), report.output.display(), report.summary());
//! }
//! # Ok::<(), pdf2md_rs::Error>(())
//! ```
//!
//! Parallelism needs the `parallel` feature (rayon); without it the files are
//! converted one after another with exactly the same results, which is what a
//! deterministic test or a memory-constrained caller wants.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::options::Options;
use crate::pipeline;
use crate::router::DocumentClass;

/// How a batch should be discovered, written and parallelised.
#[derive(Debug, Clone, Default)]
pub struct BatchOptions {
    /// Directory (or, for a single input, `.md` file) to write to. `None` writes
    /// each Markdown file next to its PDF.
    pub output: Option<PathBuf>,
    /// Descend into subdirectories.
    pub recursive: bool,
    /// How many conversions may run at once (`0` = one per CPU, capped at 16).
    pub concurrency: usize,
}

impl BatchOptions {
    /// Write every result into `output`.
    pub fn new(output: impl Into<PathBuf>) -> Self {
        BatchOptions { output: Some(output.into()), ..BatchOptions::default() }
    }

    /// Descend into subdirectories.
    pub fn recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Number of worker threads (`0` = one per CPU).
    pub fn concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// The resolved worker count.
    pub fn workers(&self) -> usize {
        if self.concurrency > 0 {
            return self.concurrency;
        }
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16)
    }
}

/// What one file of a batch produced.
#[derive(Debug, Clone)]
pub struct FileReport {
    pub input: PathBuf,
    pub output: PathBuf,
    /// The IR metrics, `None` when the conversion failed.
    pub pages: usize,
    pub blocks: usize,
    pub images: usize,
    pub class: DocumentClass,
    /// Failure message, when the conversion did not finish.
    pub error: Option<String>,
}

impl FileReport {
    /// `true` when the Markdown was written.
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// One line for a log or a UI row.
    pub fn summary(&self) -> String {
        match &self.error {
            Some(error) => format!("{} → failed: {error}", self.input.display()),
            None => format!(
                "{} → {} ({} blocks, {} images, {})",
                self.input.display(),
                self.output.display(),
                self.blocks,
                self.images,
                self.class.as_str(),
            ),
        }
    }
}

/// Expand `inputs` into a sorted, de-duplicated list of PDF files.
///
/// A directory contributes the `*.pdf` files directly inside it — or, with
/// `recursive`, everywhere below it. Symlinks are not followed: a directory tree
/// that links back to its parent would otherwise never end.
pub fn collect_inputs(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = Vec::new();
    for input in inputs {
        if input.is_file() {
            files.push(input.clone());
            continue;
        }
        if !input.is_dir() {
            return Err(Error::io(
                input,
                std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory"),
            ));
        }
        walk(input, recursive, &mut files)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn walk(directory: &Path, recursive: bool, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(directory).map_err(|e| Error::io(directory, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(directory, e))?;
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if recursive {
                walk(&path, recursive, files)?;
            }
            continue;
        }
        let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        if is_pdf && kind.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

/// Decide the Markdown path for every input.
///
/// * `output` naming a `.md` file is only honoured for a single input;
/// * anything else is treated as a directory, which is created if needed;
/// * two inputs with the same stem never share an output: the second becomes
///   `name-2.md`.
pub fn plan_outputs(files: &[PathBuf], output: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(output) = output {
        let single_file = files.len() == 1
            && !output.is_dir()
            && output.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if single_file {
            return Ok(vec![output.to_path_buf()]);
        }
        std::fs::create_dir_all(output).map_err(|e| Error::io(output, e))?;
        let mut used: HashSet<PathBuf> = HashSet::new();
        return Ok(files
            .iter()
            .map(|input| {
                let stem = stem_of(input);
                let mut candidate = output.join(format!("{stem}.md"));
                let mut counter = 2;
                while !used.insert(candidate.clone()) {
                    candidate = output.join(format!("{stem}-{counter}.md"));
                    counter += 1;
                }
                candidate
            })
            .collect());
    }

    let mut used: HashSet<PathBuf> = HashSet::new();
    Ok(files
        .iter()
        .map(|input| {
            let mut candidate = input.with_extension("md");
            let mut counter = 2;
            while !used.insert(candidate.clone()) {
                candidate = input.with_extension(format!("{counter}.md"));
                counter += 1;
            }
            candidate
        })
        .collect())
}

fn stem_of(input: &Path) -> String {
    input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "document".into())
}

/// Convert every file, writing Markdown (and, with assets enabled, images).
///
/// Images are written next to the Markdown file unless
/// [`Options::assets`](crate::options::Options::assets) names a directory, so a
/// batch result is portable by default.
pub fn convert_files(
    files: &[PathBuf],
    outputs: &[PathBuf],
    options: &Options,
    concurrency: usize,
) -> Vec<FileReport> {
    assert_eq!(files.len(), outputs.len(), "one output per input");
    let work = |index: usize| convert_one(&files[index], &outputs[index], options);
    map(work, files.len(), concurrency)
}

/// Discover, plan and convert in one call.
pub fn convert(
    inputs: &[PathBuf],
    options: &Options,
    batch: &BatchOptions,
) -> Result<Vec<FileReport>> {
    let files = collect_inputs(inputs, batch.recursive)?;
    if files.is_empty() {
        return Err(Error::io(
            inputs.first().map(PathBuf::as_path).unwrap_or(Path::new(".")),
            std::io::Error::new(std::io::ErrorKind::NotFound, "no PDF files found"),
        ));
    }
    let outputs = plan_outputs(&files, batch.output.as_deref())?;
    Ok(convert_files(&files, &outputs, options, batch.workers()))
}

fn convert_one(input: &Path, output: &Path, options: &Options) -> FileReport {
    let mut options = options.clone();
    if options.assets.dir.is_none() {
        let parent = output.parent().filter(|p| !p.as_os_str().is_empty());
        options.assets.dir = Some(parent.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")));
    }
    let failed = |error: String| FileReport {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        pages: 0,
        blocks: 0,
        images: 0,
        class: DocumentClass::PlainText,
        error: Some(error),
    };

    let conversion = match pipeline::convert_to_path(input, output, &options, &mut |_| {}) {
        Ok(conversion) => conversion,
        Err(error) => return failed(error.to_string()),
    };
    FileReport {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        pages: conversion.document.metadata.page_count,
        blocks: conversion.document.content_block_count(),
        images: conversion.assets.len(),
        class: conversion.profile.class,
        error: None,
    }
}

#[cfg(feature = "parallel")]
fn map(work: impl Fn(usize) -> FileReport + Sync, count: usize, concurrency: usize) -> Vec<FileReport> {
    use rayon::prelude::*;
    let workers = if concurrency > 0 { concurrency } else { BatchOptions::default().workers() };
    match rayon::ThreadPoolBuilder::new().num_threads(workers.max(1)).build() {
        Ok(pool) => pool.install(|| (0..count).into_par_iter().map(&work).collect()),
        // A pool can fail to start under memory pressure; the batch still
        // finishes, just without parallelism.
        Err(_) => (0..count).map(&work).collect(),
    }
}

#[cfg(not(feature = "parallel"))]
fn map(work: impl Fn(usize) -> FileReport, count: usize, _concurrency: usize) -> Vec<FileReport> {
    (0..count).map(work).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_stems_keep_their_names() {
        let files = vec![PathBuf::from("/tmp/a/x.pdf"), PathBuf::from("/tmp/b/y.pdf")];
        let outputs = plan_outputs(&files, Some(Path::new("/tmp/out"))).expect("plan");
        assert_eq!(outputs[0], PathBuf::from("/tmp/out/x.md"));
        assert_eq!(outputs[1], PathBuf::from("/tmp/out/y.md"));
    }

    #[test]
    fn outputs_never_collide() {
        let files = vec![PathBuf::from("/tmp/a/report.pdf"), PathBuf::from("/tmp/b/report.pdf")];
        let outputs = plan_outputs(&files, Some(Path::new("/tmp/out"))).expect("plan");
        assert_eq!(outputs[0], PathBuf::from("/tmp/out/report.md"));
        assert_eq!(outputs[1], PathBuf::from("/tmp/out/report-2.md"));
    }

    #[test]
    fn a_single_md_output_is_taken_literally() {
        let files = vec![PathBuf::from("/tmp/x.pdf")];
        let outputs = plan_outputs(&files, Some(Path::new("/tmp/out/renamed.md"))).expect("plan");
        assert_eq!(outputs, vec![PathBuf::from("/tmp/out/renamed.md")]);
    }

    #[test]
    fn missing_directories_are_an_error() {
        let error = collect_inputs(&[PathBuf::from("/definitely/not/here")], false)
            .expect_err("a missing directory");
        assert!(matches!(error, Error::Io { .. }), "{error}");
    }

    #[test]
    fn the_worker_count_is_bounded() {
        assert_eq!(BatchOptions::default().concurrency(3).workers(), 3);
        let automatic = BatchOptions::default().workers();
        assert!((1..=16).contains(&automatic), "{automatic}");
    }
}
