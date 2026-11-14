//! Sorting of folder entries.

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Type,
    Modified,
}

impl SortKey {
    pub const ALL: [SortKey; 4] = [
        SortKey::Name,
        SortKey::Size,
        SortKey::Type,
        SortKey::Modified,
    ];

    /// Nick used by `org.gnome.nautilus.preferences default-sort-order`.
    pub fn nick(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Type => "type",
            SortKey::Modified => "mtime",
        }
    }

    pub fn from_nick(s: &str) -> Self {
        match s {
            "size" => SortKey::Size,
            "type" => SortKey::Type,
            "mtime" => SortKey::Modified,
            _ => SortKey::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Type => "Type",
            SortKey::Modified => "Modified",
        }
    }
}

/// The fields of an entry that sorting looks at.
#[derive(Debug, Clone, Default)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    pub size: u64,
    pub type_desc: &'a str,
    pub mtime: i64,
}

/// Case-insensitive comparison treating runs of digits as numbers
/// ("file2" < "file10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut da = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    da.push(c);
                    ai.next();
                }
                let mut db = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    db.push(c);
                    bi.next();
                }
                let ta = da.trim_start_matches('0');
                let tb = db.trim_start_matches('0');
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                let la = ca.to_lowercase();
                let lb = cb.to_lowercase();
                let ord = la.cmp(lb);
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
    // Equal ignoring case and leading zeros: fall back to a stable byte order.
    a.cmp(b)
}

/// Full comparison. `reversed` flips the key order but folders stay first.
pub fn compare(a: &Entry, b: &Entry, key: SortKey, reversed: bool, dirs_first: bool) -> Ordering {
    if dirs_first && a.is_dir != b.is_dir {
        return if a.is_dir {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let by_name = || natural_cmp(a.name, b.name);
    let ord = match key {
        SortKey::Name => by_name(),
        SortKey::Size => a.size.cmp(&b.size).then_with(by_name),
        SortKey::Type => natural_cmp(a.type_desc, b.type_desc).then_with(by_name),
        // Newest first is the natural order for dates.
        SortKey::Modified => b.mtime.cmp(&a.mtime).then_with(by_name),
    };
    if reversed {
        ord.reverse()
    } else {
        ord
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, is_dir: bool, size: u64, t: &'static str, mtime: i64) -> Entry<'static> {
        Entry {
            name: Box::leak(name.to_string().into_boxed_str()),
            is_dir,
            size,
            type_desc: t,
            mtime,
        }
    }

    fn sorted(
        mut v: Vec<Entry<'static>>,
        key: SortKey,
        rev: bool,
        dirs: bool,
    ) -> Vec<&'static str> {
        v.sort_by(|a, b| compare(a, b, key, rev, dirs));
        v.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn natural() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("File", "file"), Ordering::Less); // byte tie-break
        assert_eq!(natural_cmp("apple", "Banana"), Ordering::Less);
        assert_eq!(natural_cmp("a01", "a1"), Ordering::Less); // tie-break by bytes
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("x100y", "x99y"), Ordering::Greater);
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    }

    #[test]
    fn by_name_dirs_first() {
        let v = vec![
            e("b.txt", false, 1, "Text", 0),
            e("Zeta", true, 0, "Folder", 0),
            e("a10.txt", false, 1, "Text", 0),
            e("a2.txt", false, 1, "Text", 0),
            e("alpha", true, 0, "Folder", 0),
        ];
        assert_eq!(
            sorted(v.clone(), SortKey::Name, false, true),
            ["alpha", "Zeta", "a2.txt", "a10.txt", "b.txt"]
        );
        assert_eq!(
            sorted(v.clone(), SortKey::Name, true, true),
            ["Zeta", "alpha", "b.txt", "a10.txt", "a2.txt"]
        );
        assert_eq!(
            sorted(v, SortKey::Name, false, false),
            ["a2.txt", "a10.txt", "alpha", "b.txt", "Zeta"]
        );
    }

    #[test]
    fn by_size_type_mtime() {
        let v = vec![
            e("big", false, 100, "Video", 5),
            e("small", false, 1, "Audio", 9),
            e("mid", false, 50, "Audio", 1),
        ];
        assert_eq!(
            sorted(v.clone(), SortKey::Size, false, true),
            ["small", "mid", "big"]
        );
        assert_eq!(
            sorted(v.clone(), SortKey::Size, true, true),
            ["big", "mid", "small"]
        );
        assert_eq!(
            sorted(v.clone(), SortKey::Type, false, true),
            ["mid", "small", "big"]
        );
        assert_eq!(
            sorted(v, SortKey::Modified, false, true),
            ["small", "big", "mid"]
        );
    }

    #[test]
    fn nicks() {
        for k in SortKey::ALL {
            assert_eq!(SortKey::from_nick(k.nick()), k);
        }
        assert_eq!(SortKey::from_nick("starred"), SortKey::Name);
    }
}
