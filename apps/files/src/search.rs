//! Recursive file name search (runs on a worker thread).

use gtk::gio;
use gtk::gio::prelude::*;

/// One result, plain data so it can cross threads.
#[derive(Debug, Clone)]
pub struct Hit {
    pub uri: String,
    pub name: String,
    pub display_name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: i64,
    pub content_type: String,
}

/// Normalized query: lowercase words that must all occur in the name.
#[derive(Debug, Clone)]
pub struct Query {
    words: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        Query {
            words: text.split_whitespace().map(str::to_lowercase).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        !self.words.is_empty() && self.words.iter().all(|w| lower.contains(w.as_str()))
    }
}

/// How deep to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Recursive,
    ChildrenOnly,
}

/// Whether `recursive-search` (`always`, `local-only`, `never`) allows a
/// recursive search of `root`.
pub fn depth_for(policy: &str, is_native: bool) -> Depth {
    match policy {
        "always" => Depth::Recursive,
        "never" => Depth::ChildrenOnly,
        _ if is_native => Depth::Recursive,
        _ => Depth::ChildrenOnly,
    }
}

const ATTRS: &str = "standard::name,standard::display-name,standard::type,standard::size,standard::is-hidden,standard::is-backup,standard::fast-content-type,time::modified";

/// Walks `root` and calls `sink` with batches of hits. Stops early when
/// `cancel` is triggered.
pub fn run(
    root: &gio::File,
    query: &Query,
    depth: Depth,
    show_hidden: bool,
    cancel: &gio::Cancellable,
    sink: &mut dyn FnMut(Vec<Hit>),
) {
    let mut batch = Vec::new();
    let mut last_flush = std::time::Instant::now();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        if cancel.is_cancelled() {
            return;
        }
        let Ok(en) = dir.enumerate_children(ATTRS, gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, Some(cancel)) else {
            continue;
        };
        while let Ok(Some(info)) = en.next_file(Some(cancel)) {
            if !show_hidden && (info.is_hidden() || info.is_backup()) {
                continue;
            }
            let child = dir.child(info.name());
            let is_dir = info.file_type() == gio::FileType::Directory;
            let display = info.display_name().to_string();
            if query.matches(&display) {
                batch.push(Hit {
                    uri: child.uri().to_string(),
                    name: info.name().to_string_lossy().into_owned(),
                    display_name: display,
                    is_dir,
                    size: info.size().max(0) as u64,
                    mtime: info.modification_date_time().map(|d| d.to_unix()).unwrap_or(0),
                    content_type: info
                        .attribute_string("standard::fast-content-type")
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "application/octet-stream".into()),
                });
            }
            if is_dir && depth == Depth::Recursive {
                stack.push(child);
            }
            if !batch.is_empty() && (batch.len() >= 100 || last_flush.elapsed().as_millis() > 150) {
                sink(std::mem::take(&mut batch));
                last_flush = std::time::Instant::now();
            }
        }
    }
    if !batch.is_empty() && !cancel.is_cancelled() {
        sink(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::tests::TempDir;

    #[test]
    fn query_matching() {
        let q = Query::new("  Rep 2024 ");
        assert!(q.matches("Annual report 2024.pdf"));
        assert!(!q.matches("report 2023.pdf"));
        assert!(!Query::new("").matches("x"));
        assert!(Query::new("ÄB").matches("xäbx"));
    }

    #[test]
    fn depth_policy() {
        assert_eq!(depth_for("always", false), Depth::Recursive);
        assert_eq!(depth_for("local-only", true), Depth::Recursive);
        assert_eq!(depth_for("local-only", false), Depth::ChildrenOnly);
        assert_eq!(depth_for("never", true), Depth::ChildrenOnly);
    }

    fn names(root: &gio::File, q: &str, depth: Depth, hidden: bool) -> Vec<String> {
        let mut all = Vec::new();
        run(
            root,
            &Query::new(q),
            depth,
            hidden,
            &gio::Cancellable::new(),
            &mut |b| all.extend(b.into_iter().map(|h| h.name)),
        );
        all.sort();
        all
    }

    #[test]
    fn walks_tree() {
        let t = TempDir::new("search");
        t.write("notes.txt", "");
        t.write("a/Notes-old.md", "");
        t.write("a/b/c/deep notes", "");
        t.write("a/other.txt", "");
        t.write(".hidden/notes", "");
        t.write("a/.notes-secret", "");
        let root = t.file("");
        assert_eq!(
            names(&root, "notes", Depth::Recursive, false),
            ["Notes-old.md", "deep notes", "notes.txt"]
        );
        assert_eq!(names(&root, "notes", Depth::ChildrenOnly, false), ["notes.txt"]);
        assert_eq!(names(&root, "notes", Depth::Recursive, true).len(), 5);
    }
}
