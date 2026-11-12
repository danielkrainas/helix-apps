//! Working-directory URIs (OSC 7), titles and shell quoting.

use std::path::{Path, PathBuf};

/// Parses an OSC 7 `file://HOST/PATH` URI into `(host, path)`.
/// The path is percent-decoded; the host may be empty.
pub fn parse_file_uri(uri: &str) -> Option<(String, PathBuf)> {
    let rest = uri.strip_prefix("file://").or_else(|| uri.strip_prefix("kitty-shell-cwd://"))?;
    let i = rest.find('/')?;
    let (host, path) = (&rest[..i], &rest[i..]);
    let bytes = percent_decode(path)?;
    use std::os::unix::ffi::OsStringExt;
    Some((host.to_string(), PathBuf::from(std::ffi::OsString::from_vec(bytes))))
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Whether an OSC 7 host refers to this machine.
pub fn is_local_host(host: &str, local_hostname: &str) -> bool {
    host.is_empty() || host == "localhost" || host.eq_ignore_ascii_case(local_hostname)
}

/// The local working directory reported by an OSC 7 URI, if it is local.
pub fn local_dir_from_uri(uri: &str, local_hostname: &str) -> Option<PathBuf> {
    let (host, path) = parse_file_uri(uri)?;
    is_local_host(&host, local_hostname).then_some(path)
}

/// `path` with the home directory replaced by `~`.
pub fn display_path(path: &Path, home: &Path) -> String {
    if home.as_os_str().len() > 1 {
        if path == home {
            return "~".into();
        }
        if let Ok(rest) = path.strip_prefix(home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// The title of a tab: the terminal's own title if set, else the working
/// directory, else a generic name.
pub fn tab_title(window_title: Option<&str>, cwd: Option<&Path>, home: &Path) -> String {
    match window_title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => match cwd {
            Some(p) => display_path(p, home),
            None => "Terminal".into(),
        },
    }
}

/// Quotes `s` for a POSIX shell if needed.
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || "-_./,:@%+=~".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Text to paste for dropped files: quoted paths separated by spaces, with a
/// trailing space so the user can keep typing.
pub fn dropped_paths_text<'a>(paths: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for p in paths {
        out.push_str(&shell_quote(p));
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc7_uris() {
        assert_eq!(parse_file_uri("file:///home/u"), Some((String::new(), "/home/u".into())));
        assert_eq!(
            parse_file_uri("file://box/home/u/My%20Docs"),
            Some(("box".into(), "/home/u/My Docs".into()))
        );
        assert_eq!(parse_file_uri("file://box"), None);
        assert_eq!(parse_file_uri("http://x/y"), None);
        assert_eq!(parse_file_uri("file:///bad%2"), None);
        assert_eq!(parse_file_uri("file:///%C3%A9").unwrap().1, PathBuf::from("/é"));
    }

    #[test]
    fn local_hosts() {
        assert_eq!(local_dir_from_uri("file://box/tmp", "box"), Some("/tmp".into()));
        assert_eq!(local_dir_from_uri("file://localhost/tmp", "box"), Some("/tmp".into()));
        assert_eq!(local_dir_from_uri("file://other/tmp", "box"), None);
    }

    #[test]
    fn display() {
        let home = Path::new("/home/u");
        assert_eq!(display_path(Path::new("/home/u"), home), "~");
        assert_eq!(display_path(Path::new("/home/u/src"), home), "~/src");
        assert_eq!(display_path(Path::new("/home/user2"), home), "/home/user2");
        assert_eq!(display_path(Path::new("/etc"), Path::new("/")), "/etc");
    }

    #[test]
    fn titles() {
        let home = Path::new("/home/u");
        assert_eq!(tab_title(Some("vim"), Some(Path::new("/tmp")), home), "vim");
        assert_eq!(tab_title(Some("  "), Some(Path::new("/home/u/x")), home), "~/x");
        assert_eq!(tab_title(None, None, home), "Terminal");
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/tmp/a.txt"), "/tmp/a.txt");
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(dropped_paths_text(["/a", "/b c"]), "/a '/b c' ");
    }
}
