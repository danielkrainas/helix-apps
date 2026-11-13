//! Saving documents as PDF, PNG or JPEG.

use crate::page::Page;
use crate::pdf::{self, Filter, PdfImage};
use image::codecs::jpeg::{JpegEncoder, PixelDensity};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Png,
    Jpeg,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Pdf, Format::Jpeg, Format::Png];

    pub fn mime(self) -> &'static str {
        match self {
            Format::Pdf => "application/pdf",
            Format::Png => "image/png",
            Format::Jpeg => "image/jpeg",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Png => "png",
            Format::Jpeg => "jpg",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Pdf => "PDF (multi-page document)",
            Format::Png => "PNG (lossless image)",
            Format::Jpeg => "JPEG (compressed image)",
        }
    }

    pub fn from_mime(mime: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.mime() == mime)
    }

    pub fn from_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => Some(Format::Pdf),
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpeg),
            _ => None,
        }
    }
}

/// Resolves the format for a chosen path, appending the default format's
/// extension when the path has no recognised one.
pub fn resolve_path(path: &Path, default: Format) -> (PathBuf, Format) {
    match Format::from_path(path) {
        Some(f) => (path.to_path_buf(), f),
        None => {
            let mut s = path.as_os_str().to_owned();
            s.push(".");
            s.push(default.extension());
            (PathBuf::from(s), default)
        }
    }
}

/// Output files for `n` pages: the path itself for one page (or PDF),
/// otherwise `stem-1.ext`, `stem-2.ext`, …
pub fn output_paths(path: &Path, n: usize, format: Format) -> Vec<PathBuf> {
    if format == Format::Pdf || n <= 1 {
        return vec![path.to_path_buf()];
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    (1..=n).map(|i| path.with_file_name(format!("{stem}-{i}.{ext}"))).collect()
}

fn color_type(p: &Page) -> ExtendedColorType {
    if p.is_color() {
        ExtendedColorType::Rgb8
    } else {
        ExtendedColorType::L8
    }
}

pub fn encode_jpeg(p: &Page, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    let dpi = p.dpi.round().clamp(1.0, u16::MAX as f64) as u16;
    enc.set_pixel_density(PixelDensity::dpi(dpi));
    enc.encode(&p.pixels, p.width, p.height, color_type(p)).map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn encode_png(p: &Page) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&p.pixels, p.width, p.height, color_type(p))
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn deflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

/// Builds a PDF of `pages` (crops applied). Photo pages are JPEG-encoded,
/// text pages are stored losslessly.
pub fn encode_pdf(pages: &[Page], quality: u8) -> Result<Vec<u8>, String> {
    let mut images = Vec::with_capacity(pages.len());
    for p in pages {
        let p = p.cropped();
        let (data, filter) = if p.text {
            (deflate(&p.pixels)?, Filter::Flate)
        } else {
            (encode_jpeg(&p, quality)?, Filter::Dct)
        };
        let (pw, ph) = p.size_points();
        images.push(PdfImage {
            width: p.width,
            height: p.height,
            color: p.is_color(),
            data,
            filter,
            page_width: pw,
            page_height: ph,
        });
    }
    Ok(pdf::build(&images, "Helix Document Scanner", Some(&pdf_date_now())))
}

/// Saves `pages` to `path` in `format`; returns the files written.
pub fn save(pages: &[Page], path: &Path, format: Format, quality: u8) -> Result<Vec<PathBuf>, String> {
    if pages.is_empty() {
        return Err("The document has no pages".into());
    }
    let paths = output_paths(path, pages.len(), format);
    let write = |p: &Path, data: &[u8]| {
        std::fs::write(p, data).map_err(|e| format!("Could not write {}: {e}", p.display()))
    };
    match format {
        Format::Pdf => write(path, &encode_pdf(pages, quality)?)?,
        Format::Png | Format::Jpeg => {
            for (page, out) in pages.iter().zip(&paths) {
                let page = page.cropped();
                let data =
                    if format == Format::Png { encode_png(&page)? } else { encode_jpeg(&page, quality)? };
                write(out, &data)?;
            }
        }
    }
    Ok(paths)
}

/// Current UTC time as a PDF date body (`YYYYMMDDHHmmSSZ`).
pub fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    pdf_date(secs)
}

pub fn pdf_date(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let rem = unix.rem_euclid(86400);
    // Civil-from-days (proleptic Gregorian).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Rect;

    fn page(color: bool) -> Page {
        let c = if color { 3 } else { 1 };
        Page::new(20, 10, c, (0..200 * c as u32).map(|v| v as u8).collect(), 150.0)
    }

    #[test]
    fn formats() {
        assert_eq!(Format::from_path(Path::new("a/b.JPEG")), Some(Format::Jpeg));
        assert_eq!(Format::from_path(Path::new("a/b")), None);
        assert_eq!(Format::from_mime("image/png"), Some(Format::Png));
        let (p, f) = resolve_path(Path::new("/x/Scan"), Format::Pdf);
        assert_eq!((p, f), (PathBuf::from("/x/Scan.pdf"), Format::Pdf));
        let (p, f) = resolve_path(Path::new("/x/Scan.png"), Format::Pdf);
        assert_eq!((p, f), (PathBuf::from("/x/Scan.png"), Format::Png));
    }

    #[test]
    fn multi_page_names() {
        let p = Path::new("/d/Scanned Document.jpg");
        assert_eq!(output_paths(p, 1, Format::Jpeg), vec![p.to_path_buf()]);
        assert_eq!(
            output_paths(p, 2, Format::Jpeg),
            vec![PathBuf::from("/d/Scanned Document-1.jpg"), PathBuf::from("/d/Scanned Document-2.jpg")]
        );
        assert_eq!(output_paths(Path::new("/d/x.pdf"), 3, Format::Pdf).len(), 1);
    }

    #[test]
    fn dates() {
        assert_eq!(pdf_date(0), "19700101000000Z");
        assert_eq!(pdf_date(951782400 + 3661), "20000229010101Z");
        assert_eq!(pdf_date(1704067199), "20231231235959Z");
    }

    #[test]
    fn encodes_images() {
        for color in [false, true] {
            let mut p = page(color);
            p.crop = Some(Rect::new(2, 2, 5, 4));
            let png = encode_png(&p.cropped()).unwrap();
            let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
            assert_eq!((img.width(), img.height()), (5, 4));
            let jpg = encode_jpeg(&p, 80).unwrap();
            let img = image::load_from_memory_with_format(&jpg, image::ImageFormat::Jpeg).unwrap();
            assert_eq!((img.width(), img.height()), (20, 10));
        }
    }

    #[test]
    fn saves_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = page(false);
        text.text = true;
        let pages = vec![page(true), text];
        let pdf_path = dir.path().join("doc.pdf");
        let written = save(&pages, &pdf_path, Format::Pdf, 75).unwrap();
        assert_eq!(written, vec![pdf_path.clone()]);
        let data = std::fs::read(&pdf_path).unwrap();
        assert!(data.starts_with(b"%PDF-"));
        let s: String = data.iter().map(|&b| if b.is_ascii() { b as char } else { '?' }).collect();
        assert!(s.contains("/Count 2"));
        assert!(s.contains("/DCTDecode") && s.contains("/FlateDecode"));
        // 20 px at 150 dpi = 9.6 pt.
        assert!(s.contains("/MediaBox [0 0 9.6 4.8]"));

        let written = save(&pages, &dir.path().join("p.png"), Format::Png, 75).unwrap();
        assert_eq!(written.len(), 2);
        assert!(written.iter().all(|p| p.exists()));
        assert!(save(&[], &pdf_path, Format::Pdf, 75).is_err());
    }
}
