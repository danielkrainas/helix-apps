//! Directory tree scanner.
//!
//! Pure std code (no GTK) so it can run on a worker thread and be unit
//! tested on temporary directory trees.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

pub type NodeId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub kind: Kind,
    /// Allocated size (`st_blocks * 512`), including everything below.
    pub size: u64,
    /// Apparent size (`st_size`), including everything below.
    pub apparent: u64,
    /// Number of entries below this one (0 for non-directories).
    pub items: u64,
    /// Modification time (seconds since the epoch).
    pub mtime: i64,
    /// The directory could not be (fully) read.
    pub error: bool,
    /// Removed from the tree (e.g. moved to trash).
    pub removed: bool,
}

#[derive(Debug, Clone)]
pub struct ScanTree {
    /// Absolute path of the root node.
    pub root_path: PathBuf,
    pub nodes: Vec<Node>,
    /// Number of directories that could not be read.
    pub errors: u64,
}

pub const ROOT: NodeId = 0;

impl ScanTree {
    pub fn root(&self) -> &Node {
        &self.nodes[ROOT]
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// Absolute path of a node.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = id;
        while let Some(p) = self.nodes[cur].parent {
            parts.push(self.nodes[cur].name.as_str());
            cur = p;
        }
        let mut path = self.root_path.clone();
        for part in parts.iter().rev() {
            path.push(part);
        }
        path
    }

    /// Chain of node ids from the root to `id` (inclusive).
    pub fn ancestry(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut cur = id;
        while let Some(p) = self.nodes[cur].parent {
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        chain
    }

    /// Looks up a child by name.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn child_named(&self, id: NodeId, name: &str) -> Option<NodeId> {
        self.nodes[id]
            .children
            .iter()
            .copied()
            .find(|&c| self.nodes[c].name == name)
    }

    /// Finds the node for a path below (or equal to) the root.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let rel = path.strip_prefix(&self.root_path).ok()?;
        let mut cur = ROOT;
        for comp in rel.components() {
            cur = self.child_named(cur, &comp.as_os_str().to_string_lossy())?;
        }
        Some(cur)
    }

    /// Detaches `id` from its parent and subtracts its sizes from all
    /// ancestors. Returns false for the root or an already removed node.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let Some(parent) = self.nodes[id].parent else {
            return false;
        };
        if self.nodes[id].removed {
            return false;
        }
        let (size, apparent, items) = {
            let n = &self.nodes[id];
            (n.size, n.apparent, n.items + 1)
        };
        self.nodes[id].removed = true;
        self.nodes[parent].children.retain(|&c| c != id);
        let mut cur = Some(parent);
        while let Some(p) = cur {
            let n = &mut self.nodes[p];
            n.size = n.size.saturating_sub(size);
            n.apparent = n.apparent.saturating_sub(apparent);
            n.items = n.items.saturating_sub(items);
            cur = n.parent;
        }
        true
    }

    /// Builds a tree by hand (for tests of code that consumes trees).
    #[cfg(test)]
    pub fn synthetic(root_size: u64) -> Self {
        ScanTree {
            root_path: PathBuf::from("/synthetic"),
            nodes: vec![Node {
                name: "synthetic".into(),
                parent: None,
                children: Vec::new(),
                kind: Kind::Dir,
                size: root_size,
                apparent: root_size,
                items: 0,
                mtime: 0,
                error: false,
                removed: false,
            }],
            errors: 0,
        }
    }

    #[cfg(test)]
    pub fn add_synthetic(&mut self, parent: NodeId, name: &str, kind: Kind, size: u64) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: name.into(),
            parent: Some(parent),
            children: Vec::new(),
            kind,
            size,
            apparent: size,
            items: 0,
            mtime: 0,
            error: false,
            removed: false,
        });
        self.nodes[parent].children.push(id);
        id
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Descend into folders on other file systems.
    pub cross_filesystems: bool,
    /// Absolute paths that are skipped together with everything below them.
    pub excluded: Vec<PathBuf>,
}

/// Progress shared between the scanning thread and the UI.
#[derive(Debug, Default)]
pub struct Progress {
    pub items: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<PathBuf>,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub enum ScanError {
    Cancelled,
    Io(std::io::Error),
    NotADirectory,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Cancelled => write!(f, "Scan cancelled"),
            ScanError::Io(e) => write!(f, "{e}"),
            ScanError::NotADirectory => write!(f, "Not a folder"),
        }
    }
}

struct Scanner<'a> {
    opts: &'a ScanOptions,
    progress: &'a Progress,
    root_dev: u64,
    seen_inodes: HashSet<(u64, u64)>,
    nodes: Vec<Node>,
    errors: u64,
}

fn kind_of(ft: &fs::FileType) -> Kind {
    if ft.is_dir() {
        Kind::Dir
    } else if ft.is_symlink() {
        Kind::Symlink
    } else if ft.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

impl Scanner<'_> {
    fn new_node(&mut self, name: String, parent: Option<NodeId>, md: &fs::Metadata) -> NodeId {
        let kind = kind_of(&md.file_type());
        let (mut size, mut apparent) = (md.blocks() * 512, md.size());
        if kind != Kind::Dir && md.nlink() > 1 && !self.seen_inodes.insert((md.dev(), md.ino())) {
            // Another hard link to an inode we already counted.
            size = 0;
            apparent = 0;
        }
        self.progress.items.fetch_add(1, Ordering::Relaxed);
        self.progress.bytes.fetch_add(size, Ordering::Relaxed);
        let id = self.nodes.len();
        self.nodes.push(Node {
            name,
            parent,
            children: Vec::new(),
            kind,
            size,
            apparent,
            items: 0,
            mtime: md.mtime(),
            error: false,
            removed: false,
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(id);
        }
        id
    }

    fn is_excluded(&self, path: &Path) -> bool {
        self.opts.excluded.iter().any(|e| e == path)
    }

    /// Scans the children of directory node `id` located at `path`.
    fn scan_dir(&mut self, id: NodeId, path: &Path) -> Result<(), ScanError> {
        if let Ok(mut cur) = self.progress.current.lock() {
            cur.clone_from(&path.to_path_buf());
        }
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => {
                self.nodes[id].error = true;
                self.errors += 1;
                return Ok(());
            }
        };
        for entry in entries {
            if self.progress.cancelled() {
                return Err(ScanError::Cancelled);
            }
            let Ok(entry) = entry else {
                self.nodes[id].error = true;
                continue;
            };
            let child_path = entry.path();
            if self.is_excluded(&child_path) {
                continue;
            }
            // lstat: never follow symlinks.
            let Ok(md) = fs::symlink_metadata(&child_path) else {
                self.nodes[id].error = true;
                continue;
            };
            if md.is_dir() && !self.opts.cross_filesystems && md.dev() != self.root_dev {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = self.new_node(name, Some(id), &md);
            if md.is_dir() {
                self.scan_dir(child, &child_path)?;
            }
            let (s, a, i) = {
                let c = &self.nodes[child];
                (c.size, c.apparent, c.items + 1)
            };
            let n = &mut self.nodes[id];
            n.size += s;
            n.apparent += a;
            n.items += i;
        }
        // Largest first.
        let mut children = std::mem::take(&mut self.nodes[id].children);
        children.sort_by(|&a, &b| {
            let (na, nb) = (&self.nodes[a], &self.nodes[b]);
            nb.size.cmp(&na.size).then_with(|| na.name.cmp(&nb.name))
        });
        self.nodes[id].children = children;
        Ok(())
    }
}

/// Scans `root` (which must be a directory) and returns the size tree.
pub fn scan(root: &Path, opts: &ScanOptions, progress: &Progress) -> Result<ScanTree, ScanError> {
    let md = fs::metadata(root).map_err(ScanError::Io)?;
    if !md.is_dir() {
        return Err(ScanError::NotADirectory);
    }
    let root_path = root.to_path_buf();
    let name = root_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root_path.to_string_lossy().into_owned());
    let mut scanner = Scanner {
        opts,
        progress,
        root_dev: md.dev(),
        seen_inodes: HashSet::new(),
        nodes: Vec::new(),
        errors: 0,
    };
    let id = scanner.new_node(name, None, &md);
    scanner.scan_dir(id, &root_path)?;
    Ok(ScanTree {
        root_path,
        nodes: scanner.nodes,
        errors: scanner.errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(path: &Path, len: usize) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(&vec![0x5au8; len]).unwrap();
        f.sync_all().unwrap();
    }

    fn alloc(path: &Path) -> u64 {
        fs::symlink_metadata(path).unwrap().blocks() * 512
    }

    fn run(root: &Path, opts: &ScanOptions) -> ScanTree {
        scan(root, opts, &Progress::default()).unwrap()
    }

    #[test]
    fn sizes_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir(r.join("a")).unwrap();
        fs::create_dir(r.join("a/b")).unwrap();
        write_file(&r.join("top.bin"), 10_000);
        write_file(&r.join("a/one.bin"), 5_000);
        write_file(&r.join("a/b/two.bin"), 70_000);
        let t = run(r, &ScanOptions::default());

        let root = t.root();
        assert_eq!(root.kind, Kind::Dir);
        assert_eq!(root.items, 5); // a, a/b, top.bin, a/one.bin, a/b/two.bin
        let apparent_files = 10_000 + 5_000 + 70_000;
        let dirs_apparent: u64 = [r, &r.join("a"), &r.join("a/b")]
            .iter()
            .map(|p| fs::symlink_metadata(p).unwrap().len())
            .sum();
        assert_eq!(root.apparent, apparent_files + dirs_apparent);
        let expected_alloc: u64 = ["", "a", "a/b", "top.bin", "a/one.bin", "a/b/two.bin"]
            .iter()
            .map(|p| alloc(&r.join(p)))
            .sum();
        assert_eq!(root.size, expected_alloc);

        let a = t.find(&r.join("a")).unwrap();
        assert_eq!(t.node(a).items, 3);
        let two = t.find(&r.join("a/b/two.bin")).unwrap();
        assert_eq!(t.node(two).apparent, 70_000);
        assert_eq!(t.node(two).size, alloc(&r.join("a/b/two.bin")));
        assert_eq!(t.path(two), r.join("a/b/two.bin"));
        assert_eq!(t.ancestry(two).len(), 4);

        // Children are sorted largest first: "a" (75k+) before top.bin (10k).
        let names: Vec<_> = root
            .children
            .iter()
            .map(|&c| t.node(c).name.as_str())
            .collect();
        assert_eq!(names, ["a", "top.bin"]);
    }

    #[test]
    fn sparse_file_allocated_vs_apparent() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sparse");
        let f = fs::File::create(&p).unwrap();
        f.set_len(50 * 1024 * 1024).unwrap();
        drop(f);
        let t = run(tmp.path(), &ScanOptions::default());
        let n = t.node(t.find(&p).unwrap());
        assert_eq!(n.apparent, 50 * 1024 * 1024);
        assert!(
            n.size < n.apparent,
            "sparse file should allocate less than its length"
        );
    }

    #[test]
    fn hard_links_counted_once() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir(r.join("x")).unwrap();
        write_file(&r.join("x/orig"), 40_000);
        fs::hard_link(r.join("x/orig"), r.join("link1")).unwrap();
        fs::hard_link(r.join("x/orig"), r.join("x/link2")).unwrap();
        let t = run(r, &ScanOptions::default());
        let file_alloc = alloc(&r.join("x/orig"));
        let total_files: u64 = t
            .nodes
            .iter()
            .filter(|n| n.kind == Kind::File)
            .map(|n| n.size)
            .sum();
        assert_eq!(total_files, file_alloc);
        let total_apparent: u64 = t
            .nodes
            .iter()
            .filter(|n| n.kind == Kind::File)
            .map(|n| n.apparent)
            .sum();
        assert_eq!(total_apparent, 40_000);
        // All three names are still listed.
        assert_eq!(t.nodes.iter().filter(|n| n.kind == Kind::File).count(), 3);
    }

    #[test]
    fn symlinks_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_file(&outside.path().join("big"), 200_000);
        let r = tmp.path();
        std::os::unix::fs::symlink(outside.path(), r.join("dirlink")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("big"), r.join("filelink")).unwrap();
        let t = run(r, &ScanOptions::default());
        for name in ["dirlink", "filelink"] {
            let id = t.find(&r.join(name)).unwrap();
            let n = t.node(id);
            assert_eq!(n.kind, Kind::Symlink);
            assert!(n.children.is_empty());
            assert!(
                n.apparent < 1000,
                "link counted with its own size, got {}",
                n.apparent
            );
        }
        assert!(t.root().apparent < 100_000);
        assert_eq!(t.root().items, 2);
    }

    #[test]
    fn exclusion_skips_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("keep/inner")).unwrap();
        fs::create_dir_all(r.join("skip/inner")).unwrap();
        write_file(&r.join("keep/inner/f"), 1000);
        write_file(&r.join("skip/inner/f"), 900_000);
        let opts = ScanOptions {
            excluded: vec![r.join("skip")],
            ..Default::default()
        };
        let t = run(r, &opts);
        assert!(t.find(&r.join("skip")).is_none());
        assert!(t.find(&r.join("keep/inner/f")).is_some());
        assert_eq!(t.root().items, 3);
        assert!(t.root().apparent < 900_000);
        // Excluding the root itself is ignored.
        let opts = ScanOptions {
            excluded: vec![r.to_path_buf()],
            ..Default::default()
        };
        assert_eq!(run(r, &opts).root().items, 6);
    }

    #[test]
    fn unreadable_dir_flagged() {
        // Root ignores permissions; skip the assertion there.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        write_file(&locked.join("f"), 10);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let t = run(tmp.path(), &ScanOptions::default());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let n = t.node(t.find(&locked).unwrap());
        assert!(n.error);
        assert_eq!(t.errors, 1);
    }

    #[test]
    fn cancel_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(&tmp.path().join("f"), 10);
        let p = Progress::default();
        p.cancel();
        assert!(matches!(
            scan(tmp.path(), &ScanOptions::default(), &p),
            Err(ScanError::Cancelled)
        ));
    }

    #[test]
    fn not_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("f");
        write_file(&f, 1);
        assert!(matches!(
            scan(&f, &ScanOptions::default(), &Progress::default()),
            Err(ScanError::NotADirectory)
        ));
    }

    #[test]
    fn remove_updates_ancestors() {
        let mut t = ScanTree::synthetic(100);
        let a = t.add_synthetic(ROOT, "a", Kind::Dir, 60);
        let f = t.add_synthetic(a, "f", Kind::File, 50);
        t.nodes[ROOT].items = 2;
        t.nodes[a].items = 1;
        assert!(t.remove(f));
        assert_eq!(t.node(a).size, 10);
        assert_eq!(t.root().size, 50);
        assert_eq!(t.root().items, 1);
        assert!(t.node(a).children.is_empty());
        assert!(!t.remove(f));
        assert!(!t.remove(ROOT));
    }
}
