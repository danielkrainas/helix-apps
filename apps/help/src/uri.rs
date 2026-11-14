//! Help URI parsing and formatting.
//!
//! Canonical forms produced by the viewer:
//! - `help:` — home page; `help:?search=q` — search everything
//! - `help:<doc>[/<page>][?search=q][#frag]` — installed document
//! - `help:<doc>/<relative/file>` — resource inside an installed document
//! - `ghelp:<abs path>[?page=p|?search=q|?res=rel][#frag]` — document
//!   outside the help directories (a directory or an entry file)

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocRef {
    /// Installed document id, looked up in `$XDG_DATA_DIRS/help`.
    Id(String),
    /// A directory or entry file on disk (absolute).
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Home {
        search: Option<String>,
    },
    Doc {
        doc: DocRef,
        page: Option<String>,
        search: Option<String>,
        fragment: Option<String>,
    },
    /// A file inside a document (image, video…), relative to its directory.
    Resource {
        doc: DocRef,
        path: String,
    },
    /// Anything the viewer doesn't render itself (http, mailto, …).
    External(String),
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
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

/// Percent-encodes everything except unreserved characters and the
/// characters in `keep`.
pub fn percent_encode(s: &str, keep: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || "-._~".contains(c) || (b < 0x80 && keep.contains(c)) {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode(&v.replace('+', " ")))
    })
}

fn split_fragment(s: &str) -> (&str, Option<String>) {
    match s.split_once('#') {
        Some((a, f)) if !f.is_empty() => (a, Some(percent_decode(f))),
        Some((a, _)) => (a, None),
        None => (s, None),
    }
}

fn split_query(s: &str) -> (&str, &str) {
    s.split_once('?').unwrap_or((s, ""))
}

fn absolutize(p: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(p);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    // Lexical normalisation of `.` and `..`.
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Parses any URI or path the user or a page may hand to the viewer.
/// Relative file paths are resolved against `cwd`.
pub fn parse(input: &str, cwd: &Path) -> Location {
    let input = input.trim();
    let scheme = input.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
    let scheme_ok = scheme
        .as_deref()
        .map(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        })
        .unwrap_or(false);
    let rest = |s: &str| &input[s.len() + 1..];
    match scheme.as_deref() {
        Some("help-list") => Location::Home { search: None },
        Some("help") => parse_help(rest("help")),
        Some("ghelp") | Some("gnome-help") => parse_ghelp(rest(scheme.as_deref().unwrap()), cwd),
        Some("file") => {
            let r = rest("file");
            let r = r
                .strip_prefix("//")
                .map(|x| x.find('/').map(|i| &x[i..]).unwrap_or("/"))
                .unwrap_or(r);
            let (r, frag) = split_fragment(r);
            let (path, _) = split_query(r);
            path_location(absolutize(&percent_decode(path), cwd), None, frag)
        }
        _ if scheme_ok && !input.starts_with('/') && !input.starts_with('.') => {
            Location::External(input.to_string())
        }
        _ => {
            let (r, frag) = split_fragment(input);
            path_location(absolutize(r, cwd), None, frag)
        }
    }
}

fn path_location(path: PathBuf, page: Option<String>, fragment: Option<String>) -> Location {
    if path.extension().is_some_and(|e| e == "html" || e == "htm") {
        return Location::External(format!(
            "file://{}",
            percent_encode(&path.to_string_lossy(), "/")
        ));
    }
    Location::Doc {
        doc: DocRef::Path(path),
        page,
        search: None,
        fragment,
    }
}

fn parse_help(rest: &str) -> Location {
    let (rest, fragment) = split_fragment(rest);
    let (path, query) = split_query(rest);
    let search = query_param(query, "search").filter(|s| !s.trim().is_empty());
    let path = percent_decode(path);
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return Location::Home { search };
    }
    let (doc, page) = match path.split_once('/') {
        Some((d, p)) if !p.is_empty() => (d, Some(p.to_string())),
        Some((d, _)) => (d, None),
        None => (path, None),
    };
    let doc = DocRef::Id(doc.to_string());
    match page {
        // Anything that looks like a file path is a resource.
        Some(p) if p.contains('/') || has_file_extension(&p) => Location::Resource { doc, path: p },
        page => Location::Doc {
            doc,
            page,
            search,
            fragment,
        },
    }
}

fn has_file_extension(p: &str) -> bool {
    const EXT: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "svg", "webp", "ogv", "webm", "mp4", "ogg", "oga", "mp3",
        "css", "js", "txt",
    ];
    p.rsplit_once('.')
        .is_some_and(|(_, e)| EXT.contains(&e.to_ascii_lowercase().as_str()))
}

fn parse_ghelp(rest: &str, cwd: &Path) -> Location {
    let (rest, fragment) = split_fragment(rest);
    let (path, query) = split_query(rest);
    let path = percent_decode(path.strip_prefix("//").unwrap_or(path));
    let mut page = None;
    let mut search = None;
    if !query.is_empty() {
        if query.contains('=') {
            if let Some(res) = query_param(query, "res") {
                if path.starts_with('/') {
                    return Location::Resource {
                        doc: DocRef::Path(absolutize(&path, cwd)),
                        path: res,
                    };
                }
                return Location::Resource {
                    doc: DocRef::Id(path),
                    path: res,
                };
            }
            page = query_param(query, "page");
            search = query_param(query, "search").filter(|s| !s.trim().is_empty());
        } else {
            page = Some(percent_decode(query));
        }
    }
    if path.starts_with('/') {
        match path_location(absolutize(&path, cwd), page, fragment) {
            Location::Doc {
                doc,
                page,
                fragment,
                ..
            } => Location::Doc {
                doc,
                page,
                search,
                fragment,
            },
            other => other,
        }
    } else if path.is_empty() {
        Location::Home { search }
    } else {
        Location::Doc {
            doc: DocRef::Id(path),
            page,
            search,
            fragment,
        }
    }
}

impl DocRef {
    /// URI of a page (or the index when `page` is `None`).
    pub fn page_uri(&self, page: Option<&str>, fragment: Option<&str>) -> String {
        let mut s = match (self, page) {
            (DocRef::Id(id), None) => format!("help:{}", percent_encode(id, "")),
            (DocRef::Id(id), Some(p)) => {
                format!("help:{}/{}", percent_encode(id, ""), percent_encode(p, ""))
            }
            (DocRef::Path(path), None) => {
                format!("ghelp:{}", percent_encode(&path.to_string_lossy(), "/"))
            }
            (DocRef::Path(path), Some(p)) => {
                format!(
                    "ghelp:{}?page={}",
                    percent_encode(&path.to_string_lossy(), "/"),
                    percent_encode(p, "")
                )
            }
        };
        if let Some(f) = fragment.filter(|f| !f.is_empty()) {
            s.push('#');
            s.push_str(&percent_encode(f, ""));
        }
        s
    }

    pub fn search_uri(&self, query: &str) -> String {
        let q = percent_encode(query, "");
        match self {
            DocRef::Id(id) => format!("help:{}?search={q}", percent_encode(id, "")),
            DocRef::Path(p) => format!(
                "ghelp:{}?search={q}",
                percent_encode(&p.to_string_lossy(), "/")
            ),
        }
    }

    pub fn resource_uri(&self, rel: &str) -> String {
        match self {
            DocRef::Id(id) => format!(
                "help:{}/{}",
                percent_encode(id, ""),
                percent_encode(rel, "/")
            ),
            DocRef::Path(p) => {
                format!(
                    "ghelp:{}?res={}",
                    percent_encode(&p.to_string_lossy(), "/"),
                    percent_encode(rel, "/")
                )
            }
        }
    }
}

pub fn home_search_uri(query: &str) -> String {
    format!("help:?search={}", percent_encode(query, ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Location {
        parse(s, Path::new("/work"))
    }

    fn doc(id: &str, page: Option<&str>) -> Location {
        Location::Doc {
            doc: DocRef::Id(id.into()),
            page: page.map(Into::into),
            search: None,
            fragment: None,
        }
    }

    #[test]
    fn help_scheme() {
        assert_eq!(p("help:"), Location::Home { search: None });
        assert_eq!(p("help-list:"), Location::Home { search: None });
        assert_eq!(p("help:gnome-help"), doc("gnome-help", None));
        assert_eq!(p("help:gnome-help/"), doc("gnome-help", None));
        assert_eq!(
            p("help:gnome-help/net-wireless"),
            doc("gnome-help", Some("net-wireless"))
        );
        assert_eq!(
            p("help:doc/page#sec"),
            Location::Doc {
                doc: DocRef::Id("doc".into()),
                page: Some("page".into()),
                search: None,
                fragment: Some("sec".into())
            }
        );
        assert_eq!(
            p("help:doc?search=wifi%20password"),
            Location::Doc {
                doc: DocRef::Id("doc".into()),
                page: None,
                search: Some("wifi password".into()),
                fragment: None
            }
        );
        assert_eq!(
            p("help:?search=x"),
            Location::Home {
                search: Some("x".into())
            }
        );
        assert_eq!(
            p("help:doc/figures/a.png"),
            Location::Resource {
                doc: DocRef::Id("doc".into()),
                path: "figures/a.png".into()
            }
        );
        assert_eq!(
            p("help:doc/a.svg"),
            Location::Resource {
                doc: DocRef::Id("doc".into()),
                path: "a.svg".into()
            }
        );
    }

    #[test]
    fn ghelp_and_paths() {
        assert_eq!(p("ghelp:gedit"), doc("gedit", None));
        assert_eq!(p("ghelp:gedit?prefs"), doc("gedit", Some("prefs")));
        let pathdoc = |path: &str, page: Option<&str>| Location::Doc {
            doc: DocRef::Path(path.into()),
            page: page.map(Into::into),
            search: None,
            fragment: None,
        };
        assert_eq!(
            p("ghelp:/usr/share/help/C/x"),
            pathdoc("/usr/share/help/C/x", None)
        );
        assert_eq!(p("ghelp:/d?page=intro"), pathdoc("/d", Some("intro")));
        assert_eq!(p("ghelp:/d?intro"), pathdoc("/d", Some("intro")));
        assert_eq!(p("/d/index.page"), pathdoc("/d/index.page", None));
        assert_eq!(p("./docs/../mydoc"), pathdoc("/work/mydoc", None));
        assert_eq!(p("mydoc"), pathdoc("/work/mydoc", None));
        assert_eq!(p("file:///d/My%20Doc"), pathdoc("/d/My Doc", None));
        assert_eq!(
            p("ghelp:/d?res=fig/a.png"),
            Location::Resource {
                doc: DocRef::Path("/d".into()),
                path: "fig/a.png".into()
            }
        );
        assert_eq!(
            p("/x/index.html"),
            Location::External("file:///x/index.html".into())
        );
    }

    #[test]
    fn external() {
        assert_eq!(
            p("https://example.org/"),
            Location::External("https://example.org/".into())
        );
        assert_eq!(p("mailto:a@b"), Location::External("mailto:a@b".into()));
    }

    #[test]
    fn round_trip() {
        let cwd = Path::new("/");
        for (d, page, frag) in [
            (DocRef::Id("doc".into()), Some("a b"), Some("s1")),
            (DocRef::Id("doc".into()), None, None),
            (DocRef::Path("/x y/d".into()), Some("p"), None),
            (DocRef::Path("/x/d".into()), None, Some("f")),
        ] {
            let uri = d.page_uri(page, frag);
            assert_eq!(
                parse(&uri, cwd),
                Location::Doc {
                    doc: d.clone(),
                    page: page.map(Into::into),
                    search: None,
                    fragment: frag.map(Into::into)
                },
                "{uri}"
            );
            let s = d.search_uri("a&b c");
            assert!(
                matches!(parse(&s, cwd), Location::Doc { search: Some(q), .. } if q == "a&b c")
            );
            let r = d.resource_uri("figures/x 1.png");
            assert_eq!(
                parse(&r, cwd),
                Location::Resource {
                    doc: d.clone(),
                    path: "figures/x 1.png".into()
                }
            );
        }
    }
}
