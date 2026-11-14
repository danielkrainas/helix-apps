//! Session, draft and recent-document persistence.
//!
//! Layout inside the app data directory:
//! - `session.json`: [`Session`]
//! - `drafts/<id>.txt`: unsaved buffer contents (UTF-8, `\n` line ends)
//! - `recent.json`: list of recently used URIs, newest first

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabState {
    /// Draft id (also names the draft file).
    pub id: String,
    /// Location of the document, if it has one.
    #[serde(default)]
    pub uri: Option<String>,
    /// Whether `drafts/<id>.txt` holds unsaved content.
    #[serde(default)]
    pub draft: bool,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default)]
    pub bom: bool,
    #[serde(default = "default_line_ending")]
    pub line_ending: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub column: u32,
}

fn default_encoding() -> String {
    "UTF-8".into()
}

fn default_line_ending() -> String {
    "lf".into()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    pub tabs: Vec<TabState>,
    #[serde(default)]
    pub active: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub windows: Vec<WindowState>,
}

impl Session {
    pub fn draft_ids(&self) -> impl Iterator<Item = &str> {
        self.windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .filter(|t| t.draft)
            .map(|t| t.id.as_str())
    }

    pub fn has_drafts(&self) -> bool {
        self.draft_ids().next().is_some()
    }
}

/// Storage rooted at a directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Writes `data` atomically (temp file + rename).
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    fn session_path(&self) -> PathBuf {
        self.root.join("session.json")
    }

    fn recent_path(&self) -> PathBuf {
        self.root.join("recent.json")
    }

    fn drafts_dir(&self) -> PathBuf {
        self.root.join("drafts")
    }

    pub fn draft_path(&self, id: &str) -> Option<PathBuf> {
        valid_id(id).then(|| self.drafts_dir().join(format!("{id}.txt")))
    }

    pub fn load_session(&self) -> Session {
        std::fs::read(self.session_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_session(&self, s: &Session) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
        write_atomic(&self.session_path(), &json)
    }

    pub fn write_draft(&self, id: &str, text: &str) -> io::Result<()> {
        let path = self
            .draft_path(id)
            .ok_or_else(|| io::Error::other("invalid draft id"))?;
        write_atomic(&path, text.as_bytes())
    }

    pub fn read_draft(&self, id: &str) -> Option<String> {
        std::fs::read_to_string(self.draft_path(id)?).ok()
    }

    /// Deletes draft files that `session` does not reference.
    pub fn prune_drafts(&self, session: &Session) {
        let keep: Vec<&str> = session.draft_ids().collect();
        let Ok(entries) = std::fs::read_dir(self.drafts_dir()) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if !keep.contains(&stem) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Removes the session file and all drafts.
    pub fn clear(&self) {
        let _ = std::fs::remove_file(self.session_path());
        self.prune_drafts(&Session::default());
    }

    pub fn load_recent(&self) -> Vec<String> {
        std::fs::read(self.recent_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_recent(&self, list: &[String]) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(list).map_err(io::Error::other)?;
        write_atomic(&self.recent_path(), &json)
    }
}

/// Moves `uri` to the front of `list`, capped at [`MAX_RECENT`].
pub fn push_recent(list: &mut Vec<String>, uri: &str) {
    list.retain(|u| u != uri);
    list.insert(0, uri.to_string());
    list.truncate(MAX_RECENT);
}

/// A new unique draft id.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{:x}-{:x}-{:x}",
        nanos,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("helix-te-test-{name}-{}", new_id()));
        (Store::new(&dir), dir)
    }

    fn sample() -> Session {
        Session {
            windows: vec![WindowState {
                tabs: vec![
                    TabState {
                        id: new_id(),
                        uri: Some("file:///tmp/a.txt".into()),
                        draft: false,
                        encoding: "windows-1252".into(),
                        bom: false,
                        line_ending: "crlf".into(),
                        line: 4,
                        column: 2,
                    },
                    TabState {
                        id: new_id(),
                        draft: true,
                        ..Default::default()
                    },
                ],
                active: 1,
            }],
        }
    }

    #[test]
    fn session_roundtrip() {
        let (store, dir) = temp_store("session");
        assert_eq!(store.load_session(), Session::default());
        let s = sample();
        store.save_session(&s).unwrap();
        assert_eq!(store.load_session(), s);
        assert!(s.has_drafts());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tolerant_parsing() {
        let s: Session = serde_json::from_str(r#"{"windows":[{"tabs":[{"id":"x"}]}]}"#).unwrap();
        let t = &s.windows[0].tabs[0];
        assert_eq!(t.encoding, "UTF-8");
        assert_eq!(t.line_ending, "lf");
        assert_eq!(s.windows[0].active, 0);
    }

    #[test]
    fn drafts_and_pruning() {
        let (store, dir) = temp_store("drafts");
        let s = sample();
        let kept = &s.windows[0].tabs[1].id;
        let stale = new_id();
        store.write_draft(kept, "unsaved ✓\n").unwrap();
        store.write_draft(&stale, "old").unwrap();
        store.prune_drafts(&s);
        assert_eq!(store.read_draft(kept).as_deref(), Some("unsaved ✓\n"));
        assert_eq!(store.read_draft(&stale), None);
        store.save_session(&s).unwrap();
        store.clear();
        assert_eq!(store.read_draft(kept), None);
        assert_eq!(store.load_session(), Session::default());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_path_traversal_ids() {
        let (store, _) = temp_store("ids");
        assert!(store.draft_path("../evil").is_none());
        assert!(store.draft_path("").is_none());
        assert!(store.write_draft("a/b", "x").is_err());
    }

    #[test]
    fn recent_list() {
        let (store, dir) = temp_store("recent");
        let mut list = store.load_recent();
        for i in 0..25 {
            push_recent(&mut list, &format!("file:///f{i}"));
        }
        push_recent(&mut list, "file:///f10");
        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0], "file:///f10");
        assert_eq!(list.iter().filter(|u| *u == "file:///f10").count(), 1);
        store.save_recent(&list).unwrap();
        assert_eq!(store.load_recent(), list);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ids_unique() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(valid_id(&a));
    }
}
