//! Address-bar input classification and search URL construction.

use serde::{Deserialize, Serialize};

/// What the user typed resolves to either a URL to load or a search query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Url(String),
    Search(String),
}

const KNOWN_SCHEMES: &[&str] = &[
    "http",
    "https",
    "file",
    "about",
    "data",
    "blob",
    "view-source",
    "ftp",
    "javascript",
    "mailto",
    "webkit",
];

/// Classifies address-bar text. `home` is used to expand `~/`.
pub fn classify(text: &str, home: &str) -> Option<Input> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(q) = text.strip_prefix('?') {
        let q = q.trim();
        return (!q.is_empty()).then(|| Input::Search(q.to_string()));
    }
    if let Some(scheme) = scheme_of(text) {
        let lower = scheme.to_ascii_lowercase();
        let rest = &text[scheme.len() + 1..];
        if KNOWN_SCHEMES.contains(&lower.as_str()) || rest.starts_with("//") {
            return Some(Input::Url(text.to_string()));
        }
    }
    if text.starts_with('/') {
        return Some(Input::Url(file_uri(text)));
    }
    if let Some(rest) = text.strip_prefix("~/") {
        let home = home.trim_end_matches('/');
        return Some(Input::Url(file_uri(&format!("{home}/{rest}"))));
    }
    if text.chars().any(char::is_whitespace) {
        return Some(Input::Search(text.to_string()));
    }
    let (authority, _) = split_authority(text);
    let (host, port) = split_port(authority);
    if port.is_some_and(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return Some(Input::Search(text.to_string()));
    }
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "localhost" || is_ipv4(host) || is_ipv6_literal(host) {
        return Some(Input::Url(format!("http://{text}")));
    }
    if is_domain(host) {
        return Some(Input::Url(format!("https://{text}")));
    }
    Some(Input::Search(text.to_string()))
}

/// Returns the scheme part of `text` if it starts with `scheme:`.
fn scheme_of(text: &str) -> Option<&str> {
    let idx = text.find(':')?;
    let scheme = &text[..idx];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    chars
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .then_some(scheme)
}

fn split_authority(text: &str) -> (&str, &str) {
    match text.find(['/', '?', '#']) {
        Some(i) => (&text[..i], &text[i..]),
        None => (text, ""),
    }
}

fn split_port(authority: &str) -> (&str, Option<&str>) {
    if authority.starts_with('[') {
        if let Some(end) = authority.find(']') {
            let host = &authority[..=end];
            let rest = &authority[end + 1..];
            return (host, rest.strip_prefix(':'));
        }
        return (authority, None);
    }
    match authority.rfind(':') {
        Some(i) => (&authority[..i], Some(&authority[i + 1..])),
        None => (authority, None),
    }
}

fn is_ipv4(host: &str) -> bool {
    host.parse::<std::net::Ipv4Addr>().is_ok()
}

fn is_ipv6_literal(host: &str) -> bool {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .is_some_and(|h| h.parse::<std::net::Ipv6Addr>().is_ok())
}

fn is_domain(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_alphanumeric() || c == '-')
    };
    if !labels.iter().all(|l| label_ok(l)) {
        return false;
    }
    let tld = labels[labels.len() - 1].to_ascii_lowercase();
    tld.starts_with("xn--") || (tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic))
}

/// Builds a `file://` URI from an absolute path, percent-encoding as needed.
pub fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `application/x-www-form-urlencoded` encoding of a query string.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b' ' => out.push('+'),
            b if b.is_ascii_alphanumeric() || b"-._~".contains(&b) => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchEngine {
    pub name: String,
    /// URL template; `%s` is replaced by the encoded query.
    pub url: String,
}

impl SearchEngine {
    pub fn new(name: &str, url: &str) -> Self {
        SearchEngine {
            name: name.into(),
            url: url.into(),
        }
    }

    pub fn search_url(&self, query: &str) -> String {
        self.url.replace("%s", &form_encode(query))
    }
}

pub fn default_engines() -> Vec<SearchEngine> {
    vec![
        SearchEngine::new("DuckDuckGo", "https://duckduckgo.com/?q=%s"),
        SearchEngine::new("Google", "https://www.google.com/search?q=%s"),
        SearchEngine::new("Bing", "https://www.bing.com/search?q=%s"),
        SearchEngine::new(
            "Wikipedia",
            "https://en.wikipedia.org/w/index.php?search=%s",
        ),
        SearchEngine::new("Startpage", "https://www.startpage.com/do/search?query=%s"),
        SearchEngine::new("Brave Search", "https://search.brave.com/search?q=%s"),
    ]
}

/// Validates a user-supplied engine template.
pub fn valid_template(url: &str) -> bool {
    let url = url.trim();
    url.contains("%s") && (url.starts_with("https://") || url.starts_with("http://"))
}

/// Resolves address-bar text to the URL that should be loaded.
pub fn resolve(text: &str, home: &str, engine: &SearchEngine) -> Option<String> {
    match classify(text, home)? {
        Input::Url(u) => Some(u),
        Input::Search(q) => Some(engine.search_url(&q)),
    }
}

/// Returns `scheme://host[:port]` for http(s)/file URIs, used as the key for
/// per-site decisions.
pub fn origin_of(uri: &str) -> Option<String> {
    let scheme = scheme_of(uri)?.to_ascii_lowercase();
    let rest = uri[scheme.len() + 1..].strip_prefix("//")?;
    let (authority, _) = split_authority(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return (scheme == "file").then(|| "file://".to_string());
    }
    let (host, port) = split_port(authority);
    let host = host.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => Some("80"),
        "https" => Some("443"),
        _ => None,
    };
    match port {
        Some(p) if !p.is_empty() && Some(p) != default_port => {
            Some(format!("{scheme}://{host}:{p}"))
        }
        _ => Some(format!("{scheme}://{host}")),
    }
}

/// Host part of a URI, for display.
pub fn host_of(uri: &str) -> Option<String> {
    let origin = origin_of(uri)?;
    let rest = origin.split_once("://")?.1;
    (!rest.is_empty()).then(|| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Option<Input> {
        Some(Input::Url(s.into()))
    }
    fn search(s: &str) -> Option<Input> {
        Some(Input::Search(s.into()))
    }
    fn c(s: &str) -> Option<Input> {
        classify(s, "/home/u")
    }

    #[test]
    fn schemes_pass_through() {
        assert_eq!(c("https://example.com"), url("https://example.com"));
        assert_eq!(c("  http://a/b  "), url("http://a/b"));
        assert_eq!(c("about:blank"), url("about:blank"));
        assert_eq!(c("file:///tmp/x.html"), url("file:///tmp/x.html"));
        assert_eq!(c("data:text/html,hi"), url("data:text/html,hi"));
        assert_eq!(c("gemini://x.y/"), url("gemini://x.y/"));
        assert_eq!(c("HTTPS://EXAMPLE.COM"), url("HTTPS://EXAMPLE.COM"));
    }

    #[test]
    fn domains_get_https() {
        assert_eq!(c("example.com"), url("https://example.com"));
        assert_eq!(
            c("www.gnome.org/foo?bar=1"),
            url("https://www.gnome.org/foo?bar=1")
        );
        assert_eq!(c("example.com:8443/x"), url("https://example.com:8443/x"));
        assert_eq!(
            c("xn--bcher-kva.example"),
            url("https://xn--bcher-kva.example")
        );
        assert_eq!(c("bücher.de"), url("https://bücher.de"));
    }

    #[test]
    fn local_hosts_get_http() {
        assert_eq!(c("localhost"), url("http://localhost"));
        assert_eq!(c("localhost:8080/app"), url("http://localhost:8080/app"));
        assert_eq!(c("192.168.1.1"), url("http://192.168.1.1"));
        assert_eq!(c("10.0.0.2:3000"), url("http://10.0.0.2:3000"));
        assert_eq!(c("[::1]:8080"), url("http://[::1]:8080"));
    }

    #[test]
    fn paths_become_file_uris() {
        assert_eq!(c("/tmp/a b.html"), url("file:///tmp/a%20b.html"));
        assert_eq!(c("~/page.html"), url("file:///home/u/page.html"));
    }

    #[test]
    fn searches() {
        assert_eq!(c("rust gtk"), search("rust gtk"));
        assert_eq!(c("hello"), search("hello"));
        assert_eq!(c("what is 1.5"), search("what is 1.5"));
        assert_eq!(c("1.5"), search("1.5"));
        assert_eq!(c("?example.com"), search("example.com"));
        assert_eq!(c("foo:bar"), search("foo:bar"));
        assert_eq!(c("a..b"), search("a..b"));
        assert_eq!(c("-a.com"), search("-a.com"));
        assert_eq!(c(""), None);
        assert_eq!(c("   "), None);
        assert_eq!(c("?"), None);
    }

    #[test]
    fn search_urls() {
        let ddg = &default_engines()[0];
        assert_eq!(ddg.name, "DuckDuckGo");
        assert_eq!(ddg.search_url("a b&c"), "https://duckduckgo.com/?q=a+b%26c");
        assert_eq!(ddg.search_url("é"), "https://duckduckgo.com/?q=%C3%A9");
        assert_eq!(
            resolve("rust lang", "/h", ddg).unwrap(),
            "https://duckduckgo.com/?q=rust+lang"
        );
        assert!(valid_template("https://x.org/?q=%s"));
        assert!(!valid_template("https://x.org/"));
        assert!(!valid_template("ftp://x.org/%s"));
    }

    #[test]
    fn origins() {
        assert_eq!(
            origin_of("https://Example.com/a?b").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("https://example.com:443/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("http://a.b:8080").as_deref(),
            Some("http://a.b:8080")
        );
        assert_eq!(
            origin_of("https://u:p@h.org/").as_deref(),
            Some("https://h.org")
        );
        assert_eq!(origin_of("file:///tmp/x").as_deref(), Some("file://"));
        assert_eq!(origin_of("about:blank"), None);
        assert_eq!(host_of("https://h.org/x").as_deref(), Some("h.org"));
        assert_eq!(host_of("file:///x"), None);
    }
}
