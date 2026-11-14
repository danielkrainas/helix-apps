//! Search filtering.

use crate::journal::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchField {
    #[default]
    All,
    Message,
    Process,
    Unit,
}

impl SearchField {
    pub const ALL: [SearchField; 4] = [
        SearchField::All,
        SearchField::Message,
        SearchField::Process,
        SearchField::Unit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchField::All => "All Fields",
            SearchField::Message => "Message",
            SearchField::Process => "Process",
            SearchField::Unit => "Unit",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Term {
    /// Lower-cased word matched against the selected field(s).
    Word(String),
    /// `FIELD=value`: lower-cased value matched against a journal field.
    Field(String, String),
}

/// A parsed search query. The empty query matches everything.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Search {
    field: SearchField,
    terms: Vec<Term>,
}

fn is_field_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    if haystack.is_ascii() && needle_lower.is_ascii() {
        // Fast path without allocating.
        let h = haystack.as_bytes();
        let n = needle_lower.as_bytes();
        if n.is_empty() {
            return true;
        }
        return h
            .windows(n.len())
            .any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b));
    }
    haystack.to_lowercase().contains(needle_lower)
}

impl Search {
    pub fn new(text: &str, field: SearchField) -> Self {
        let terms = text
            .split_whitespace()
            .map(|w| match w.split_once('=') {
                Some((k, v)) if is_field_name(k) => Term::Field(k.to_string(), v.to_lowercase()),
                _ => Term::Word(w.to_lowercase()),
            })
            .collect();
        Search { field, terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn matches(&self, e: &Entry) -> bool {
        self.terms.iter().all(|t| match t {
            Term::Field(k, v) => e.get_all(k).any(|x| contains_ci(x, v)),
            Term::Word(w) => match self.field {
                SearchField::Message => contains_ci(e.message(), w),
                SearchField::Process => contains_ci(e.process(), w),
                SearchField::Unit => contains_ci(e.unit(), w),
                SearchField::All => {
                    contains_ci(e.message(), w)
                        || contains_ci(e.process(), w)
                        || contains_ci(e.unit(), w)
                }
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Entry> {
        include_str!("../tests/fixtures/journal.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| Entry::parse(l).unwrap())
            .collect()
    }

    fn hits(text: &str, field: SearchField) -> Vec<usize> {
        let s = Search::new(text, field);
        fixture()
            .iter()
            .enumerate()
            .filter(|(_, e)| s.matches(e))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn empty_matches_all() {
        assert!(Search::new("  ", SearchField::All).is_empty());
        assert_eq!(hits("", SearchField::Message).len(), 7);
    }

    #[test]
    fn all_fields_case_insensitive() {
        assert_eq!(hits("NETWORKMANAGER", SearchField::All), [5]);
        assert_eq!(hits("gvfs", SearchField::All), [0]); // via unit / process
        assert_eq!(hits("failed", SearchField::All), [0, 5]);
    }

    #[test]
    fn field_restricted() {
        assert_eq!(hits("sshd", SearchField::Message), Vec::<usize>::new());
        assert_eq!(hits("sshd", SearchField::Process), [3]);
        assert_eq!(hits("ssh.service", SearchField::Unit), [3]);
        assert_eq!(hits("wsdd", SearchField::Process), [0]);
        assert_eq!(hits("kernel", SearchField::Process), [2]);
    }

    #[test]
    fn words_are_anded() {
        assert_eq!(hits("failed wlp2s0", SearchField::All), [5]);
        assert_eq!(
            hits("failed nothing-here", SearchField::All),
            Vec::<usize>::new()
        );
        // Words may match different fields.
        assert_eq!(hits("networkmanager change", SearchField::All), [5]);
    }

    #[test]
    fn explicit_field_terms() {
        assert_eq!(hits("_PID=999", SearchField::Message), [3]);
        assert_eq!(hits("_TRANSPORT=kernel usb", SearchField::All), [2]);
        assert_eq!(hits("SYSLOG_FACILITY=10", SearchField::All), [3]);
        // Lowercase key is a plain word.
        assert_eq!(hits("pid=1000", SearchField::All), [4]);
    }

    #[test]
    fn unicode() {
        let e = Entry::from_fields([("MESSAGE", "Größe ÜBER")]);
        assert!(Search::new("über", SearchField::Message).matches(&e));
        assert!(Search::new("GRÖßE", SearchField::Message).matches(&e));
        assert!(!Search::new("gross", SearchField::Message).matches(&e));
    }
}
