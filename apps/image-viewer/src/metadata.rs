//! File and EXIF metadata for the properties panel.

use std::path::Path;

use exif::{In, Tag, Value};

use crate::orientation::Orientation;

/// Human-readable EXIF fields (all optional).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifInfo {
    pub orientation: Orientation,
    pub date_taken: Option<String>,
    pub camera: Option<String>,
    pub exposure: Option<String>,
    pub aperture: Option<String>,
    pub iso: Option<String>,
    pub focal_length: Option<String>,
    /// Decimal degrees (latitude, longitude).
    pub gps: Option<(f64, f64)>,
}

impl ExifInfo {
    /// Whether there is anything worth showing besides orientation.
    pub fn has_details(&self) -> bool {
        self.date_taken.is_some()
            || self.camera.is_some()
            || self.exposure.is_some()
            || self.aperture.is_some()
            || self.iso.is_some()
            || self.focal_length.is_some()
            || self.gps.is_some()
    }
}

/// Reads EXIF from any container kamadak-exif understands (JPEG, TIFF, PNG,
/// WebP, HEIF). Returns `None` when there is no EXIF block.
pub fn read_exif(path: &Path) -> Option<ExifInfo> {
    let file = std::fs::File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    Some(from_exif(&exif))
}

fn ascii(exif: &exif::Exif, tag: Tag) -> Option<String> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Ascii(v) => {
            let s = v
                .iter()
                .map(|b| String::from_utf8_lossy(b))
                .collect::<Vec<_>>()
                .join(" ");
            let s = s
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    }
}

fn rational(exif: &exif::Exif, tag: Tag) -> Option<(u32, u32)> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Rational(v) => v.first().map(|r| (r.num, r.denom)).filter(|(_, d)| *d != 0),
        Value::SRational(v) => v
            .first()
            .filter(|r| r.num >= 0 && r.denom > 0)
            .map(|r| (r.num as u32, r.denom as u32)),
        _ => None,
    }
}

fn gps_coord(exif: &exif::Exif, tag: Tag, ref_tag: Tag, negative: &str) -> Option<f64> {
    let Value::Rational(v) = &exif.get_field(tag, In::PRIMARY)?.value else {
        return None;
    };
    if v.len() < 3 || v.iter().any(|r| r.denom == 0) {
        return None;
    }
    let deg = v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0;
    let sign = match ascii(exif, ref_tag) {
        Some(r) if r.eq_ignore_ascii_case(negative) => -1.0,
        _ => 1.0,
    };
    Some(sign * deg)
}

pub fn from_exif(exif: &exif::Exif) -> ExifInfo {
    let orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map(Orientation::from_exif)
        .unwrap_or_default();
    let iso = exif
        .get_field(Tag::PhotographicSensitivity, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .filter(|v| *v > 0)
        .map(|v| v.to_string());
    let gps = gps_coord(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef, "S").zip(gps_coord(
        exif,
        Tag::GPSLongitude,
        Tag::GPSLongitudeRef,
        "W",
    ));
    ExifInfo {
        orientation,
        date_taken: ascii(exif, Tag::DateTimeOriginal)
            .or_else(|| ascii(exif, Tag::DateTime))
            .map(|d| format_exif_date(&d)),
        camera: camera_name(
            ascii(exif, Tag::Make).as_deref(),
            ascii(exif, Tag::Model).as_deref(),
        ),
        exposure: rational(exif, Tag::ExposureTime).map(|(n, d)| format_exposure(n, d)),
        aperture: rational(exif, Tag::FNumber).map(|(n, d)| format_aperture(n as f64 / d as f64)),
        iso,
        focal_length: rational(exif, Tag::FocalLength)
            .map(|(n, d)| format_focal_length(n as f64 / d as f64)),
        gps,
    }
}

/// "Canon" + "Canon EOS R5" → "Canon EOS R5"; "NIKON CORPORATION" +
/// "NIKON D750" → "NIKON D750"; "Google" + "Pixel 7" → "Google Pixel 7".
pub fn camera_name(make: Option<&str>, model: Option<&str>) -> Option<String> {
    match (
        make.map(str::trim).filter(|s| !s.is_empty()),
        model.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (None, None) => None,
        (Some(m), None) => Some(m.to_string()),
        (None, Some(m)) => Some(m.to_string()),
        (Some(make), Some(model)) => {
            let first = make
                .split_whitespace()
                .next()
                .unwrap_or(make)
                .to_lowercase();
            if model.to_lowercase().starts_with(&first) {
                Some(model.to_string())
            } else {
                Some(format!("{make} {model}"))
            }
        }
    }
}

fn trim_decimal(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// Exposure time: `1/250 s` below one second, `2 s` / `1.5 s` otherwise.
pub fn format_exposure(num: u32, den: u32) -> String {
    if num == 0 || den == 0 {
        return "0 s".into();
    }
    let secs = num as f64 / den as f64;
    if secs >= 1.0 {
        format!("{} s", trim_decimal(secs))
    } else {
        format!("1/{} s", (den as f64 / num as f64).round() as u64)
    }
}

pub fn format_aperture(f: f64) -> String {
    format!("ƒ/{}", trim_decimal(f))
}

pub fn format_focal_length(mm: f64) -> String {
    format!("{} mm", trim_decimal(mm))
}

/// `2024:01:10 12:34:56` → `2024-01-10 12:34:56`.
pub fn format_exif_date(s: &str) -> String {
    let s = s.trim();
    match s.split_once(' ') {
        Some((date, time)) if date.len() == 10 => format!("{} {}", date.replace(':', "-"), time),
        None if s.len() == 10 => s.replace(':', "-"),
        _ => s.to_string(),
    }
}

pub fn format_gps(lat: f64, lon: f64) -> String {
    format!(
        "{:.5}° {}, {:.5}° {}",
        lat.abs(),
        if lat < 0.0 { 'S' } else { 'N' },
        lon.abs(),
        if lon < 0.0 { 'W' } else { 'E' }
    )
}

/// RFC 5870 `geo:` URI.
pub fn geo_uri(lat: f64, lon: f64) -> String {
    format!("geo:{lat:.6},{lon:.6}")
}

/// `W × H (N MP)`.
pub fn format_dimensions(w: u32, h: u32) -> String {
    let mp = w as f64 * h as f64 / 1_000_000.0;
    if mp >= 0.1 {
        format!("{w} × {h} ({} MP)", trim_decimal(mp))
    } else {
        format!("{w} × {h}")
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use exif::{experimental::Writer, Field, Rational};
    use gtk::gdk_pixbuf::{Colorspace, Pixbuf};

    pub fn field(tag: Tag, value: Value) -> Field {
        Field {
            tag,
            ifd_num: In::PRIMARY,
            value,
        }
    }

    fn r(num: u32, denom: u32) -> Rational {
        Rational { num, denom }
    }

    /// Builds a TIFF/EXIF block from `fields`.
    pub fn exif_block(fields: &[Field]) -> Vec<u8> {
        let mut w = Writer::new();
        for f in fields {
            w.push_field(f);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        w.write(&mut buf, false).unwrap();
        buf.into_inner()
    }

    /// Writes a `w`×`h` JPEG whose pixels are a left/right two-colour split,
    /// with an APP1 EXIF segment built from `fields`.
    pub fn write_jpeg(path: &Path, w: i32, h: i32, fields: &[Field]) {
        let pb = Pixbuf::new(Colorspace::Rgb, false, 8, w, h).unwrap();
        pb.fill(0x0000_00ff);
        // Red left half.
        pb.new_subpixbuf(0, 0, w / 2, h).fill(0xff00_00ff);
        let jpeg = pb.save_to_bufferv("jpeg", &[("quality", "95")]).unwrap();
        let mut out = jpeg[..2].to_vec(); // SOI
        if !fields.is_empty() {
            let tiff = exif_block(fields);
            let len = (tiff.len() + 8) as u16;
            out.extend_from_slice(&[0xFF, 0xE1]);
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(b"Exif\0\0");
            out.extend_from_slice(&tiff);
        }
        out.extend_from_slice(&jpeg[2..]);
        std::fs::write(path, out).unwrap();
    }

    pub fn orientation_field(v: u16) -> Field {
        field(Tag::Orientation, Value::Short(vec![v]))
    }

    #[test]
    fn reads_camera_fields_and_gps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        let fields = vec![
            field(Tag::Make, Value::Ascii(vec![b"Canon".to_vec()])),
            field(Tag::Model, Value::Ascii(vec![b"Canon EOS R5".to_vec()])),
            field(
                Tag::DateTimeOriginal,
                Value::Ascii(vec![b"2024:01:10 12:34:56".to_vec()]),
            ),
            field(Tag::ExposureTime, Value::Rational(vec![r(1, 250)])),
            field(Tag::FNumber, Value::Rational(vec![r(28, 10)])),
            field(Tag::PhotographicSensitivity, Value::Short(vec![400])),
            field(Tag::FocalLength, Value::Rational(vec![r(50, 1)])),
            field(Tag::GPSLatitudeRef, Value::Ascii(vec![b"N".to_vec()])),
            field(
                Tag::GPSLatitude,
                Value::Rational(vec![r(48, 1), r(51, 1), r(3024, 100)]),
            ),
            field(Tag::GPSLongitudeRef, Value::Ascii(vec![b"W".to_vec()])),
            field(
                Tag::GPSLongitude,
                Value::Rational(vec![r(2, 1), r(17, 1), r(4020, 100)]),
            ),
            orientation_field(6),
        ];
        write_jpeg(&path, 8, 4, &fields);
        let info = read_exif(&path).expect("exif");
        assert_eq!(info.orientation, Orientation::from_exif(6));
        assert_eq!(info.camera.as_deref(), Some("Canon EOS R5"));
        assert_eq!(info.date_taken.as_deref(), Some("2024-01-10 12:34:56"));
        assert_eq!(info.exposure.as_deref(), Some("1/250 s"));
        assert_eq!(info.aperture.as_deref(), Some("ƒ/2.8"));
        assert_eq!(info.iso.as_deref(), Some("400"));
        assert_eq!(info.focal_length.as_deref(), Some("50 mm"));
        let (lat, lon) = info.gps.unwrap();
        assert!((lat - 48.8584).abs() < 1e-4, "{lat}");
        assert!((lon + 2.2945).abs() < 1e-4, "{lon}");
        assert_eq!(format_gps(lat, lon), "48.85840° N, 2.29450° W");
        assert!(info.has_details());
    }

    #[test]
    fn no_exif_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.jpg");
        write_jpeg(&path, 4, 4, &[]);
        assert_eq!(read_exif(&path), None);
        assert_eq!(read_exif(&dir.path().join("missing.jpg")), None);
    }

    #[test]
    fn orientation_only_has_no_details() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("o.jpg");
        write_jpeg(&path, 4, 2, &[orientation_field(8)]);
        let info = read_exif(&path).unwrap();
        assert_eq!(info.orientation.value(), 8);
        assert!(!info.has_details());
    }

    #[test]
    fn formatting() {
        assert_eq!(format_exposure(1, 60), "1/60 s");
        assert_eq!(format_exposure(10, 3200), "1/320 s");
        assert_eq!(format_exposure(2, 1), "2 s");
        assert_eq!(format_exposure(15, 10), "1.5 s");
        assert_eq!(format_aperture(8.0), "ƒ/8");
        assert_eq!(format_focal_length(4.25), "4.2 mm");
        assert_eq!(format_exif_date("2020:02:29"), "2020-02-29");
        assert_eq!(format_dimensions(6000, 4000), "6000 × 4000 (24 MP)");
        assert_eq!(format_dimensions(16, 16), "16 × 16");
        assert_eq!(geo_uri(1.5, -2.25), "geo:1.500000,-2.250000");
        assert_eq!(
            camera_name(Some("NIKON CORPORATION"), Some("NIKON D750")).unwrap(),
            "NIKON D750"
        );
        assert_eq!(
            camera_name(Some("Google"), Some("Pixel 7")).unwrap(),
            "Google Pixel 7"
        );
        assert_eq!(camera_name(None, Some("X")).unwrap(), "X");
        assert_eq!(camera_name(Some(" "), None), None);
    }
}
