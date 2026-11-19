//! Moving to the trash and restoring from it (freedesktop.org Trash spec).
//!
//! Trashing goes through `gio::File::trash`. Restoring locates the matching
//! `.trashinfo` in the trash directories that could hold the file and moves
//! the file back. Only trash directories GIO itself would use count: the
//! home trash and the trash at the top of the file's mount, owned by the
//! user and not writable by others, never through a symbolic link, and a
//! shared `.Trash` only with its sticky bit set (the spec's checks, as
//! `glocalfile.c` makes them). Anything else could have been planted by
//! another user or on a removable drive.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use helix_app_common::i18n::tr;

/// Moves `path` to the trash (blocking; call from a worker thread).
pub fn trash(path: &Path) -> Result<(), String> {
    gio::File::for_path(path)
        .trash(gio::Cancellable::NONE)
        .map_err(|e| e.message().to_string())
}

/// Percent-decodes a trashinfo `Path=` value.
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Parses a `.trashinfo` file into (original path, deletion date string).
/// Relative paths are resolved against `topdir`.
pub fn parse_trashinfo(contents: &str, topdir: &Path) -> Option<(PathBuf, String)> {
    let mut in_section = false;
    let mut path = None;
    let mut date = String::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Trash Info]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(v) = line.strip_prefix("Path=") {
            use std::os::unix::ffi::OsStringExt;
            let p = PathBuf::from(std::ffi::OsString::from_vec(percent_decode(v)));
            path = Some(if p.is_absolute() { p } else { topdir.join(p) });
        } else if let Some(v) = line.strip_prefix("DeletionDate=") {
            date = v.to_string();
        }
    }
    path.map(|p| (p, date))
}

/// The top folder of the mount holding `original`'s folder.
fn mount_top(original: &Path) -> Option<PathBuf> {
    let mut top = original.parent()?.to_path_buf();
    let dev = std::fs::metadata(&top).ok()?.dev();
    while let Some(up) = top.parent() {
        if std::fs::metadata(up).ok()?.dev() != dev {
            break;
        }
        top = up.to_path_buf();
    }
    Some(top)
}

/// Trash directories that may hold a trashed copy of `original`.
pub fn candidate_trash_dirs(original: &Path) -> Vec<(PathBuf, PathBuf)> {
    // (trash dir, topdir for relative paths)
    let mut dirs = vec![(glib::user_data_dir().join("Trash"), PathBuf::from("/"))];
    let uid = unsafe { libc::getuid() };
    if let Some(top) = mount_top(original) {
        dirs.push((top.join(format!(".Trash-{uid}")), top.clone()));
        dirs.push((top.join(".Trash").join(uid.to_string()), top));
    }
    dirs
}

/// Whether `path` is a real folder (not a symbolic link) owned by `uid`
/// that no one else can write to.
fn owned_private_dir(path: &Path, uid: u32) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o022 == 0)
}

/// Whether `trash` is a trash directory to trust for user `uid`: a
/// private folder of the user's with private `files` and `info`
/// folders, and inside a shared `.Trash` only when that is a real
/// folder with the sticky bit.
fn trusted_trash_dir(trash: &Path, uid: u32) -> bool {
    if !owned_private_dir(trash, uid) {
        return false;
    }
    if trash.file_name() == Some(uid.to_string().as_ref()) {
        if let Some(shared) = trash.parent().filter(|p| p.ends_with(".Trash")) {
            let sticky = std::fs::symlink_metadata(shared)
                .is_ok_and(|m| m.is_dir() && m.mode() & 0o1000 != 0);
            if !sticky {
                return false;
            }
        }
    }
    owned_private_dir(&trash.join("files"), uid) && owned_private_dir(&trash.join("info"), uid)
}

/// A trashed file: its location in `files/` and its `.trashinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    pub file: PathBuf,
    pub info: PathBuf,
}

/// Finds the most recently trashed entry whose original path is `original`.
pub fn find_trashed(original: &Path, dirs: &[(PathBuf, PathBuf)]) -> Option<Trashed> {
    let mut best: Option<(String, Trashed)> = None;
    let uid = unsafe { libc::getuid() };
    for (trash, top) in dirs {
        if !trusted_trash_dir(trash, uid) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(trash.join("info")) else {
            continue;
        };
        for entry in rd.filter_map(Result::ok) {
            let info = entry.path();
            if info.extension().and_then(|e| e.to_str()) != Some("trashinfo") {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&info) else {
                continue;
            };
            let Some((orig, date)) = parse_trashinfo(&contents, top) else {
                continue;
            };
            if orig != original {
                continue;
            }
            let Some(stem) = info.file_stem() else {
                continue;
            };
            let file = trash.join("files").join(stem);
            if std::fs::symlink_metadata(&file).is_err() {
                continue;
            }
            // ISO 8601 dates compare correctly as strings.
            if best.as_ref().is_none_or(|(d, _)| date >= *d) {
                best = Some((date, Trashed { file, info }));
            }
        }
    }
    best.map(|(_, t)| t)
}

/// Restores `original` from the trash. Fails if something now occupies the
/// original path.
pub fn restore(original: &Path, dirs: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    if original.exists() {
        return Err(tr("A file with the same name already exists").into());
    }
    let t = find_trashed(original, dirs)
        .ok_or_else(|| tr("The file was not found in the trash").to_string())?;
    if std::fs::rename(&t.file, original).is_err() {
        // Different file system (e.g. home trash for a file elsewhere).
        let src = gio::File::for_path(&t.file);
        src.move_(
            &gio::File::for_path(original),
            gio::FileCopyFlags::NONE,
            gio::Cancellable::NONE,
            None,
        )
        .map_err(|e| e.message().to_string())?;
    }
    let _ = std::fs::remove_file(&t.info);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Creates a fake trash entry by hand (no real trash is touched).
    fn fake_trash(trash: &Path, name: &str, orig: &str, date: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(trash.join("files")).unwrap();
        fs::create_dir_all(trash.join("info")).unwrap();
        for d in [trash.to_path_buf(), trash.join("files"), trash.join("info")] {
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(trash.join("files").join(name), name.as_bytes()).unwrap();
        fs::write(
            trash.join("info").join(format!("{name}.trashinfo")),
            format!("[Trash Info]\nPath={orig}\nDeletionDate={date}\n"),
        )
        .unwrap();
    }

    #[test]
    fn parses_trashinfo() {
        let (p, d) = parse_trashinfo(
            "[Trash Info]\nPath=/home/u/My%20Pictures/caf%C3%A9.png\nDeletionDate=2024-05-01T10:00:00\n",
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(p, Path::new("/home/u/My Pictures/café.png"));
        assert_eq!(d, "2024-05-01T10:00:00");
        let (p, _) =
            parse_trashinfo("[Trash Info]\nPath=photos/a.png\n", Path::new("/media/usb")).unwrap();
        assert_eq!(p, Path::new("/media/usb/photos/a.png"));
        assert!(parse_trashinfo("[Other]\nPath=/x\n", Path::new("/")).is_none());
        assert_eq!(percent_decode("100%"), b"100%");
    }

    #[test]
    fn restores_latest_matching_entry() {
        let root = tempfile::tempdir().unwrap();
        let trash = root.path().join("Trash");
        let pics = root.path().join("pics");
        fs::create_dir(&pics).unwrap();
        let orig = pics.join("a b.png");
        let orig_s = orig.to_str().unwrap().replace(' ', "%20");
        fake_trash(&trash, "a b.png", &orig_s, "2024-01-01T00:00:00");
        fake_trash(&trash, "a b.2.png", &orig_s, "2024-06-01T00:00:00");
        fake_trash(
            &trash,
            "other.png",
            "/elsewhere/other.png",
            "2025-01-01T00:00:00",
        );
        let dirs = vec![(trash.clone(), PathBuf::from("/"))];

        let t = find_trashed(&orig, &dirs).unwrap();
        assert_eq!(t.file, trash.join("files/a b.2.png"));

        restore(&orig, &dirs).unwrap();
        assert_eq!(fs::read(&orig).unwrap(), b"a b.2.png");
        assert!(!trash.join("info/a b.2.png.trashinfo").exists());
        // Refuses to overwrite.
        assert!(restore(&orig, &dirs).is_err());
        fs::remove_file(&orig).unwrap();
        restore(&orig, &dirs).unwrap();
        assert_eq!(fs::read(&orig).unwrap(), b"a b.png");
        fs::remove_file(&orig).unwrap();
        assert!(restore(&orig, &dirs).is_err());
    }

    #[test]
    fn topdir_relative_entries() {
        let root = tempfile::tempdir().unwrap();
        let trash = root.path().join(".Trash-1000");
        fake_trash(&trash, "x.png", "dir/x.png", "2024-01-01T00:00:00");
        fs::create_dir(root.path().join("dir")).unwrap();
        let dirs = vec![(trash, root.path().to_path_buf())];
        restore(&root.path().join("dir/x.png"), &dirs).unwrap();
        assert!(root.path().join("dir/x.png").exists());
    }

    #[test]
    fn candidates_cover_home_and_mount_top() {
        let root = tempfile::tempdir().unwrap();
        let pics = root.path().join("pics");
        fs::create_dir(&pics).unwrap();
        let d = candidate_trash_dirs(&pics.join("a.png"));
        assert_eq!(d[0].0, glib::user_data_dir().join("Trash"));
        // Only the mount's top folder, not every folder above the file.
        assert_eq!(d.len(), 3);
        let top = mount_top(&pics.join("a.png")).unwrap();
        assert!(pics.starts_with(&top));
        assert!(d[1..]
            .iter()
            .all(|(t, tp)| t.starts_with(&top) && *tp == top));
        // A file whose folder is gone has only the home trash.
        assert_eq!(
            candidate_trash_dirs(Path::new("/nonexistent/x/a.png")).len(),
            1
        );
    }

    #[test]
    fn untrusted_trash_dirs_are_ignored() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let uid = unsafe { libc::getuid() };
        let root = tempfile::tempdir().unwrap();
        let orig = root.path().join("a.png");
        let orig_s = orig.to_str().unwrap();
        let mode =
            |p: &Path, m: u32| fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();

        // Writable by others: ignored, until fixed.
        let own = root.path().join(format!(".Trash-{uid}"));
        fake_trash(&own, "a.png", orig_s, "2024-01-01T00:00:00");
        mode(&own, 0o777);
        let dirs = vec![(own.clone(), root.path().to_path_buf())];
        assert!(find_trashed(&orig, &dirs).is_none());
        mode(&own, 0o700);
        mode(&own.join("files"), 0o777);
        assert!(find_trashed(&orig, &dirs).is_none());
        mode(&own.join("files"), 0o700);
        assert!(find_trashed(&orig, &dirs).is_some());

        // Reached through a symbolic link: ignored.
        let link = root.path().join("linked");
        symlink(&own, &link).unwrap();
        assert!(find_trashed(&orig, &[(link, root.path().to_path_buf())]).is_none());

        // A shared .Trash without the sticky bit: ignored.
        let shared = root.path().join(".Trash");
        let mine = shared.join(uid.to_string());
        fake_trash(&mine, "a.png", orig_s, "2025-01-01T00:00:00");
        mode(&mine, 0o700);
        mode(&shared, 0o755);
        let dirs = vec![(mine.clone(), root.path().to_path_buf())];
        assert!(find_trashed(&orig, &dirs).is_none());
        mode(&shared, 0o1777);
        assert_eq!(
            find_trashed(&orig, &dirs).unwrap().file,
            mine.join("files/a.png")
        );
        mode(&shared, 0o755);
    }
}
