//! Thumbnails for the bottom strip (worker-thread side; no widgets).
//!
//! Existing freedesktop thumbnails (`$XDG_CACHE_HOME/thumbnails/{normal,large}/
//! <md5(uri)>.png`, valid when their `Thumb::MTime` matches the file) are
//! reused; otherwise the image is decoded at thumbnail size. The cache is
//! only read, never written.

use std::path::{Path, PathBuf};

use gtk::gdk_pixbuf::{InterpType, Pixbuf};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::decode::{self, PixelFormat};

/// Longest side of strip thumbnails, in pixels.
pub const SIZE: i32 = 128;

/// Decoded thumbnail pixels (Send, unlike `Pixbuf`).
#[derive(Debug, Clone)]
pub struct RawThumb {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub has_alpha: bool,
    pub bytes: glib::Bytes,
}

/// The freedesktop thumbnail path for `uri` in `flavour` (`normal`, `large`).
pub fn cache_path(cache_root: &Path, uri: &str, flavour: &str) -> PathBuf {
    let md5 = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)
        .map(|s| s.to_string())
        .unwrap_or_default();
    cache_root
        .join("thumbnails")
        .join(flavour)
        .join(format!("{md5}.png"))
}

/// Size that fits `w`×`h` into a `max`×`max` box (never upscaled).
pub fn fit(w: i32, h: i32, max: i32) -> (i32, i32) {
    if w <= max && h <= max {
        return (w.max(1), h.max(1));
    }
    if w >= h {
        (max, ((h as i64 * max as i64) / w as i64).max(1) as i32)
    } else {
        (((w as i64 * max as i64) / h as i64).max(1) as i32, max)
    }
}

fn mtime(path: &Path) -> Option<u64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    m.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn raw(pb: &Pixbuf) -> Option<RawThumb> {
    let bpp = if pb.has_alpha() { 4 } else { 3 };
    if pb.bits_per_sample() != 8 || pb.n_channels() != bpp {
        return None;
    }
    Some(RawThumb {
        width: pb.width(),
        height: pb.height(),
        stride: pb.rowstride() as usize,
        has_alpha: pb.has_alpha(),
        bytes: pb.read_pixel_bytes(),
    })
}

fn scaled(pb: Pixbuf, size: i32) -> Option<Pixbuf> {
    let (w, h) = fit(pb.width(), pb.height(), size);
    if (w, h) == (pb.width(), pb.height()) {
        Some(pb)
    } else {
        pb.scale_simple(w, h, InterpType::Bilinear)
    }
}

/// A valid cached thumbnail for `path`, if one exists.
fn from_cache(path: &Path, cache_root: &Path, size: i32) -> Option<RawThumb> {
    let uri = gio::File::for_path(path).uri();
    let want = mtime(path)?.to_string();
    for flavour in ["normal", "large"] {
        let p = cache_path(cache_root, &uri, flavour);
        let Ok(pb) = Pixbuf::from_file(&p) else {
            continue;
        };
        if pb.option("tEXt::Thumb::MTime").as_deref() == Some(want.as_str()) {
            return scaled(pb, size).as_ref().and_then(raw);
        }
    }
    None
}

/// Decodes `path` at thumbnail size (with EXIF orientation applied).
fn decode_small(path: &Path, size: i32) -> Option<RawThumb> {
    if let Ok(pb) = Pixbuf::from_file_at_scale(path, size, size, true) {
        let pb = pb.apply_embedded_orientation().unwrap_or(pb);
        return raw(&pb);
    }
    // Formats only the Rust decoder handles (WebP/QOI without loaders…).
    let d = decode::decode(path).ok()?;
    let f = d.frames.into_iter().next()?;
    let pb = Pixbuf::from_bytes(
        &f.bytes,
        gtk::gdk_pixbuf::Colorspace::Rgb,
        f.format == PixelFormat::Rgba8,
        8,
        f.width as i32,
        f.height as i32,
        f.stride as i32,
    );
    scaled(pb, size).as_ref().and_then(raw)
}

/// Loads a thumbnail for `path`: cache first, then decoding.
pub fn load(path: &Path, cache_root: &Path, size: i32) -> Option<RawThumb> {
    from_cache(path, cache_root, size).or_else(|| decode_small(path, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::gdk_pixbuf::Colorspace;

    #[test]
    fn spec_example_cache_path() {
        // Example from the freedesktop thumbnail specification.
        let p = cache_path(Path::new("/c"), "file:///home/jens/photos/me.png", "normal");
        assert_eq!(
            p,
            Path::new("/c/thumbnails/normal/c6ee772d9e49320e97ec29a7eb5b1697.png")
        );
    }

    #[test]
    fn fitting() {
        assert_eq!(fit(1000, 500, 128), (128, 64));
        assert_eq!(fit(500, 1000, 128), (64, 128));
        assert_eq!(fit(100, 50, 128), (100, 50));
        assert_eq!(fit(4000, 1, 128), (128, 1));
    }

    #[test]
    fn decodes_and_uses_valid_cache_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let img = dir.path().join("big.png");
        let pb = Pixbuf::new(Colorspace::Rgb, false, 8, 400, 200).unwrap();
        pb.fill(0xff00_00ff);
        pb.savev(&img, "png", &[]).unwrap();

        let t = load(&img, &cache, SIZE).unwrap();
        assert_eq!((t.width, t.height), (128, 64));

        // A cached thumbnail with the right mtime is preferred.
        let uri = gio::File::for_path(&img).uri();
        let thumb = cache_path(&cache, &uri, "normal");
        std::fs::create_dir_all(thumb.parent().unwrap()).unwrap();
        let small = Pixbuf::new(Colorspace::Rgb, false, 8, 10, 7).unwrap();
        small.fill(0x00ff_00ff);
        let m = mtime(&img).unwrap().to_string();
        small
            .savev(&thumb, "png", &[("tEXt::Thumb::MTime", m.as_str())])
            .unwrap();
        let t = load(&img, &cache, SIZE).unwrap();
        assert_eq!((t.width, t.height), (10, 7));

        // A stale one is ignored.
        small
            .savev(&thumb, "png", &[("tEXt::Thumb::MTime", "1")])
            .unwrap();
        let t = load(&img, &cache, SIZE).unwrap();
        assert_eq!((t.width, t.height), (128, 64));

        assert!(load(&dir.path().join("missing.png"), &cache, SIZE).is_none());
    }
}
