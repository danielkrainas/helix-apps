//! What rendered help pages may contain and load: the content security
//! policy, the link schemes kept when converting documents, the file
//! types served from a document's directory and the links handed to other
//! applications.

use std::path::{Path, PathBuf};

/// Content security policy of rendered help pages. Pages need inline
/// styles and figures/media from the help schemes, nothing else.
pub const PAGE_CSP: &str = "default-src 'none'; \
     img-src help: ghelp: data:; \
     media-src help: ghelp:; \
     style-src 'unsafe-inline'; \
     frame-src 'none'; \
     form-action 'none'; \
     base-uri 'none'";

/// Policy sent with files served from a document's directory: nothing
/// they could load or run, should one be shown as a document.
pub const RESOURCE_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/// The policy for a scheme response of type `mime`.
pub fn csp_for(mime: &str) -> &'static str {
    if mime.starts_with("text/html") {
        PAGE_CSP
    } else {
        RESOURCE_CSP
    }
}

/// Schemes a link in a help document may use.
const LINK_SCHEMES: &[&str] = &[
    "help",
    "ghelp",
    "gnome-help",
    "help-list",
    "http",
    "https",
    "mailto",
];

/// Schemes handed to other applications.
const EXTERNAL_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The scheme of `href` as a browser would read it (lower case), `None`
/// for a relative reference. Browsers drop tabs and newlines anywhere in
/// a URL and leading/trailing spaces and control characters, so those are
/// ignored here too. Anything with a colon before the first `/`, `?` or
/// `#` counts as having a scheme, valid or not.
pub fn scheme(href: &str) -> Option<String> {
    let cleaned: String = href
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let end = cleaned.find(['/', '?', '#']).unwrap_or(cleaned.len());
    cleaned[..end]
        .find(':')
        .map(|i| cleaned[..i].to_ascii_lowercase())
}

/// Whether a link to `href` may be emitted: help URIs, web and mail
/// links, and relative or fragment references.
pub fn link_allowed(href: &str) -> bool {
    match scheme(href) {
        None => true,
        Some(s) => LINK_SCHEMES.contains(&s.as_str()),
    }
}

/// Whether `uri` may be opened in another application.
pub fn external_allowed(uri: &str) -> bool {
    scheme(uri).is_some_and(|s| EXTERNAL_SCHEMES.contains(&s.as_str()))
}

/// Media type of a file served from a document's directory, `None` for
/// types that are not served (pages, scripts, style sheets, anything
/// unknown). Only figures, audio and video are.
pub fn resource_mime(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        "mp4" => "video/mp4",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        _ => return None,
    })
}

/// Whether a served file may also be opened as a page of its own (shown
/// at the top level). SVG is a document format, so it is only shown as a
/// figure inside a page.
pub fn resource_navigable(path: &str) -> bool {
    resource_mime(path).is_some_and(|m| m != "image/svg+xml")
}

/// `rel` resolved inside `base`, following symbolic links; `None` when
/// the result is not a regular file inside `base`.
pub fn confined(base: &Path, rel: &str) -> Option<PathBuf> {
    let base = base.canonicalize().ok()?;
    let path = base.join(rel.trim_start_matches('/')).canonicalize().ok()?;
    (path.starts_with(&base) && path.is_file()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn link_schemes() {
        for ok in [
            "help:gnome-help/net",
            "ghelp:/usr/share/help/C/x",
            "HTTPS://example.org/",
            "http://example.org/a:b",
            "mailto:someone@example.org",
            "#section",
            "other.page",
            "figures/a.png",
            "./a/b:c",
            "?page=x",
            "",
        ] {
            assert!(link_allowed(ok), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(1)",
            "\u{1}javascript:alert(1)",
            "java\tscript:alert(1)",
            "java\nscript:alert(1)",
            "vbscript:x",
            "data:text/html,<p>",
            "file:///etc/passwd",
            "blob:help:x",
            "about:blank",
            "x-unknown:foo",
            "foo:bar/baz",
        ] {
            assert!(!link_allowed(bad), "{bad:?}");
        }
    }

    #[test]
    fn external_schemes() {
        assert!(external_allowed("https://example.org"));
        assert!(external_allowed("http://example.org"));
        assert!(external_allowed("mailto:a@b.c"));
        for bad in [
            "file:///etc/passwd",
            "javascript:x",
            "help:x",
            "apt:foo",
            "smb://host/share",
            "relative",
            "",
        ] {
            assert!(!external_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn served_types() {
        assert_eq!(resource_mime("figures/a.PNG"), Some("image/png"));
        assert_eq!(resource_mime("a.jpeg"), Some("image/jpeg"));
        assert_eq!(resource_mime("f/v.webm"), Some("video/webm"));
        assert_eq!(resource_mime("f/s.oga"), Some("audio/ogg"));
        assert_eq!(resource_mime("f/logo.svg"), Some("image/svg+xml"));
        for refused in [
            "x.html",
            "x.HTM",
            "x.xhtml",
            "x.xht",
            "x.xml",
            "x.svgz",
            "x.js",
            "x.css",
            "x.txt",
            "x.page",
            "x.pdf",
            "noext",
            "dir.png/noext",
        ] {
            assert_eq!(resource_mime(refused), None, "{refused}");
        }
        assert!(resource_navigable("a.png"));
        assert!(!resource_navigable("a.svg"));
        assert!(!resource_navigable("a.html"));
    }

    #[test]
    fn page_policy() {
        assert_eq!(csp_for("text/html"), PAGE_CSP);
        assert_eq!(csp_for("image/svg+xml"), RESOURCE_CSP);
        assert!(PAGE_CSP.starts_with("default-src 'none'"));
        assert!(!PAGE_CSP.contains("script-src"));
        assert!(RESOURCE_CSP.contains("sandbox"));
    }

    #[test]
    fn confinement() {
        let root = std::env::temp_dir().join(format!("helix-help-policy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let doc = root.join("doc");
        fs::create_dir_all(doc.join("figures")).unwrap();
        fs::write(doc.join("figures/a.png"), b"PNG").unwrap();
        fs::write(root.join("secret.png"), b"SECRET").unwrap();
        std::os::unix::fs::symlink(root.join("secret.png"), doc.join("figures/link.png")).unwrap();
        std::os::unix::fs::symlink(doc.join("figures/a.png"), doc.join("inside.png")).unwrap();
        std::os::unix::fs::symlink(&root, doc.join("up")).unwrap();

        let a = doc.join("figures/a.png").canonicalize().unwrap();
        assert_eq!(confined(&doc, "figures/a.png"), Some(a.clone()));
        assert_eq!(confined(&doc, "/figures/a.png"), Some(a.clone()));
        assert_eq!(confined(&doc, "figures/../figures/a.png"), Some(a.clone()));
        // A link that stays inside the directory is fine.
        assert_eq!(confined(&doc, "inside.png"), Some(a));
        assert_eq!(confined(&doc, "../secret.png"), None);
        assert_eq!(confined(&doc, "figures/../../secret.png"), None);
        assert_eq!(
            confined(&doc, &root.join("secret.png").to_string_lossy()),
            None
        );
        assert_eq!(confined(&doc, "figures/link.png"), None);
        assert_eq!(confined(&doc, "up/secret.png"), None);
        assert_eq!(confined(&doc, "figures"), None);
        assert_eq!(confined(&doc, "missing.png"), None);
        fs::remove_dir_all(&root).unwrap();
    }
}
