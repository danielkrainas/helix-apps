//! Recently used characters list.

pub const DEFAULT_MAX: u32 = 100;

fn cap(max: u32) -> usize {
    if max == 0 {
        DEFAULT_MAX as usize
    } else {
        max as usize
    }
}

/// Moves `item` to the front of `list`, de-duplicating and truncating to `max`
/// (a `max` of 0 means the default).
pub fn push(list: &[String], item: &str, max: u32) -> Vec<String> {
    std::iter::once(item.to_string())
        .chain(list.iter().filter(|s| *s != item).cloned())
        .take(cap(max))
        .collect()
}

/// Cleans a list read from settings: drops empty and duplicate entries.
pub fn sanitize(list: Vec<String>, max: u32) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .take(cap(max))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn push_front_dedup_truncate() {
        assert_eq!(push(&[], "a", 3), v(&["a"]));
        assert_eq!(push(&v(&["a", "b", "c"]), "b", 3), v(&["b", "a", "c"]));
        assert_eq!(push(&v(&["a", "b", "c"]), "d", 3), v(&["d", "a", "b"]));
        assert_eq!(push(&v(&["a"]), "😀", 0), v(&["😀", "a"]));
    }

    #[test]
    fn sanitize_list() {
        assert_eq!(sanitize(v(&["a", "", "a", "b", "c"]), 2), v(&["a", "b"]));
    }
}
