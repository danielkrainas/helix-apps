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
    /// Where the match is in `context` (byte range), if it was found.
    pub matched: Option<std::ops::Range<usize>>,
}

impl Hit {
    /// `context` as Pango markup with the match in bold, so it is marked
    /// by weight and not only by colour.
    pub fn context_markup(&self) -> String {
        let esc = |s: &str| gtk::glib::markup_escape_text(s).to_string();
        match &self.matched {
            Some(r) if self.context.get(r.clone()).is_some() => format!(
                "{}<b>{}</b>{}",
                esc(&self.context[..r.start]),
                esc(&self.context[r.clone()]),
                esc(&self.context[r.end..])
            ),
            _ => esc(&self.context),
        }
    }
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

/// Characters of context kept before the match, so that the match itself
/// stays visible in a narrow sidebar (the row is ellipsized at the end).
const LEFT_CONTEXT: usize = 18;

/// Shortens a line of context to at most `max` characters starting a few
/// words before the first occurrence of `query` (case-insensitive), adding
/// ellipses. Returns the snippet and the byte range of the match in it.
pub fn snippet(line: &str, query: &str, max: usize) -> (String, Option<std::ops::Range<usize>>) {
    let line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = line.chars().collect();
    let lower: Vec<char> = line.to_lowercase().chars().collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    // Character position of the match (lower-casing can change the length
    // of some characters; then the match position is unknown).
    let pos = if q.is_empty() || lower.len() != chars.len() {
        None
    } else {
        lower.windows(q.len()).position(|w| w == q.as_slice())
    };
    let mut start = match pos {
        Some(p) if p > LEFT_CONTEXT => {
            // Start at a word boundary within the left context.
            let from = p - LEFT_CONTEXT;
            (from..p)
                .find(|&i| i > 0 && chars[i - 1] == ' ')
                .unwrap_or(from)
        }
        _ => 0,
    };
    if chars.len() > max {
        start = start.min(chars.len() - max);
    } else {
        start = start.min(pos.unwrap_or(0));
    }
    let end = (start + max).min(chars.len());
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    let mut matched = None;
    for (i, c) in chars[start..end].iter().enumerate() {
        let at = start + i;
        if pos == Some(at) {
            let b = s.len();
            let m: String = chars[at..(at + q.len()).min(end)].iter().collect();
            matched = Some(b..b + m.len());
        }
        s.push(*c);
    }
    if end < chars.len() {
        s.push('…');
    }
    (s, matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(page: usize) -> Hit {
        Hit {
            page,
            rect: Rect::default(),
            context: String::new(),
            matched: None,
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
        let (s, m) = snippet("  short   line ", "line", 40);
        assert_eq!(s, "short line");
        assert_eq!(&s[m.unwrap()], "line");
        let long = "a ".repeat(50) + "needle" + &" b".repeat(50);
        let (s, m) = snippet(&long, "NEEDLE", 40);
        assert_eq!(&s[m.clone().unwrap()], "needle");
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert_eq!(s.chars().count(), 42);
        // Only a few words of left context, so the match is near the start
        // and survives end-ellipsizing in a narrow sidebar.
        assert!(s[..m.unwrap().start].chars().count() <= 1 + LEFT_CONTEXT);
        // The match keeps the document's case.
        let (s, m) = snippet("the quick brown Fox jumps over the lazy dog", "fox", 60);
        assert_eq!(&s[m.unwrap()], "Fox");
        // Non-ASCII text before the match.
        let (s, m) = snippet("ünïcödé wörds before the needle here", "needle", 60);
        assert_eq!(&s[m.unwrap()], "needle");
        // Not found: no range.
        assert_eq!(snippet("nothing here", "zzz", 60).1, None);
    }

    #[test]
    fn markup_bolds_the_match() {
        let (context, matched) = snippet("a <b> & fox here", "fox", 60);
        let h = Hit {
            page: 0,
            rect: Rect::default(),
            context,
            matched,
        };
        assert_eq!(h.context_markup(), "a &lt;b&gt; &amp; <b>fox</b> here");
    }
}
