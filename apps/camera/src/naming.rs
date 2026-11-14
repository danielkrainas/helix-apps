//! Output directory and capture file naming.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Photo,
    Video,
}

impl CaptureKind {
    fn prefix(self) -> &'static str {
        match self {
            CaptureKind::Photo => "Photo from",
            CaptureKind::Video => "Video from",
        }
    }
}

/// A broken-down local timestamp (kept independent of glib for testing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Stamp {
    pub fn now() -> Stamp {
        match glib::DateTime::now_local() {
            Ok(d) => Stamp {
                year: d.year(),
                month: d.month() as u32,
                day: d.day_of_month() as u32,
                hour: d.hour() as u32,
                minute: d.minute() as u32,
                second: d.second() as u32,
            },
            Err(_) => Stamp {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            },
        }
    }
}

/// Base name without extension, e.g. `Photo from 2024-03-09 07-05-01`.
pub fn base_name(kind: CaptureKind, s: &Stamp) -> String {
    format!(
        "{} {:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        kind.prefix(),
        s.year,
        s.month,
        s.day,
        s.hour,
        s.minute,
        s.second
    )
}

/// Picks a file name in `dir` that `exists` reports as free, appending
/// ` (2)`, ` (3)`, … on clashes.
pub fn unique_path(
    dir: &Path,
    kind: CaptureKind,
    stamp: &Stamp,
    ext: &str,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let base = base_name(kind, stamp);
    let first = dir.join(format!("{base}.{ext}"));
    if !exists(&first) {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{base} ({n}).{ext}")))
        .find(|p| !exists(p))
        .expect("unbounded search")
}

/// Unique path on the real filesystem for a capture taken now.
pub fn new_capture_path(dir: &Path, kind: CaptureKind, ext: &str) -> PathBuf {
    unique_path(dir, kind, &Stamp::now(), ext, |p| p.exists())
}

/// `<Pictures>/Camera`, honouring `HELIX_CAMERA_OUTPUT_DIR`.
pub fn output_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("HELIX_CAMERA_OUTPUT_DIR") {
        return PathBuf::from(d);
    }
    let pictures = glib::user_special_dir(glib::UserDirectory::Pictures)
        .unwrap_or_else(|| glib::home_dir().join("Pictures"));
    pictures.join("Camera")
}

/// Whether `name` looks like one of our captures.
pub fn is_capture_name(name: &str) -> Option<CaptureKind> {
    let lower = name.to_ascii_lowercase();
    if name.starts_with(CaptureKind::Photo.prefix())
        && (lower.ends_with(".jpg") || lower.ends_with(".jpeg"))
    {
        Some(CaptureKind::Photo)
    } else if name.starts_with(CaptureKind::Video.prefix())
        && (lower.ends_with(".webm") || lower.ends_with(".mp4") || lower.ends_with(".mkv"))
    {
        Some(CaptureKind::Video)
    } else {
        None
    }
}

/// Newest capture (by modification time) in `dir`.
pub fn latest_capture(dir: &Path) -> Option<(PathBuf, CaptureKind)> {
    let rd = std::fs::read_dir(dir).ok()?;
    rd.filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let kind = is_capture_name(&name)?;
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, e.path(), kind))
        })
        .max_by_key(|(t, _, _)| *t)
        .map(|(_, p, k)| (p, k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn stamp() -> Stamp {
        Stamp {
            year: 2024,
            month: 3,
            day: 9,
            hour: 7,
            minute: 5,
            second: 1,
        }
    }

    #[test]
    fn base_names_are_zero_padded() {
        assert_eq!(
            base_name(CaptureKind::Photo, &stamp()),
            "Photo from 2024-03-09 07-05-01"
        );
        assert_eq!(
            base_name(CaptureKind::Video, &stamp()),
            "Video from 2024-03-09 07-05-01"
        );
    }

    #[test]
    fn unique_path_appends_counter() {
        let dir = Path::new("/x");
        let taken: HashSet<PathBuf> = [
            "/x/Photo from 2024-03-09 07-05-01.jpg",
            "/x/Photo from 2024-03-09 07-05-01 (2).jpg",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let p = unique_path(dir, CaptureKind::Photo, &stamp(), "jpg", |p| {
            taken.contains(p)
        });
        assert_eq!(
            p,
            PathBuf::from("/x/Photo from 2024-03-09 07-05-01 (3).jpg")
        );
        let free = unique_path(dir, CaptureKind::Video, &stamp(), "webm", |p| {
            taken.contains(p)
        });
        assert_eq!(
            free,
            PathBuf::from("/x/Video from 2024-03-09 07-05-01.webm")
        );
    }

    #[test]
    fn recognises_capture_names() {
        assert_eq!(
            is_capture_name("Photo from 2024-01-01 00-00-00.jpg"),
            Some(CaptureKind::Photo)
        );
        assert_eq!(
            is_capture_name("Video from 2024-01-01 00-00-00 (2).webm"),
            Some(CaptureKind::Video)
        );
        assert_eq!(
            is_capture_name("Video from x.mp4"),
            Some(CaptureKind::Video)
        );
        assert_eq!(is_capture_name("holiday.jpg"), None);
        assert_eq!(is_capture_name("Photo from x.txt"), None);
    }

    #[test]
    fn latest_capture_uses_mtime() {
        let dir = std::env::temp_dir().join(format!("helix-camera-naming-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("Photo from 2024-01-01 00-00-00.jpg");
        let b = dir.join("Video from 2024-01-01 00-00-01.webm");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(dir.join("other.jpg"), b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&b, b"b").unwrap();
        assert_eq!(latest_capture(&dir), Some((b.clone(), CaptureKind::Video)));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(latest_capture(&dir), None);
    }
}
