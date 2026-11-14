//! The text of a page with the position of every character, and the
//! character / word / line arithmetic used by caret navigation and by the
//! view's accessible text.
//!
//! Offsets count characters (not bytes), as GTK's accessible text and
//! poppler's text layout do. Poppler separates lines with `'\n'`.

use crate::layout::Rect;

/// What an offset range is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Char,
    Word,
    Sentence,
    Line,
    Paragraph,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub chars: Vec<char>,
    /// One box per character, top-left-origin page points.
    pub boxes: Vec<Rect>,
}

impl PageText {
    /// Pairs `text` with poppler's per-character boxes; extra characters
    /// or boxes on either side are dropped.
    pub fn new(text: &str, boxes: Vec<Rect>) -> PageText {
        let mut chars: Vec<char> = text.chars().collect();
        let mut boxes = boxes;
        let n = chars.len().min(boxes.len());
        chars.truncate(n);
        boxes.truncate(n);
        PageText { chars, boxes }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn slice(&self, start: usize, end: usize) -> String {
        let end = end.min(self.len());
        let start = start.min(end);
        self.chars[start..end].iter().collect()
    }

    /// The range of the `unit` containing `offset`. The range of a line
    /// or paragraph includes its terminating newline; a word range covers
    /// the word and the spaces after it.
    pub fn range_at(&self, offset: usize, unit: Unit) -> (usize, usize) {
        let n = self.len();
        let o = offset.min(n);
        match unit {
            Unit::Char => (o, (o + 1).min(n)),
            Unit::Line => {
                let (s, e) = self.line_bounds(o);
                (s, (e + 1).min(n))
            }
            Unit::Paragraph => {
                // A paragraph ends at an empty line (two newlines in a row)
                // or the end of the page.
                let mut s = o;
                while s > 0 && !(self.chars[s - 1] == '\n' && s >= 2 && self.chars[s - 2] == '\n') {
                    s -= 1;
                }
                let mut e = o;
                while e < n && !(self.chars[e] == '\n' && e + 1 < n && self.chars[e + 1] == '\n') {
                    e += 1;
                }
                (s, (e + 2).min(n))
            }
            Unit::Word => {
                let s = self.word_start_at(o);
                let mut e = s;
                while e < n && !self.chars[e].is_whitespace() {
                    e += 1;
                }
                while e < n && self.chars[e].is_whitespace() && self.chars[e] != '\n' {
                    e += 1;
                }
                (s, e.max((s + 1).min(n)))
            }
            Unit::Sentence => {
                let ends = |i: usize| {
                    matches!(self.chars[i], '.' | '!' | '?')
                        && (i + 1 >= n || self.chars[i + 1].is_whitespace())
                };
                let mut s = o;
                while s > 0 && !ends(s - 1) {
                    s -= 1;
                }
                while s < n && self.chars[s].is_whitespace() && s < o {
                    s += 1;
                }
                let mut e = o;
                while e < n && !ends(e) {
                    e += 1;
                }
                e = (e + 1).min(n);
                while e < n && self.chars[e].is_whitespace() {
                    e += 1;
                }
                (s, e)
            }
        }
    }

    /// Start of the word at or before `o` (`o` itself when it starts one).
    fn word_start_at(&self, o: usize) -> usize {
        let mut s = o.min(self.len());
        // On whitespace: belong to the word before.
        while s > 0 && s < self.len() && self.chars[s].is_whitespace() {
            s -= 1;
        }
        while s > 0 && !self.chars[s - 1].is_whitespace() {
            s -= 1;
        }
        s
    }

    /// `(start, end)` of the line containing `o`; `end` is the offset of
    /// its newline (or the text length).
    pub fn line_bounds(&self, o: usize) -> (usize, usize) {
        let n = self.len();
        let o = o.min(n);
        let mut s = o;
        while s > 0 && self.chars[s - 1] != '\n' {
            s -= 1;
        }
        let mut e = o;
        while e < n && self.chars[e] != '\n' {
            e += 1;
        }
        (s, e)
    }

    /// Offset of the next word start after `o` (or the end).
    pub fn next_word(&self, o: usize) -> usize {
        let n = self.len();
        let mut i = o.min(n);
        while i < n && !self.chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && self.chars[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Offset of the word start before `o` (or 0).
    pub fn prev_word(&self, o: usize) -> usize {
        let mut i = o.min(self.len());
        while i > 0 && self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// The x position of the caret at `o` (left edge of its character, or
    /// right edge of the last character).
    pub fn caret_x(&self, o: usize) -> Option<f64> {
        if let Some(b) = self.boxes.get(o) {
            Some(b.x)
        } else {
            self.boxes.last().map(|b| b.right())
        }
    }

    /// The caret box at `o`: a zero-width rectangle at the left edge of
    /// the character, as tall as the character.
    pub fn caret_rect(&self, o: usize) -> Option<Rect> {
        if let Some(b) = self.boxes.get(o) {
            // A newline's box can be empty; use the previous character's.
            if b.h <= 0.0 && o > 0 {
                let p = self.boxes[o - 1];
                return Some(Rect::new(p.right(), p.y, 0.0, p.h));
            }
            Some(Rect::new(b.x, b.y, 0.0, b.h))
        } else {
            self.boxes
                .last()
                .map(|b| Rect::new(b.right(), b.y, 0.0, b.h))
        }
    }

    /// The offset on the line before (`up`) or after the line of `o`
    /// closest to horizontal position `x`; `None` past the first / last
    /// line.
    pub fn vertical(&self, o: usize, x: f64, up: bool) -> Option<usize> {
        let (s, e) = self.line_bounds(o);
        let (ls, le) = if up {
            if s == 0 {
                return None;
            }
            self.line_bounds(s - 1)
        } else {
            if e >= self.len() {
                return None;
            }
            self.line_bounds(e + 1)
        };
        Some(self.closest_on_line(ls, le, x))
    }

    /// The offset in `[s, e]` whose caret is closest to `x`.
    pub fn closest_on_line(&self, s: usize, e: usize, x: f64) -> usize {
        (s..=e)
            .min_by(|a, b| {
                let da = (self.caret_x(*a).unwrap_or(0.0) - x).abs();
                let db = (self.caret_x(*b).unwrap_or(0.0) - x).abs();
                da.total_cmp(&db)
            })
            .unwrap_or(s)
    }

    /// Highlight rectangles for `[start, end)`: the boxes of each line
    /// merged into one.
    pub fn range_rects(&self, start: usize, end: usize) -> Vec<Rect> {
        let end = end.min(self.len());
        let mut out: Vec<Rect> = Vec::new();
        let mut cur: Option<Rect> = None;
        for i in start.min(end)..end {
            let b = self.boxes[i];
            if self.chars[i] == '\n' || b.w <= 0.0 || b.h <= 0.0 {
                if let Some(c) = cur.take() {
                    out.push(c);
                }
                continue;
            }
            cur = Some(match cur {
                Some(c) => Rect::from_corners(
                    c.x.min(b.x),
                    c.y.min(b.y),
                    c.right().max(b.right()),
                    c.bottom().max(b.bottom()),
                ),
                None => b,
            });
        }
        out.extend(cur);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 10×10 box per character, lines 12 pt apart.
    fn page(text: &str) -> PageText {
        let mut boxes = Vec::new();
        let (mut x, mut y) = (0.0, 0.0);
        for c in text.chars() {
            boxes.push(Rect::new(x, y, 10.0, 10.0));
            if c == '\n' {
                x = 0.0;
                y += 12.0;
            } else {
                x += 10.0;
            }
        }
        PageText::new(text, boxes)
    }

    #[test]
    fn pairs_text_with_boxes() {
        let t = PageText::new("abc", vec![Rect::new(0.0, 0.0, 1.0, 1.0)]);
        assert_eq!(t.len(), 1);
        assert_eq!(t.slice(0, 10), "a");
    }

    #[test]
    fn words() {
        let t = page("Hello big world\nnext");
        assert_eq!(t.range_at(0, Unit::Word), (0, 6));
        assert_eq!(t.slice(6, 10), "big ");
        assert_eq!(t.range_at(7, Unit::Word), (6, 10));
        // On the space: the word before.
        assert_eq!(t.range_at(5, Unit::Word), (0, 6));
        assert_eq!(t.range_at(16, Unit::Word), (16, 20));
        assert_eq!(t.next_word(0), 6);
        assert_eq!(t.next_word(10), 16);
        assert_eq!(t.prev_word(10), 6);
        assert_eq!(t.prev_word(6), 0);
        assert_eq!(t.prev_word(0), 0);
    }

    #[test]
    fn lines() {
        let t = page("one\ntwo three\nfour");
        assert_eq!(t.line_bounds(5), (4, 13));
        assert_eq!(t.range_at(5, Unit::Line), (4, 14));
        assert_eq!(t.slice(4, 14), "two three\n");
        assert_eq!(t.range_at(15, Unit::Line), (14, 18));
        assert_eq!(t.range_at(0, Unit::Char), (0, 1));
        assert_eq!(t.range_at(18, Unit::Char), (18, 18));
    }

    #[test]
    fn sentences_and_paragraphs() {
        let t = page("One two. Three!\n\nNew para");
        let (s, e) = t.range_at(2, Unit::Sentence);
        assert_eq!(t.slice(s, e), "One two. ");
        let (s, e) = t.range_at(10, Unit::Sentence);
        assert_eq!(t.slice(s, e), "Three!\n\n");
        let (s, e) = t.range_at(3, Unit::Paragraph);
        assert_eq!(t.slice(s, e), "One two. Three!\n\n");
        let (s, e) = t.range_at(20, Unit::Paragraph);
        assert_eq!(t.slice(s, e), "New para");
    }

    #[test]
    fn vertical_moves_keep_column() {
        let t = page("abcdef\nxy\nlonger line");
        // From 'd' (x = 30) down: line "xy" ends at 9, caret x 20.
        assert_eq!(t.vertical(3, 30.0, false), Some(9));
        // Down again keeps the remembered column.
        assert_eq!(t.vertical(9, 30.0, false), Some(13));
        assert_eq!(t.vertical(13, 30.0, true), Some(9));
        assert_eq!(t.vertical(3, 30.0, true), None);
        assert_eq!(t.vertical(13, 30.0, false), None);
    }

    #[test]
    fn highlight_rects_per_line() {
        let t = page("ab\ncd");
        let r = t.range_rects(1, 5);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], Rect::new(10.0, 0.0, 10.0, 10.0));
        assert_eq!(r[1], Rect::new(0.0, 12.0, 20.0, 10.0));
    }

    #[test]
    fn caret_rects() {
        let t = page("ab");
        assert_eq!(t.caret_rect(1), Some(Rect::new(10.0, 0.0, 0.0, 10.0)));
        assert_eq!(t.caret_rect(2), Some(Rect::new(20.0, 0.0, 0.0, 10.0)));
        assert_eq!(PageText::default().caret_rect(0), None);
    }
}
