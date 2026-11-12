//! Name / keyword search over all characters and emoji.

use crate::category::all_code_points;
use crate::charinfo::{parse_codepoint, unicode_name};
use std::collections::HashSet;
use std::sync::OnceLock;

pub const MAX_RESULTS: usize = 5000;

pub struct Entry {
    pub text: String,
    /// Lower-case primary name.
    name: String,
    /// Lower-case extra keywords.
    keywords: Vec<String>,
    is_emoji: bool,
}

pub struct Index {
    entries: Vec<Entry>,
}

static INDEX: OnceLock<Index> = OnceLock::new();

/// The shared index, built on first use (call from a worker thread to warm).
pub fn index() -> &'static Index {
    INDEX.get_or_init(Index::build)
}

fn strip_vs16(s: &str) -> String {
    s.chars().filter(|&c| c != '\u{FE0F}').collect()
}

fn is_word_start(hay: &str, term: &str) -> bool {
    hay.match_indices(term).any(|(i, _)| {
        i == 0 || matches!(hay[..i].chars().next_back(), Some(' ' | '-' | '_' | ':' | ','))
    })
}

impl Index {
    pub fn build() -> Index {
        let mut entries = Vec::new();
        let mut emoji_singles = HashSet::new();
        for e in emojis::iter() {
            let mut keywords: Vec<String> = e.shortcodes().map(|s| s.replace('_', " ")).collect();
            let base = strip_vs16(e.as_str());
            let mut chars = base.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                emoji_singles.insert(c);
                if let Some(n) = unicode_name(c) {
                    keywords.push(n.to_lowercase());
                }
            }
            entries.push(Entry { text: e.as_str().to_string(), name: e.name().to_lowercase(), keywords, is_emoji: true });
        }
        for c in all_code_points() {
            if emoji_singles.contains(&c) {
                continue;
            }
            if let Some(n) = unicode_name(c) {
                entries.push(Entry { text: c.to_string(), name: n.to_lowercase(), keywords: Vec::new(), is_emoji: false });
            }
        }
        Index { entries }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns matching texts, best first.
    pub fn search(&self, query: &str) -> Vec<String> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<String> = Vec::new();
        // Literal character / emoji or code point first.
        let mut qc = query.chars();
        if let (Some(c), None) = (qc.next(), qc.next()) {
            out.push(c.to_string());
        } else if let Some(e) = emojis::get(query) {
            out.push(e.as_str().to_string());
        }
        if let Some(c) = parse_codepoint(query) {
            out.push(c.to_string());
        }

        let q = query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        let mut scored: Vec<(u8, bool, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let fields = || std::iter::once(e.name.as_str()).chain(e.keywords.iter().map(String::as_str));
                if !terms.iter().all(|t| fields().any(|f| f.contains(t))) {
                    return None;
                }
                let score = if fields().any(|f| f == q) {
                    0
                } else if fields().any(|f| f.starts_with(&q)) {
                    1
                } else if terms.iter().all(|t| fields().any(|f| is_word_start(f, t))) {
                    2
                } else {
                    3
                };
                Some((score, !e.is_emoji, i))
            })
            .collect();
        scored.sort_unstable();
        out.extend(scored.into_iter().map(|(_, _, i)| self.entries[i].text.clone()));
        let mut seen = HashSet::new();
        out.retain(|s| seen.insert(s.clone()));
        out.truncate(MAX_RESULTS);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_unicode_name() {
        let r = index().search("latin small letter a");
        assert_eq!(r[0], "a");
        assert!(r.contains(&"á".to_string()));
    }

    #[test]
    fn finds_emoji_by_name_and_shortcode() {
        let r = index().search("grinning");
        assert!(r.iter().take(5).any(|s| s == "😀"), "{r:?}");
        let r = index().search("thumbsup");
        assert_eq!(r[0], "👍");
        let r = index().search("THUMBS UP");
        assert_eq!(r[0], "👍");
    }

    #[test]
    fn emoji_keyword_from_unicode_name() {
        let r = index().search("white smiling face");
        assert!(r[0].starts_with('☺'), "{r:?}");
        // Not listed twice (once as emoji, once as plain char).
        assert_eq!(r.iter().filter(|s| s.starts_with('☺')).count(), 1);
    }

    #[test]
    fn all_terms_must_match() {
        let r = index().search("arrow rightwards double");
        assert!(r.contains(&"⇒".to_string()));
        assert!(!r.contains(&"→".to_string()));
        assert!(index().search("zzqqxx nonexistent").is_empty());
    }

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let r = index().search("euro sign");
        assert_eq!(r[0], "€");
        let r = index().search("dog");
        let dog = r.iter().position(|s| s == "🐶" || s == "🐕").unwrap();
        assert!(dog < 5, "{r:?}");
    }

    #[test]
    fn literal_and_codepoint() {
        assert_eq!(index().search("é")[0], "é");
        assert_eq!(index().search("U+2603")[0], "☃");
        assert_eq!(index().search("👍🏽")[0], "👍🏽");
        assert!(index().search("   ").is_empty());
    }

    #[test]
    fn capped() {
        assert!(index().search("e").len() <= MAX_RESULTS);
        assert!(index().len() > 100_000);
    }

    #[test]
    fn word_start() {
        assert!(is_word_start("latin small letter a", "small"));
        assert!(!is_word_start("latin small letter a", "mall"));
        assert!(is_word_start("cjk unified ideograph-4e00", "4e00"));
    }
}
