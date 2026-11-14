//! Natural ("human") ordering of file names: `img2` sorts before `img10`.

use std::cmp::Ordering;

enum Chunk<'a> {
    Digits(&'a str),
    Text(&'a str),
}

fn chunks(s: &str) -> impl Iterator<Item = Chunk<'_>> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let digit = first.is_ascii_digit();
        let end = rest
            .find(|c: char| c.is_ascii_digit() != digit)
            .unwrap_or(rest.len());
        let (head, tail) = rest.split_at(end);
        rest = tail;
        Some(if digit {
            Chunk::Digits(head)
        } else {
            Chunk::Text(head)
        })
    })
}

fn cmp_digits(a: &str, b: &str) -> Ordering {
    let ta = a.trim_start_matches('0');
    let tb = b.trim_start_matches('0');
    ta.len()
        .cmp(&tb.len())
        .then_with(|| ta.cmp(tb))
        // "01" after "1" so that the order is total and stable.
        .then_with(|| a.len().cmp(&b.len()))
}

fn cmp_text(a: &str, b: &str) -> Ordering {
    let la = a.chars().flat_map(char::to_lowercase);
    let lb = b.chars().flat_map(char::to_lowercase);
    la.cmp(lb)
}

/// Natural, case-insensitive comparison. Ties are broken by the raw string so
/// distinct names never compare equal.
pub fn compare(a: &str, b: &str) -> Ordering {
    let mut ca = chunks(a);
    let mut cb = chunks(b);
    loop {
        let ord = match (ca.next(), cb.next()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(Chunk::Digits(x)), Some(Chunk::Digits(y))) => cmp_digits(x, y),
            (Some(Chunk::Text(x)), Some(Chunk::Text(y))) => cmp_text(x, y),
            // Numbers sort before text at the same position.
            (Some(Chunk::Digits(_)), Some(Chunk::Text(_))) => Ordering::Less,
            (Some(Chunk::Text(_)), Some(Chunk::Digits(_))) => Ordering::Greater,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.cmp(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(v: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = v.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| compare(a, b));
        v
    }

    #[test]
    fn numbers_compare_numerically() {
        assert_eq!(
            sorted(&["img10.png", "img2.png", "img1.png"]),
            ["img1.png", "img2.png", "img10.png"]
        );
        assert_eq!(compare("a100", "a99"), Ordering::Greater);
    }

    #[test]
    fn case_insensitive_with_stable_tiebreak() {
        assert_eq!(
            sorted(&["b.png", "A.png", "a.png"]),
            ["A.png", "a.png", "b.png"]
        );
        assert_eq!(compare("Photo", "photo"), Ordering::Less);
    }

    #[test]
    fn leading_zeros() {
        assert_eq!(
            sorted(&["x010", "x9", "x1", "x01"]),
            ["x1", "x01", "x9", "x010"]
        );
        assert_eq!(compare("x001", "x1"), Ordering::Greater);
    }

    #[test]
    fn prefixes_and_mixed() {
        assert_eq!(
            sorted(&["abc", "ab", "ab1", "1ab"]),
            ["1ab", "ab", "ab1", "abc"]
        );
        assert_eq!(
            sorted(&[
                "IMG_2024-01-10 (2).jpg",
                "IMG_2024-01-10 (10).jpg",
                "IMG_2024-01-09.jpg"
            ]),
            [
                "IMG_2024-01-09.jpg",
                "IMG_2024-01-10 (2).jpg",
                "IMG_2024-01-10 (10).jpg"
            ]
        );
    }

    #[test]
    fn huge_numbers_and_unicode() {
        assert_eq!(
            compare("n123456789012345678901234567890", "n99"),
            Ordering::Greater
        );
        assert_eq!(sorted(&["Émile2", "émile10"]), ["Émile2", "émile10"]);
        assert_eq!(compare("", "a"), Ordering::Less);
        assert_eq!(compare("same", "same"), Ordering::Equal);
    }
}
