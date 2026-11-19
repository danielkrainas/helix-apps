//! Download destination naming, and which downloads start without
//! asking.

use std::path::{Path, PathBuf};

/// Compound extensions kept together when numbering duplicates.
const COMPOUND: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz", ".tar.Z",
];

/// Makes a server-suggested file name safe to create in a directory.
pub fn sanitize(name: &str) -> String {
    // Only the last path component counts.
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            // Control and formatting characters (with a right-to-left
            // override, "invoice<RLO>fdp.desktop" looks like it ends in
            // ".pdf").
            if c.is_control()
                || helix_app_common::links::is_format_char(c)
                || matches!(c, '/' | '\\' | '\0')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    // Keep names within common filesystem limits (255 bytes).
    let mut out = String::new();
    for c in cleaned.chars() {
        if out.len() + c.len_utf8() > 200 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "download".into()
    } else {
        out
    }
}

/// Whether a page's downloads may start without asking: the first one
/// after a navigation the user started (a click, a typed address, back or
/// reload). Anything a page starts on its own, and every download after
/// the first, is confirmed.
#[derive(Debug, Default, Clone, Copy)]
pub struct Gate {
    user_started: bool,
    admitted: u32,
}

impl Gate {
    /// A navigation began; redirects continue the one before.
    pub fn navigation(&mut self, by_user: bool, redirect: bool) {
        if !redirect {
            self.user_started = by_user;
            self.admitted = 0;
        }
    }

    /// Whether a download may start without asking (counting it if so).
    pub fn admit(&mut self) -> bool {
        let ok = self.user_started && self.admitted == 0;
        if ok {
            self.admitted += 1;
        }
        ok
    }
}

/// Splits `name` into stem and extension (with the dot), honouring
/// compound extensions such as `.tar.gz`.
pub fn split_ext(name: &str) -> (&str, &str) {
    for ext in COMPOUND {
        if name.len() > ext.len()
            && name
                .to_ascii_lowercase()
                .ends_with(&ext.to_ascii_lowercase())
        {
            let i = name.len() - ext.len();
            return (&name[..i], &name[i..]);
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Returns `dir/name`, or `dir/stem (N).ext` for the first N ≥ 1 for which
/// `taken` is false.
pub fn unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let name = sanitize(name);
    let first = dir.join(&name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = split_ext(&name);
    (1u32..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("unbounded search")
}

/// Best-effort file name from a URI when the server suggests none.
pub fn name_from_uri(uri: &str) -> String {
    let path = uri.split(['?', '#']).next().unwrap_or("");
    let path = path.split_once("://").map(|(_, r)| r).unwrap_or(path);
    let last = path
        .split('/')
        .skip(1)
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or("");
    percent_decode(last)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_characters_are_replaced() {
        assert_eq!(
            sanitize("invoice\u{202E}fdp.desktop"),
            "invoice_fdp.desktop"
        );
        assert_eq!(sanitize("a\u{2066}b\u{200B}c.txt"), "a_b_c.txt");
        assert_eq!(sanitize("café.pdf"), "café.pdf");
    }

    #[test]
    fn only_the_first_user_started_download_is_automatic() {
        let mut g = Gate::default();
        // Nothing the user started yet.
        assert!(!g.admit());
        g.navigation(true, false);
        assert!(g.admit());
        assert!(!g.admit(), "a second download from the page asks");
        // A redirect continues the user's navigation, without a new allowance.
        g.navigation(false, true);
        assert!(!g.admit());
        g.navigation(true, false);
        g.navigation(false, true);
        assert!(g.admit());
        // A page navigating by itself asks.
        g.navigation(false, false);
        assert!(!g.admit());
    }

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize("report.pdf"), "report.pdf");
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("a\\b\\c.txt"), "c.txt");
        assert_eq!(sanitize(".hidden"), "hidden");
        assert_eq!(sanitize("  "), "download");
        assert_eq!(sanitize(""), "download");
        assert_eq!(sanitize("..."), "download");
        assert_eq!(sanitize("a\nb.txt"), "a_b.txt");
        assert!(sanitize(&"x".repeat(1000)).len() <= 200);
    }

    #[test]
    fn extensions() {
        assert_eq!(split_ext("a.tar.gz"), ("a", ".tar.gz"));
        assert_eq!(split_ext("photo.JPG"), ("photo", ".JPG"));
        assert_eq!(split_ext("README"), ("README", ""));
        assert_eq!(split_ext(".tar.gz"), (".tar", ".gz"));
        assert_eq!(split_ext("v1.2.zip"), ("v1.2", ".zip"));
    }

    #[test]
    fn uniquing_in_temp_dir() {
        let dir = crate::store::temp_dir("downloads");
        let exists = |p: &Path| p.exists();
        let p = unique_path(&dir, "file.tar.gz", exists);
        assert_eq!(p, dir.join("file.tar.gz"));
        std::fs::write(&p, b"x").unwrap();
        let p1 = unique_path(&dir, "file.tar.gz", exists);
        assert_eq!(p1, dir.join("file (1).tar.gz"));
        std::fs::write(&p1, b"x").unwrap();
        assert_eq!(
            unique_path(&dir, "file.tar.gz", exists),
            dir.join("file (2).tar.gz")
        );
        std::fs::write(dir.join("noext"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "noext", exists), dir.join("noext (1)"));
        // Reserved (in-flight) names count as taken.
        let reserved = [dir.join("r.txt")];
        assert_eq!(
            unique_path(&dir, "r.txt", |p| p.exists()
                || reserved.iter().any(|r| r == p)),
            dir.join("r (1).txt")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_from_uris() {
        assert_eq!(name_from_uri("https://x.org/a/b%20c.zip?d=1#e"), "b c.zip");
        assert_eq!(name_from_uri("https://x.org/"), "");
        assert_eq!(name_from_uri("https://x.org/dir/"), "dir");
        assert_eq!(name_from_uri("https://x.org/100%"), "100%");
    }
}
