//! Extraction of embedded raster images (`/XObject /Subtype /Image`).
//!
//! Only what a text-extraction pass can do cheaply is supported: pass JPEG
//! through untouched, and re-encode Flate/raw samples (Gray, RGB, CMYK,
//! Indexed, 1/2/4/8 bit) as PNG. JPEG 2000, JBIG2 and CCITT fax are reported as
//! unsupported rather than silently dropped, so callers can log them.

use std::io::Cursor;

use image::{DynamicImage, ExtendedColorType, ImageBuffer, Luma, Rgb};
use lopdf::{Dictionary, Object, Stream};

use crate::error::{Error, Result};

/// Payload format of an extracted image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
}

impl ImageFormat {
    /// `jpg` / `png`.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
        }
    }

    /// The MIME type, for callers that hand the bytes to a model or a UI.
    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        }
    }
}

/// One decoded image, ready to be written to disk.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
}

impl ImageData {
    /// Pixel count, used to drop rules and bullets.
    pub fn pixels(&self) -> u32 {
        self.width.saturating_mul(self.height)
    }
}

/// Colour model of the raw samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorModel {
    Gray,
    Rgb,
    Cmyk,
    Indexed,
}

/// Extract the samples of an image XObject.
pub fn extract(stream: &Stream) -> Result<ImageData> {
    let dict = &stream.dict;
    let width = int(dict, b"Width").unwrap_or(0).max(0) as u32;
    let height = int(dict, b"Height").unwrap_or(0).max(0) as u32;
    if width == 0 || height == 0 {
        return Err(Error::UnsupportedImage("image has a zero dimension".into()));
    }

    let filters = filter_names(dict);
    if filters.iter().any(|f| f == "DCTDecode" || f == "DCT") {
        // Already a JPEG file; keep the bytes bit-exact.
        return Ok(ImageData {
            bytes: stream.content.clone(),
            format: ImageFormat::Jpeg,
            width,
            height,
        });
    }
    if let Some(unsupported) = filters
        .iter()
        .find(|f| matches!(f.as_str(), "JPXDecode" | "JBIG2Decode" | "CCITTFaxDecode"))
    {
        return Err(Error::UnsupportedImage(format!("{unsupported} images are not decoded")));
    }

    let data = stream
        .decompressed_content()
        .map_err(|e| Error::UnsupportedImage(format!("cannot decompress image stream: {e}")))?;

    let bpc = int(dict, b"BitsPerComponent").unwrap_or(8).clamp(1, 16) as u8;
    let model = color_model(dict)?;
    let palette = if model == ColorModel::Indexed { Some(indexed_palette(dict)?) } else { None };

    let samples = expand_samples(&data, width, height, bpc, channels(model))?;
    let rgb: Vec<u8> = match model {
        ColorModel::Gray => samples.iter().flat_map(|g| [*g, *g, *g]).collect(),
        ColorModel::Rgb => samples,
        ColorModel::Cmyk => cmyk_to_rgb(&samples),
        ColorModel::Indexed => {
            let palette = palette.ok_or_else(|| Error::UnsupportedImage("missing palette".into()))?;
            samples
                .iter()
                .map(|i| {
                    let base = (*i as usize) * 3;
                    if base + 2 < palette.len() {
                        [palette[base], palette[base + 1], palette[base + 2]]
                    } else {
                        [0, 0, 0]
                    }
                })
                .flatten()
                .collect()
        }
    };

    let buffer: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, rgb)
        .ok_or_else(|| Error::UnsupportedImage("sample count does not match the image size".into()))?;
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(buffer)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| Error::ImageEncode(e.to_string()))?;
    Ok(ImageData { bytes: out, format: ImageFormat::Png, width, height })
}

/// Decode an image already held in memory (a plugin point for formula OCR).
pub fn encode_png_gray(width: u32, height: u32, gray: &[u8]) -> Result<Vec<u8>> {
    let buffer: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, gray.to_vec())
        .ok_or_else(|| Error::UnsupportedImage("gray sample count mismatch".into()))?;
    let mut out = Vec::new();
    DynamicImage::ImageLuma8(buffer)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| Error::ImageEncode(e.to_string()))?;
    Ok(out)
}

fn int(dict: &Dictionary, key: &[u8]) -> Option<i64> {
    dict.get(key).ok().and_then(|o| o.as_i64().ok()).or_else(|| {
        dict.get(key).ok().and_then(|o| o.as_float().ok()).map(|f| f as i64)
    })
}

fn filter_names(dict: &Dictionary) -> Vec<String> {
    let Ok(filter) = dict.get(b"Filter") else {
        return Vec::new();
    };
    match filter {
        Object::Name(name) => vec![String::from_utf8_lossy(name).to_string()],
        Object::Array(items) => items
            .iter()
            .filter_map(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn channels(model: ColorModel) -> usize {
    match model {
        ColorModel::Gray | ColorModel::Indexed => 1,
        ColorModel::Rgb => 3,
        ColorModel::Cmyk => 4,
    }
}

fn color_model(dict: &Dictionary) -> Result<ColorModel> {
    let Ok(space) = dict.get(b"ColorSpace") else {
        return Ok(ColorModel::Gray);
    };
    let name = match space {
        Object::Name(name) => String::from_utf8_lossy(name).to_string(),
        Object::Array(items) => {
            let base = items
                .first()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string())
                .unwrap_or_default();
            if base == "Indexed" || base == "I" {
                return Ok(ColorModel::Indexed);
            }
            if base == "ICCBased" {
                // A stream whose `/N` gives the component count.
                let n = items
                    .get(1)
                    .and_then(|o| o.as_stream().ok())
                    .and_then(|s| s.dict.get(b"N").ok())
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(3);
                return Ok(if n <= 1 {
                    ColorModel::Gray
                } else if n == 4 {
                    ColorModel::Cmyk
                } else {
                    ColorModel::Rgb
                });
            }
            base
        }
        _ => return Ok(ColorModel::Gray),
    };
    Ok(match name.as_str() {
        "DeviceGray" | "G" | "CalGray" => ColorModel::Gray,
        "DeviceRGB" | "RGB" | "CalRGB" => ColorModel::Rgb,
        "DeviceCMYK" | "CMYK" => ColorModel::Cmyk,
        "Indexed" | "I" => ColorModel::Indexed,
        other => return Err(Error::UnsupportedImage(format!("colour space {other}"))),
    })
}

/// `/Lookup` of an indexed colour space, expanded to an RGB byte triple per entry.
fn indexed_palette(dict: &Dictionary) -> Result<Vec<u8>> {
    let space = dict
        .get(b"ColorSpace")
        .map_err(|_| Error::UnsupportedImage("missing ColorSpace".into()))?;
    let items = space
        .as_array()
        .map_err(|_| Error::UnsupportedImage("indexed ColorSpace is not an array".into()))?;
    let lookup = items
        .get(3)
        .ok_or_else(|| Error::UnsupportedImage("indexed ColorSpace has no lookup".into()))?;
    let bytes = match lookup {
        Object::String(bytes, _) => bytes.clone(),
        Object::Stream(stream) => stream.decompressed_content().unwrap_or_default(),
        _ => return Err(Error::UnsupportedImage("indexed lookup is not a string".into())),
    };
    Ok(bytes)
}

/// Expand `bpc`-bit samples to one byte per sample.
fn expand_samples(data: &[u8], width: u32, height: u32, bpc: u8, channels: usize) -> Result<Vec<u8>> {
    let expected = width as usize * height as usize * channels;
    if bpc == 8 {
        let mut out = data.to_vec();
        if out.len() < expected {
            out.resize(expected, 0);
        }
        out.truncate(expected);
        return Ok(out);
    }
    if bpc != 1 && bpc != 2 && bpc != 4 {
        return Err(Error::UnsupportedImage(format!("{bpc} bits per component")));
    }
    let per_byte = (8 / bpc) as usize;
    let mask = (1u16 << bpc) - 1;
    let max = mask as f32;
    let mut out = Vec::with_capacity(expected);
    let row_bytes = (width as usize * channels * bpc as usize).div_ceil(8);
    for row in 0..height as usize {
        let start = row * row_bytes;
        let row_data = data.get(start..start + row_bytes).unwrap_or(&[]);
        for index in 0..width as usize * channels {
            let byte_index = index / per_byte;
            let byte = *row_data.get(byte_index).unwrap_or(&0) as usize;
            let shift = 8 - bpc as usize * (index % per_byte + 1);
            let value = (byte >> shift) & mask as usize;
            out.push(((value as f32 / max) * 255.0).round() as u8);
        }
    }
    Ok(out)
}

fn cmyk_to_rgb(samples: &[u8]) -> Vec<u8> {
    samples
        .chunks_exact(4)
        .flat_map(|px| {
            let k = px[3] as f32 / 255.0;
            let f = |c: u8| ((1.0 - c as f32 / 255.0) * (1.0 - k) * 255.0).round() as u8;
            [f(px[0]), f(px[1]), f(px[2])]
        })
        .collect()
}

/// Overlay a 1-bit `/ImageMask` (stencil) as a gray bitmap — used by some
/// scanned figures and by LaTeX-rendered math.
pub fn image_mask(stream: &Stream, width: u32, height: u32) -> Result<ImageData> {
    let data = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
    let gray = expand_samples(&data, width, height, 1, 1)?;
    let png = encode_png_gray(width, height, &gray)?;
    Ok(ImageData { bytes: png, format: ImageFormat::Png, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Dictionary, Stream};

    fn image_stream(dict: Dictionary, content: Vec<u8>) -> Stream {
        Stream::new(dict, content)
    }

    #[test]
    fn jpeg_is_passed_through_untouched() {
        let mut dict = Dictionary::new();
        dict.set("Width", 10);
        dict.set("Height", 20);
        dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        let payload = b"\xff\xd8\xff\xd9".to_vec();
        let data = extract(&image_stream(dict, payload.clone())).unwrap();
        assert_eq!(data.format, ImageFormat::Jpeg);
        assert_eq!(data.bytes, payload);
        assert_eq!(data.width * data.height, 200);
    }

    #[test]
    fn raw_gray_samples_become_png() {
        let mut dict = Dictionary::new();
        dict.set("Width", 2);
        dict.set("Height", 2);
        dict.set("BitsPerComponent", 8);
        dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        let data = extract(&image_stream(dict, vec![0, 64, 128, 255])).unwrap();
        assert_eq!(data.format, ImageFormat::Png);
        assert!(data.bytes.starts_with(b"\x89PNG"));
    }

    #[test]
    fn jpeg2000_is_reported_unsupported() {
        let mut dict = Dictionary::new();
        dict.set("Width", 4);
        dict.set("Height", 4);
        dict.set("Filter", Object::Name(b"JPXDecode".to_vec()));
        assert!(matches!(
            extract(&image_stream(dict, vec![])),
            Err(Error::UnsupportedImage(_))
        ));
    }

    #[test]
    fn bit_expansion_scales_to_bytes() {
        let out = expand_samples(&[0b1000_0000], 8, 1, 1, 1).unwrap();
        assert_eq!(out, vec![255, 0, 0, 0, 0, 0, 0, 0]);
    }
}
