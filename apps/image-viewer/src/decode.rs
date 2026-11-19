//! Image decoding (runs on a worker thread; no GTK widgets here).
//!
//! gdk-pixbuf is the primary decoder. The pure-Rust `image` crate (GIF, WebP
//! and QOI only) decodes animations with exact frame delays and covers
//! WebP/QOI when the pixbuf loaders are missing.

use std::io::BufReader;
use std::path::Path;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use image::{AnimationDecoder, ImageDecoder};

use crate::folder::Extensions;
use crate::metadata::{self, ExifInfo};
use crate::orientation::{self, Orientation};
use helix_app_common::i18n::tr;

/// Decoded animations larger than this only keep their first frame.
const MAX_ANIMATION_BYTES: usize = 512 * 1024 * 1024;
/// Animations with more frames only keep their first frame.
const MAX_FRAMES: usize = 10_000;
/// Largest still WebP canvas: 16384², the most a VP8/VP8L bitstream can
/// hold (1 GiB as RGBA). Only an extended (VP8X) header can declare more.
const MAX_STILL_PIXELS: u64 = 16384 * 16384;
/// Largest GIF or animated WebP canvas: 64 Mpx (8192², 256 MiB as RGBA).
/// The `image` crate composites every frame onto a full canvas and keeps
/// up to three canvas-sized buffers while doing so; GIF headers can
/// declare 65535² (17 GB) for a file of a few bytes.
const MAX_ANIMATION_PIXELS: u64 = 8192 * 8192;
/// Allocation limit handed to the GIF decoder: the three canvas buffers
/// plus one frame (the `gif` crate caps frames at 50 MB itself).
const MAX_GIF_ALLOC: u64 = 3 * 4 * MAX_ANIMATION_PIXELS + 64 * 1024 * 1024;
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
        "webp" => tr("WebP").into(),
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

/// Why the `image` crate decoder gave up.
#[derive(Debug)]
enum RustError {
    /// Over the size limits. Not retried with gdk-pixbuf, which would meet
    /// the same decompression bomb without limits.
    TooLarge,
    Failed(String),
}

impl RustError {
    fn message(&self) -> String {
        match self {
            RustError::TooLarge => tr("The image is too large to open").into(),
            RustError::Failed(e) => e.clone(),
        }
    }
}

impl From<image::ImageError> for RustError {
    fn from(e: image::ImageError) -> Self {
        match e {
            image::ImageError::Limits(_) => RustError::TooLarge,
            e => RustError::Failed(e.to_string()),
        }
    }
}

impl From<std::io::Error> for RustError {
    fn from(e: std::io::Error) -> Self {
        RustError::Failed(e.to_string())
    }
}

/// Checks a declared canvas against the limits; its RGBA size in bytes.
fn canvas_bytes(width: u32, height: u32, max_pixels: u64) -> Result<usize, RustError> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels > max_pixels {
        return Err(RustError::TooLarge);
    }
    Ok(pixels as usize * 4)
}

/// How many decoded frames an animation may keep.
#[derive(Debug, Clone, Copy)]
struct FrameBudget {
    bytes: usize,
    frames: usize,
}

const FRAME_BUDGET: FrameBudget = FrameBudget {
    bytes: MAX_ANIMATION_BYTES,
    frames: MAX_FRAMES,
};

/// Collects animation frames, each `frame_bytes` large (the crate always
/// yields full-canvas RGBA frames). Animations over `budget` keep only
/// their first frame; at most one frame past the budget is decoded to
/// find that out.
fn collect_frames(
    frames: image::Frames<'_>,
    frame_bytes: usize,
    budget: FrameBudget,
) -> Result<Vec<Frame>, RustError> {
    let mut out = Vec::new();
    for f in frames {
        let f = f?;
        if !out.is_empty()
            && (out.len() >= budget.frames
                || (out.len() + 1).saturating_mul(frame_bytes) > budget.bytes)
        {
            out.truncate(1);
            break;
        }
        let delay = delay_ms(f.delay());
        out.push(rgba_frame(f.into_buffer(), delay));
    }
    if out.is_empty() {
        return Err(RustError::Failed(tr("No frames").into()));
    }
    Ok(out)
}

fn gif_decoder(
    reader: BufReader<std::fs::File>,
) -> Result<
    (
        image::codecs::gif::GifDecoder<BufReader<std::fs::File>>,
        usize,
    ),
    RustError,
> {
    // Reads the header only; the canvas is allocated by the frame iterator.
    let mut dec = image::codecs::gif::GifDecoder::new(reader)?;
    let (w, h) = dec.dimensions();
    let bytes = canvas_bytes(w, h, MAX_ANIMATION_PIXELS)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_GIF_ALLOC);
    dec.set_limits(limits)?;
    Ok((dec, bytes))
}

fn webp_decoder(
    reader: BufReader<std::fs::File>,
) -> Result<
    (
        image::codecs::webp::WebPDecoder<BufReader<std::fs::File>>,
        usize,
    ),
    RustError,
> {
    // `image` ignores `Limits` for WebP (only the dimensions are checked,
    // and none are set by default): check the declared canvas here.
    let dec = image::codecs::webp::WebPDecoder::new(reader)?;
    let (w, h) = dec.dimensions();
    let max = if dec.has_animation() {
        MAX_ANIMATION_PIXELS
    } else {
        MAX_STILL_PIXELS
    };
    let bytes = canvas_bytes(w, h, max)?;
    Ok((dec, bytes))
}

/// Whether `path` is a GIF or WebP whose header declares a canvas over the
/// limits. Callers that would hand the file to gdk-pixbuf check this first.
pub fn too_large(path: &Path) -> bool {
    let Some(kind @ ("gif" | "webp")) = sniff(path) else {
        return false;
    };
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let reader = BufReader::new(file);
    let r = match kind {
        "gif" => gif_decoder(reader).map(|_| ()),
        _ => webp_decoder(reader).map(|_| ()),
    };
    matches!(r, Err(RustError::TooLarge))
}

/// Decodes with the `image` crate (GIF and WebP with animation, QOI).
fn decode_rust(path: &Path, kind: &str) -> Result<Vec<Frame>, RustError> {
    decode_rust_with(path, kind, FRAME_BUDGET)
}

fn decode_rust_with(path: &Path, kind: &str, budget: FrameBudget) -> Result<Vec<Frame>, RustError> {
    let reader = BufReader::new(std::fs::File::open(path)?);
    match kind {
        "gif" => {
            let (dec, bytes) = gif_decoder(reader)?;
            collect_frames(dec.into_frames(), bytes, budget)
        }
        "webp" => {
            let (dec, bytes) = webp_decoder(reader)?;
            if dec.has_animation() {
                collect_frames(dec.into_frames(), bytes, budget)
            } else {
                let img = image::DynamicImage::from_decoder(dec)?;
                Ok(vec![rgba_frame(img.into_rgba8(), 0)])
            }
        }
        _ => {
            // `ImageReader` applies the crate's default limits (512 MiB).
            let img = image::ImageReader::new(reader)
                .with_guessed_format()?
                .decode()?;
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
        return Err(tr("Unsupported pixel layout").into());
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
            Err(e @ RustError::TooLarge) => return Err(e.message()),
            Err(e) => match decode_pixbuf(path) {
                Ok((f, name)) => (vec![f], name),
                Err(_) => return Err(e.message()),
            },
        },
        _ => match decode_pixbuf(path) {
            Ok((f, name)) => (vec![f], name),
            Err(e) => match decode_rust(path, kind.unwrap_or("")) {
                Ok(f) => (f, kind.unwrap_or("").to_string()),
                Err(e2 @ RustError::TooLarge) => return Err(e2.message()),
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

    fn write_gif(path: &Path, colors: &[[u8; 4]]) {
        use image::codecs::gif::{GifEncoder, Repeat};
        let file = std::fs::File::create(path).unwrap();
        let mut enc = GifEncoder::new(file);
        enc.set_repeat(Repeat::Infinite).unwrap();
        let frames = colors.iter().map(|c| {
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(4, 4, image::Rgba(*c)),
                0,
                0,
                image::Delay::from_numer_denom_ms(50, 1),
            )
        });
        enc.encode_frames(frames).unwrap();
    }

    fn too_large_msg() -> String {
        RustError::TooLarge.message()
    }

    #[test]
    fn gif_bomb_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bomb.gif");
        write_gif(&path, &[[255, 0, 0, 255]]);
        assert!(!too_large(&path));
        // A 4×4 frame on a declared 65535×65535 logical screen (17 GB as
        // RGBA): refused from the header, and not retried with gdk-pixbuf.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[6..10].copy_from_slice(&[0xff; 4]);
        std::fs::write(&path, &bytes).unwrap();
        assert!(too_large(&path));
        assert_eq!(decode(&path).unwrap_err(), too_large_msg());
        // No thumbnail either (gdk-pixbuf is not tried).
        let cache = dir.path().join("cache");
        assert!(crate::thumbs::load(&path, &cache, 128).is_none());
        // Just over the animation canvas limit.
        bytes[6..10].copy_from_slice(&[0x01, 0x20, 0x00, 0x20]);
        std::fs::write(&path, &bytes).unwrap();
        assert!(too_large(&path));
    }

    #[test]
    fn animation_frame_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anim.gif");
        write_gif(
            &path,
            &[[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]],
        );
        let frame = 4 * 4 * 4;
        let n = |bytes, frames| {
            decode_rust_with(&path, "gif", FrameBudget { bytes, frames })
                .unwrap()
                .len()
        };
        assert_eq!(n(3 * frame, 3), 3);
        // Over the byte or frame budget: only the first frame.
        assert_eq!(n(3 * frame - 1, 3), 1);
        assert_eq!(n(3 * frame, 2), 1);
    }

    /// An extended WebP: VP8X with the given canvas, holding one 1×1
    /// lossless image (as an animation frame when `animated`).
    fn write_webp(path: &Path, width: u32, height: u32, animated: bool) {
        fn chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(id);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
            if data.len() % 2 == 1 {
                out.push(0);
            }
        }
        let u24 = |v: u32| v.to_le_bytes()[..3].to_vec();
        let mut simple = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut simple)
            .encode(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        // "RIFF" size "WEBP", then the VP8L chunk.
        let vp8l = &simple[12..];
        let mut body = Vec::new();
        let flags: u8 = if animated { 0x12 } else { 0x10 };
        let mut vp8x = vec![flags, 0, 0, 0];
        vp8x.extend(u24(width - 1));
        vp8x.extend(u24(height - 1));
        chunk(&mut body, b"VP8X", &vp8x);
        if animated {
            chunk(&mut body, b"ANIM", &[0; 6]);
            let mut anmf = [u24(0), u24(0), u24(0), u24(0), u24(100)].concat();
            anmf.push(0);
            anmf.extend_from_slice(vp8l);
            chunk(&mut body, b"ANMF", &anmf);
        } else {
            body.extend_from_slice(vp8l);
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend(body);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn webp_bomb_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.webp");
        // The construction decodes at a sane size.
        write_webp(&path, 1, 1, true);
        let d = decode(&path).unwrap();
        assert_eq!(d.size(), (1, 1));
        assert!(is_red(pixel(&d.frames[0], 0, 0)));

        // Animated: 60000×60000 (14 GB per frame), and just over the limit.
        for (w, h) in [(60_000, 60_000), (8193, 8192)] {
            write_webp(&path, w, h, true);
            assert!(too_large(&path), "{w}×{h}");
            assert_eq!(decode(&path).unwrap_err(), too_large_msg(), "{w}×{h}");
        }
        // Still: over 16384² is refused before any pixel buffer exists.
        write_webp(&path, 16385, 16384, false);
        assert!(too_large(&path));
        assert_eq!(decode(&path).unwrap_err(), too_large_msg());
        // A large still canvas within the limit is not "too large" (its
        // 1×1 bitstream does not match it, so decoding fails differently).
        write_webp(&path, 16384, 16384, false);
        assert!(!too_large(&path));
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
