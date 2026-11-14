//! File names and folders.

use gtk::glib;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A local date and time, to the second.
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
    pub fn now() -> Self {
        match glib::DateTime::now_local() {
            Ok(t) => Stamp {
                year: t.year(),
                month: t.month() as u32,
                day: t.day_of_month() as u32,
                hour: t.hour() as u32,
                minute: t.minute() as u32,
                second: t.second() as u32,
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

/// `Screenshot from YYYY-MM-DD HH-MM-SS.png`.
pub fn default_file_name(t: Stamp) -> String {
    format!(
        "Screenshot from {:04}-{:02}-{:02} {:02}-{:02}-{:02}.png",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// The folder the save dialog starts in: the last one used if it still
/// exists, else the pictures folder, else home.
pub fn save_folder(last: Option<&Path>) -> PathBuf {
    if let Some(dir) = last.filter(|d| d.is_dir()) {
        return dir.to_path_buf();
    }
    glib::user_special_dir(glib::UserDirectory::Pictures)
        .filter(|d| d.is_dir())
        .unwrap_or_else(glib::home_dir)
}

/// A fresh path for the shell to write a capture to, in the runtime
/// directory (or the cache directory without one).
pub fn temp_capture_path() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute() && d.is_dir())
        .unwrap_or_else(glib::user_cache_dir);
    temp_capture_path_in(
        &dir,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    )
}

pub fn temp_capture_path_in(dir: &Path, pid: u32, n: u32) -> PathBuf {
    dir.join(format!("helix-screenshot-{pid}-{n}.png"))
}

/// Resolves a `--file` argument against the working directory.
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_is_zero_padded() {
        let t = Stamp {
            year: 2026,
            month: 3,
            day: 7,
            hour: 9,
            minute: 5,
            second: 1,
        };
        assert_eq!(
            default_file_name(t),
            "Screenshot from 2026-03-07 09-05-01.png"
        );
        let t = Stamp {
            year: 2026,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 58,
        };
        assert_eq!(
            default_file_name(t),
            "Screenshot from 2026-12-31 23-59-58.png"
        );
    }

    #[test]
    fn now_is_plausible() {
        let t = Stamp::now();
        assert!(t.year >= 2024);
        assert!((1..=12).contains(&t.month) && (1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.minute < 60 && t.second < 61);
    }

    #[test]
    fn temp_paths_are_distinct() {
        let a = temp_capture_path();
        let b = temp_capture_path();
        assert_ne!(a, b);
        assert!(a.is_absolute());
        assert_eq!(
            temp_capture_path_in(Path::new("/run/user/1000"), 42, 3),
            PathBuf::from("/run/user/1000/helix-screenshot-42-3.png")
        );
    }

    #[test]
    fn save_folder_prefers_existing_last_folder() {
        let tmp = std::env::temp_dir();
        assert_eq!(save_folder(Some(&tmp)), tmp);
        let gone = tmp.join("helix-screenshot-no-such-dir-xyz");
        assert_ne!(save_folder(Some(&gone)), gone);
    }

    #[test]
    fn absolute_resolves_relative_paths() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute(Path::new("a.png")), cwd.join("a.png"));
        assert_eq!(absolute(Path::new("/x/a.png")), PathBuf::from("/x/a.png"));
    }
}
