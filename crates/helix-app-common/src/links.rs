//! Opening links that came from untrusted content (documents, terminal
//! output, contact cards) in other applications.
//!
//! Web and mail links ([`DIRECT_SCHEMES`]) open straight away. Anything
//! else (`file:`, `smb:`, `sftp:`, a custom scheme handler, …) can mount
//! a share, run a launcher or start some other application, so
//! [`launch`] first asks, showing the full target.
//!
//! ```ignore
//! links::launch(&window, &uri);
//! ```

use crate::i18n::tr;
use adw::prelude::*;

/// Schemes opened without asking.
pub const DIRECT_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The scheme of `uri` in lower case, `None` for a relative reference or
/// text without a valid scheme. Leading and trailing spaces and control
/// characters, and tabs and newlines anywhere, are ignored, as browsers
/// do.
pub fn scheme(uri: &str) -> Option<String> {
    let cleaned: String = uri
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let (s, _) = cleaned.split_once(':')?;
    let mut chars = s.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| s.to_ascii_lowercase())
}

/// Whether `uri` is a web or mail link, opened without asking.
pub fn is_direct(uri: &str) -> bool {
    scheme(uri).is_some_and(|s| DIRECT_SCHEMES.contains(&s.as_str()))
}

/// Whether `uri` is an `http:` or `https:` link.
pub fn is_web(uri: &str) -> bool {
    scheme(uri).is_some_and(|s| s == "http" || s == "https")
}

/// The local path a `file:` URI names (`file:///…` or
/// `file://localhost/…`), `None` for any other URI.
pub fn local_path(uri: &str) -> Option<std::path::PathBuf> {
    if scheme(uri).as_deref() != Some("file") {
        return None;
    }
    let (path, host) = glib::filename_from_uri(uri.trim()).ok()?;
    host.is_none_or(|h| h.is_empty() || h == "localhost")
        .then_some(path)
}

/// `uri` as shown in the confirmation: control and bidirectional
/// formatting characters made visible, so the text cannot pretend to be
/// something else.
pub fn display_form(uri: &str) -> String {
    uri.chars()
        .map(|c| {
            if c.is_control() || is_format_char(c) {
                format!("\\u{{{:04X}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Unicode format characters (general category Cf) that change how text
/// around them is displayed: bidirectional controls, zero-width
/// characters, soft hyphen and the like.
pub fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}'
        | '\u{0600}'..='\u{0605}'
        | '\u{061C}'
        | '\u{06DD}'
        | '\u{070F}'
        | '\u{0890}'..='\u{0891}'
        | '\u{08E2}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{110BD}'
        | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}'
        | '\u{E0020}'..='\u{E007F}')
}

/// Opens `uri` in its default application without asking.
pub fn launch_now(parent: &impl IsA<gtk::Widget>, uri: &str) {
    let window = parent.root().and_downcast::<gtk::Window>();
    gtk::UriLauncher::new(uri).launch(window.as_ref(), None::<&gio::Cancellable>, |res| {
        if let Err(e) = res {
            eprintln!("Could not open link: {}", e.message());
        }
    });
}

/// Opens `uri`: web and mail links at once, anything else after the user
/// confirms in a dialog that shows the full target.
pub fn launch(parent: &impl IsA<gtk::Widget>, uri: &str) {
    if is_direct(uri) {
        launch_now(parent, uri);
    } else {
        confirm(parent, uri);
    }
}

/// Asks whether to open `uri`, and opens it if the user agrees.
pub fn confirm(parent: &impl IsA<gtk::Widget>, uri: &str) {
    let dialog = adw::AlertDialog::new(
        Some(tr("Open This Link?")),
        Some(tr(
            "The link points to the location below, which is not a web page. \
             Opening it may access files or network shares, or start another application.",
        )),
    );
    let target = gtk::Label::builder()
        .label(display_form(uri))
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .css_classes(["monospace"])
        .build();
    dialog.set_extra_child(Some(&target));
    dialog.add_responses(&[("cancel", tr("_Cancel")), ("open", tr("_Open Link"))]);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let parent_widget = parent.as_ref().clone();
    let uri = uri.to_string();
    dialog.connect_response(None, move |_, response| {
        if response == "open" {
            launch_now(&parent_widget, &uri);
        }
    });
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes() {
        assert_eq!(scheme("HTTPS://example.org").as_deref(), Some("https"));
        assert_eq!(scheme("  mailto:a@b").as_deref(), Some("mailto"));
        assert_eq!(scheme("ht\ttp://x").as_deref(), Some("http"));
        assert_eq!(
            scheme("x-custom+v1.2:foo").as_deref(),
            Some("x-custom+v1.2")
        );
        assert_eq!(scheme("relative/path"), None);
        assert_eq!(scheme("1abc:foo"), None);
        assert_eq!(scheme("a b:foo"), None);
        assert_eq!(scheme(""), None);
    }

    #[test]
    fn direct_links() {
        for ok in ["http://a", "https://a/b", "mailto:x@y", "HTTPS://A"] {
            assert!(is_direct(ok), "{ok}");
        }
        for ask in [
            "file:///home/u/x.desktop",
            "smb://host/share",
            "sftp://h/",
            "ftp://h/",
            "javascript:alert(1)",
            "x-handler:run",
            "/tmp/x",
            "",
        ] {
            assert!(!is_direct(ask), "{ask}");
        }
        assert!(is_web("http://a") && is_web("https://a"));
        assert!(!is_web("mailto:a@b") && !is_web("file:///"));
    }

    #[test]
    fn local_paths() {
        assert_eq!(
            local_path("file:///tmp/a%20b.ogg").as_deref(),
            Some(std::path::Path::new("/tmp/a b.ogg"))
        );
        assert!(local_path("FILE:///tmp/x").is_some());
        assert!(local_path("file://localhost/tmp/x").is_some());
        assert!(local_path("file://otherhost/tmp/x").is_none());
        assert!(local_path("http://host/x.ogg").is_none());
        assert!(local_path("smb://host/share/x").is_none());
        assert!(local_path("/tmp/x").is_none());
    }

    #[test]
    fn display_form_reveals_hidden_characters() {
        assert_eq!(display_form("https://a/b"), "https://a/b");
        assert_eq!(
            display_form("file:///x\u{202E}fdp.desktop"),
            "file:///x\\u{202E}fdp.desktop"
        );
        assert_eq!(display_form("a\nb"), "a\\u{000A}b");
        assert!(is_format_char('\u{2066}') && is_format_char('\u{200B}'));
        assert!(!is_format_char('a') && !is_format_char('é'));
    }
}
