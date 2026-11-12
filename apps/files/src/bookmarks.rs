//! GTK bookmarks file (`$XDG_CONFIG_HOME/gtk-3.0/bookmarks`).
//!
//! One bookmark per line: a URI, optionally followed by a space and a
//! custom label.

use crate::pathbar::percent_decode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub uri: String,
    pub label: Option<String>,
}

impl Bookmark {
    /// The label to display: the custom one, else the last path segment.
    pub fn display_label(&self) -> String {
        self.label.clone().unwrap_or_else(|| default_label(&self.uri))
    }
}

/// Last decoded path segment of `uri`, or its host / the URI itself.
pub fn default_label(uri: &str) -> String {
    let after_scheme = uri
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(uri)
        .trim_end_matches('/');
    match after_scheme.rsplit_once('/') {
        Some((_, last)) if !last.is_empty() => percent_decode(last),
        _ if !after_scheme.is_empty() => percent_decode(after_scheme),
        _ => uri.to_string(),
    }
}

pub fn parse(text: &str) -> Vec<Bookmark> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| match line.split_once(' ') {
            Some((uri, label)) => {
                let label = label.trim();
                Bookmark {
                    uri: uri.to_string(),
                    label: (!label.is_empty()).then(|| label.to_string()),
                }
            }
            None => Bookmark {
                uri: line.to_string(),
                label: None,
            },
        })
        .collect()
}

pub fn serialize(bookmarks: &[Bookmark]) -> String {
    let mut out = String::new();
    for b in bookmarks {
        out.push_str(&b.uri);
        if let Some(label) = &b.label {
            out.push(' ');
            out.push_str(label);
        }
        out.push('\n');
    }
    out
}

/// Path of the bookmarks file.
pub fn file_path() -> std::path::PathBuf {
    gtk::glib::user_config_dir().join("gtk-3.0").join("bookmarks")
}

pub fn load() -> Vec<Bookmark> {
    std::fs::read_to_string(file_path())
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// Writes atomically (temp file + rename).
pub fn save(bookmarks: &[Bookmark]) -> std::io::Result<()> {
    let path = file_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("helix-tmp");
    std::fs::write(&tmp, serialize(bookmarks))?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines() {
        let text = "file:///home/u/Projects\nfile:///home/u/My%20Stuff Stuff  \n\nsftp://host/srv Server files\n";
        let b = parse(text);
        assert_eq!(b.len(), 3);
        assert_eq!(
            b[0],
            Bookmark {
                uri: "file:///home/u/Projects".into(),
                label: None
            }
        );
        assert_eq!(b[1].label.as_deref(), Some("Stuff"));
        assert_eq!(b[2].label.as_deref(), Some("Server files"));
        assert_eq!(b[0].display_label(), "Projects");
        assert_eq!(parse("file:///x/My%20Stuff")[0].display_label(), "My Stuff");
    }

    #[test]
    fn round_trip() {
        let b = vec![
            Bookmark {
                uri: "file:///a".into(),
                label: None,
            },
            Bookmark {
                uri: "smb://srv/share".into(),
                label: Some("Share drive".into()),
            },
        ];
        let text = serialize(&b);
        assert_eq!(text, "file:///a\nsmb://srv/share Share drive\n");
        assert_eq!(parse(&text), b);
    }

    #[test]
    fn labels() {
        assert_eq!(default_label("file:///home/u/Music/"), "Music");
        assert_eq!(default_label("sftp://example.org"), "example.org");
        assert_eq!(default_label("file:///"), "file:///");
    }
}
