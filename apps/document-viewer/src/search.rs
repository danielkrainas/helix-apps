//! Search results: geometry conversion and navigation.

use crate::layout::Rect;

/// Converts a poppler rectangle with a bottom-left origin (as returned by
/// `poppler_page_find_text` and the link / annotation mappings) to a
/// top-left-origin page rectangle.
pub fn flip_rect(x1: f64, y1: f64, x2: f64, y2: f64, page_h: f64) -> Rect {
    Rect::from_corners(x1, page_h - y1, x2, page_h - y2)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub page: usize,
    /// Top-left-origin page points.
    pub rect: Rect,
    /// Line of text around the match.
    pub context: String,
}

#[derive(Debug, Default)]
pub struct Results {
    hits: Vec<Hit>,
    current: Option<usize>,
}

impl Results {
    pub fn clear(&mut self) {
        self.hits.clear();
        self.current = None;
    }

    /// Adds the hits of one page, keeping hits ordered by page.
    /// Returns the index of the first inserted hit.
    pub fn add_page(&mut self, page: usize, hits: Vec<Hit>) -> usize {
        let at = self.hits.partition_point(|h| h.page <= page);
        let n = hits.len();
        self.hits.splice(at..at, hits);
        if let Some(c) = self.current.as_mut() {
            if *c >= at {
                *c += n;
            }
        }
        at
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    pub fn len(&self) -> usize {
        self.hits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn set_current(&mut self, i: Option<usize>) {
        self.current = i.filter(|i| *i < self.hits.len());
    }

    /// Next match: after the current one, or the first one on/after
    /// `page` when there is no current match. Wraps around.
    pub fn next(&mut self, page: usize) -> Option<usize> {
        if self.hits.is_empty() {
            return None;
        }
        let n = self.hits.len();
        let i = match self.current {
            Some(c) => (c + 1) % n,
            None => self.hits.iter().position(|h| h.page >= page).unwrap_or(0),
        };
        self.current = Some(i);
        Some(i)
    }

    /// Previous match, symmetric to [`Results::next`].
    pub fn prev(&mut self, page: usize) -> Option<usize> {
        if self.hits.is_empty() {
            return None;
        }
        let n = self.hits.len();
        let i = match self.current {
            Some(c) => (c + n - 1) % n,
            None => self
                .hits
                .iter()
                .rposition(|h| h.page <= page)
                .unwrap_or(n - 1),
        };
        self.current = Some(i);
        Some(i)
    }

    /// Indices of hits on `page`.
    pub fn on_page(&self, page: usize) -> std::ops::Range<usize> {
        let start = self.hits.partition_point(|h| h.page < page);
        let end = self.hits.partition_point(|h| h.page <= page);
        start..end
    }
}

/// Shortens a line of context to at most `max` characters around the first
/// occurrence of `query` (case-insensitive), adding ellipses.
pub fn snippet(line: &str, query: &str, max: usize) -> String {
    let line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= max {
        return line;
    }
    let lower: Vec<char> = line.to_lowercase().chars().collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let pos = if q.is_empty() || lower.len() != chars.len() {
        0
    } else {
        lower
            .windows(q.len())
            .position(|w| w == q.as_slice())
            .unwrap_or(0)
    };
    let half = max.saturating_sub(q.len()) / 2;
    let start = pos.saturating_sub(half).min(chars.len() - max);
    let end = (start + max).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(page: usize) -> Hit {
        Hit {
            page,
            rect: Rect::default(),
            context: String::new(),
        }
    }

    #[test]
    fn flip() {
        let r = flip_rect(10.0, 700.0, 50.0, 720.0, 800.0);
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 80.0, 40.0, 20.0));
    }

    #[test]
    fn ordering_and_navigation() {
        let mut r = Results::default();
        assert_eq!(r.next(0), None);
        r.add_page(5, vec![hit(5), hit(5)]);
        r.add_page(1, vec![hit(1)]);
        r.add_page(9, vec![hit(9)]);
        let pages: Vec<usize> = r.hits().iter().map(|h| h.page).collect();
        assert_eq!(pages, vec![1, 5, 5, 9]);
        assert_eq!(r.on_page(5), 1..3);
        assert_eq!(r.on_page(4), 1..1);
        // First "next" starts at the current page.
        assert_eq!(r.next(4), Some(1));
        assert_eq!(r.next(4), Some(2));
        assert_eq!(r.next(4), Some(3));
        assert_eq!(r.next(4), Some(0)); // wraps
        assert_eq!(r.prev(0), Some(3));
        r.set_current(None);
        assert_eq!(r.prev(6), Some(2));
        r.set_current(None);
        assert_eq!(r.next(10), Some(0));
        // Inserting before the current hit keeps it pointing at the same hit.
        r.set_current(Some(3));
        r.add_page(0, vec![hit(0)]);
        assert_eq!(r.current(), Some(4));
        assert_eq!(r.hits()[4].page, 9);
        r.set_current(Some(99));
        assert_eq!(r.current(), None);
    }

    #[test]
    fn snippets() {
        assert_eq!(snippet("  short   line ", "line", 40), "short line");
        let long = "a ".repeat(50) + "needle" + &" b".repeat(50);
        let s = snippet(&long, "NEEDLE", 20);
        assert!(s.contains("needle"));
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert_eq!(s.chars().count(), 22);
    }
}
