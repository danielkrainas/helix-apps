//! Screenshot file naming.

use helix_app_common::i18n::tr;
use helix_app_common::trf;
use std::path::{Path, PathBuf};

/// Directory screenshots go to: XDG Pictures, else the home directory.
pub fn target_dir() -> PathBuf {
    gtk::glib::user_special_dir(gtk::glib::UserDirectory::Pictures)
        .filter(|d| d.is_dir())
        .unwrap_or_else(gtk::glib::home_dir)
}

/// Base name (without extension) for a screenshot of the forecast for
/// `place`, taken at `stamp` (e.g. "2026-11-19 14-05-09").
pub fn base_name(place: &str, stamp: &str) -> String {
    let place = place.trim();
    let name = if place.is_empty() {
        format!("{} {stamp}", tr("Weather"))
    } else {
        // Translators: a screenshot's file name, e.g. "Weather in Berlin
        // 2026-11-19 14-05-09".
        trf!("Weather in {place} {time}", place = place, time = stamp)
    };
    // Keep the name a single path component.
    name.chars()
        .map(|c| if c == '/' || c == '\0' { '_' } else { c })
        .collect()
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
        assert_eq!(
            base_name("Berlin", "2026-11-19 14-05-09"),
            "Weather in Berlin 2026-11-19 14-05-09"
        );
        assert_eq!(
            base_name("  ", "2026-11-19 14-05-09"),
            "Weather 2026-11-19 14-05-09"
        );
        assert_eq!(base_name("A/B", "t"), "Weather in A_B t");
    }

    #[test]
    fn uniquing() {
        let dir = Path::new("/pics");
        let taken: HashSet<PathBuf> = [dir.join("x.png"), dir.join("x (2).png")].into();
        assert_eq!(
            unique_path(dir, "y", "png", |p| taken.contains(p)),
            dir.join("y.png")
        );
        assert_eq!(
            unique_path(dir, "x", "png", |p| taken.contains(p)),
            dir.join("x (3).png")
        );
    }
}
