//! Bookmarks store and Netscape bookmark-file import/export.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Unix seconds.
    #[serde(default)]
    pub added: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
}

pub struct Bookmarks {
    path: Option<PathBuf>,
    items: Vec<Bookmark>,
}

impl Bookmarks {
    #[cfg(test)]
    pub fn memory() -> Self {
        Bookmarks {
            path: None,
            items: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Self {
        let file: File = crate::store::load(path);
        let mut b = Bookmarks {
            path: Some(path.to_owned()),
            items: Vec::new(),
        };
        for bm in file.bookmarks {
            b.add(bm);
        }
        b
    }

    pub fn serialize(&self) -> Option<(PathBuf, String)> {
        let path = self.path.clone()?;
        Some((
            path,
            crate::store::to_json(&File {
                bookmarks: self.items.clone(),
            }),
        ))
    }

    #[cfg(test)]
    pub fn all(&self) -> &[Bookmark] {
        &self.items
    }

    pub fn get(&self, url: &str) -> Option<&Bookmark> {
        self.items.iter().find(|b| b.url == url)
    }

    pub fn contains(&self, url: &str) -> bool {
        self.get(url).is_some()
    }

    /// Adds a bookmark; returns false if the URL is already bookmarked.
    pub fn add(&mut self, mut bm: Bookmark) -> bool {
        if bm.url.is_empty() || self.contains(&bm.url) {
            return false;
        }
        bm.tags = normalize_tags(bm.tags.iter().map(String::as_str));
        self.items.push(bm);
        true
    }

    pub fn update(&mut self, url: &str, title: &str, tags: &[String]) -> bool {
        match self.items.iter_mut().find(|b| b.url == url) {
            Some(b) => {
                b.title = title.to_string();
                b.tags = normalize_tags(tags.iter().map(String::as_str));
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, url: &str) -> bool {
        let n = self.items.len();
        self.items.retain(|b| b.url != url);
        n != self.items.len()
    }

    /// Bookmarks whose title, URL or tags contain all `query` terms.
    pub fn search(&self, query: &str) -> Vec<Bookmark> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.items
            .iter()
            .filter(|b| {
                let hay = format!("{} {} {}", b.title, b.url, b.tags.join(" ")).to_lowercase();
                terms.iter().all(|t| hay.contains(t.as_str()))
            })
            .cloned()
            .collect()
    }

    /// Imports a Netscape bookmark file; returns how many were added.
    pub fn import_html(&mut self, html: &str, now: i64) -> usize {
        parse_html(html, now)
            .into_iter()
            .filter(|b| self.add(b.clone()))
            .count()
    }

    pub fn export_html(&self) -> String {
        export_html(&self.items)
    }
}

/// Splits/normalises a comma-separated tag list.
pub fn parse_tags(text: &str) -> Vec<String> {
    normalize_tags(text.split(','))
}

fn normalize_tags<'a>(tags: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim();
        if !t.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(t)) {
            out.push(t.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub fn unescape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[..rest.len().min(12)].find(';');
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((c, end + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parses `name="value"` / `name=value` attributes of a tag body.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = tag.char_indices().peekable();
    // Skip the tag name.
    while let Some(&(_, c)) = chars.peek() {
        if c.is_whitespace() {
            break;
        }
        chars.next();
    }
    loop {
        while chars
            .peek()
            .is_some_and(|&(_, c)| c.is_whitespace() || c == '/')
        {
            chars.next();
        }
        let Some(&(start, _)) = chars.peek() else {
            break;
        };
        let mut end = start;
        while let Some(&(i, c)) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            end = i + c.len_utf8();
            chars.next();
        }
        let name = tag[start..end].to_ascii_lowercase();
        if name.is_empty() {
            chars.next();
            continue;
        }
        if chars.peek().map(|&(_, c)| c) != Some('=') {
            out.push((name, String::new()));
            continue;
        }
        chars.next();
        let value = match chars.peek().map(|&(_, c)| c) {
            Some(q @ ('"' | '\'')) => {
                chars.next();
                let mut v = String::new();
                for (_, c) in chars.by_ref() {
                    if c == q {
                        break;
                    }
                    v.push(c);
                }
                v
            }
            _ => {
                let mut v = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    v.push(c);
                    chars.next();
                }
                v
            }
        };
        out.push((name, unescape_html(&value)));
    }
    out
}

fn tag_name(tag: &str) -> String {
    tag.split(|c: char| c.is_whitespace() || c == '>')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Parses a Netscape bookmark file. Folder names become tags.
pub fn parse_html(html: &str, now: i64) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    // A folder heading applies to the next <DL> opened.
    let mut pending_folder: Option<String> = None;
    // Per open <DL>: whether it pushed a folder.
    let mut dl_stack: Vec<bool> = Vec::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        if rest.starts_with("!--") {
            rest = rest.find("-->").map(|i| &rest[i + 3..]).unwrap_or("");
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[..gt];
        rest = &rest[gt + 1..];
        match tag_name(tag).as_str() {
            "h3" => {
                let end = find_ci(rest, "</h3").unwrap_or(rest.len());
                pending_folder = Some(unescape_html(strip_tags(&rest[..end]).trim()));
                rest = &rest[end..];
            }
            "dl" => match pending_folder.take() {
                Some(f) => {
                    folders.push(f);
                    dl_stack.push(true);
                }
                None => dl_stack.push(false),
            },
            "/dl" => {
                if dl_stack.pop() == Some(true) {
                    folders.pop();
                }
            }
            "a" => {
                let attrs = attributes(tag);
                let get = |n: &str| attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
                let Some(href) = get("href").filter(|h| !h.trim().is_empty()) else {
                    continue;
                };
                let href = href.trim().to_string();
                if href.to_ascii_lowercase().starts_with("javascript:")
                    || href.to_ascii_lowercase().starts_with("place:")
                {
                    continue;
                }
                let end = find_ci(rest, "</a").unwrap_or(rest.len());
                let title = unescape_html(strip_tags(&rest[..end]).trim());
                rest = &rest[end..];
                let mut tags: Vec<String> = folders.clone();
                if let Some(t) = get("tags") {
                    tags.extend(parse_tags(&t));
                }
                let added = get("add_date")
                    .and_then(|d| d.trim().parse().ok())
                    .unwrap_or(now);
                out.push(Bookmark {
                    url: href,
                    title,
                    tags: normalize_tags(tags.iter().map(String::as_str)),
                    added,
                });
            }
            _ => {}
        }
    }
    out
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let lower = hay.to_ascii_lowercase();
    lower.find(needle)
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn write_link(out: &mut String, b: &Bookmark, indent: &str) {
    out.push_str(&format!(
        "{indent}<DT><A HREF=\"{}\" ADD_DATE=\"{}\"{}>{}</A>\n",
        escape_html(&b.url),
        b.added,
        if b.tags.is_empty() {
            String::new()
        } else {
            format!(" TAGS=\"{}\"", escape_html(&b.tags.join(",")))
        },
        escape_html(&b.title),
    ));
}

/// Writes a Netscape bookmark file: untagged bookmarks at the top level,
/// tagged ones once under a folder named after their first tag (the full
/// tag set is preserved in `TAGS`).
pub fn export_html(items: &[Bookmark]) -> String {
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n\
         <!-- This is an automatically generated file. It will be read and overwritten. DO NOT EDIT! -->\n\
         <META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n\
         <TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
    );
    let mut folders: Vec<&str> = Vec::new();
    for b in items {
        match b.tags.first() {
            None => write_link(&mut out, b, "    "),
            Some(t) if !folders.contains(&t.as_str()) => folders.push(t),
            _ => {}
        }
    }
    for folder in folders {
        out.push_str(&format!(
            "    <DT><H3>{}</H3>\n    <DL><p>\n",
            escape_html(folder)
        ));
        for b in items
            .iter()
            .filter(|b| b.tags.first().map(String::as_str) == Some(folder))
        {
            write_link(&mut out, b, "        ");
        }
        out.push_str("    </DL><p>\n");
    }
    out.push_str("</DL><p>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm(url: &str, title: &str, tags: &[&str]) -> Bookmark {
        Bookmark {
            url: url.into(),
            title: title.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            added: 7,
        }
    }

    #[test]
    fn store_ops() {
        let mut b = Bookmarks::memory();
        assert!(b.add(bm("https://a/", "A", &["x", " X ", "y"])));
        assert!(!b.add(bm("https://a/", "dup", &[])));
        assert_eq!(b.get("https://a/").unwrap().tags, vec!["x", "y"]);
        assert!(b.update("https://a/", "AA", &parse_tags("one, two,,one")));
        assert_eq!(b.get("https://a/").unwrap().tags, vec!["one", "two"]);
        assert_eq!(b.search("aa two").len(), 1);
        assert_eq!(b.search("zzz").len(), 0);
        assert!(b.remove("https://a/"));
        assert!(!b.contains("https://a/"));
    }

    #[test]
    fn entities() {
        assert_eq!(
            unescape_html("a &amp; b &lt;c&gt; &#39;&#x41;&quot; &bogus; &"),
            "a & b <c> 'A\" &bogus; &"
        );
        assert_eq!(
            escape_html("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn import_nested_folders() {
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<!-- comment <A HREF="https://ignored/">x</A> -->
<TITLE>Bookmarks</TITLE>
<DL><p>
  <DT><A HREF="https://top.example/" ADD_DATE="123">Top &amp; Co</A>
  <DT><H3 ADD_DATE="1">Dev</H3>
  <DL><p>
    <DT><a href='https://docs.rs/' tags="rust,docs">Docs <b>rs</b></a>
    <DT><H3>Nested</H3>
    <DL><p>
      <DT><A HREF="https://deep.example/">Deep</A>
    </DL><p>
    <DT><A HREF="https://after.example/">After</A>
  </DL><p>
  <DT><A HREF="javascript:alert(1)">bookmarklet</A>
  <DT><A HREF="https://last.example/">Last</A>
</DL>"#;
        let v = parse_html(html, 99);
        let urls: Vec<&str> = v.iter().map(|b| b.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://top.example/",
                "https://docs.rs/",
                "https://deep.example/",
                "https://after.example/",
                "https://last.example/"
            ]
        );
        assert_eq!(v[0].title, "Top & Co");
        assert_eq!(v[0].added, 123);
        assert!(v[0].tags.is_empty());
        assert_eq!(v[1].title, "Docs rs");
        assert_eq!(v[1].tags, vec!["Dev", "rust", "docs"]);
        assert_eq!(v[2].tags, vec!["Dev", "Nested"]);
        assert_eq!(v[3].tags, vec!["Dev"]);
        assert!(v[4].tags.is_empty());
        assert_eq!(v[4].added, 99);
    }

    #[test]
    fn export_roundtrip() {
        let items = vec![
            bm("https://a.example/?x=1&y=\"2\"", "A <b>", &[]),
            bm("https://b.example/", "B", &["work", "rust"]),
            bm("https://c.example/", "C", &["work"]),
            bm("https://d.example/", "Ünïcode", &["misc"]),
        ];
        let html = export_html(&items);
        assert!(html.starts_with("<!DOCTYPE NETSCAPE-Bookmark-file-1>"));
        let back = parse_html(&html, 0);
        assert_eq!(back.len(), items.len());
        for orig in &items {
            let got = back
                .iter()
                .find(|b| b.url == orig.url)
                .expect("bookmark exported");
            assert_eq!(got.title, orig.title);
            assert_eq!(got.tags, orig.tags);
            assert_eq!(got.added, orig.added);
        }
        let mut store = Bookmarks::memory();
        assert_eq!(store.import_html(&html, 0), 4);
        assert_eq!(store.import_html(&html, 0), 0);
    }

    #[test]
    fn persists() {
        let dir = crate::store::temp_dir("bookmarks");
        let path = dir.join("bookmarks.json");
        let mut b = Bookmarks::load(&path);
        b.add(bm("https://a/", "A", &["t"]));
        let (p, s) = b.serialize().unwrap();
        crate::store::write_atomic(&p, &s).unwrap();
        assert_eq!(Bookmarks::load(&path).all(), b.all());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
