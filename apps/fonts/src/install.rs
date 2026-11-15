//! Installing a font file into the user's font directory.

use helix_app_common::trf;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Outcome of [`install_into`].
#[derive(Debug, PartialEq)]
pub enum Installed {
    /// Copied to this path.
    Copied(PathBuf),
    /// An identical file was already present at this path.
    AlreadyPresent(PathBuf),
}

impl Installed {
    pub fn path(&self) -> &Path {
        match self {
            Installed::Copied(p) | Installed::AlreadyPresent(p) => p,
        }
    }
}

fn same_contents(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) if ma.len() == mb.len() => {
            matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
        }
        _ => false,
    }
}

/// Returns the path of an identical copy of `src` inside `fonts_dir`
/// (top level only), if any.
pub fn find_installed_copy(src: &Path, fonts_dir: &Path) -> Option<PathBuf> {
    let src_canon = src.canonicalize().ok();
    if let (Some(s), Ok(d)) = (&src_canon, fonts_dir.canonicalize()) {
        if s.starts_with(&d) {
            return Some(s.clone());
        }
    }
    let name = src.file_name()?;
    let direct = fonts_dir.join(name);
    if same_contents(src, &direct) {
        return Some(direct);
    }
    // Copies made with a numeric suffix.
    let rd = std::fs::read_dir(fonts_dir).ok()?;
    let stem = src.file_stem()?.to_string_lossy().to_string();
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_stem()
                .is_some_and(|s| s.to_string_lossy().starts_with(&stem))
        })
        .find(|p| same_contents(src, p))
}

/// Where `src` is already installed: itself if it lives under one of
/// `system_dirs`, or an identical copy in `user_dir`.
pub fn installed_location(src: &Path, system_dirs: &[PathBuf], user_dir: &Path) -> Option<PathBuf> {
    let canon = src.canonicalize().ok()?;
    if system_dirs
        .iter()
        .filter_map(|d| d.canonicalize().ok())
        .any(|d| canon.starts_with(d))
    {
        return Some(canon);
    }
    find_installed_copy(src, user_dir)
}

/// Picks a free file name in `dir` for `name`: `name`, then
/// `stem-1.ext`, `stem-2.ext`, ...
pub fn unique_target(dir: &Path, name: &Path) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "font".into());
    let ext = name
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|i| dir.join(format!("{stem}-{i}{ext}")))
        .find(|p| !p.exists())
        .expect("infinite iterator")
}

/// Copies `src` into `fonts_dir` (created if needed) without overwriting a
/// different existing file.
pub fn install_into(src: &Path, fonts_dir: &Path) -> io::Result<Installed> {
    crate::fontinfo::read_file(src)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::create_dir_all(fonts_dir)?;
    if let Some(p) = find_installed_copy(src, fonts_dir) {
        return Ok(Installed::AlreadyPresent(p));
    }
    let name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let target = unique_target(fonts_dir, Path::new(name));
    // Copy to a temporary name then rename, so a crash never leaves a
    // half-written font for fontconfig to choke on.
    let tmp = fonts_dir.join(format!(
        ".{}.part",
        target.file_name().unwrap().to_string_lossy()
    ));
    std::fs::copy(src, &tmp)?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(Installed::Copied(target))
}

/// Refreshes fontconfig's cache for `dir`. Failure (e.g. `fc-cache`
/// missing) is reported but the font is still installed.
pub fn refresh_cache(dir: &Path) -> io::Result<()> {
    let status = Command::new("fc-cache").arg("-f").arg(dir).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(trf!(
            "fc-cache exited with {status}",
            status = status
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_without_overwriting() {
        let Some(src) = crate::fontinfo::tests::any_ttf() else {
            eprintln!("no system font; skipping");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let fonts = tmp.path().join("data/fonts");
        let r = install_into(&src, &fonts).unwrap();
        let Installed::Copied(p) = &r else {
            panic!("{r:?}")
        };
        assert_eq!(p.parent().unwrap(), fonts);
        assert_eq!(std::fs::read(p).unwrap(), std::fs::read(&src).unwrap());
        // Second install detects the identical copy.
        assert_eq!(
            install_into(&src, &fonts).unwrap(),
            Installed::AlreadyPresent(p.clone())
        );
        // A different file with the same name is not overwritten.
        let other_dir = tmp.path().join("other");
        std::fs::create_dir_all(&other_dir).unwrap();
        let mut data = std::fs::read(&src).unwrap();
        data.extend_from_slice(&[0; 4]);
        let other = other_dir.join(src.file_name().unwrap());
        std::fs::write(&other, &data).unwrap();
        let r2 = install_into(&other, &fonts).unwrap();
        let Installed::Copied(p2) = &r2 else {
            panic!("{r2:?}")
        };
        assert_ne!(p2, p);
        assert!(p2.file_name().unwrap().to_string_lossy().contains("-1."));
        assert_eq!(std::fs::read(p).unwrap(), std::fs::read(&src).unwrap());
        assert_eq!(find_installed_copy(&other, &fonts).as_ref(), Some(p2));
        // Installing a file that already lives in the dir is a no-op.
        assert!(matches!(
            install_into(p, &fonts).unwrap(),
            Installed::AlreadyPresent(_)
        ));
        // installed_location: system dirs and user copies.
        let empty = tmp.path().join("empty");
        assert_eq!(
            installed_location(&other, std::slice::from_ref(&other_dir), &empty),
            Some(other.canonicalize().unwrap())
        );
        assert_eq!(installed_location(&other, &[], &fonts).as_ref(), Some(p2));
        assert_eq!(installed_location(&other, &[], &empty), None);
        // No stray temp files.
        assert!(std::fs::read_dir(&fonts)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
    }

    #[test]
    fn refuses_non_fonts() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad.ttf");
        std::fs::write(&bad, b"nope").unwrap();
        assert!(install_into(&bad, &tmp.path().join("fonts")).is_err());
        assert!(!tmp.path().join("fonts").exists());
    }

    #[test]
    fn unique_names() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_target(tmp.path(), Path::new("a.ttf")),
            tmp.path().join("a.ttf")
        );
        std::fs::write(tmp.path().join("a.ttf"), b"").unwrap();
        std::fs::write(tmp.path().join("a-1.ttf"), b"").unwrap();
        assert_eq!(
            unique_target(tmp.path(), Path::new("a.ttf")),
            tmp.path().join("a-2.ttf")
        );
    }
}
