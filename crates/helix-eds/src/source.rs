//! EDS data sources (address books, calendars, accounts) as described by the
//! source registry (`org.gnome.evolution.dataserver.Sources5`).

use crate::keyfile::{user_languages, KeyFile};

pub const GROUP_SOURCE: &str = "Data Source";
pub const GROUP_ADDRESS_BOOK: &str = "Address Book";
pub const GROUP_CALENDAR: &str = "Calendar";
pub const GROUP_TASK_LIST: &str = "Task List";
pub const GROUP_MEMO_LIST: &str = "Memo List";
pub const GROUP_COLLECTION: &str = "Collection";
pub const GROUP_AUTHENTICATION: &str = "Authentication";
pub const GROUP_SECURITY: &str = "Security";
pub const GROUP_WEBDAV: &str = "WebDAV Backend";
pub const GROUP_REFRESH: &str = "Refresh";
pub const GROUP_OFFLINE: &str = "Offline";

/// The kind of data a source holds (from its key-file extension groups).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    AddressBook,
    Calendar,
    TaskList,
    MemoList,
    Collection,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub uid: String,
    /// D-Bus object path in the registry (empty for sources not yet created).
    pub object_path: String,
    pub data: KeyFile,
    /// Has the `Source.Writable` interface (its data can be rewritten).
    pub writable: bool,
    /// Has the `Source.Removable` interface.
    pub removable: bool,
}

impl Source {
    pub fn from_data(uid: &str, object_path: &str, data: &str) -> Source {
        Source {
            uid: uid.to_string(),
            object_path: object_path.to_string(),
            data: KeyFile::parse(data),
            writable: false,
            removable: false,
        }
    }

    pub fn display_name(&self) -> String {
        let langs = user_languages();
        self.data
            .get_locale(GROUP_SOURCE, "DisplayName", &langs)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.uid)
            .to_string()
    }

    pub fn parent(&self) -> Option<&str> {
        self.data
            .get(GROUP_SOURCE, "Parent")
            .filter(|p| !p.is_empty())
    }

    pub fn enabled(&self) -> bool {
        self.data.get_bool(GROUP_SOURCE, "Enabled").unwrap_or(true)
    }

    pub fn kinds(&self) -> Vec<SourceKind> {
        let mut out = Vec::new();
        for (group, kind) in [
            (GROUP_ADDRESS_BOOK, SourceKind::AddressBook),
            (GROUP_CALENDAR, SourceKind::Calendar),
            (GROUP_TASK_LIST, SourceKind::TaskList),
            (GROUP_MEMO_LIST, SourceKind::MemoList),
            (GROUP_COLLECTION, SourceKind::Collection),
        ] {
            if self.data.has_group(group) {
                out.push(kind);
            }
        }
        if out.is_empty() {
            out.push(SourceKind::Other);
        }
        out
    }

    pub fn is(&self, kind: SourceKind) -> bool {
        self.kinds().contains(&kind)
    }

    fn group_for(kind: SourceKind) -> &'static str {
        match kind {
            SourceKind::AddressBook => GROUP_ADDRESS_BOOK,
            SourceKind::Calendar => GROUP_CALENDAR,
            SourceKind::TaskList => GROUP_TASK_LIST,
            SourceKind::MemoList => GROUP_MEMO_LIST,
            SourceKind::Collection => GROUP_COLLECTION,
            SourceKind::Other => GROUP_SOURCE,
        }
    }

    pub fn backend_name(&self, kind: SourceKind) -> Option<&str> {
        self.data.get(Self::group_for(kind), "BackendName")
    }

    /// Calendar colour (`#rrggbb`), if set.
    pub fn color(&self) -> Option<&str> {
        self.data
            .get(GROUP_CALENDAR, "Color")
            .filter(|c| !c.is_empty())
    }

    /// Whether the calendar is shown ("Selected" in the calendar extension).
    pub fn selected(&self) -> bool {
        self.data
            .get_bool(GROUP_CALENDAR, "Selected")
            .unwrap_or(true)
    }

    pub fn set_selected(&mut self, selected: bool) {
        self.data.set_bool(GROUP_CALENDAR, "Selected", selected);
    }

    pub fn set_color(&mut self, color: &str) {
        self.data.set(GROUP_CALENDAR, "Color", color);
    }

    pub fn set_display_name(&mut self, name: &str) {
        self.data.set_unlocalized(GROUP_SOURCE, "DisplayName", name);
    }

    /// Serialized key-file data, suitable for `Source.Writable.Write` or
    /// `SourceManager.CreateSources`.
    pub fn data_string(&self) -> String {
        self.data.to_string()
    }
}

/// Key-file data for a new local ("On This Computer") calendar.
pub fn new_local_calendar(display_name: &str, color: &str) -> String {
    let mut kf = KeyFile::default();
    kf.set(GROUP_SOURCE, "DisplayName", display_name);
    kf.set(GROUP_SOURCE, "Enabled", "true");
    kf.set(GROUP_SOURCE, "Parent", "local-stub");
    kf.set(GROUP_CALENDAR, "BackendName", "local");
    kf.set(GROUP_CALENDAR, "Color", color);
    kf.set(GROUP_CALENDAR, "Selected", "true");
    kf.to_string()
}

/// Key-file data for a new local address book.
pub fn new_local_address_book(display_name: &str) -> String {
    let mut kf = KeyFile::default();
    kf.set(GROUP_SOURCE, "DisplayName", display_name);
    kf.set(GROUP_SOURCE, "Enabled", "true");
    kf.set(GROUP_SOURCE, "Parent", "local-stub");
    kf.set(GROUP_ADDRESS_BOOK, "BackendName", "local");
    kf.to_string()
}

/// A validated calendar subscription URL (`http`, `https` or `webcal`),
/// split the way EDS stores it (Authentication / Security / WebDAV Backend).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebcalUrl {
    /// `true` for `https` and `webcal`/`webcals` (fetched over TLS).
    pub secure: bool,
    /// Host name, or an IPv6 literal without brackets.
    pub host: String,
    pub port: u16,
    /// Path, always starting with `/`.
    pub path: String,
    /// Query without the leading `?` (may be empty).
    pub query: String,
}

impl WebcalUrl {
    /// The URL EDS will fetch (`webcal` is rewritten to `https`).
    pub fn http_url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default_port = if self.secure { 443 } else { 80 };
        let port = if self.port == default_port {
            String::new()
        } else {
            format!(":{}", self.port)
        };
        let query = if self.query.is_empty() {
            String::new()
        } else {
            format!("?{}", self.query)
        };
        format!("{scheme}://{host}{port}{}{query}", self.path)
    }
}

/// Why a calendar URL was rejected (messages are user-facing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    Whitespace,
    Scheme,
    NoHost,
    BadHost,
    BadPort,
    Credentials,
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UrlError::Empty => "Enter the address of the calendar",
            UrlError::Whitespace => "The address must not contain spaces",
            UrlError::Scheme => "The address must start with http://, https:// or webcal://",
            UrlError::NoHost => "The address has no server name",
            UrlError::BadHost => "The server name is not valid",
            UrlError::BadPort => "The port number is not valid",
            UrlError::Credentials => "Remove the user name and password from the address",
        })
    }
}

impl std::error::Error for UrlError {}

/// Validates a calendar subscription URL. Accepted schemes: `http`,
/// `https`, `webcal`, `webcals` (case-insensitive; `webcal` means HTTPS).
pub fn parse_webcal_url(input: &str) -> Result<WebcalUrl, UrlError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(UrlError::Empty);
    }
    if input.chars().any(char::is_whitespace) {
        return Err(UrlError::Whitespace);
    }
    let (scheme, rest) = input.split_once("://").ok_or(UrlError::Scheme)?;
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "http" => false,
        "https" | "webcal" | "webcals" => true,
        _ => return Err(UrlError::Scheme),
    };
    let rest = rest.split('#').next().unwrap_or("");
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return Err(UrlError::Credentials);
    }
    if authority.is_empty() {
        return Err(UrlError::NoHost);
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or(UrlError::BadHost)?;
        if h.is_empty()
            || !h
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return Err(UrlError::BadHost);
        }
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':').ok_or(UrlError::BadHost)?),
        };
        (h.to_string(), port)
    } else {
        let (h, p) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        if h.is_empty() {
            return Err(UrlError::NoHost);
        }
        let label_ok = |l: &str| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        };
        if h.len() > 253 || !h.trim_end_matches('.').split('.').all(label_ok) {
            return Err(UrlError::BadHost);
        }
        (h.to_lowercase(), p)
    };
    let port = match port {
        None | Some("") => {
            if secure {
                443
            } else {
                80
            }
        }
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 && p.chars().all(|c| c.is_ascii_digit()) => n,
            _ => return Err(UrlError::BadPort),
        },
    };
    let (path, query) = match tail.split_once('?') {
        Some((p, q)) => (p, q),
        None => (tail, ""),
    };
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };
    Ok(WebcalUrl {
        secure,
        host,
        port,
        path,
        query: query.to_string(),
    })
}

/// Key-file data for a calendar subscribed from a URL (EDS `webcal`
/// backend, refreshed by EDS every 30 minutes and kept for offline use).
pub fn new_webcal_calendar(display_name: &str, color: &str, url: &WebcalUrl) -> String {
    let mut kf = KeyFile::default();
    kf.set(GROUP_SOURCE, "DisplayName", display_name);
    kf.set(GROUP_SOURCE, "Enabled", "true");
    kf.set(GROUP_SOURCE, "Parent", "webcal-stub");
    kf.set(
        GROUP_SECURITY,
        "Method",
        if url.secure { "tls" } else { "none" },
    );
    kf.set(GROUP_AUTHENTICATION, "Host", &url.host);
    kf.set(GROUP_AUTHENTICATION, "Method", "none");
    kf.set(GROUP_AUTHENTICATION, "Port", &url.port.to_string());
    kf.set(GROUP_AUTHENTICATION, "ProxyUid", "system-proxy");
    kf.set(GROUP_AUTHENTICATION, "User", "");
    kf.set(GROUP_REFRESH, "Enabled", "true");
    kf.set(GROUP_REFRESH, "IntervalMinutes", "30");
    kf.set(GROUP_WEBDAV, "ResourcePath", &url.path);
    kf.set(GROUP_WEBDAV, "ResourceQuery", &url.query);
    kf.set(GROUP_CALENDAR, "BackendName", "webcal");
    kf.set(GROUP_CALENDAR, "Color", color);
    kf.set(GROUP_CALENDAR, "Selected", "true");
    kf.set(GROUP_OFFLINE, "StaySynchronized", "true");
    kf.to_string()
}

/// The subscription URL of a `webcal` source, if it has one.
pub fn webcal_url(source: &Source) -> Option<String> {
    let path = source.data.get(GROUP_WEBDAV, "ResourcePath")?;
    let host = source.data.get(GROUP_AUTHENTICATION, "Host")?;
    let secure = source
        .data
        .get(GROUP_SECURITY, "Method")
        .is_some_and(|m| m != "none");
    let port = source
        .data
        .get(GROUP_AUTHENTICATION, "Port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let port = if port == 0 {
        if secure {
            443
        } else {
            80
        }
    } else {
        port
    };
    let query = source
        .data
        .get(GROUP_WEBDAV, "ResourceQuery")
        .unwrap_or("")
        .to_string();
    Some(
        WebcalUrl {
            secure,
            host: host.to_string(),
            port,
            path: path.to_string(),
            query,
        }
        .http_url(),
    )
}

/// A fresh source UID (40 hex chars, like EDS generates).
pub fn new_uid() -> String {
    crate::new_id()
}

/// Label for the account a source belongs to (its parent's display name, or
/// its own when it has no parent).
pub fn account_label(source: &Source, all: &[Source]) -> String {
    match source
        .parent()
        .and_then(|p| all.iter().find(|s| s.uid == p))
    {
        Some(parent) => parent.display_name(),
        None => source.display_name(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_source_fields() {
        let s = Source::from_data(
            "abc",
            "/p",
            "[Data Source]\nDisplayName=Work\nParent=google-stub\n\n[Calendar]\nBackendName=caldav\nColor=#ff0000\nSelected=false\n",
        );
        assert_eq!(s.display_name(), "Work");
        assert_eq!(s.parent(), Some("google-stub"));
        assert_eq!(s.kinds(), vec![SourceKind::Calendar]);
        assert_eq!(s.backend_name(SourceKind::Calendar), Some("caldav"));
        assert_eq!(s.color(), Some("#ff0000"));
        assert!(!s.selected());
        assert!(s.enabled());
    }

    #[test]
    fn edit_and_serialize() {
        let mut s = Source::from_data("abc", "/p", &new_local_calendar("Home", "#00ff00"));
        s.set_selected(false);
        s.set_color("#123456");
        s.set_display_name("House");
        let again = Source::from_data("abc", "/p", &s.data_string());
        assert_eq!(again.display_name(), "House");
        assert_eq!(again.color(), Some("#123456"));
        assert!(!again.selected());
        assert_eq!(again.parent(), Some("local-stub"));
    }

    #[test]
    fn webcal_urls() {
        let u = parse_webcal_url("  webcal://Example.org/cal/x.ics?token=a%20b#frag ").unwrap();
        assert_eq!(
            u,
            WebcalUrl {
                secure: true,
                host: "example.org".into(),
                port: 443,
                path: "/cal/x.ics".into(),
                query: "token=a%20b".into()
            }
        );
        assert_eq!(u.http_url(), "https://example.org/cal/x.ics?token=a%20b");
        let u = parse_webcal_url("HTTP://127.0.0.1:8080").unwrap();
        assert_eq!((u.secure, u.port, u.path.as_str()), (false, 8080, "/"));
        assert_eq!(u.http_url(), "http://127.0.0.1:8080/");
        let u = parse_webcal_url("https://[::1]:8443/a.ics").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("::1", 8443));
        assert_eq!(u.http_url(), "https://[::1]:8443/a.ics");
        assert_eq!(parse_webcal_url(""), Err(UrlError::Empty));
        assert_eq!(parse_webcal_url("https://a b/"), Err(UrlError::Whitespace));
        assert_eq!(
            parse_webcal_url("ftp://example.org/x.ics"),
            Err(UrlError::Scheme)
        );
        assert_eq!(parse_webcal_url("example.org/x.ics"), Err(UrlError::Scheme));
        assert_eq!(parse_webcal_url("file:///tmp/x.ics"), Err(UrlError::Scheme));
        assert_eq!(parse_webcal_url("https:///x.ics"), Err(UrlError::NoHost));
        assert_eq!(parse_webcal_url("https://:80/x"), Err(UrlError::NoHost));
        assert_eq!(
            parse_webcal_url("https://exa$mple.org/"),
            Err(UrlError::BadHost)
        );
        assert_eq!(
            parse_webcal_url("https://-bad.org/"),
            Err(UrlError::BadHost)
        );
        assert_eq!(
            parse_webcal_url("https://example.org:99999/"),
            Err(UrlError::BadPort)
        );
        assert_eq!(
            parse_webcal_url("https://example.org:0/"),
            Err(UrlError::BadPort)
        );
        assert_eq!(
            parse_webcal_url("https://example.org:+1/"),
            Err(UrlError::BadPort)
        );
        assert_eq!(
            parse_webcal_url("https://me:pw@example.org/"),
            Err(UrlError::Credentials)
        );
        assert!(!UrlError::Scheme.to_string().is_empty());
    }

    #[test]
    fn webcal_source_layout() {
        let url = parse_webcal_url("http://localhost:8000/feed.ics?x=1").unwrap();
        let s = Source::from_data("w", "/w", &new_webcal_calendar("Holidays", "#3584e4", &url));
        assert_eq!(s.parent(), Some("webcal-stub"));
        assert_eq!(s.backend_name(SourceKind::Calendar), Some("webcal"));
        assert_eq!(s.color(), Some("#3584e4"));
        assert!(s.selected());
        assert_eq!(s.data.get(GROUP_SECURITY, "Method"), Some("none"));
        assert_eq!(s.data.get(GROUP_AUTHENTICATION, "Host"), Some("localhost"));
        assert_eq!(s.data.get(GROUP_AUTHENTICATION, "Port"), Some("8000"));
        assert_eq!(s.data.get(GROUP_WEBDAV, "ResourcePath"), Some("/feed.ics"));
        assert_eq!(s.data.get(GROUP_WEBDAV, "ResourceQuery"), Some("x=1"));
        assert_eq!(s.data.get(GROUP_REFRESH, "Enabled"), Some("true"));
        assert_eq!(
            webcal_url(&s).as_deref(),
            Some("http://localhost:8000/feed.ics?x=1")
        );
        assert_eq!(
            webcal_url(&Source::from_data(
                "l",
                "/l",
                &new_local_calendar("L", "#000000")
            )),
            None
        );
    }

    #[test]
    fn account_label_uses_parent() {
        let parent = Source::from_data(
            "local-stub",
            "/a",
            "[Data Source]\nDisplayName=On This Computer\n",
        );
        let child = Source::from_data("x", "/b", &new_local_address_book("Friends"));
        assert!(child.is(SourceKind::AddressBook));
        assert_eq!(
            account_label(&child, &[parent.clone(), child.clone()]),
            "On This Computer"
        );
        assert_eq!(
            account_label(&parent, std::slice::from_ref(&parent)),
            "On This Computer"
        );
    }
}
