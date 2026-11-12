//! Browsing history store (JSON, in memory, capped).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub visits: u32,
    /// Unix seconds of the last visit.
    #[serde(default)]
    pub last_visit: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    entries: Vec<Entry>,
}

pub struct History {
    path: Option<PathBuf>,
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
}

/// Whether a URL is worth recording.
pub fn recordable(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://") || l.starts_with("file://")
}

impl History {
    /// In-memory history (private windows, tests).
    #[cfg(test)]
    pub fn memory() -> Self {
        History {
            path: None,
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn load(path: &Path) -> Self {
        let file: File = crate::store::load(path);
        let mut h = History {
            path: Some(path.to_owned()),
            entries: file.entries,
            index: HashMap::new(),
        };
        h.entries.retain(|e| recordable(&e.url));
        h.reindex();
        h
    }

    fn reindex(&mut self) {
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.url.clone(), i))
            .collect();
    }

    pub fn serialize(&self) -> Option<(PathBuf, String)> {
        let path = self.path.clone()?;
        Some((
            path,
            crate::store::to_json(&File {
                entries: self.entries.clone(),
            }),
        ))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn get(&self, url: &str) -> Option<&Entry> {
        self.index.get(url).map(|&i| &self.entries[i])
    }

    /// Records a visit at time `now`. Returns false when the URL is ignored.
    pub fn add_visit(&mut self, url: &str, title: &str, now: i64) -> bool {
        if !recordable(url) {
            return false;
        }
        match self.index.get(url) {
            Some(&i) => {
                let e = &mut self.entries[i];
                e.visits = e.visits.saturating_add(1);
                e.last_visit = now;
                if !title.is_empty() {
                    e.title = title.to_string();
                }
            }
            None => {
                self.entries.push(Entry {
                    url: url.into(),
                    title: title.into(),
                    visits: 1,
                    last_visit: now,
                });
                self.index.insert(url.into(), self.entries.len() - 1);
                if self.entries.len() > MAX_ENTRIES {
                    self.prune();
                }
            }
        }
        true
    }

    pub fn set_title(&mut self, url: &str, title: &str) -> bool {
        match self.index.get(url) {
            Some(&i) if !title.is_empty() && self.entries[i].title != title => {
                self.entries[i].title = title.to_string();
                true
            }
            _ => false,
        }
    }

    fn prune(&mut self) {
        self.entries
            .sort_by_key(|e| std::cmp::Reverse(e.last_visit));
        self.entries.truncate(MAX_ENTRIES);
        self.reindex();
    }

    pub fn remove(&mut self, url: &str) -> bool {
        if self.index.remove(url).is_none() {
            return false;
        }
        self.entries.retain(|e| e.url != url);
        self.reindex();
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }

    /// Newest first, filtered by `query` terms.
    pub fn recent(&self, query: &str, limit: usize) -> Vec<Entry> {
        let terms = terms(query);
        let mut v: Vec<&Entry> = self.entries.iter().filter(|e| matches(e, &terms)).collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.last_visit));
        v.into_iter().take(limit).cloned().collect()
    }

    /// Most visited entries, by frecency.
    pub fn most_visited(&self, limit: usize, now: i64) -> Vec<Entry> {
        let mut v: Vec<(f64, &Entry)> =
            self.entries.iter().map(|e| (frecency(e, now), e)).collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v.into_iter().take(limit).map(|(_, e)| e.clone()).collect()
    }

    /// Address-bar suggestions: every term must match title or URL.
    pub fn suggest(&self, query: &str, limit: usize, now: i64) -> Vec<Entry> {
        let terms = terms(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let first = &terms[0];
        let mut v: Vec<(f64, &Entry)> = self
            .entries
            .iter()
            .filter(|e| matches(e, &terms))
            .map(|e| {
                let mut score = frecency(e, now);
                let host = strip_scheme(&e.url).to_lowercase();
                if host.starts_with(first.as_str()) || host.starts_with(&format!("www.{first}")) {
                    score *= 4.0;
                }
                (score, e)
            })
            .collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v.into_iter().take(limit).map(|(_, e)| e.clone()).collect()
    }
}

pub fn strip_scheme(url: &str) -> &str {
    url.split_once("://").map(|(_, r)| r).unwrap_or(url)
}

fn terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

fn matches(e: &Entry, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let url = e.url.to_lowercase();
    let title = e.title.to_lowercase();
    terms
        .iter()
        .all(|t| url.contains(t.as_str()) || title.contains(t.as_str()))
}

/// Visits weighted by how recently the page was last seen.
pub fn frecency(e: &Entry, now: i64) -> f64 {
    let age_days = ((now - e.last_visit).max(0) as f64) / 86_400.0;
    let weight = if age_days < 4.0 {
        100.0
    } else if age_days < 14.0 {
        70.0
    } else if age_days < 31.0 {
        50.0
    } else if age_days < 90.0 {
        30.0
    } else {
        10.0
    };
    e.visits.max(1) as f64 * weight
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn visits_and_titles() {
        let mut h = History::memory();
        assert!(h.add_visit("https://a.org/", "A", 10));
        assert!(h.add_visit("https://a.org/", "", 20));
        assert!(!h.add_visit("about:blank", "", 20));
        let e = h.get("https://a.org/").unwrap();
        assert_eq!((e.visits, e.last_visit, e.title.as_str()), (2, 20, "A"));
        assert!(h.set_title("https://a.org/", "New"));
        assert!(!h.set_title("https://a.org/", "New"));
        assert_eq!(h.get("https://a.org/").unwrap().title, "New");
    }

    #[test]
    fn suggest_ranks_and_filters() {
        let now = 100 * DAY;
        let mut h = History::memory();
        for _ in 0..5 {
            h.add_visit("https://docs.rs/gtk4", "gtk4 docs", now - 200 * DAY);
        }
        h.add_visit("https://gtk.org/", "The GTK Project", now - DAY);
        h.add_visit("https://example.com/", "Example", now);
        let s = h.suggest("gtk", 10, now);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].url, "https://gtk.org/");
        let s = h.suggest("gtk docs", 10, now);
        assert_eq!(s.len(), 1);
        assert!(h.suggest("", 10, now).is_empty());
        assert_eq!(h.suggest("EXAMPLE", 10, now).len(), 1);
    }

    #[test]
    fn recent_remove_clear_and_cap() {
        let mut h = History::memory();
        h.add_visit("https://a/", "", 1);
        h.add_visit("https://b/", "", 2);
        let r = h.recent("", 10);
        assert_eq!(r[0].url, "https://b/");
        assert!(h.remove("https://b/"));
        assert!(!h.remove("https://b/"));
        assert_eq!(h.len(), 1);
        h.clear();
        assert_eq!(h.len(), 0);
        for i in 0..(MAX_ENTRIES + 10) {
            h.add_visit(&format!("https://x/{i}"), "", i as i64);
        }
        assert_eq!(h.len(), MAX_ENTRIES);
        assert!(h.get("https://x/0").is_none());
        assert!(h.get(&format!("https://x/{}", MAX_ENTRIES + 9)).is_some());
    }

    #[test]
    fn persists() {
        let dir = crate::store::temp_dir("history");
        let path = dir.join("history.json");
        let mut h = History::load(&path);
        h.add_visit("https://a.org/", "A", 5);
        let (p, json) = h.serialize().unwrap();
        crate::store::write_atomic(&p, &json).unwrap();
        let h2 = History::load(&path);
        assert_eq!(h2.get("https://a.org/").unwrap().title, "A");
        assert!(History::memory().serialize().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
