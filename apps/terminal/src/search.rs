//! Search and link-matching patterns, PCRE2 flags and zoom steps.

// PCRE2 compile flags (from pcre2.h; stable ABI values).
pub const PCRE2_CASELESS: u32 = 0x0000_0008;
pub const PCRE2_MULTILINE: u32 = 0x0000_0400;
pub const PCRE2_UCP: u32 = 0x0002_0000;
pub const PCRE2_UTF: u32 = 0x0008_0000;
pub const PCRE2_NO_UTF_CHECK: u32 = 0x4000_0000;

/// Flags VTE requires for every regex (UTF mode, multiline).
pub const BASE_FLAGS: u32 = PCRE2_UTF | PCRE2_NO_UTF_CHECK | PCRE2_UCP | PCRE2_MULTILINE;

/// PCRE2 flags for a find-bar search.
pub fn search_flags(match_case: bool) -> u32 {
    if match_case {
        BASE_FLAGS
    } else {
        BASE_FLAGS | PCRE2_CASELESS
    }
}

/// The pattern for a find-bar query: escaped unless `is_regex`.
pub fn search_pattern(query: &str, is_regex: bool) -> String {
    if is_regex {
        query.to_string()
    } else {
        escape(query)
    }
}

/// Escapes PCRE metacharacters.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\^$.|?*+()[]{}/-#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// URLs highlighted in the terminal (Ctrl+click opens them).
pub const URL_PATTERN: &str = concat!(
    r"(?:(?:https?|ftp|sftp|file)://|mailto:|www\.)",
    r"[\w\-.~:/?#\[\]@!$&'*+,;=%()]*",
    r"[\w\-~/#@$&*+=%]"
);

/// Normalises a matched URL so a launcher can open it.
pub fn normalize_url(m: &str) -> String {
    if m.starts_with("www.") {
        format!("http://{m}")
    } else {
        m.to_string()
    }
}

/// The URL that a selection consists of (surrounding spaces, brackets
/// and trailing sentence punctuation ignored), normalised for a launcher;
/// `None` when the selection is not a single URL.
pub fn url_in_selection(sel: &str) -> Option<String> {
    let t = sel
        .trim()
        .trim_end_matches(['.', ',', ';', ':', '!', '?'])
        .trim_start_matches(['<', '(', '"', '\''])
        .trim_end_matches(['>', '"', '\'']);
    let t = match t.strip_suffix(')') {
        Some(inner) if !inner.contains('(') => inner,
        _ => t,
    };
    const SCHEMES: [&str; 7] = [
        "http://", "https://", "ftp://", "sftp://", "file://", "mailto:", "www.",
    ];
    let lower = t.to_ascii_lowercase();
    let ok = !t.chars().any(char::is_whitespace)
        && SCHEMES
            .iter()
            .any(|s| lower.starts_with(s) && t.len() > s.len());
    ok.then(|| normalize_url(t))
}

pub const MIN_SCALE: f64 = 0.5;
pub const MAX_SCALE: f64 = 4.0;
const STEP: f64 = 1.1;

pub fn clamp_scale(s: f64) -> f64 {
    if s.is_finite() {
        s.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        1.0
    }
}

pub fn zoom_in(s: f64) -> f64 {
    snap(clamp_scale(clamp_scale(s) * STEP))
}

pub fn zoom_out(s: f64) -> f64 {
    snap(clamp_scale(clamp_scale(s) / STEP))
}

/// Rounds to 3 decimals and snaps to 1.0 to avoid drift when zooming back.
fn snap(s: f64) -> f64 {
    let r = (s * 1000.0).round() / 1000.0;
    if (r - 1.0).abs() < 0.02 {
        1.0
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_urls() {
        assert_eq!(
            url_in_selection("  https://example.org/a?b=c \n").as_deref(),
            Some("https://example.org/a?b=c")
        );
        assert_eq!(
            url_in_selection("www.example.org.").as_deref(),
            Some("http://www.example.org")
        );
        assert_eq!(
            url_in_selection("(https://example.org/x)").as_deref(),
            Some("https://example.org/x")
        );
        assert_eq!(
            url_in_selection("https://en.wikipedia.org/wiki/Rust_(language)").as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(language)")
        );
        assert_eq!(url_in_selection("see https://example.org"), None);
        assert_eq!(url_in_selection("https://"), None);
        assert_eq!(url_in_selection("ls -la"), None);
        assert_eq!(url_in_selection(""), None);
    }

    #[test]
    fn escaping() {
        assert_eq!(escape("a.b*c"), r"a\.b\*c");
        assert_eq!(escape("(x)[y]{z}"), r"\(x\)\[y\]\{z\}");
        assert_eq!(escape("plain é"), "plain é");
        assert_eq!(search_pattern("a.b", true), "a.b");
        assert_eq!(search_pattern("a.b", false), r"a\.b");
    }

    #[test]
    fn flags() {
        assert_eq!(search_flags(true) & PCRE2_CASELESS, 0);
        assert_ne!(search_flags(false) & PCRE2_CASELESS, 0);
        assert_ne!(search_flags(true) & PCRE2_UTF, 0);
    }

    #[test]
    fn patterns_compile_in_vte() {
        assert!(vte4::Regex::for_match(URL_PATTERN, BASE_FLAGS).is_ok());
        assert!(vte4::Regex::for_search(&escape("a(b[c"), search_flags(false)).is_ok());
        assert!(vte4::Regex::for_search("a(b", BASE_FLAGS).is_err());
    }

    #[test]
    fn urls() {
        assert_eq!(normalize_url("www.example.org"), "http://www.example.org");
        assert_eq!(normalize_url("https://x.y/"), "https://x.y/");
    }

    #[test]
    fn zoom_steps() {
        assert!(zoom_in(1.0) > 1.0);
        assert_eq!(zoom_out(zoom_in(1.0)), 1.0);
        assert_eq!(zoom_in(MAX_SCALE), MAX_SCALE);
        assert_eq!(zoom_out(MIN_SCALE), MIN_SCALE);
        assert_eq!(clamp_scale(f64::NAN), 1.0);
        let mut s = 1.0;
        for _ in 0..5 {
            s = zoom_in(s);
        }
        for _ in 0..5 {
            s = zoom_out(s);
        }
        assert_eq!(s, 1.0);
    }
}
