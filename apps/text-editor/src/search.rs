//! Find / replace logic, independent of GTK.
//!
//! Ranges are byte offsets into the searched text; [`to_char_ranges`]
//! converts them to the character offsets `GtkTextIter` uses.

use regex::{Regex, RegexBuilder};
use std::ops::Range;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// A compiled query.
#[derive(Debug, Clone)]
pub struct Matcher {
    re: Regex,
    opts: SearchOptions,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Matcher {
    pub fn new(query: &str, opts: SearchOptions) -> Result<Self, String> {
        if query.is_empty() {
            return Err("empty query".into());
        }
        let pattern = if opts.regex { query.to_string() } else { regex::escape(query) };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!opts.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Matcher { re, opts })
    }

    fn word_bounded(text: &str, r: &Range<usize>) -> bool {
        let before = text[..r.start].chars().next_back().is_some_and(is_word_char);
        let after = text[r.end..].chars().next().is_some_and(is_word_char);
        !before && !after
    }

    /// All non-empty, non-overlapping matches.
    pub fn find_all(&self, text: &str) -> Vec<Range<usize>> {
        self.re
            .find_iter(text)
            .map(|m| m.range())
            .filter(|r| !r.is_empty())
            .filter(|r| !self.opts.whole_word || Self::word_bounded(text, r))
            .collect()
    }

    /// The replacement text for the match at `range` (expands `$1` etc. in
    /// regex mode; literal otherwise).
    pub fn replacement_for(&self, text: &str, range: Range<usize>, replacement: &str) -> String {
        if !self.opts.regex {
            return replacement.to_string();
        }
        // Re-run the regex anchored at the match start to obtain captures.
        match self.re.captures_at(text, range.start) {
            Some(caps) if caps.get(0).map(|m| m.range()) == Some(range.clone()) => {
                let mut out = String::new();
                caps.expand(replacement, &mut out);
                out
            }
            _ => replacement.to_string(),
        }
    }

    /// Replaces every match; returns the new text and the number replaced.
    #[cfg(test)]
    pub fn replace_all(&self, text: &str, replacement: &str) -> (String, usize) {
        let matches = self.find_all(text);
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for r in &matches {
            out.push_str(&text[last..r.start]);
            out.push_str(&self.replacement_for(text, r.clone(), replacement));
            last = r.end;
        }
        out.push_str(&text[last..]);
        (out, matches.len())
    }
}

/// Converts sorted byte ranges into character-offset ranges.
pub fn to_char_ranges(text: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out = Vec::with_capacity(ranges.len());
    let (mut byte, mut chars) = (0usize, 0usize);
    let mut advance = |to: usize| {
        chars += text[byte..to].chars().count();
        byte = to;
        chars
    };
    for r in ranges {
        let s = advance(r.start);
        let e = advance(r.end);
        out.push(s..e);
    }
    out
}

/// Index of the first match starting at or after `offset` (wrapping), or
/// the last match starting before `offset` when `backward`.
pub fn pick(matches: &[Range<usize>], offset: usize, backward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if backward {
        matches.iter().rposition(|m| m.start < offset).or(Some(matches.len() - 1))
    } else {
        matches.iter().position(|m| m.start >= offset).or(Some(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(case_sensitive: bool, whole_word: bool, regex: bool) -> SearchOptions {
        SearchOptions { case_sensitive, whole_word, regex }
    }

    #[test]
    fn literal_case_insensitive() {
        let m = Matcher::new("foo", opts(false, false, false)).unwrap();
        assert_eq!(m.find_all("Foo foo FOObar"), vec![0..3, 4..7, 8..11]);
    }

    #[test]
    fn literal_case_sensitive_and_escaping() {
        let m = Matcher::new("a.b", opts(true, false, false)).unwrap();
        assert_eq!(m.find_all("a.b axb A.B"), vec![0..3]);
    }

    #[test]
    fn whole_word() {
        let m = Matcher::new("cat", opts(false, true, false)).unwrap();
        assert_eq!(m.find_all("cat concat cats cat_ (cat)"), vec![0..3, 22..25]);
        // Non-word query characters still respect neighbours.
        let m = Matcher::new("+", opts(false, true, false)).unwrap();
        assert_eq!(m.find_all("a+b + c"), vec![4..5]);
    }

    #[test]
    fn regex_and_captures() {
        let m = Matcher::new(r"(\w+)@(\w+)", opts(true, false, true)).unwrap();
        let text = "mail bob@example now";
        let found = m.find_all(text);
        assert_eq!(found, vec![5..16]);
        assert_eq!(m.replacement_for(text, found[0].clone(), "$2:$1"), "example:bob");
        let (out, n) = m.replace_all(text, "<$1>");
        assert_eq!((out.as_str(), n), ("mail <bob> now", 1));
    }

    #[test]
    fn regex_line_anchors() {
        let m = Matcher::new("^x", opts(true, false, true)).unwrap();
        assert_eq!(m.find_all("x\nax\nx"), vec![0..1, 5..6]);
    }

    #[test]
    fn invalid_and_empty() {
        assert!(Matcher::new("(", opts(true, false, true)).is_err());
        assert!(Matcher::new("", opts(true, false, false)).is_err());
        // Empty regex matches are dropped.
        let m = Matcher::new("a*", opts(true, false, true)).unwrap();
        assert_eq!(m.find_all("baab"), vec![1..3]);
    }

    #[test]
    fn literal_replacement_not_expanded() {
        let m = Matcher::new("x", opts(true, false, false)).unwrap();
        assert_eq!(m.replace_all("axbx", "$1").0, "a$1b$1");
    }

    #[test]
    fn char_ranges() {
        let text = "héllo wörld héllo";
        let m = Matcher::new("héllo", opts(true, false, false)).unwrap();
        let b = m.find_all(text);
        assert_eq!(to_char_ranges(text, &b), vec![0..5, 12..17]);
    }

    #[test]
    fn picking() {
        let ms = vec![0..1, 5..6, 10..11];
        assert_eq!(pick(&ms, 0, false), Some(0));
        assert_eq!(pick(&ms, 2, false), Some(1));
        assert_eq!(pick(&ms, 11, false), Some(0));
        assert_eq!(pick(&ms, 5, true), Some(0));
        assert_eq!(pick(&ms, 0, true), Some(2));
        assert_eq!(pick(&[], 0, true), None);
    }
}
