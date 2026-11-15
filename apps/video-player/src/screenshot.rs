//! Screenshot file naming.

use helix_app_common::i18n::tr;
use std::path::{Path, PathBuf};

/// Directory screenshots go to: XDG Pictures, else the home directory.
pub fn target_dir() -> PathBuf {
    gtk::glib::user_special_dir(gtk::glib::UserDirectory::Pictures)
        .filter(|d| d.is_dir())
        .unwrap_or_else(gtk::glib::home_dir)
}

/// Base name (without extension) for a screenshot of `video` at `secs`.
pub fn base_name(video_name: &str, secs: f64) -> String {
    let stem = Path::new(video_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(tr("Video"));
    // Keep the name a single path component.
    let stem: String = stem
        .chars()
        .map(|c| if c == '/' || c == '\0' { '_' } else { c })
        .collect();
    format!("{stem} {}", crate::timefmt::file_stamp(secs))
}

/// `dir/base.ext`, or `dir/base (N).ext` with the smallest N ≥ 2 for which
/// `exists` returns false.
pub fn unique_path(dir: &Path, base: &str, ext: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(format!("{base}.{ext}"));
    if !exists(&first) {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{base} ({n}).{ext}")))
        .find(|p| !exists(p))
        .expect("unbounded search")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names() {
        assert_eq!(base_name("Holiday.mkv", 65.4), "Holiday 0-01-05");
        assert_eq!(base_name("", 0.0), "Video 0-00-00");
        assert_eq!(base_name("a.b.webm", 3725.0), "a.b 1-02-05");
    }

    #[test]
    fn uniquing() {
        let dir = Path::new("/pics");
        let taken: HashSet<PathBuf> = [
            dir.join("x.png"),
            dir.join("x (2).png"),
            dir.join("x (4).png"),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            unique_path(dir, "y", "png", |p| taken.contains(p)),
            dir.join("y.png")
        );
        assert_eq!(
            unique_path(dir, "x", "png", |p| taken.contains(p)),
            dir.join("x (3).png")
        );
    }

    #[test]
    fn uniquing_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let a = unique_path(tmp.path(), "shot", "png", |p| p.exists());
        std::fs::write(&a, b"").unwrap();
        let b = unique_path(tmp.path(), "shot", "png", |p| p.exists());
        assert_eq!(b, tmp.path().join("shot (2).png"));
    }
}
