//! Saving a rotated image back to its file (runs on a worker thread).
//!
//! The file is decoded again with gdk-pixbuf, its EXIF orientation and the
//! view rotation are applied to the pixels, and it is re-encoded in the same
//! format with the pixbuf savers (JPEG, PNG, TIFF, BMP). Metadata is kept
//! where practical:
//! - JPEG: the original EXIF APP1 segment is copied into the new file with
//!   its Orientation tag reset to 1 (the pixels are now upright); the ICC
//!   profile is carried over; quality 95.
//! - PNG: ICC profile and common `tEXt` chunks are carried over.
//! - TIFF: ICC profile is carried over.
//!
//! The new file is written next to the original and renamed over it, keeping
//! the original permissions.

use std::path::Path;

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::glib;

use crate::metadata;
use crate::orientation::{self, Orientation};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

/// Format labels (as shown by the decoder) that can be saved.
pub fn can_save(format_label: &str) -> bool {
    pixbuf_format(format_label).is_some()
}

/// Pixbuf saver name for a decoder format label.
fn pixbuf_format(label: &str) -> Option<&'static str> {
    match label {
        "JPEG" => Some("jpeg"),
        "PNG" => Some("png"),
        "TIFF" => Some("tiff"),
        "BMP" => Some("bmp"),
        _ => None,
    }
}

/// The EXIF orientation equivalent to a clockwise view rotation.
pub fn rotation_orientation(degrees: u32) -> Orientation {
    match degrees % 360 {
        90 => Orientation::from_exif(6),
        180 => Orientation::from_exif(3),
        270 => Orientation::from_exif(8),
        _ => Orientation::NORMAL,
    }
}

/// Rotates a pixel buffer clockwise by `degrees` (multiple of 90). Returns
/// the packed buffer and its size.
pub fn rotate_pixels(
    data: &[u8],
    w: u32,
    h: u32,
    stride: usize,
    bpp: usize,
    degrees: u32,
) -> (Vec<u8>, u32, u32) {
    orientation::apply(data, w, h, stride, bpp, rotation_orientation(degrees))
}

/// Applies the stored EXIF orientation and then the view rotation, i.e.
/// produces exactly what the viewer displays.
pub fn transform(
    data: &[u8],
    w: u32,
    h: u32,
    stride: usize,
    bpp: usize,
    exif: Orientation,
    degrees: u32,
) -> (Vec<u8>, u32, u32) {
    let (upright, uw, uh) = orientation::apply(data, w, h, stride, bpp, exif);
    rotate_pixels(&upright, uw, uh, uw as usize * bpp, bpp, degrees)
}

// ---- JPEG EXIF segment handling ------------------------------------------

/// Iterates the marker segments before the scan: `(marker, start, end)` where
/// `start..end` covers the whole segment including `FF xx` and the length.
fn jpeg_segments(jpeg: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return out;
    }
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
        let marker = jpeg[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        let end = i + 2 + len;
        if len < 2 || end > jpeg.len() {
            break;
        }
        out.push((marker, i, end));
        i = end;
    }
    out
}

/// The complete APP1 `Exif\0\0` segment of a JPEG, if any.
pub fn exif_segment(jpeg: &[u8]) -> Option<Vec<u8>> {
    jpeg_segments(jpeg)
        .into_iter()
        .find(|&(m, s, e)| m == 0xE1 && e - s > 10 && &jpeg[s + 4..s + 10] == b"Exif\0\0")
        .map(|(_, s, e)| jpeg[s..e].to_vec())
}

/// Sets the IFD0 Orientation tag of an APP1 EXIF segment to 1 (in place).
/// Returns whether a tag was found and changed.
pub fn reset_orientation(app1: &mut [u8]) -> bool {
    const TIFF: usize = 10; // FF E1 len(2) "Exif\0\0"
    let Some(tiff) = app1.get(TIFF..) else {
        return false;
    };
    if tiff.len() < 8 {
        return false;
    }
    let le = match &tiff[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return false,
    };
    let u16_at = |b: &[u8], i: usize| -> Option<u16> {
        let v = [*b.get(i)?, *b.get(i + 1)?];
        Some(if le {
            u16::from_le_bytes(v)
        } else {
            u16::from_be_bytes(v)
        })
    };
    let u32_at = |b: &[u8], i: usize| -> Option<u32> {
        let v = [*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?];
        Some(if le {
            u32::from_le_bytes(v)
        } else {
            u32::from_be_bytes(v)
        })
    };
    let Some(ifd) = u32_at(tiff, 4).map(|v| v as usize) else {
        return false;
    };
    let Some(count) = u16_at(tiff, ifd) else {
        return false;
    };
    for n in 0..count as usize {
        let entry = ifd + 2 + n * 12;
        let (Some(tag), Some(ty)) = (u16_at(tiff, entry), u16_at(tiff, entry + 2)) else {
            return false;
        };
        // SHORT (3), inline value.
        if tag == 0x0112 && ty == 3 {
            let pos = TIFF + entry + 8;
            if pos + 2 > app1.len() {
                return false;
            }
            let one = if le {
                1u16.to_le_bytes()
            } else {
                1u16.to_be_bytes()
            };
            app1[pos..pos + 2].copy_from_slice(&one);
            return true;
        }
    }
    false
}

/// Inserts `segment` after SOI (and after a leading JFIF APP0, which must
/// come first), dropping any existing EXIF APP1 segment.
pub fn insert_segment(jpeg: &[u8], segment: &[u8]) -> Vec<u8> {
    let segs = jpeg_segments(jpeg);
    let mut at = 2;
    if let Some(&(0xE0, _, end)) = segs.first() {
        at = end;
    }
    let mut out = Vec::with_capacity(jpeg.len() + segment.len());
    out.extend_from_slice(&jpeg[..at]);
    out.extend_from_slice(segment);
    let mut pos = at;
    for &(m, s, e) in segs.iter().filter(|(_, s, _)| *s >= at) {
        if m == 0xE1 && e - s > 10 && &jpeg[s + 4..s + 10] == b"Exif\0\0" {
            out.extend_from_slice(&jpeg[pos..s]);
            pos = e;
        }
    }
    out.extend_from_slice(&jpeg[pos..]);
    out
}

// ---- Saving ----------------------------------------------------------------

/// PNG text chunks carried over when present.
const PNG_TEXT_KEYS: &[&str] = &[
    "tEXt::Title",
    "tEXt::Author",
    "tEXt::Description",
    "tEXt::Copyright",
    "tEXt::Creation Time",
    "tEXt::Software",
    "tEXt::Comment",
    "tEXt::Source",
];

/// Re-encodes `path` rotated clockwise by `degrees` (90/180/270).
pub fn save_rotated(path: &Path, degrees: u32) -> Result<(), String> {
    if !degrees.is_multiple_of(90) || degrees.is_multiple_of(360) {
        return Ok(());
    }
    let name = Pixbuf::file_info(path)
        .and_then(|(f, _, _)| f.name())
        .map(|n| n.to_string())
        .ok_or_else(|| "Unknown image format".to_string())?;
    let label = match name.as_str() {
        "jpeg" => "JPEG",
        "png" => "PNG",
        "tiff" => "TIFF",
        "bmp" => "BMP",
        other => {
            return Err(trf!(
                "Saving {} images is not supported",
                other.to_uppercase()
            ))
        }
    };
    let saver = pixbuf_format(label).expect("savable");
    let original = std::fs::read(path).map_err(|e| e.to_string())?;
    let pb = Pixbuf::from_file(path).map_err(|e| e.message().to_string())?;
    let bpp = pb.n_channels() as usize;
    if pb.bits_per_sample() != 8 || bpp != if pb.has_alpha() { 4 } else { 3 } {
        return Err(tr("Unsupported pixel layout").into());
    }
    let exif_o = metadata::read_exif(path)
        .map(|e| e.orientation)
        .unwrap_or_default();
    let (pixels, w, h) = transform(
        &pb.read_pixel_bytes(),
        pb.width() as u32,
        pb.height() as u32,
        pb.rowstride() as usize,
        bpp,
        exif_o,
        degrees,
    );
    let out = Pixbuf::from_bytes(
        &glib::Bytes::from_owned(pixels),
        Colorspace::Rgb,
        pb.has_alpha(),
        8,
        w as i32,
        h as i32,
        (w as usize * bpp) as i32,
    );

    let icc = pb.option("icc-profile").map(|s| s.to_string());
    let mut opts: Vec<(String, String)> = Vec::new();
    match saver {
        "jpeg" => opts.push(("quality".into(), "95".into())),
        "png" => {
            for key in PNG_TEXT_KEYS {
                if let Some(v) = pb.option(key) {
                    opts.push((key.to_string(), v.to_string()));
                }
            }
        }
        _ => {}
    }
    if let (Some(icc), "jpeg" | "png" | "tiff") = (icc, saver) {
        opts.push(("icc-profile".into(), icc));
    }
    let opt_refs: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut bytes = out
        .save_to_bufferv(saver, &opt_refs)
        .map_err(|e| e.message().to_string())?;
    if saver == "jpeg" {
        if let Some(mut seg) = exif_segment(&original) {
            reset_orientation(&mut seg);
            bytes = insert_segment(&bytes, &seg);
        }
    }
    write_replacing(path, &bytes)
}

/// Writes `bytes` to a temporary file in the same folder and renames it over
/// `path`, keeping the original permissions.
fn write_replacing(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or(tr("No parent folder"))?;
    let file_name = path
        .file_name()
        .ok_or(tr("No file name"))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{file_name}.helix-save-{}", std::process::id()));
    let result = (|| {
        std::fs::write(&tmp, bytes)?;
        if let Ok(md) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, md.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;
    use crate::metadata::tests::{exif_block, field, orientation_field, write_jpeg};
    use exif::{Tag, Value};

    /// 3×2 single-byte image:
    /// ```text
    /// 1 2 3
    /// 4 5 6
    /// ```
    const IMG: [u8; 6] = [1, 2, 3, 4, 5, 6];

    #[test]
    fn rotation_transform() {
        assert_eq!(rotate_pixels(&IMG, 3, 2, 3, 1, 0), (IMG.to_vec(), 3, 2));
        assert_eq!(rotate_pixels(&IMG, 3, 2, 3, 1, 360), (IMG.to_vec(), 3, 2));
        // Clockwise: left column (4, 1 bottom-up) becomes the top row.
        assert_eq!(
            rotate_pixels(&IMG, 3, 2, 3, 1, 90),
            (vec![4, 1, 5, 2, 6, 3], 2, 3)
        );
        assert_eq!(
            rotate_pixels(&IMG, 3, 2, 3, 1, 180),
            (vec![6, 5, 4, 3, 2, 1], 3, 2)
        );
        assert_eq!(
            rotate_pixels(&IMG, 3, 2, 3, 1, 270),
            (vec![3, 6, 2, 5, 1, 4], 2, 3)
        );
        // Four quarter turns are the identity.
        let (mut d, mut w, mut h) = (IMG.to_vec(), 3, 2);
        for _ in 0..4 {
            let r = rotate_pixels(&d, w, h, w as usize, 1, 90);
            (d, w, h) = r;
        }
        assert_eq!((d, w, h), (IMG.to_vec(), 3, 2));
    }

    #[test]
    fn transform_applies_exif_then_rotation() {
        // EXIF 6 (90° CW) + view 270° CW = identity.
        let (d, w, h) = transform(&IMG, 3, 2, 3, 1, Orientation::from_exif(6), 270);
        assert_eq!((d, w, h), (IMG.to_vec(), 3, 2));
        // EXIF 2 (mirror) + 180° = vertical flip.
        let (d, _, _) = transform(&IMG, 3, 2, 3, 1, Orientation::from_exif(2), 180);
        assert_eq!(d, vec![4, 5, 6, 1, 2, 3]);
        // Stride padding and RGB pixels.
        let data = [1, 1, 1, 2, 2, 2, 0, 0];
        let (d, w, h) = transform(&data, 2, 1, 8, 3, Orientation::NORMAL, 90);
        assert_eq!((d, w, h), (vec![1, 1, 1, 2, 2, 2], 1, 2));
    }

    fn app1(tiff: &[u8]) -> Vec<u8> {
        let mut seg = vec![0xFF, 0xE1];
        seg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        seg.extend_from_slice(b"Exif\0\0");
        seg.extend_from_slice(tiff);
        seg
    }

    #[test]
    fn orientation_reset_both_byte_orders() {
        // Big-endian block from the exif writer.
        let mut seg = app1(&exif_block(&[orientation_field(8)]));
        assert!(reset_orientation(&mut seg));
        let tiff = seg[10..].to_vec();
        let exif = exif::Reader::new().read_raw(tiff).unwrap();
        let o = exif.get_field(Tag::Orientation, exif::In::PRIMARY).unwrap();
        assert_eq!(o.value.get_uint(0), Some(1));

        // Hand-made little-endian block: one entry, Orientation = 6.
        let mut tiff = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        let mut seg = app1(&tiff);
        assert!(reset_orientation(&mut seg));
        assert_eq!(&seg[10 + 18..10 + 20], &[1, 0]);

        // No orientation tag.
        let mut seg = app1(&exif_block(&[field(
            Tag::Make,
            Value::Ascii(vec![b"X".to_vec()]),
        )]));
        assert!(!reset_orientation(&mut seg));
        assert!(!reset_orientation(&mut [0xFF, 0xE1]));
    }

    #[test]
    fn segment_insertion() {
        let jfif = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0xAA, 0xBB, 0xFF, 0xDA, 0x00, 0x02, 0x11, 0xFF,
            0xD9,
        ];
        let seg = app1(b"MM\x00\x2a\x00\x00\x00\x08\x00\x00");
        let out = insert_segment(&jfif, &seg);
        assert_eq!(&out[..8], &jfif[..8]);
        assert_eq!(&out[8..8 + seg.len()], &seg[..]);
        assert_eq!(&out[8 + seg.len()..], &jfif[8..]);
        assert_eq!(exif_segment(&out), Some(seg.clone()));
        // An existing EXIF segment is replaced, not duplicated.
        let again = insert_segment(&out, &seg);
        assert_eq!(again, out);
        // Not a JPEG.
        assert_eq!(exif_segment(b"PNG"), None);
    }

    fn is_red(p: &[u8]) -> bool {
        p[0] > 200 && p[1] < 60 && p[2] < 60
    }

    fn pixel(f: &decode::Frame, x: u32, y: u32) -> Vec<u8> {
        let i = y as usize * f.stride + x as usize * f.format.bpp();
        f.bytes[i..i + f.format.bpp()].to_vec()
    }

    #[test]
    fn saves_rotated_jpeg_keeping_exif() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        // Stored 16×8 red-left with EXIF 6 → displayed 8×16 red-top.
        write_jpeg(
            &path,
            16,
            8,
            &[
                field(Tag::Make, Value::Ascii(vec![b"Helix".to_vec()])),
                orientation_field(6),
            ],
        );
        // Rotating the display 90° CW puts red on the right: 16×8.
        save_rotated(&path, 90).unwrap();
        let d = decode::decode(&path).unwrap();
        assert_eq!(d.format, "JPEG");
        assert_eq!(d.size(), (16, 8));
        assert!(is_red(&pixel(&d.frames[0], 13, 4)));
        assert!(!is_red(&pixel(&d.frames[0], 2, 4)));
        let raw = std::fs::read(&path).unwrap();
        let exif = exif::Reader::new()
            .read_from_container(&mut std::io::Cursor::new(&raw))
            .unwrap();
        assert_eq!(
            exif.get_field(Tag::Orientation, exif::In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0),
            Some(1)
        );
        let make = exif.get_field(Tag::Make, exif::In::PRIMARY).unwrap();
        assert_eq!(make.display_value().to_string(), "\"Helix\"");
        // No leftover temporary file.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn saves_rotated_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        let pb = Pixbuf::new(Colorspace::Rgb, true, 8, 4, 2).unwrap();
        pb.fill(0x0000_ffff);
        pb.new_subpixbuf(0, 0, 1, 1).fill(0xff00_00ff); // red top-left
        pb.savev(&path, "png", &[("tEXt::Comment", "kept")])
            .unwrap();
        save_rotated(&path, 270).unwrap();
        // 270° CW: top-left goes to bottom-left, size 2×4.
        let back = Pixbuf::from_file(&path).unwrap();
        assert_eq!((back.width(), back.height()), (2, 4));
        let d = decode::decode(&path).unwrap();
        assert!(is_red(&pixel(&d.frames[0], 0, 3)));
        assert!(!is_red(&pixel(&d.frames[0], 0, 0)));
        assert_eq!(back.option("tEXt::Comment").as_deref(), Some("kept"));
    }

    #[test]
    fn unsupported_and_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.gif");
        std::fs::write(&path, b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").unwrap();
        assert!(save_rotated(&path, 90).is_err());
        // Zero rotation leaves the file alone.
        assert!(save_rotated(&path, 0).is_ok());
        assert!(can_save("JPEG") && can_save("PNG") && !can_save("GIF") && !can_save("WebP"));
    }
}
