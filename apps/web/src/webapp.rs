//! "Install Site as Web Application" through the XDG DynamicLauncher portal
//! (`org.freedesktop.portal.DynamicLauncher`), and the web-app mode the
//! installed launcher starts (`helix-web --web-app=ID URL`).
//!
//! Each web app runs as its own application instance with the ID
//! `org.helixdesktop.Web.WebApp_<ID>` and keeps its own WebKit data in
//! `$XDG_DATA_HOME/helix-web/web-apps/<ID>/`.

use helix_app_common::i18n::tr;
use std::collections::HashMap;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

pub const APP_ID_PREFIX: &str = "org.helixdesktop.Web.WebApp_";
/// Desktop entry key recording the start URL.
pub const URL_KEY: &str = "X-Helix-Web-App-Url";

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const LAUNCHER: &str = "org.freedesktop.portal.DynamicLauncher";
const REQUEST: &str = "org.freedesktop.portal.Request";
/// `launcher_type` value for a web application.
const LAUNCHER_WEBAPP: u32 = 2;

/// Web-app identity used by the running instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebApp {
    pub id: String,
    pub url: String,
}

impl WebApp {
    pub fn app_id(&self) -> String {
        format!("{APP_ID_PREFIX}{}", self.id)
    }

    pub fn desktop_file_id(&self) -> String {
        format!("{}.desktop", self.app_id())
    }

    /// Whether `uri` belongs to this web app (same host, or a subdomain
    /// of it); other links open in the regular browser.
    pub fn owns(&self, uri: &str) -> bool {
        let (Some(home), Some(other)) = (host(&self.url), host(uri)) else {
            return false;
        };
        let home = home.trim_start_matches("www.");
        other == home || other.ends_with(&format!(".{home}"))
    }

    /// Whether a link to `uri` stays inside the web app: the app's own site
    /// or one of the extra allowed sites in `prefs`.
    pub fn keeps(&self, uri: &str, prefs: &Prefs) -> bool {
        self.owns(uri) || prefs.allows(uri)
    }
}

/// Per-web-app preferences, stored as `web-app.json` in the web app's data
/// directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Prefs {
    /// Extra sites whose links stay inside the web app (canonical entries
    /// from [`normalize_site`]), like Epiphany's `additional-urls`.
    #[serde(rename = "additional-urls", default)]
    pub additional_urls: Vec<String>,
}

pub const PREFS_FILE: &str = "web-app.json";

impl Prefs {
    /// Adds a site; returns the canonical entry, or `None` if `input` is
    /// not a valid site. Adding an existing site changes nothing.
    pub fn add(&mut self, input: &str) -> Option<String> {
        let site = normalize_site(input)?;
        if !self.additional_urls.contains(&site) {
            self.additional_urls.push(site.clone());
        }
        Some(site)
    }

    pub fn remove(&mut self, site: &str) {
        self.additional_urls.retain(|s| s != site);
    }

    /// Whether `uri` is on one of the extra allowed sites.
    pub fn allows(&self, uri: &str) -> bool {
        let Ok(u) = url::Url::parse(uri) else {
            return false;
        };
        self.additional_urls
            .iter()
            .filter_map(|s| SiteRule::parse(s))
            .any(|r| r.matches(&u))
    }
}

/// One extra allowed site: `[scheme://][*.]host[:port]`.
///
/// - Without a scheme both http and https match; with one, only that scheme.
/// - `*.host` matches the host and all its subdomains; otherwise the host
///   must match exactly.
/// - Without a port only the default port of the link's scheme matches;
///   with one, only that port (a scheme-less entry is parsed as https, so
///   `host:443` is the same as `host`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SiteRule {
    scheme: Option<String>,
    host: String,
    subdomains: bool,
    port: Option<u16>,
}

impl SiteRule {
    fn parse(input: &str) -> Option<SiteRule> {
        let input = input.trim();
        let (scheme, rest) = match input.split_once("://") {
            Some((s, r)) => (Some(s.to_ascii_lowercase()), r),
            None => (None, input),
        };
        if scheme
            .as_deref()
            .is_some_and(|s| s != "http" && s != "https")
        {
            return None;
        }
        let (subdomains, rest) = match rest.strip_prefix("*.") {
            Some(r) => (true, r),
            None => (false, rest),
        };
        // Let the URL parser validate and normalise (IDNA, case, default
        // port) the authority; any path, query or fragment is dropped.
        let u = url::Url::parse(&format!(
            "{}://{rest}",
            scheme.as_deref().unwrap_or("https")
        ))
        .ok()?;
        if !u.username().is_empty() || u.password().is_some() {
            return None;
        }
        let host = match u.host()? {
            url::Host::Domain(d) => {
                if d.split('.').any(str::is_empty) {
                    return None;
                }
                d.to_string()
            }
            // `*.` only makes sense for names.
            _ if subdomains => return None,
            h => h.to_string(),
        };
        let port = u.port();
        Some(SiteRule {
            scheme,
            host,
            subdomains,
            port,
        })
    }

    fn matches(&self, u: &url::Url) -> bool {
        let scheme = u.scheme();
        if !matches!(scheme, "http" | "https") {
            return false;
        }
        if self.scheme.as_deref().is_some_and(|s| s != scheme) {
            return false;
        }
        let port_ok = match self.port {
            Some(p) => u.port_or_known_default() == Some(p),
            None => u.port().is_none(),
        };
        let Some(host) = u.host_str() else {
            return false;
        };
        let host = host.to_ascii_lowercase();
        let host_ok =
            host == self.host || (self.subdomains && host.ends_with(&format!(".{}", self.host)));
        port_ok && host_ok
    }
}

impl std::fmt::Display for SiteRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(s) = &self.scheme {
            write!(f, "{s}://")?;
        }
        if self.subdomains {
            write!(f, "*.")?;
        }
        write!(f, "{}", self.host)?;
        if let Some(p) = self.port {
            write!(f, ":{p}")?;
        }
        Ok(())
    }
}

/// The canonical form of a site typed by the user (`Example.com/login` →
/// `example.com`, `https://SSO.example.com:443/x` → `https://sso.example.com`,
/// `*.Example.org` → `*.example.org`), or `None` if it is not a valid
/// http(s) site.
pub fn normalize_site(input: &str) -> Option<String> {
    SiteRule::parse(input).map(|r| r.to_string())
}

fn host(uri: &str) -> Option<String> {
    let u = url::Url::parse(uri).ok()?;
    matches!(u.scheme(), "http" | "https")
        .then(|| u.host_str().map(|h| h.to_ascii_lowercase()))
        .flatten()
}

/// An application-ID-safe identifier from a name (letters, digits and
/// underscores; never starts with a digit; max 64 chars).
pub fn make_id(name: &str, url: &str) -> String {
    let mut id = String::new();
    let mut last_us = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
            last_us = false;
        } else if !last_us {
            id.push('_');
            last_us = true;
        }
    }
    let mut id = id.trim_end_matches('_').to_string();
    if id.is_empty() {
        id = host(url)
            .unwrap_or_else(|| "site".into())
            .replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    }
    if id.starts_with(|c: char| c.is_ascii_digit()) {
        id.insert(0, '_');
    }
    // Disambiguate apps with the same name for different sites.
    let hash = url.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    id.truncate(56);
    format!("{id}_{:06x}", hash & 0xff_ffff)
}

/// Parses `--web-app=ID` (validated).
pub fn parse_arg(arg: &str) -> Option<String> {
    let id = arg.strip_prefix("--web-app=")?;
    (!id.is_empty()
        && id.len() <= 80
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !id.starts_with(|c: char| c.is_ascii_digit()))
    .then(|| id.to_string())
}

/// Quotes one argument for a desktop entry `Exec` line (Desktop Entry
/// Specification, "The Exec key"), including the string-value escaping
/// of backslashes.
pub fn exec_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@+,".contains(c));
    let escaped_percent = arg.replace('%', "%%");
    if plain {
        return escaped_percent;
    }
    let mut inner = String::new();
    for c in escaped_percent.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            inner.push('\\');
        }
        inner.push(c);
    }
    // Desktop-entry string escaping doubles every backslash once more.
    format!("\"{}\"", inner.replace('\\', "\\\\"))
}

/// Strips line breaks so a value cannot inject extra desktop entry keys.
fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Desktop entry body for the portal's `Install` call (the portal adds
/// `Name` and `Icon`).
pub fn desktop_entry(app: &WebApp, exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Exec={} --web-app={} {}\n\
         Terminal=false\n\
         StartupNotify=true\n\
         StartupWMClass={}\n\
         Categories=Network;GTK;\n\
         {URL_KEY}={}\n",
        exec_quote(exe),
        app.id,
        exec_quote(&one_line(&app.url)),
        app.app_id(),
        one_line(&app.url),
    )
}

/// Request object path the portal will use for `token`.
pub fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

#[derive(Debug)]
pub enum Error {
    Bus(zbus::Error),
    Cancelled,
    Failed(String),
}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::Bus(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::Cancelled => write!(f, "cancelled"),
            Error::Failed(s) => write!(f, "{s}"),
        }
    }
}

/// Asks the portal to install a launcher. `icon` is PNG/JPEG/SVG data.
/// Blocks until the user confirms or cancels; call from a worker thread.
/// Returns the installed web app (its ID derives from the confirmed name).
pub fn install(name: &str, url: &str, icon: &[u8], exe: &str) -> Result<(WebApp, String), Error> {
    let conn = Connection::session()?;
    let unique = conn
        .unique_name()
        .map(|n| n.to_string())
        .ok_or_else(|| Error::Failed(tr("no unique bus name").into()))?;
    let token = format!("helix_web_{}", std::process::id());
    let req_path = request_path(&unique, &token);
    let request = Proxy::new(&conn, PORTAL_BUS, req_path.as_str(), REQUEST)?;
    let mut responses = request.receive_signal("Response")?;

    let launcher = Proxy::new(&conn, PORTAL_BUS, PORTAL_PATH, LAUNCHER)?;
    let icon_variant = Value::from(("bytes", Value::from(icon.to_vec())));
    let options: HashMap<&str, Value> = HashMap::from([
        ("handle_token", Value::from(token.as_str())),
        ("launcher_type", Value::from(LAUNCHER_WEBAPP)),
        ("target", Value::from(url)),
        ("editable_name", Value::from(true)),
        ("editable_icon", Value::from(true)),
        ("modal", Value::from(true)),
    ]);
    let _handle: OwnedObjectPath =
        launcher.call("PrepareInstall", &("", name, icon_variant, options))?;
    let msg = responses
        .next()
        .ok_or_else(|| Error::Failed(tr("no response from portal").into()))?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    if code != 0 {
        return Err(Error::Cancelled);
    }
    let chosen = results
        .get("name")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_else(|| name.to_string());
    let install_token = results
        .get("token")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .ok_or_else(|| Error::Failed(tr("portal returned no token").into()))?;

    let app = WebApp {
        id: make_id(&chosen, url),
        url: url.to_string(),
    };
    let entry = desktop_entry(&app, exe);
    let no_options: HashMap<&str, Value> = HashMap::new();
    let () = launcher.call(
        "Install",
        &(
            install_token.as_str(),
            app.desktop_file_id().as_str(),
            entry.as_str(),
            no_options,
        ),
    )?;
    Ok((app, chosen))
}

/// Removes an installed launcher.
pub fn uninstall(app: &WebApp) -> Result<(), Error> {
    let conn = Connection::session()?;
    let launcher = Proxy::new(&conn, PORTAL_BUS, PORTAL_PATH, LAUNCHER)?;
    let no_options: HashMap<&str, Value> = HashMap::new();
    let () = launcher.call("Uninstall", &(app.desktop_file_id().as_str(), no_options))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let a = make_id("My Mail!", "https://mail.example.com/");
        assert!(a.starts_with("my_mail_"), "{a}");
        assert_ne!(a, make_id("My Mail!", "https://other.example.com/"));
        assert_eq!(a, make_id("My Mail!", "https://mail.example.com/"));
        let b = make_id("", "https://mail.example.com/");
        assert!(b.starts_with("mail_example_com_"), "{b}");
        assert!(make_id("123", "https://x/").starts_with("_123_"));
        let long = make_id(&"x".repeat(200), "https://x/");
        assert!(long.len() <= 64);
        for id in [a, b, long] {
            assert_eq!(parse_arg(&format!("--web-app={id}")), Some(id.clone()));
            let app = WebApp {
                id,
                url: "https://x/".into(),
            };
            assert!(gtk::gio::Application::id_is_valid(&app.app_id()));
        }
        assert_eq!(parse_arg("--web-app="), None);
        assert_eq!(parse_arg("--web-app=../x"), None);
        assert_eq!(parse_arg("--web-app=a.b"), None);
        assert_eq!(parse_arg("--private"), None);
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(exec_quote("/usr/bin/helix-web"), "/usr/bin/helix-web");
        assert_eq!(exec_quote("https://e.com/a?b=1"), "\"https://e.com/a?b=1\"");
        assert_eq!(exec_quote("https://e.com/%20"), "\"https://e.com/%%20\"");
        assert_eq!(
            exec_quote("/opt/my app/helix-web"),
            "\"/opt/my app/helix-web\""
        );
        assert_eq!(
            exec_quote("a\"b$c`d\\e"),
            "\"a\\\\\"b\\\\$c\\\\`d\\\\\\\\e\""
        );
    }

    #[test]
    fn entries() {
        let app = WebApp {
            id: "mail_abc123".into(),
            url: "https://mail.example.com/inbox?x=1\nExec=evil".into(),
        };
        let e = desktop_entry(&app, "/usr/bin/helix-web");
        assert!(e.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(e.contains("Exec=/usr/bin/helix-web --web-app=mail_abc123 \"https://mail.example.com/inbox?x=1 Exec=evil\"\n"));
        assert!(!e.lines().any(|l| l.starts_with("Exec=evil")));
        assert!(e.contains("StartupWMClass=org.helixdesktop.Web.WebApp_mail_abc123\n"));
        assert!(!e.contains("\nName="));
        assert!(!e.contains("\nIcon="));
        assert_eq!(
            app.desktop_file_id(),
            "org.helixdesktop.Web.WebApp_mail_abc123.desktop"
        );
    }

    #[test]
    fn ownership_and_paths() {
        let app = WebApp {
            id: "x".into(),
            url: "https://www.example.com/app".into(),
        };
        assert!(app.owns("https://example.com/other"));
        assert!(app.owns("https://login.example.com/"));
        assert!(!app.owns("https://example.org/"));
        assert!(!app.owns("https://badexample.com/"));
        assert!(!app.owns("mailto:a@example.com"));
        assert_eq!(
            request_path(":1.42", "tok"),
            "/org/freedesktop/portal/desktop/request/1_42/tok"
        );
    }

    #[test]
    fn site_normalisation() {
        assert_eq!(
            normalize_site("Example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_site("  example.com/login?x=1 ").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_site("HTTPS://SSO.Example.com:443/path").as_deref(),
            Some("https://sso.example.com")
        );
        assert_eq!(
            normalize_site("http://e.com:8080").as_deref(),
            Some("http://e.com:8080")
        );
        assert_eq!(normalize_site("e.com:8443").as_deref(), Some("e.com:8443"));
        assert_eq!(
            normalize_site("*.Example.org").as_deref(),
            Some("*.example.org")
        );
        assert_eq!(
            normalize_site("bücher.de").as_deref(),
            Some("xn--bcher-kva.de")
        );
        assert_eq!(
            normalize_site("127.0.0.1:3000").as_deref(),
            Some("127.0.0.1:3000")
        );
        let bad = [
            "",
            "   ",
            "ftp://e.com",
            "mailto:a@b.c",
            "*.",
            "*.1.2.3.4",
            "a..b",
            "https://",
            "user:pw@e.com",
            "e com",
        ];
        for b in bad {
            assert_eq!(normalize_site(b), None, "{b}");
        }
    }

    #[test]
    fn additional_site_matching() {
        let mut p = Prefs::default();
        assert_eq!(
            p.add("accounts.example.net/signin").as_deref(),
            Some("accounts.example.net")
        );
        assert_eq!(
            p.add("ACCOUNTS.example.net").as_deref(),
            Some("accounts.example.net")
        );
        assert_eq!(p.additional_urls.len(), 1, "duplicates are not added");
        p.add("https://pay.example.org").unwrap();
        p.add("*.cdn.example.com").unwrap();
        p.add("localhost:8080").unwrap();
        assert!(p.add("not a site").is_none());

        // Exact host, either scheme, default port only.
        assert!(p.allows("https://accounts.example.net/o/auth?x=1"));
        assert!(p.allows("http://accounts.example.net/"));
        assert!(p.allows("https://Accounts.Example.NET/"));
        assert!(!p.allows("https://accounts.example.net:8443/"));
        assert!(!p.allows("https://login.accounts.example.net/"));
        assert!(!p.allows("https://example.net/"));
        assert!(!p.allows("https://evilaccounts.example.net/"));
        assert!(!p.allows("https://accounts.example.net.evil.com/"));
        // Scheme pinned.
        assert!(p.allows("https://pay.example.org/checkout"));
        assert!(!p.allows("http://pay.example.org/checkout"));
        // Wildcard: the host and its subdomains.
        assert!(p.allows("https://cdn.example.com/"));
        assert!(p.allows("https://a.b.cdn.example.com/x"));
        assert!(!p.allows("https://xcdn.example.com/"));
        // Port pinned.
        assert!(p.allows("http://localhost:8080/"));
        assert!(!p.allows("http://localhost/"));
        // Never other schemes.
        assert!(!p.allows("ftp://accounts.example.net/"));
        assert!(!p.allows("javascript:alert(1)"));
        assert!(!p.allows("garbage"));

        let app = WebApp {
            id: "x".into(),
            url: "https://mail.example.com/".into(),
        };
        assert!(app.keeps("https://mail.example.com/inbox", &p));
        assert!(app.keeps("https://accounts.example.net/", &p));
        assert!(!app.keeps("https://news.example.net/", &p));

        p.remove("accounts.example.net");
        assert!(!p.allows("https://accounts.example.net/"));
    }

    #[test]
    fn prefs_json() {
        let mut p = Prefs::default();
        p.add("sso.example.com").unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, r#"{"additional-urls":["sso.example.com"]}"#);
        assert_eq!(serde_json::from_str::<Prefs>(&json).unwrap(), p);
        assert_eq!(
            serde_json::from_str::<Prefs>("{}").unwrap(),
            Prefs::default()
        );
    }
}
