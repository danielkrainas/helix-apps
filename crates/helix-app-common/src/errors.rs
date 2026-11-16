//! User-facing text for errors from libraries (zbus, GLib/GIO,
//! `std::io`), whose own messages are English and often technical.
//!
//! Common kinds of failure (not found, permission denied, timed out, a
//! service that is not running, cancelled, …) get a translated sentence;
//! anything else is shown as the library said it, without a leading
//! D-Bus error name. The raw text is always logged on stderr, and dialogs
//! can show it under a "Details" expander ([`alert`], [`details_expander`]).
//!
//! ```ignore
//! toast(&trf!("Could not open: {}", errors::glib_detail(&e)));
//! errors::alert(tr("Error attaching disk image"), &raw).present(Some(&window));
//! ```

use crate::i18n::{n_, tr};
use adw::prelude::*;

/// A common kind of failure, with a translated sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    NotFound,
    PermissionDenied,
    AuthDismissed,
    TimedOut,
    ServiceMissing,
    Cancelled,
    Exists,
    NoSpace,
    ReadOnly,
    Busy,
    NotSupported,
    Unreachable,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::NotFound,
        Kind::PermissionDenied,
        Kind::AuthDismissed,
        Kind::TimedOut,
        Kind::ServiceMissing,
        Kind::Cancelled,
        Kind::Exists,
        Kind::NoSpace,
        Kind::ReadOnly,
        Kind::Busy,
        Kind::NotSupported,
        Kind::Unreachable,
    ];

    /// The untranslated sentence (the catalogue's msgid).
    pub fn msgid(self) -> &'static str {
        match self {
            Kind::NotFound => n_("The file or item does not exist."),
            Kind::PermissionDenied => n_("You do not have permission to do this."),
            Kind::AuthDismissed => n_("Authentication was dismissed."),
            Kind::TimedOut => n_("There was no answer in time."),
            Kind::ServiceMissing => n_("A service this needs is not running."),
            Kind::Cancelled => n_("The operation was cancelled."),
            Kind::Exists => n_("A file or item with that name already exists."),
            Kind::NoSpace => n_("There is not enough free space."),
            Kind::ReadOnly => n_("The location is read-only."),
            Kind::Busy => n_("The device or file is in use."),
            Kind::NotSupported => n_("This is not supported here."),
            Kind::Unreachable => n_("The network or server cannot be reached."),
        }
    }

    /// The translated sentence.
    pub fn sentence(self) -> &'static str {
        tr(self.msgid())
    }
}

/// What a D-Bus error name says, if it is a common kind.
fn from_dbus_name(name: &str) -> Option<Kind> {
    let last = name.rsplit('.').next().unwrap_or(name);
    Some(match last {
        "ServiceUnknown"
        | "NameHasNoOwner"
        | "ServiceNotFound"
        | "SpawnServiceNotFound"
        | "SpawnExecFailed"
        | "SpawnChildExited"
        | "Disconnected" => Kind::ServiceMissing,
        "NoReply" | "Timeout" | "TimedOut" => Kind::TimedOut,
        "NotAuthorizedDismissed" | "AuthenticationCanceled" | "AuthenticationCancelled" => {
            Kind::AuthDismissed
        }
        "AccessDenied"
        | "AuthFailed"
        | "NotAuthorized"
        | "NotAuthorizedCanObtain"
        | "PermissionDenied"
        | "InteractiveAuthorizationRequired" => Kind::PermissionDenied,
        "Cancelled" | "Canceled" => Kind::Cancelled,
        "FileNotFound" | "NotFound" | "NoSuchSource" | "ObjectNotFound" => Kind::NotFound,
        "FileExists" | "AlreadyExists" | "Exists" => Kind::Exists,
        "NotSupported" => Kind::NotSupported,
        "DeviceBusy" | "Busy" => Kind::Busy,
        "ReadOnly" => Kind::ReadOnly,
        _ => return None,
    })
}

/// What an errno says, if it is a common kind.
fn from_errno(code: i32) -> Option<Kind> {
    Some(match code {
        2 | 6 | 19 => Kind::NotFound,
        1 | 13 => Kind::PermissionDenied,
        17 | 39 => Kind::Exists,
        28 | 122 => Kind::NoSpace,
        30 => Kind::ReadOnly,
        16 | 26 => Kind::Busy,
        62 | 110 => Kind::TimedOut,
        95 => Kind::NotSupported,
        101 | 111 | 112 | 113 => Kind::Unreachable,
        125 => Kind::Cancelled,
        _ => return None,
    })
}

/// A leading D-Bus error name ("org.freedesktop.DBus.Error.NoReply: …"),
/// as zbus and GIO print them.
fn leading_dbus_name(raw: &str) -> Option<&str> {
    let raw = raw.trim_start();
    let raw = raw.strip_prefix("GDBus.Error:").unwrap_or(raw);
    let end = raw.find(|c: char| c == ':' || c.is_whitespace())?;
    let name = &raw[..end];
    let dotted = name.split('.').count() >= 3;
    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    (dotted && valid && name.starts_with(|c: char| c.is_ascii_alphabetic())).then_some(name)
}

/// The kind of failure a library's message describes, if it is a common
/// one: from a D-Bus error name anywhere in it, an `(os error N)`, or the
/// English wording of strerror, GLib and zbus.
pub fn classify(raw: &str) -> Option<Kind> {
    // D-Bus names, wherever they are ("Could not open: org.….NoReply: …").
    for (i, _) in raw
        .match_indices("org.")
        .chain(raw.match_indices("GDBus.Error:"))
    {
        if let Some(kind) = leading_dbus_name(&raw[i..]).and_then(from_dbus_name) {
            return Some(kind);
        }
    }
    if let Some(i) = raw.find("os error ") {
        let digits: String = raw[i + 9..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Some(kind) = digits.parse().ok().and_then(from_errno) {
            return Some(kind);
        }
    }
    let lower = raw.to_lowercase();
    const PHRASES: &[(&str, Kind)] = &[
        ("no such file or directory", Kind::NotFound),
        ("no such device", Kind::NotFound),
        ("does not exist", Kind::NotFound),
        ("permission denied", Kind::PermissionDenied),
        ("operation not permitted", Kind::PermissionDenied),
        ("not authorized", Kind::PermissionDenied),
        ("file exists", Kind::Exists),
        ("already exists", Kind::Exists),
        ("no space left on device", Kind::NoSpace),
        ("disk quota exceeded", Kind::NoSpace),
        ("read-only file system", Kind::ReadOnly),
        ("device or resource busy", Kind::Busy),
        ("target is busy", Kind::Busy),
        ("timeout was reached", Kind::TimedOut),
        ("timed out", Kind::TimedOut),
        ("did not receive a reply", Kind::TimedOut),
        ("operation was cancelled", Kind::Cancelled),
        ("operation was canceled", Kind::Cancelled),
        ("operation not supported", Kind::NotSupported),
        ("connection refused", Kind::Unreachable),
        ("network is unreachable", Kind::Unreachable),
        ("no route to host", Kind::Unreachable),
        ("could not connect", Kind::Unreachable),
        (
            "was not provided by any .service files",
            Kind::ServiceMissing,
        ),
        ("name has no owner", Kind::ServiceMissing),
    ];
    PHRASES
        .iter()
        .find(|(p, _)| lower.contains(p))
        .map(|(_, k)| *k)
}

/// The kind of a GLib error (GIO's I/O errors, GDBus errors).
pub fn classify_glib(e: &glib::Error) -> Option<Kind> {
    use gio::IOErrorEnum as E;
    if let Some(io) = e.kind::<gio::IOErrorEnum>() {
        let kind = match io {
            E::NotFound | E::NotMounted => Some(Kind::NotFound),
            E::Exists => Some(Kind::Exists),
            E::PermissionDenied => Some(Kind::PermissionDenied),
            E::NoSpace => Some(Kind::NoSpace),
            E::ReadOnly => Some(Kind::ReadOnly),
            E::Busy => Some(Kind::Busy),
            E::TimedOut => Some(Kind::TimedOut),
            E::Cancelled => Some(Kind::Cancelled),
            E::NotSupported => Some(Kind::NotSupported),
            E::HostNotFound | E::HostUnreachable | E::NetworkUnreachable | E::ConnectionRefused => {
                Some(Kind::Unreachable)
            }
            _ => None,
        };
        if kind.is_some() {
            return kind;
        }
    }
    if let Some(name) = gio::DBusError::remote_error(e) {
        if let Some(kind) = from_dbus_name(&name) {
            return Some(kind);
        }
    }
    if let Some(d) = e.kind::<gio::DBusError>() {
        use gio::DBusError as D;
        let kind = match d {
            D::ServiceUnknown | D::NameHasNoOwner | D::SpawnServiceNotFound => {
                Some(Kind::ServiceMissing)
            }
            D::NoReply | D::Timeout | D::TimedOut => Some(Kind::TimedOut),
            D::AccessDenied | D::AuthFailed => Some(Kind::PermissionDenied),
            D::FileNotFound => Some(Kind::NotFound),
            D::FileExists => Some(Kind::Exists),
            D::NotSupported => Some(Kind::NotSupported),
            _ => None,
        };
        if kind.is_some() {
            return kind;
        }
    }
    classify(e.message())
}

/// The kind of an I/O error.
pub fn classify_io(e: &std::io::Error) -> Option<Kind> {
    use std::io::ErrorKind as E;
    if let Some(kind) = e.raw_os_error().and_then(from_errno) {
        return Some(kind);
    }
    match e.kind() {
        E::NotFound => Some(Kind::NotFound),
        E::PermissionDenied => Some(Kind::PermissionDenied),
        E::AlreadyExists => Some(Kind::Exists),
        E::TimedOut => Some(Kind::TimedOut),
        E::ConnectionRefused => Some(Kind::Unreachable),
        E::Unsupported => Some(Kind::NotSupported),
        _ => classify(&e.to_string()),
    }
}

/// The library's text as it is shown when it is not a common kind:
/// without a leading D-Bus error name.
pub fn without_dbus_name(raw: &str) -> &str {
    let t = raw.trim();
    let body = t.strip_prefix("GDBus.Error:").unwrap_or(t);
    match leading_dbus_name(body) {
        Some(name) => {
            let rest = body[name.len()..].trim_start();
            let rest = rest.strip_prefix(':').unwrap_or(rest).trim();
            if rest.is_empty() {
                t
            } else {
                rest
            }
        }
        None => t,
    }
}

fn log(raw: &str) {
    let app = glib::prgname().map(|p| p.to_string()).unwrap_or_default();
    eprintln!("{app}: error: {raw}");
}

fn shown(kind: Option<Kind>, raw: &str) -> String {
    log(raw);
    quiet(kind, raw)
}

fn quiet(kind: Option<Kind>, raw: &str) -> String {
    match kind {
        Some(k) => k.sentence().to_owned(),
        None => without_dbus_name(raw).to_owned(),
    }
}

/// [`detail`] without logging, for text that is shown again and again
/// (log it once with [`detail`]).
pub fn text(raw: &str) -> String {
    quiet(classify(raw), raw)
}

/// [`glib_detail`] without logging.
pub fn glib_text(e: &glib::Error) -> String {
    quiet(classify_glib(e), e.message())
}

/// The text to show for a library's error message (logged as it is).
pub fn detail(raw: &str) -> String {
    shown(classify(raw), raw)
}

/// [`detail`] for a GLib error.
pub fn glib_detail(e: &glib::Error) -> String {
    shown(classify_glib(e), e.message())
}

/// [`detail`] for an I/O error.
pub fn io_detail(e: &std::io::Error) -> String {
    shown(classify_io(e), &e.to_string())
}

/// A collapsed "Details" expander with `raw` in a selectable label, for
/// people who report the problem.
pub fn details_expander(raw: &str) -> gtk::Expander {
    let label = gtk::Label::builder()
        .label(raw)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .build();
    label.add_css_class("monospace");
    label.add_css_class("dim-label");
    let expander = gtk::Expander::builder()
        .label(tr("Details"))
        .child(&label)
        .build();
    expander.update_property(&[gtk::accessible::Property::Description(raw)]);
    expander
}

/// An alert with `heading`, the translated explanation of `raw` as its
/// body and, when that differs from what the library said, the raw text
/// under "Details". Has a Close response; the caller presents it.
pub fn alert(heading: &str, raw: &str) -> adw::AlertDialog {
    let kind = classify(raw);
    let body = shown(kind, raw);
    let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
    if kind.is_some() {
        dialog.set_extra_child(Some(&details_expander(raw)));
    }
    dialog.add_response("close", tr("_Close"));
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_names() {
        let cases = [
            (
                "org.freedesktop.DBus.Error.ServiceUnknown: The name org.freedesktop.UDisks2 was not provided by any .service files",
                Kind::ServiceMissing,
            ),
            (
                "org.freedesktop.DBus.Error.NoReply: Did not receive a reply.",
                Kind::TimedOut,
            ),
            ("org.freedesktop.DBus.Error.AccessDenied: no", Kind::PermissionDenied),
            (
                "org.freedesktop.UDisks2.Error.NotAuthorizedDismissed: dismissed",
                Kind::AuthDismissed,
            ),
            (
                "org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain: Not authorized to perform operation",
                Kind::PermissionDenied,
            ),
            ("org.freedesktop.UDisks2.Error.DeviceBusy: target is busy", Kind::Busy),
            (
                "GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown: The name is not activatable",
                Kind::ServiceMissing,
            ),
            (
                "Could not create event: org.gnome.evolution.dataserver.Client.PermissionDenied: x",
                Kind::PermissionDenied,
            ),
        ];
        for (raw, kind) in cases {
            assert_eq!(classify(raw), Some(kind), "{raw}");
        }
    }

    #[test]
    fn io_and_english_text() {
        assert_eq!(
            classify("I/O error: No such file or directory (os error 2)"),
            Some(Kind::NotFound)
        );
        assert_eq!(
            classify("Error opening file: Permission denied"),
            Some(Kind::PermissionDenied)
        );
        assert_eq!(classify("Timeout was reached"), Some(Kind::TimedOut));
        assert_eq!(classify("Operation was cancelled"), Some(Kind::Cancelled));
        assert_eq!(
            classify("Error writing: No space left on device"),
            Some(Kind::NoSpace)
        );
        assert_eq!(classify("wrong fs type, bad option, bad superblock"), None);
        let e = std::io::Error::from_raw_os_error(30);
        assert_eq!(classify_io(&e), Some(Kind::ReadOnly));
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        assert_eq!(classify_io(&e), Some(Kind::NotFound));
    }

    #[test]
    fn glib_errors() {
        let e = glib::Error::new(
            gio::IOErrorEnum::NotFound,
            "Error opening file x: No such file",
        );
        assert_eq!(classify_glib(&e), Some(Kind::NotFound));
        let e = glib::Error::new(gio::IOErrorEnum::Cancelled, "Operation was cancelled");
        assert_eq!(classify_glib(&e), Some(Kind::Cancelled));
        let e = glib::Error::new(gio::DBusError::ServiceUnknown, "whatever");
        assert_eq!(classify_glib(&e), Some(Kind::ServiceMissing));
        let e = glib::Error::new(gio::IOErrorEnum::Failed, "Something odd");
        assert_eq!(classify_glib(&e), None);
        assert_eq!(glib_detail(&e), "Something odd");
    }

    #[test]
    fn unknown_errors_lose_only_the_dbus_name() {
        assert_eq!(
            detail("org.freedesktop.UDisks2.Error.Failed: Error mounting /dev/sdb1: wrong fs type"),
            "Error mounting /dev/sdb1: wrong fs type"
        );
        assert_eq!(detail("Plain words"), "Plain words");
        assert_eq!(
            without_dbus_name("org.example.Error.Failed"),
            "org.example.Error.Failed"
        );
        // Before i18n::init, the sentence is the English msgid.
        assert_eq!(
            detail("org.freedesktop.DBus.Error.NoReply: Did not receive a reply"),
            Kind::TimedOut.msgid()
        );
    }
}
