//! The list of images being browsed and navigation within it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::natsort;

/// Lower-case file extensions the viewer can decode.
#[derive(Debug, Clone, Default)]
pub struct Extensions(HashSet<String>);

impl Extensions {
    pub fn new<I: IntoIterator<Item = S>, S: AsRef<str>>(exts: I) -> Self {
        Extensions(exts.into_iter().map(|e| e.as_ref().to_ascii_lowercase()).collect())
    }

    pub fn supports(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.0.contains(&e.to_ascii_lowercase()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| natsort::compare(&file_name(a), &file_name(b)));
}

/// Supported, non-hidden regular files of `dir`, naturally sorted. `keep` is
/// always included when it lives in `dir` (e.g. an explicitly opened hidden
/// file).
pub fn list_images(dir: &Path, exts: &Extensions, keep: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| !file_name(p).starts_with('.') && exts.supports(p) && p.is_file())
                .collect()
        })
        .unwrap_or_default();
    if let Some(k) = keep {
        if k.parent() == Some(dir) && !out.iter().any(|p| p == k) && k.exists() {
            out.push(k.to_path_buf());
        }
    }
    sort_paths(&mut out);
    out
}

/// Where the image list comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Everything in a folder (refreshed when the folder changes).
    Folder(PathBuf),
    /// An explicit set of files.
    Files,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub source: Source,
    files: Vec<PathBuf>,
    index: Option<usize>,
}

impl Default for Playlist {
    fn default() -> Self {
        Playlist { source: Source::Files, files: Vec::new(), index: None }
    }
}

impl Playlist {
    /// The list for a single opened file: its folder's images.
    pub fn for_file(path: &Path, exts: &Extensions) -> Self {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let files = list_images(&dir, exts, Some(path));
        let index = files.iter().position(|p| p == path);
        // An unsupported/missing file still gets shown (and fails visibly).
        let (files, index) = match index {
            Some(i) => (files, Some(i)),
            None => (vec![path.to_path_buf()], Some(0)),
        };
        Playlist { source: Source::Folder(dir), files, index }
    }

    /// The list for an opened directory, starting at its first image.
    pub fn for_dir(dir: &Path, exts: &Extensions) -> Self {
        let files = list_images(dir, exts, None);
        let index = if files.is_empty() { None } else { Some(0) };
        Playlist { source: Source::Folder(dir.to_path_buf()), files, index }
    }

    /// An explicit list of files (kept in the given order).
    pub fn for_files(paths: Vec<PathBuf>, exts: &Extensions) -> Self {
        let files: Vec<PathBuf> = paths.into_iter().filter(|p| exts.supports(p)).collect();
        let index = if files.is_empty() { None } else { Some(0) };
        Playlist { source: Source::Files, files, index }
    }

    /// Picks the right constructor for what the user opened.
    pub fn open(paths: Vec<PathBuf>, exts: &Extensions) -> Self {
        match paths.as_slice() {
            [] => Playlist::default(),
            [one] if one.is_dir() => Playlist::for_dir(one, exts),
            [one] => Playlist::for_file(one, exts),
            _ => Playlist::for_files(paths, exts),
        }
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn index(&self) -> Option<usize> {
        self.index
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Makes the image at `i` current. Returns whether it changed.
    pub fn go_to(&mut self, i: usize) -> bool {
        i < self.files.len() && self.jump(i)
    }

    pub fn current(&self) -> Option<&Path> {
        self.index.and_then(|i| self.files.get(i)).map(PathBuf::as_path)
    }

    pub fn has_next(&self) -> bool {
        self.index.is_some_and(|i| i + 1 < self.files.len())
    }

    pub fn has_prev(&self) -> bool {
        self.index.is_some_and(|i| i > 0)
    }

    /// Moves by `delta` without wrapping. Returns whether the index changed.
    pub fn step(&mut self, delta: isize) -> bool {
        let Some(i) = self.index else { return false };
        let target = (i as isize + delta).clamp(0, self.files.len() as isize - 1) as usize;
        self.index = Some(target);
        target != i
    }

    pub fn first(&mut self) -> bool {
        self.jump(0)
    }

    pub fn last(&mut self) -> bool {
        self.jump(self.files.len().saturating_sub(1))
    }

    fn jump(&mut self, i: usize) -> bool {
        if self.files.is_empty() || self.index == Some(i) {
            return false;
        }
        self.index = Some(i);
        true
    }

    /// Removes `path` (e.g. after trashing it). If it was current, the image
    /// that takes its place (or the new last one) becomes current.
    pub fn remove(&mut self, path: &Path) {
        let Some(pos) = self.files.iter().position(|p| p == path) else { return };
        self.files.remove(pos);
        self.index = match self.index {
            _ if self.files.is_empty() => None,
            Some(i) if i > pos => Some(i - 1),
            Some(i) => Some(i.min(self.files.len() - 1)),
            None => None,
        };
    }

    /// Inserts `path` back in sorted position (folder lists) or at `at`.
    pub fn insert(&mut self, path: PathBuf, at: usize) {
        if self.files.contains(&path) {
            return;
        }
        let current = self.current().map(Path::to_path_buf);
        match self.source {
            Source::Folder(_) => {
                self.files.push(path);
                sort_paths(&mut self.files);
            }
            Source::Files => self.files.insert(at.min(self.files.len()), path),
        }
        self.index = match current {
            Some(c) => self.files.iter().position(|p| *p == c),
            None => Some(0),
        };
    }

    pub fn select(&mut self, path: &Path) -> bool {
        match self.files.iter().position(|p| p == path) {
            Some(i) => {
                self.index = Some(i);
                true
            }
            None => false,
        }
    }

    pub fn position_of(&self, path: &Path) -> Option<usize> {
        self.files.iter().position(|p| p == path)
    }

    /// Replaces the list with a fresh folder listing. Keeps the current image
    /// if it still exists; otherwise selects the image now at its old index.
    /// Returns whether the current image changed.
    pub fn refresh(&mut self, fresh: Vec<PathBuf>) -> bool {
        let old_current = self.current().map(Path::to_path_buf);
        let old_index = self.index;
        self.files = fresh;
        self.index = match (&old_current, old_index) {
            (Some(c), _) if self.files.contains(c) => self.files.iter().position(|p| p == c),
            _ if self.files.is_empty() => None,
            (_, Some(i)) => Some(i.min(self.files.len() - 1)),
            (_, None) => Some(0),
        };
        self.current().map(Path::to_path_buf) != old_current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn exts() -> Extensions {
        Extensions::new(["png", "jpg", "jpeg", "gif"])
    }

    fn touch(dir: &Path, names: &[&str]) {
        for n in names {
            fs::write(dir.join(n), b"x").unwrap();
        }
    }

    fn names(p: &Playlist) -> Vec<String> {
        p.files.iter().map(|f| file_name(f)).collect()
    }

    #[test]
    fn folder_listing_filters_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &["b10.png", "b2.JPG", "a.gif", ".hidden.png", "notes.txt", "c1.jpeg"]);
        fs::create_dir(dir.path().join("sub.png")).unwrap();
        let list = list_images(dir.path(), &exts(), None);
        let n: Vec<String> = list.iter().map(|f| file_name(f)).collect();
        assert_eq!(n, ["a.gif", "b2.JPG", "b10.png", "c1.jpeg"]);
    }

    #[test]
    fn open_single_file_lists_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &["1.png", "3.png", "20.png", ".secret.png"]);
        let p = Playlist::open(vec![dir.path().join("3.png")], &exts());
        assert_eq!(names(&p), ["1.png", "3.png", "20.png"]);
        assert_eq!(p.index(), Some(1));
        assert_eq!(p.source, Source::Folder(dir.path().to_path_buf()));

        // A hidden file opened explicitly is kept in the list.
        let p = Playlist::open(vec![dir.path().join(".secret.png")], &exts());
        assert_eq!(p.len(), 4);
        assert_eq!(p.current().unwrap(), dir.path().join(".secret.png"));
    }

    #[test]
    fn open_dir_and_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &["b.png", "a.png", "x.txt"]);
        let p = Playlist::open(vec![dir.path().to_path_buf()], &exts());
        assert_eq!(p.current().unwrap(), dir.path().join("a.png"));

        let p = Playlist::open(
            vec![dir.path().join("b.png"), dir.path().join("x.txt"), dir.path().join("a.png")],
            &exts(),
        );
        assert_eq!(names(&p), ["b.png", "a.png"]);
        assert_eq!(p.source, Source::Files);

        let empty = tempfile::tempdir().unwrap();
        let p = Playlist::open(vec![empty.path().to_path_buf()], &exts());
        assert_eq!(p.current(), None);
    }

    #[test]
    fn unsupported_or_missing_file_is_still_current() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &["a.png", "readme.txt"]);
        // Explicitly opened: shown (and fails visibly), siblings navigable.
        let p = Playlist::open(vec![dir.path().join("readme.txt")], &exts());
        assert_eq!(p.current().unwrap(), dir.path().join("readme.txt"));
        assert_eq!(p.len(), 2);
        let p = Playlist::open(vec![dir.path().join("gone.png")], &exts());
        assert_eq!(p.current().unwrap(), dir.path().join("gone.png"));
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn navigation_does_not_wrap() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &["1.png", "2.png", "3.png"]);
        let mut p = Playlist::open(vec![dir.path().join("1.png")], &exts());
        assert!(!p.has_prev());
        assert!(!p.step(-1));
        assert!(p.step(1));
        assert!(p.step(1));
        assert!(!p.has_next());
        assert!(!p.step(1));
        assert_eq!(p.current().unwrap(), dir.path().join("3.png"));
        assert!(p.first());
        assert_eq!(p.index(), Some(0));
        assert!(p.last());
        assert!(!p.last());
        assert_eq!(p.index(), Some(2));
    }

    #[test]
    fn remove_and_reinsert() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &["1.png", "2.png", "3.png"]);
        let mut p = Playlist::open(vec![dir.path().join("2.png")], &exts());
        p.remove(&dir.path().join("2.png"));
        assert_eq!(p.current().unwrap(), dir.path().join("3.png"));
        p.insert(dir.path().join("2.png"), 1);
        assert_eq!(names(&p), ["1.png", "2.png", "3.png"]);
        assert_eq!(p.current().unwrap(), dir.path().join("3.png"));
        assert!(p.select(&dir.path().join("2.png")));

        // Removing the last element selects the new last.
        p.last();
        p.remove(&dir.path().join("3.png"));
        assert_eq!(p.current().unwrap(), dir.path().join("2.png"));
        // Removing an earlier element keeps the current one.
        p.remove(&dir.path().join("1.png"));
        assert_eq!(p.current().unwrap(), dir.path().join("2.png"));
        p.remove(&dir.path().join("2.png"));
        assert_eq!(p.current(), None);
        p.insert(dir.path().join("2.png"), 0);
        assert_eq!(p.current().unwrap(), dir.path().join("2.png"));
    }

    #[test]
    fn refresh_keeps_current_or_takes_neighbour() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &["a.png", "b.png", "c.png"]);
        let mut p = Playlist::open(vec![dir.path().join("b.png")], &exts());

        touch(dir.path(), &["a0.png"]);
        let changed = p.refresh(list_images(dir.path(), &exts(), None));
        assert!(!changed);
        assert_eq!(p.current().unwrap(), dir.path().join("b.png"));
        assert_eq!(p.index(), Some(2));

        fs::remove_file(dir.path().join("b.png")).unwrap();
        let changed = p.refresh(list_images(dir.path(), &exts(), None));
        assert!(changed);
        assert_eq!(p.current().unwrap(), dir.path().join("c.png"));

        for n in ["a.png", "a0.png", "c.png"] {
            fs::remove_file(dir.path().join(n)).unwrap();
        }
        assert!(p.refresh(list_images(dir.path(), &exts(), None)));
        assert_eq!(p.current(), None);
    }
}
