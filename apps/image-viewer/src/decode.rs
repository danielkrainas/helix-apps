//! Image decoding (runs on a worker thread; no GTK widgets here).
//!
//! gdk-pixbuf is the primary decoder. The pure-Rust `image` crate (GIF, WebP
//! and QOI only) decodes animations with exact frame delays and covers
//! WebP/QOI when the pixbuf loaders are missing.

use std::io::BufReader;
use std::path::Path;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use image::AnimationDecoder;

use crate::folder::Extensions;
use crate::metadata::{self, ExifInfo};
use crate::orientation::{self, Orientation};

/// Decoded animations larger than this only keep their first frame.
const MAX_ANIMATION_BYTES: usize = 512 * 1024 * 1024;
const MIN_FRAME_DELAY_MS: u32 = 20;
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 3 bytes per pixel.
    Rgb8,
    /// 4 bytes per pixel, straight (non-premultiplied) alpha.
    Rgba8,
}

impl PixelFormat {
    pub fn bpp(self) -> usize {
        match self {
            PixelFormat::Rgb8 => 3,
            PixelFormat::Rgba8 => 4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub bytes: glib::Bytes,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    /// Display time for animation frames.
    pub delay_ms: u32,
}

#[derive(Debug, Clone)]
pub struct Decoded {
    pub frames: Vec<Frame>,
    /// Short format label, e.g. "PNG".
    pub format: String,
    pub exif: Option<ExifInfo>,
}

impl Decoded {
    pub fn size(&self) -> (u32, u32) {
        self.frames
            .first()
            .map(|f| (f.width, f.height))
            .unwrap_or((0, 0))
    }

    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }
}

/// Extensions handled by the `image` crate fallback.
const RUST_EXTENSIONS: [&str; 3] = ["gif", "webp", "qoi"];

/// Extensions of installed pixbuf loaders plus the fallback decoder's.
pub fn supported_extensions() -> Extensions {
    let mut exts: Vec<String> = Pixbuf::formats()
        .iter()
        .filter(|f| !f.is_disabled())
        .flat_map(|f| f.extensions().into_iter().map(|e| e.to_string()))
        .collect();
    exts.extend(RUST_EXTENSIONS.iter().map(|s| s.to_string()));
    Extensions::new(exts)
}

/// MIME types of installed pixbuf loaders plus the fallback decoder's.
pub fn supported_mime_types() -> Vec<String> {
    let mut v: Vec<String> = Pixbuf::formats()
        .iter()
        .filter(|f| !f.is_disabled())
        .flat_map(|f| f.mime_types().into_iter().map(|e| e.to_string()))
        .collect();
    v.extend(["image/gif", "image/webp", "image/qoi"].map(String::from));
    v.sort();
    v.dedup();
    v
}

fn format_label(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "jpeg" => "JPEG".into(),
        "png" => "PNG".into(),
        "gif" => "GIF".into(),
        "webp" => "WebP".into(),
        "tiff" => "TIFF".into(),
        "svg" => "SVG".into(),
        "heif" => "HEIF".into(),
        "avif" => "AVIF".into(),
        "bmp" => "BMP".into(),
        "ico" => "ICO".into(),
        "qoi" => "QOI".into(),
        other => other.to_uppercase(),
    }
}

/// Sniffs the leading bytes for formats the `image` crate handles here.
fn sniff(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 12];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut head).ok()?;
    let head = &head[..n];
    if head.starts_with(b"GIF8") {
        Some("gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else if head.starts_with(b"qoif") {
        Some("qoi")
    } else {
        None
    }
}

fn delay_ms(d: image::Delay) -> u32 {
    let (n, den) = d.numer_denom_ms();
    let ms = n.checked_div(den).unwrap_or(0);
    if ms == 0 {
        DEFAULT_FRAME_DELAY_MS
    } else {
        ms.max(MIN_FRAME_DELAY_MS)
    }
}

fn rgba_frame(img: image::RgbaImage, delay_ms: u32) -> Frame {
    let (width, height) = img.dimensions();
    Frame {
        bytes: glib::Bytes::from_owned(img.into_raw()),
        width,
        height,
        stride: width as usize * 4,
        format: PixelFormat::Rgba8,
        delay_ms,
    }
}

fn collect_frames<'a>(frames: image::Frames<'a>) -> Result<Vec<Frame>, String> {
    let mut out = Vec::new();
    let mut total = 0usize;
    for f in frames {
        let f = f.map_err(|e| e.to_string())?;
        let delay = delay_ms(f.delay());
        let buf = f.into_buffer();
        total += buf.as_raw().len();
        if total > MAX_ANIMATION_BYTES && !out.is_empty() {
            out.truncate(1);
            break;
        }
        out.push(rgba_frame(buf, delay));
    }
    if out.is_empty() {
        return Err("No frames".into());
    }
    Ok(out)
}

/// Decodes with the `image` crate (GIF and WebP with animation, QOI).
fn decode_rust(path: &Path, kind: &str) -> Result<Vec<Frame>, String> {
    let reader = BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    match kind {
        "gif" => {
            let dec = image::codecs::gif::GifDecoder::new(reader).map_err(|e| e.to_string())?;
            collect_frames(dec.into_frames())
        }
        "webp" => {
            let dec = image::codecs::webp::WebPDecoder::new(reader).map_err(|e| e.to_string())?;
            if dec.has_animation() {
                collect_frames(dec.into_frames())
            } else {
                let img = image::DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
                Ok(vec![rgba_frame(img.into_rgba8(), 0)])
            }
        }
        _ => {
            let img = image::ImageReader::new(reader)
                .with_guessed_format()
                .map_err(|e| e.to_string())?
                .decode()
                .map_err(|e| e.to_string())?;
            Ok(vec![rgba_frame(img.into_rgba8(), 0)])
        }
    }
}

fn decode_pixbuf(path: &Path) -> Result<(Frame, String), String> {
    let name = Pixbuf::file_info(path)
        .and_then(|(f, _, _)| f.name())
        .map(|n| n.to_string())
        .unwrap_or_default();
    let pb = Pixbuf::from_file(path).map_err(|e| e.message().to_string())?;
    let format = if pb.has_alpha() {
        PixelFormat::Rgba8
    } else {
        PixelFormat::Rgb8
    };
    if pb.bits_per_sample() != 8 || pb.n_channels() as usize != format.bpp() {
        return Err("Unsupported pixel layout".into());
    }
    let frame = Frame {
        bytes: pb.read_pixel_bytes(),
        width: pb.width() as u32,
        height: pb.height() as u32,
        stride: pb.rowstride() as usize,
        format,
        delay_ms: 0,
    };
    Ok((frame, name))
}

fn orient(frame: Frame, o: Orientation) -> Frame {
    if o.is_normal() {
        return frame;
    }
    let bpp = frame.format.bpp();
    let (data, width, height) = orientation::apply(
        &frame.bytes,
        frame.width,
        frame.height,
        frame.stride,
        bpp,
        o,
    );
    Frame {
        bytes: glib::Bytes::from_owned(data),
        width,
        height,
        stride: width as usize * bpp,
        ..frame
    }
}

/// Decodes `path`, applying its EXIF orientation.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    let exif = metadata::read_exif(path);
    let kind = sniff(path);
    let (frames, format) = match kind {
        // GIF / WebP go through the Rust decoder first for animation support.
        Some(k @ ("gif" | "webp")) => match decode_rust(path, k) {
            Ok(f) => (f, k.to_string()),
            Err(e) => match decode_pixbuf(path) {
                Ok((f, name)) => (vec![f], name),
                Err(_) => return Err(e),
            },
        },
        _ => match decode_pixbuf(path) {
            Ok((f, name)) => (vec![f], name),
            Err(e) => match decode_rust(path, kind.unwrap_or("")) {
                Ok(f) => (f, kind.unwrap_or("").to_string()),
                Err(_) => return Err(e),
            },
        },
    };
    // HEIF/AVIF decoders already apply the container's transforms.
    let o = match format.as_str() {
        "heif" | "avif" => Orientation::NORMAL,
        _ => exif.as_ref().map(|e| e.orientation).unwrap_or_default(),
    };
    let frames = frames.into_iter().map(|f| orient(f, o)).collect();
    Ok(Decoded {
        frames,
        format: format_label(&format),
        exif,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::tests::{orientation_field, write_jpeg};
    use gtk::gdk_pixbuf::Colorspace;

    fn pixel(f: &Frame, x: u32, y: u32) -> &[u8] {
        let i = y as usize * f.stride + x as usize * f.format.bpp();
        &f.bytes[i..i + f.format.bpp()]
    }

    fn is_red(p: &[u8]) -> bool {
        p[0] > 200 && p[1] < 60 && p[2] < 60
    }

    #[test]
    fn decodes_png_with_alpha() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        let pb = Pixbuf::new(Colorspace::Rgb, true, 8, 5, 3).unwrap();
        pb.fill(0x11223344);
        pb.savev(&path, "png", &[]).unwrap();
        let d = decode(&path).unwrap();
        assert_eq!(d.size(), (5, 3));
        assert_eq!(d.format, "PNG");
        assert!(!d.is_animated());
        assert_eq!(d.frames[0].format, PixelFormat::Rgba8);
        assert_eq!(pixel(&d.frames[0], 4, 2), &[0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn jpeg_exif_orientation_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        // Stored 16×8, red on the left half. Orientation 6 → rotate 90° CW:
        // displayed 8×16, red on the top half.
        let path = dir.path().join("rot.jpg");
        write_jpeg(&path, 16, 8, &[orientation_field(6)]);
        let d = decode(&path).unwrap();
        assert_eq!(d.format, "JPEG");
        assert_eq!(d.size(), (8, 16));
        let f = &d.frames[0];
        assert!(is_red(pixel(f, 4, 2)));
        assert!(!is_red(pixel(f, 4, 13)));

        // Orientation 8 → rotate 90° CCW: red ends up at the bottom.
        let path = dir.path().join("rot8.jpg");
        write_jpeg(&path, 16, 8, &[orientation_field(8)]);
        let f = &decode(&path).unwrap().frames[0];
        assert!(!is_red(pixel(f, 4, 2)));
        assert!(is_red(pixel(f, 4, 13)));

        // Orientation 2 → mirror: red on the right.
        let path = dir.path().join("mirror.jpg");
        write_jpeg(&path, 16, 8, &[orientation_field(2)]);
        let d = decode(&path).unwrap();
        assert_eq!(d.size(), (16, 8));
        assert!(is_red(pixel(&d.frames[0], 13, 4)));
    }

    #[test]
    fn animated_gif_frames_and_delays() {
        use image::codecs::gif::{GifEncoder, Repeat};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anim.gif");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = GifEncoder::new(file);
            enc.set_repeat(Repeat::Infinite).unwrap();
            let frames = [([255u8, 0, 0, 255], 70), ([0, 0, 255, 255], 0)].map(|(c, ms)| {
                image::Frame::from_parts(
                    image::RgbaImage::from_pixel(4, 4, image::Rgba(c)),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(ms, 1),
                )
            });
            enc.encode_frames(frames).unwrap();
        }
        let d = decode(&path).unwrap();
        assert_eq!(d.format, "GIF");
        assert!(d.is_animated());
        assert_eq!(d.frames.len(), 2);
        assert_eq!(d.frames[0].delay_ms, 70);
        assert_eq!(d.frames[1].delay_ms, DEFAULT_FRAME_DELAY_MS);
        assert!(is_red(pixel(&d.frames[0], 1, 1)));
    }

    #[test]
    fn garbage_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.png");
        std::fs::write(&path, b"definitely not an image").unwrap();
        assert!(decode(&path).is_err());
        assert!(decode(&dir.path().join("missing.png")).is_err());
    }

    #[test]
    fn extensions_include_common_formats() {
        let e = supported_extensions();
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "qoi"] {
            assert!(e.supports(Path::new(&format!("x.{ext}"))), "{ext}");
        }
        assert!(!e.supports(Path::new("x.txt")));
    }
}
