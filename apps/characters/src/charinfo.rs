//! Information about a single character or emoji sequence.

use unicode_general_category::{get_general_category, GeneralCategory as Gc};

/// Whether a code point is worth showing in a grid: assigned, not a control,
/// format character, surrogate, private-use or separator.
pub fn is_displayable(c: char) -> bool {
    !matches!(
        get_general_category(c),
        Gc::Control
            | Gc::Format
            | Gc::Surrogate
            | Gc::PrivateUse
            | Gc::Unassigned
            | Gc::LineSeparator
            | Gc::ParagraphSeparator
            | Gc::SpaceSeparator
    )
}

/// Whether `c` is a combining mark (needs a base to be visible).
pub fn is_mark(c: char) -> bool {
    matches!(get_general_category(c), Gc::NonspacingMark | Gc::SpacingMark | Gc::EnclosingMark)
}

/// The Unicode name of a code point, upper case, if it has one.
pub fn unicode_name(c: char) -> Option<String> {
    unicode_names2::name(c).map(|n| n.to_string())
}

fn sentence_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn fallback_name(c: char) -> String {
    match get_general_category(c) {
        Gc::Control => "<control>".into(),
        Gc::PrivateUse => "Private use character".into(),
        Gc::Surrogate => "Surrogate".into(),
        Gc::Unassigned => "Unassigned code point".into(),
        _ => "Unknown character".into(),
    }
}

/// Human-readable name: the CLDR name (sentence case) for emoji, otherwise
/// the Unicode name(s) of the code point(s).
pub fn display_name(s: &str) -> String {
    if let Some(e) = emojis::get(s) {
        return sentence_case(e.name());
    }
    let names: Vec<String> =
        s.chars().filter(|&c| c != '\u{FE0F}').map(|c| unicode_name(c).unwrap_or_else(|| fallback_name(c))).collect();
    if names.is_empty() {
        String::new()
    } else {
        names.join(", ")
    }
}

/// `U+0041`, space separated for sequences.
pub fn codepoints(s: &str) -> String {
    s.chars().map(|c| format!("U+{:04X}", c as u32)).collect::<Vec<_>>().join(" ")
}

/// UTF-8 bytes in upper-case hex, space separated.
pub fn utf8_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Text to show in a grid cell: combining marks get a dotted-circle base.
pub fn grid_label(s: &str) -> String {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if is_mark(c) => format!("\u{25CC}{c}"),
        _ => s.to_string(),
    }
}

/// Related characters for the "See also" section: skin-tone variants for
/// emoji, case counterparts for letters. Never contains `s` itself.
pub fn see_also(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(e) = emojis::get(s) {
        if let Some(tones) = e.skin_tones() {
            out.extend(tones.map(|t| t.as_str().to_string()));
        }
    } else {
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let forms = [c.to_uppercase().collect::<String>(), c.to_lowercase().collect::<String>()];
            for f in forms {
                if f.chars().count() == 1 {
                    out.push(f);
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| v != s && seen.insert(v.clone()));
    out
}

/// Parses a code point query: `U+1F600`, `u+41`, `0x41`, `\u41`.
pub fn parse_codepoint(q: &str) -> Option<char> {
    let q = q.trim();
    let lower = q.to_ascii_lowercase();
    let hex = ["u+", "0x", "\\u"].iter().find_map(|p| lower.strip_prefix(p))?;
    if hex.is_empty() || hex.len() > 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(display_name("A"), "LATIN CAPITAL LETTER A");
        assert_eq!(display_name("😀"), "Grinning face");
        assert_eq!(display_name("\u{E000}"), "Private use character");
        assert_eq!(display_name("一"), "CJK UNIFIED IDEOGRAPH-4E00");
    }

    #[test]
    fn codes() {
        assert_eq!(codepoints("A"), "U+0041");
        assert_eq!(codepoints("😀"), "U+1F600");
        assert_eq!(codepoints("👍🏽"), "U+1F44D U+1F3FD");
        assert_eq!(utf8_hex("😀"), "F0 9F 98 80");
        assert_eq!(utf8_hex("é"), "C3 A9");
    }

    #[test]
    fn labels() {
        assert_eq!(grid_label("\u{301}"), "\u{25CC}\u{301}");
        assert_eq!(grid_label("a"), "a");
    }

    #[test]
    fn variants() {
        let v = see_also("👍");
        assert_eq!(v.len(), 5);
        assert!(v.contains(&"👍🏽".to_string()));
        assert!(see_also("👍🏽").contains(&"👍".to_string()));
        assert_eq!(see_also("a"), vec!["A"]);
        assert_eq!(see_also("Ω"), vec!["ω"]);
        assert!(see_also("1").is_empty());
        assert!(see_also("😀").is_empty());
    }

    #[test]
    fn parse() {
        assert_eq!(parse_codepoint("U+1F600"), Some('😀'));
        assert_eq!(parse_codepoint("0x41"), Some('A'));
        assert_eq!(parse_codepoint("\\u00e9"), Some('é'));
        assert_eq!(parse_codepoint("u+"), None);
        assert_eq!(parse_codepoint("U+D800"), None);
        assert_eq!(parse_codepoint("hello"), None);
    }

    #[test]
    fn displayable() {
        assert!(is_displayable('a'));
        assert!(!is_displayable('\u{7}'));
        assert!(!is_displayable(' '));
        assert!(!is_displayable('\u{E000}'));
    }
}
