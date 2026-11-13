//! Persistent link groups: contacts (identified by book + contact UID) that
//! the user linked into one individual.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// (address-book source UID, contact UID).
pub type Key = (String, String);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Links {
    pub groups: Vec<Vec<Key>>,
}

impl Links {
    pub fn load(path: &Path) -> Links {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp: PathBuf = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }

    #[cfg(test)]
    pub fn group_of(&self, key: &Key) -> Option<&Vec<Key>> {
        self.groups.iter().find(|g| g.contains(key))
    }

    /// Links all `keys` (and anything already linked to them) into one group.
    pub fn link(&mut self, keys: &[Key]) {
        let mut merged: Vec<Key> = Vec::new();
        let mut rest = Vec::new();
        for g in self.groups.drain(..) {
            if g.iter().any(|k| keys.contains(k)) {
                merged.extend(g);
            } else {
                rest.push(g);
            }
        }
        merged.extend(keys.iter().cloned());
        let mut seen = Vec::new();
        merged.retain(|k| {
            if seen.contains(k) {
                false
            } else {
                seen.push(k.clone());
                true
            }
        });
        rest.push(merged);
        self.groups = rest;
        self.normalize();
    }

    /// Dissolves the group containing `key`.
    pub fn unlink(&mut self, key: &Key) {
        self.groups.retain(|g| !g.contains(key));
    }

    /// Removes a contact from its group (e.g. after deletion).
    pub fn forget(&mut self, key: &Key) {
        for g in &mut self.groups {
            g.retain(|k| k != key);
        }
        self.normalize();
    }

    /// Drops members that no longer exist (`exists` returns false).
    pub fn prune(&mut self, exists: impl Fn(&Key) -> bool) -> bool {
        let before = self.clone();
        for g in &mut self.groups {
            g.retain(&exists);
        }
        self.normalize();
        *self != before
    }

    fn normalize(&mut self) {
        self.groups.retain(|g| g.len() > 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(b: &str, u: &str) -> Key {
        (b.into(), u.into())
    }

    #[test]
    fn link_merges_overlapping_groups() {
        let mut l = Links::default();
        l.link(&[k("a", "1"), k("a", "2")]);
        l.link(&[k("b", "9"), k("c", "5")]);
        l.link(&[k("a", "2"), k("b", "9")]);
        assert_eq!(l.groups.len(), 1);
        assert_eq!(l.groups[0].len(), 4);
        assert!(l.group_of(&k("c", "5")).is_some());
    }

    #[test]
    fn unlink_and_forget() {
        let mut l = Links::default();
        l.link(&[k("a", "1"), k("a", "2"), k("a", "3")]);
        l.forget(&k("a", "3"));
        assert_eq!(l.groups[0].len(), 2);
        l.forget(&k("a", "2"));
        assert!(l.groups.is_empty(), "single-member groups dissolve");
        l.link(&[k("a", "1"), k("a", "2")]);
        l.unlink(&k("a", "1"));
        assert!(l.groups.is_empty());
    }

    #[test]
    fn prune_and_persist() {
        let dir = std::env::temp_dir().join(format!("helix-contacts-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("links.json");
        let mut l = Links::default();
        l.link(&[k("a", "1"), k("a", "2"), k("b", "3")]);
        assert!(l.prune(|key| key.1 != "2"));
        assert!(!l.prune(|_| true));
        l.save(&path).unwrap();
        assert_eq!(Links::load(&path), l);
        assert_eq!(Links::load(&dir.join("missing.json")), Links::default());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
