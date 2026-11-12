//! Per-document view state (last page, zoom, …) persisted as JSON.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MAX_ENTRIES: usize = 1000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocState {
    /// 0-based page index.
    pub page: u32,
    /// `ZoomMode::as_str` value.
    pub zoom_mode: String,
    /// Zoom factor (1.0 = 100 %), meaningful for the "free" mode.
    pub zoom: f64,
    pub rotation: u32,
    pub dual: bool,
    pub sidebar: bool,
    /// Seconds since the Unix epoch.
    pub last_used: i64,
}

impl Default for DocState {
    fn default() -> Self {
        DocState {
            page: 0,
            zoom_mode: "automatic".into(),
            zoom: 1.0,
            rotation: 0,
            dual: false,
            sidebar: false,
            last_used: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct StateStore {
    path: PathBuf,
    entries: HashMap<String, DocState>,
}

impl StateStore {
    /// Loads the store from `path`; a missing or corrupt file gives an empty
    /// store.
    pub fn load(path: &Path) -> Self {
        let entries = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<HashMap<String, DocState>>(&b).ok())
            .unwrap_or_default();
        StateStore {
            path: path.to_path_buf(),
            entries,
        }
    }

    pub fn get(&self, uri: &str) -> Option<&DocState> {
        self.entries.get(uri)
    }

    /// Records state for `uri`, pruning the least recently used entries
    /// beyond [`MAX_ENTRIES`].
    pub fn set(&mut self, uri: &str, state: DocState) {
        self.entries.insert(uri.to_string(), state);
        if self.entries.len() > MAX_ENTRIES {
            let mut by_age: Vec<(i64, String)> = self
                .entries
                .iter()
                .map(|(k, v)| (v.last_used, k.clone()))
                .collect();
            by_age.sort();
            let excess = self.entries.len() - MAX_ENTRIES;
            for (_, k) in by_age.into_iter().take(excess) {
                self.entries.remove(&k);
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Writes the store atomically (temp file + rename).
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&self.entries).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &self.path)
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/state.json");
        let mut s = StateStore::load(&path);
        assert_eq!(s.len(), 0);
        let st = DocState {
            page: 7,
            zoom_mode: "free".into(),
            zoom: 1.5,
            rotation: 90,
            dual: true,
            sidebar: true,
            last_used: 42,
        };
        s.set("file:///a.pdf", st.clone());
        s.save().unwrap();
        let s2 = StateStore::load(&path);
        assert_eq!(s2.get("file:///a.pdf"), Some(&st));
        assert!(s2.get("file:///b.pdf").is_none());
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn corrupt_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(StateStore::load(&path).len(), 0);
        // Missing fields take defaults.
        std::fs::write(&path, br#"{"file:///x.pdf": {"page": 3}}"#).unwrap();
        let s = StateStore::load(&path);
        let st = s.get("file:///x.pdf").unwrap();
        assert_eq!(st.page, 3);
        assert_eq!(st.zoom_mode, "automatic");
        assert_eq!(st.zoom, 1.0);
    }

    #[test]
    fn prunes_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = StateStore::load(&dir.path().join("s.json"));
        for i in 0..(MAX_ENTRIES + 5) {
            s.set(
                &format!("file:///{i}.pdf"),
                DocState {
                    last_used: i as i64,
                    ..Default::default()
                },
            );
        }
        assert_eq!(s.len(), MAX_ENTRIES);
        assert!(s.get("file:///0.pdf").is_none());
        assert!(s.get("file:///4.pdf").is_none());
        assert!(s.get("file:///5.pdf").is_some());
    }
}
