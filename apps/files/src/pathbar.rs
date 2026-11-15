//! Path bar segmentation: turns a location into breadcrumbs.

use helix_app_common::i18n::tr;
use std::path::{Path, PathBuf};

/// Where a breadcrumb leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Path(PathBuf),
    Uri(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub icon: Option<&'static str>,
    pub target: Target,
}

impl Crumb {
    /// The crumb's name for assistive technology: its label, or
    /// "Computer" for the file-system root, which shows only an icon.
    pub fn name(&self) -> String {
        if self.label.is_empty() {
            tr("Computer").into()
        } else {
            self.label.clone()
        }
    }
}

/// A local directory that starts its own breadcrumb trail (home, mount
/// points).
#[derive(Debug, Clone)]
pub struct Root {
    pub path: PathBuf,
    pub label: String,
    pub icon: &'static str,
}

/// Breadcrumbs for a local path. The deepest matching root (home folder or
/// a mount point) becomes the first crumb; otherwise the trail starts at `/`.
pub fn crumbs_for_path(path: &Path, roots: &[Root]) -> Vec<Crumb> {
    let root = roots
        .iter()
        .filter(|r| path.starts_with(&r.path))
        .max_by_key(|r| r.path.components().count());
    let (mut crumbs, mut current, rest) = match root {
        Some(r) => (
            vec![Crumb {
                label: r.label.clone(),
                icon: Some(r.icon),
                target: Target::Path(r.path.clone()),
            }],
            r.path.clone(),
            path.strip_prefix(&r.path)
                .unwrap_or(Path::new(""))
                .to_path_buf(),
        ),
        None => (
            vec![Crumb {
                label: String::new(),
                icon: Some("drive-harddisk-symbolic"),
                target: Target::Path("/".into()),
            }],
            PathBuf::from("/"),
            path.strip_prefix("/").unwrap_or(path).to_path_buf(),
        ),
    };
    for comp in rest.components() {
        if let std::path::Component::Normal(name) = comp {
            current.push(name);
            crumbs.push(Crumb {
                label: name.to_string_lossy().into_owned(),
                icon: None,
                target: Target::Path(current.clone()),
            });
        }
    }
    crumbs
}

/// Decodes `%XX` escapes (invalid escapes are kept verbatim).
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = hex {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Friendly label for the root of a non-file URI scheme.
pub fn scheme_root_label(scheme: &str, authority: &str) -> String {
    match scheme {
        "trash" => tr("Trash").into(),
        "recent" => tr("Recent").into(),
        "network" => tr("Network").into(),
        "starred" => tr("Starred").into(),
        "computer" => tr("Computer").into(),
        "other-locations" => tr("Other Locations").into(),
        "admin" => tr("Administrator Root").into(),
        _ if !authority.is_empty() => {
            // Drop user info and port for display.
            let host = authority.rsplit('@').next().unwrap_or(authority);
            let host = if host.starts_with('[') {
                host.split(']')
                    .next()
                    .map(|h| format!("{h}]"))
                    .unwrap_or_else(|| host.to_string())
            } else {
                host.split(':').next().unwrap_or(host).to_string()
            };
            percent_decode(&host)
        }
        _ => scheme.to_string(),
    }
}

/// Icon for the root of a URI scheme.
pub fn scheme_root_icon(scheme: &str) -> &'static str {
    match scheme {
        "trash" => "user-trash-symbolic",
        "recent" => "document-open-recent-symbolic",
        "network" => "network-workgroup-symbolic",
        "starred" => "starred-symbolic",
        "other-locations" => "list-add-symbolic",
        _ => "folder-remote-symbolic",
    }
}

/// Breadcrumbs for a non-`file` URI such as `sftp://host/a/b` or
/// `trash:///x`. Returns `None` if `uri` is not of the form `scheme:…`.
pub fn crumbs_for_uri(uri: &str) -> Option<Vec<Crumb>> {
    let colon = uri.find(':')?;
    let scheme = &uri[..colon];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return None;
    }
    let rest = &uri[colon + 1..];
    let (authority, path) = match rest.strip_prefix("//") {
        Some(after) => match after.find('/') {
            Some(i) => (&after[..i], &after[i..]),
            None => (after, ""),
        },
        None => ("", rest),
    };
    // Strip query/fragment.
    let path = path.split(['?', '#']).next().unwrap_or("");
    let root_uri = if rest.starts_with("//") {
        format!("{scheme}://{authority}/")
    } else {
        format!("{scheme}:")
    };
    let mut crumbs = vec![Crumb {
        label: scheme_root_label(scheme, authority),
        icon: Some(scheme_root_icon(scheme)),
        target: Target::Uri(root_uri.clone()),
    }];
    if !rest.starts_with("//") {
        return Some(crumbs);
    }
    let mut current = format!("{scheme}://{authority}");
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        current.push('/');
        current.push_str(seg);
        crumbs.push(Crumb {
            label: percent_decode(seg),
            icon: None,
            target: Target::Uri(current.clone()),
        });
    }
    Some(crumbs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_roots() -> Vec<Root> {
        vec![
            Root {
                path: "/home/u".into(),
                label: "Home".into(),
                icon: "user-home-symbolic",
            },
            Root {
                path: "/media/u/USB".into(),
                label: "USB".into(),
                icon: "drive-removable-media-symbolic",
            },
        ]
    }

    fn labels(c: &[Crumb]) -> Vec<&str> {
        c.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn home_paths() {
        let c = crumbs_for_path(Path::new("/home/u/Documents/Work"), &home_roots());
        assert_eq!(labels(&c), ["Home", "Documents", "Work"]);
        assert_eq!(c[0].target, Target::Path("/home/u".into()));
        assert_eq!(c[2].target, Target::Path("/home/u/Documents/Work".into()));
        let c = crumbs_for_path(Path::new("/home/u"), &home_roots());
        assert_eq!(labels(&c), ["Home"]);
    }

    #[test]
    fn root_paths() {
        let c = crumbs_for_path(Path::new("/usr/share"), &home_roots());
        assert_eq!(labels(&c), ["", "usr", "share"]);
        assert_eq!(c[0].target, Target::Path("/".into()));
        assert_eq!(c[0].name(), "Computer");
        assert_eq!(c[2].target, Target::Path("/usr/share".into()));
        let c = crumbs_for_path(Path::new("/"), &home_roots());
        assert_eq!(c.len(), 1);
        // "/home/user2" must not match the "/home/u" root.
        let c = crumbs_for_path(Path::new("/home/user2"), &home_roots());
        assert_eq!(labels(&c), ["", "home", "user2"]);
    }

    #[test]
    fn mount_roots_win_when_deeper() {
        let c = crumbs_for_path(Path::new("/media/u/USB/photos"), &home_roots());
        assert_eq!(labels(&c), ["USB", "photos"]);
    }

    #[test]
    fn remote_uris() {
        let c = crumbs_for_uri("sftp://me@example.org:2222/srv/my%20dir").unwrap();
        assert_eq!(labels(&c), ["example.org", "srv", "my dir"]);
        assert_eq!(
            c[0].target,
            Target::Uri("sftp://me@example.org:2222/".into())
        );
        assert_eq!(
            c[1].target,
            Target::Uri("sftp://me@example.org:2222/srv".into())
        );
        assert_eq!(
            c[2].target,
            Target::Uri("sftp://me@example.org:2222/srv/my%20dir".into())
        );
    }

    #[test]
    fn virtual_uris() {
        let c = crumbs_for_uri("trash:///").unwrap();
        assert_eq!(labels(&c), ["Trash"]);
        assert_eq!(c[0].target, Target::Uri("trash:///".into()));
        let c = crumbs_for_uri("trash:///old%20stuff").unwrap();
        assert_eq!(labels(&c), ["Trash", "old stuff"]);
        assert_eq!(c[1].target, Target::Uri("trash:///old%20stuff".into()));
        assert_eq!(labels(&crumbs_for_uri("recent:///").unwrap()), ["Recent"]);
        assert_eq!(
            labels(&crumbs_for_uri("other-locations:").unwrap()),
            ["Other Locations"]
        );
        assert!(crumbs_for_uri("no scheme here").is_none());
    }

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
    }
}
