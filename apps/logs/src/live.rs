//! Merging live (followed) journal entries into the loaded view.
//!
//! The view is filled by a paged query; a second `journalctl --follow`
//! process streams entries written after the query started. The two streams
//! can overlap (the paged query reads lazily and may itself reach entries
//! written after it started), so entries are de-duplicated by `__CURSOR`.

use crate::journal::Entry;
use std::collections::HashSet;

/// Where live entries go in the list model.
#[derive(Debug, PartialEq)]
pub enum Placement {
    /// Insert at position 0, in the given order (newest first).
    Prepend(Vec<Entry>),
    /// Append at the end, in the given order (oldest first).
    Append(Vec<Entry>),
    /// Held back until the paged query has finished (see [`Live::finish`]).
    Deferred,
}

#[derive(Debug, Default)]
pub struct Live {
    seen: HashSet<String>,
    pending: Vec<Entry>,
}

fn cursor(e: &Entry) -> Option<&str> {
    e.get("__CURSOR")
}

impl Live {
    pub fn clear(&mut self) {
        self.seen.clear();
        self.pending.clear();
    }

    /// Keeps entries not shown yet and remembers their cursors.
    fn fresh(&mut self, entries: Vec<Entry>) -> Vec<Entry> {
        entries
            .into_iter()
            .filter(|e| match cursor(e) {
                Some(c) => self.seen.insert(c.to_string()),
                None => true,
            })
            .collect()
    }

    /// Filters a batch of the paged query.
    pub fn loaded(&mut self, entries: Vec<Entry>) -> Vec<Entry> {
        // Live entries still pending may now arrive through the query.
        if !self.pending.is_empty() {
            let cursors: HashSet<&str> = entries.iter().filter_map(cursor).collect();
            self.pending.retain(|p| cursor(p).is_none_or(|c| !cursors.contains(c)));
        }
        self.fresh(entries)
    }

    /// Places followed entries (oldest first, as journalctl emits them).
    /// Newest-first views get them on top immediately; oldest-first views
    /// can only append once every older entry is loaded (`query_done`).
    pub fn followed(&mut self, entries: Vec<Entry>, descending: bool, query_done: bool) -> Placement {
        if descending {
            let mut v = self.fresh(entries);
            v.reverse();
            return Placement::Prepend(v);
        }
        if query_done {
            return Placement::Append(self.fresh(entries));
        }
        // Not yet marked as seen: the paged query may still deliver them.
        self.pending.extend(entries);
        Placement::Deferred
    }

    /// Called when the paged query has finished: returns the deferred
    /// entries to append (oldest-first views).
    pub fn finish(&mut self) -> Vec<Entry> {
        let pending = std::mem::take(&mut self.pending);
        self.fresh(pending)
    }
}

/// Whether the view shows the edge where new entries appear: the top for
/// newest-first, the bottom for oldest-first. Only then does the view
/// follow new entries; otherwise its position is kept.
pub fn at_live_edge(descending: bool, value: f64, page_size: f64, upper: f64) -> bool {
    const SLACK: f64 = 2.0;
    if descending {
        value <= SLACK
    } else {
        upper - (value + page_size) <= SLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(c: &str) -> Entry {
        Entry::from_fields([("__CURSOR", c), ("MESSAGE", c)])
    }

    fn msgs(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.message()).collect()
    }

    #[test]
    fn newest_first_prepends_reversed_and_dedupes() {
        let mut l = Live::default();
        let shown = l.loaded(vec![e("c3"), e("c2"), e("c1")]);
        assert_eq!(msgs(&shown), ["c3", "c2", "c1"]);
        match l.followed(vec![e("c3"), e("c4"), e("c5")], true, false) {
            Placement::Prepend(v) => assert_eq!(msgs(&v), ["c5", "c4"]),
            p => panic!("{p:?}"),
        }
        // A later page that overlaps the live stream drops duplicates.
        assert_eq!(msgs(&l.loaded(vec![e("c5"), e("c0")])), ["c0"]);
    }

    #[test]
    fn oldest_first_defers_until_done() {
        let mut l = Live::default();
        l.loaded(vec![e("a"), e("b")]);
        assert_eq!(l.followed(vec![e("c"), e("d")], false, false), Placement::Deferred);
        // The paged query reaches "c" itself.
        assert_eq!(msgs(&l.loaded(vec![e("c")])), ["c"]);
        assert_eq!(msgs(&l.finish()), ["d"]);
        assert!(l.finish().is_empty());
        match l.followed(vec![e("d"), e("e")], false, true) {
            Placement::Append(v) => assert_eq!(msgs(&v), ["e"]),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn entries_without_cursor_are_kept() {
        let mut l = Live::default();
        let no_cursor = Entry::from_fields([("MESSAGE", "x")]);
        assert_eq!(l.loaded(vec![no_cursor.clone(), no_cursor.clone()]).len(), 2);
        l.clear();
        assert_eq!(msgs(&l.loaded(vec![e("a")])), ["a"]);
    }

    #[test]
    fn live_edge() {
        // Newest first: at the top.
        assert!(at_live_edge(true, 0.0, 100.0, 1000.0));
        assert!(!at_live_edge(true, 50.0, 100.0, 1000.0));
        // Oldest first: at the bottom (or everything fits).
        assert!(at_live_edge(false, 900.0, 100.0, 1000.0));
        assert!(at_live_edge(false, 0.0, 100.0, 50.0));
        assert!(!at_live_edge(false, 500.0, 100.0, 1000.0));
    }
}
