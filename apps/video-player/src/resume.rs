//! Per-file playback position memory, stored as JSON in the data dir.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Positions closer than this to the start or the end are not remembered.
pub const MARGIN_SECS: f64 = 10.0;
/// Positions past this fraction of the duration count as "finished".
pub const FINISHED_FRACTION: f64 = 0.95;
/// Maximum number of remembered files.
pub const MAX_ENTRIES: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub position: f64,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub updated: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Default)]
pub struct ResumeStore {
    path: Option<PathBuf>,
    entries: HashMap<String, Entry>,
}

/// Whether `position` is worth remembering for a video of `duration`.
pub fn worth_remembering(position: f64, duration: Option<f64>) -> bool {
    if !position.is_finite() || position < MARGIN_SECS {
        return false;
    }
    match duration {
        Some(d) if d.is_finite() && d > 0.0 => {
            position < d - MARGIN_SECS && position < d * FINISHED_FRACTION
        }
        _ => true,
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ResumeStore {
    /// Loads the store from `path`; a missing or corrupt file yields an
    /// empty store that will overwrite it on the next save.
    pub fn load(path: &Path) -> Self {
        let entries = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<FileFormat>(&bytes).ok())
            .map(|f| f.entries)
            .unwrap_or_default();
        ResumeStore { path: Some(path.to_path_buf()), entries }
    }

    /// Stored position for `uri`, if any.
    pub fn lookup(&self, uri: &str) -> Option<f64> {
        self.entries.get(uri).map(|e| e.position)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Records (or forgets, when not worth remembering) the position of
    /// `uri`. Returns whether the store changed.
    pub fn record(&mut self, uri: &str, position: f64, duration: Option<f64>, now: u64) -> bool {
        if !worth_remembering(position, duration) {
            return self.entries.remove(uri).is_some();
        }
        let entry = Entry { position, duration: duration.unwrap_or(0.0), updated: now };
        let changed = self.entries.get(uri) != Some(&entry);
        self.entries.insert(uri.to_string(), entry);
        self.prune();
        changed
    }

    pub fn forget(&mut self, uri: &str) -> bool {
        self.entries.remove(uri).is_some()
    }

    fn prune(&mut self) {
        if self.entries.len() <= MAX_ENTRIES {
            return;
        }
        let mut by_age: Vec<(u64, String)> =
            self.entries.iter().map(|(k, e)| (e.updated, k.clone())).collect();
        by_age.sort();
        let excess = self.entries.len() - MAX_ENTRIES;
        for (_, key) in by_age.into_iter().take(excess) {
            self.entries.remove(&key);
        }
    }

    /// Writes the store atomically (temp file + rename).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let data = FileFormat { version: 1, entries: self.entries.clone() };
        let json = serde_json::to_vec_pretty(&data).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        assert!(!worth_remembering(5.0, Some(600.0)));
        assert!(worth_remembering(30.0, Some(600.0)));
        assert!(!worth_remembering(595.0, Some(600.0)));
        // 95 % rule for long videos.
        assert!(!worth_remembering(5800.0, Some(6000.0)));
        assert!(worth_remembering(30.0, None));
        assert!(!worth_remembering(f64::NAN, None));
    }

    #[test]
    fn record_lookup_forget() {
        let mut s = ResumeStore::default();
        assert!(s.record("file:///a.mkv", 42.0, Some(600.0), 1));
        assert_eq!(s.lookup("file:///a.mkv"), Some(42.0));
        assert!(!s.record("file:///a.mkv", 42.0, Some(600.0), 1));
        // Near the end: entry removed.
        assert!(s.record("file:///a.mkv", 599.0, Some(600.0), 2));
        assert_eq!(s.lookup("file:///a.mkv"), None);
        s.record("file:///b.mkv", 20.0, None, 3);
        assert!(s.forget("file:///b.mkv"));
        assert!(!s.forget("file:///b.mkv"));
    }

    #[test]
    fn prune_oldest() {
        let mut s = ResumeStore::default();
        for i in 0..(MAX_ENTRIES + 5) {
            s.record(&format!("file:///{i}"), 20.0, None, i as u64);
        }
        assert_eq!(s.len(), MAX_ENTRIES);
        assert_eq!(s.lookup("file:///0"), None);
        assert_eq!(s.lookup("file:///4"), None);
        assert_eq!(s.lookup("file:///5"), Some(20.0));
    }

    #[test]
    fn persist_roundtrip_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("positions.json");
        let mut s = ResumeStore::load(&path);
        assert_eq!(s.len(), 0);
        s.record("file:///x%20y.webm", 123.5, Some(1000.0), 7);
        s.save().unwrap();
        let s2 = ResumeStore::load(&path);
        assert_eq!(s2.lookup("file:///x%20y.webm"), Some(123.5));

        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(ResumeStore::load(&path).len(), 0);
    }
}
