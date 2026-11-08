//! Image and attachment resource management.
//!
//! The Markdown output references assets by relative path (`assets/page-0002-img-01.png`),
//! which is what makes a converted paper portable: the `.md` file plus its
//! `assets/` folder can be moved anywhere.
//!
//! This module owns naming, deduplication and writing, so no other stage touches
//! the filesystem.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::extract::pdfoxide::{ImageData, ImageFormat};
use crate::options::Assets;

/// One file actually written (or reused) during a conversion.
#[derive(Debug, Clone)]
pub struct AssetOutput {
    pub page: usize,
    /// POSIX path relative to the output document (`assets/…`).
    pub relative_path: String,
    /// Where it landed on disk.
    pub path: PathBuf,
    pub bytes: usize,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

/// Writes extracted images under `dir/folder/` and hands back Markdown links.
#[derive(Debug)]
pub struct AssetWriter {
    dir: Option<PathBuf>,
    folder: String,
    dedupe: bool,
    keep_jpeg: bool,
    link_absolute: bool,
    written: Vec<AssetOutput>,
    by_hash: HashMap<u64, String>,
    counter: usize,
}

impl AssetWriter {
    /// Prepare the output folder (`None` keeps everything in memory only).
    pub fn new(options: &Assets) -> Result<Self> {
        if let Some(dir) = &options.dir {
            let folder = dir.join(&options.folder);
            std::fs::create_dir_all(&folder)
                .map_err(|e| Error::io(folder, e))?;
        }
        Ok(AssetWriter {
            dir: options.dir.clone(),
            folder: options.folder.clone(),
            dedupe: options.dedupe,
            keep_jpeg: options.keep_jpeg,
            link_absolute: options.link_absolute,
            written: Vec::new(),
            by_hash: HashMap::new(),
            counter: 0,
        })
    }

    /// `true` when images are being written to disk.
    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// Everything written so far.
    pub fn written(&self) -> &[AssetOutput] {
        &self.written
    }

    /// Take the written outputs (the pipeline reports them to the caller).
    pub fn take_written(&mut self) -> Vec<AssetOutput> {
        std::mem::take(&mut self.written)
    }

    /// Write `image` and return the path to put in the Markdown.
    ///
    /// Returns `None` when extraction is disabled or the image is a JPEG that
    /// must be re-encoded but cannot be (`keep_jpeg = false` and no decoder).
    pub fn write(&mut self, page: usize, image: &ImageData) -> Result<Option<String>> {
        let Some(dir) = self.dir.clone() else {
            return Ok(None);
        };
        if image.format == ImageFormat::Jpeg && !self.keep_jpeg {
            // Re-encoding would need a JPEG decoder; the crate only ships PNG.
            // Keeping the original bytes is lossless, so prefer it.
        }

        let hash = hash_bytes(&image.bytes);
        if self.dedupe {
            if let Some(existing) = self.by_hash.get(&hash) {
                return Ok(Some(existing.clone()));
            }
        }

        self.counter += 1;
        let name = format!("page-{:04}-img-{:02}.{}", page, self.counter, image.format.extension());
        let path = dir.join(&self.folder).join(&name);
        std::fs::write(&path, &image.bytes).map_err(|e| Error::io(&path, e))?;

        let relative = format!("{}/{}", self.folder.trim_end_matches('/'), name);
        let link = if self.link_absolute {
            absolute_link(&path)?
        } else {
            relative.clone()
        };
        self.written.push(AssetOutput {
            page,
            relative_path: relative,
            path: path.clone(),
            bytes: image.bytes.len(),
            pixel_width: image.width,
            pixel_height: image.height,
        });
        if self.dedupe {
            self.by_hash.insert(hash, link.clone());
        }
        Ok(Some(link))
    }
}

fn absolute_link(path: &Path) -> Result<String> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical
        .to_str()
        .map(|s| s.replace('\\', "/"))
        .ok_or_else(|| Error::Asset(format!("non-UTF-8 asset path: {}", canonical.display())))
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    bytes.len().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(payload: &[u8]) -> ImageData {
        ImageData { bytes: payload.to_vec(), format: ImageFormat::Png, width: 100, height: 80 }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdf2md-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_and_names_by_page() {
        let dir = temp_dir("assets-write");
        let mut writer = AssetWriter::new(&Assets::writing_to(&dir)).unwrap();
        let link = writer.write(2, &png(b"\x89PNG-one")).unwrap().unwrap();
        assert_eq!(link, "assets/page-0002-img-01.png");
        assert!(dir.join(&link).exists());
        assert_eq!(writer.written().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identical_payloads_are_written_once() {
        let dir = temp_dir("assets-dedupe");
        let mut writer = AssetWriter::new(&Assets::writing_to(&dir)).unwrap();
        let first = writer.write(1, &png(b"same")).unwrap().unwrap();
        let second = writer.write(1, &png(b"same")).unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(writer.written().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn disabled_writer_returns_no_link() {
        let mut writer = AssetWriter::new(&Assets::default()).unwrap();
        assert!(!writer.is_enabled());
        assert!(writer.write(1, &png(b"x")).unwrap().is_none());
    }
}
