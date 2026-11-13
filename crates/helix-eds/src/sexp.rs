//! EDS query S-expressions (the `query` argument of GetView / GetObjectList /
//! GetContactList).

use chrono::{DateTime, Utc};

/// Quotes a string literal for an S-expression.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Matches every contact.
pub fn all_contacts() -> String {
    "(contains \"x-evolution-any-field\" \"\")".to_string()
}

/// Contacts with any field containing `text`.
pub fn contacts_containing(text: &str) -> String {
    format!("(contains \"x-evolution-any-field\" {})", quote(text))
}

/// Contacts having the exact email address.
pub fn contacts_with_email(email: &str) -> String {
    format!("(is \"email\" {})", quote(email))
}

fn make_time(t: DateTime<Utc>) -> String {
    format!("(make-time {})", quote(&t.format("%Y%m%dT%H%M%SZ").to_string()))
}

/// Calendar components with an occurrence in `[start, end)`.
pub fn occur_in_range(start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    format!("(occur-in-time-range? {} {})", make_time(start), make_time(end))
}

/// Calendar components whose summary, description or location contains `text`.
pub fn events_containing(text: &str) -> String {
    let q = quote(text);
    format!("(or (contains? \"summary\" {q}) (contains? \"description\" {q}) (contains? \"location\" {q}))")
}

/// Matches all calendar components.
pub fn all_events() -> String {
    "#t".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn quoting() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(contacts_containing("x"), "(contains \"x-evolution-any-field\" \"x\")");
    }

    #[test]
    fn time_range() {
        let a = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap();
        assert_eq!(
            occur_in_range(a, b),
            "(occur-in-time-range? (make-time \"20261101T000000Z\") (make-time \"20261201T000000Z\"))"
        );
        assert!(events_containing("lunch").starts_with("(or (contains? \"summary\" \"lunch\")"));
    }
}
