//! "Email": hands the document to the user's mail client as attachments.
//!
//! The document is saved into a fresh temporary directory (one PDF, or one
//! image per page for the PNG/JPEG `save-format`), then
//! `org.freedesktop.portal.Email.ComposeEmail(s parent_window, a{sv} options)`
//! is called on `org.freedesktop.portal.Desktop` at
//! `/org/freedesktop/portal/desktop` with `attachment_fds` (`ah`, read-only
//! file descriptors) and `subject`. Without the portal, `xdg-email
//! --attach FILE…` is run instead (argv, no shell). Nothing is ever sent:
//! both only open a compose window.

use crate::export::{self, Format};
use crate::page::Page;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use zbus::zvariant::{Fd, Value};

pub const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub const PORTAL_IFACE: &str = "org.freedesktop.portal.Email";
pub const SUBJECT: &str = "Scanned Document";
/// Temporary email directories older than this are removed.
const KEEP: Duration = Duration::from_secs(24 * 3600);

/// Directory that holds the per-email temporary folders.
pub fn base_dir() -> PathBuf {
    gtk::glib::user_cache_dir().join("helix-document-scanner").join("email")
}

/// Removes temporary email folders older than a day.
pub fn prune(base: &Path, now: SystemTime) {
    for e in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > KEEP);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Saves `pages` for attaching into a new folder below `base`.
pub fn write_attachments(base: &Path, pages: &[Page], format: Format, quality: u8) -> Result<Vec<PathBuf>, String> {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = base.join(format!("{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(format!("{SUBJECT}.{}", format.extension()));
    export::save(pages, &path, format, quality)
}

/// Asks the Email portal on `conn` to open a compose window with `files`
/// attached. Returns the request object path.
pub fn compose_via_portal(conn: &zbus::blocking::Connection, files: &[PathBuf], parent: &str) -> Result<String, String> {
    let opened: Vec<std::fs::File> = files
        .iter()
        .map(|f| std::fs::File::open(f).map_err(|e| format!("{}: {e}", f.display())))
        .collect::<Result<_, _>>()?;
    let fds: Vec<Fd> = opened.iter().map(Fd::from).collect();
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("attachment_fds", Value::from(fds));
    options.insert("subject", Value::from(SUBJECT));
    let reply = conn
        .call_method(Some(PORTAL_BUS), PORTAL_PATH, Some(PORTAL_IFACE), "ComposeEmail", &(parent, options))
        .map_err(|e| e.to_string())?;
    let handle: zbus::zvariant::OwnedObjectPath = reply.body().deserialize().map_err(|e| e.to_string())?;
    Ok(handle.to_string())
}

/// Arguments for the `xdg-email` fallback.
pub fn xdg_email_args(files: &[PathBuf]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--subject".into(), SUBJECT.into()];
    for f in files {
        args.push("--attach".into());
        args.push(f.as_os_str().to_owned());
    }
    args
}

/// Opens the mail client: the portal on the session bus when available,
/// otherwise `xdg-email`. Blocking; call from a worker thread.
pub fn compose(files: &[PathBuf], parent: &str) -> Result<(), String> {
    let portal = zbus::blocking::Connection::session()
        .map_err(|e| e.to_string())
        .and_then(|c| compose_via_portal(&c, files, parent));
    match portal {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("helix-document-scanner: Email portal unavailable ({e}); using xdg-email");
            let status = std::process::Command::new("xdg-email")
                .args(xdg_email_args(files))
                .stdin(std::process::Stdio::null())
                .status()
                .map_err(|e| format!("Could not run xdg-email: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("xdg-email failed ({status})"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::AsFd;
    use std::sync::mpsc;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue};

    fn page() -> Page {
        Page::new(8, 4, 1, vec![128; 32], 150.0)
    }

    #[test]
    fn fallback_arguments() {
        let args = xdg_email_args(&[PathBuf::from("/t/a b.jpg"), PathBuf::from("/t/c.jpg")]);
        let args: Vec<&str> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["--subject", "Scanned Document", "--attach", "/t/a b.jpg", "--attach", "/t/c.jpg"]);
    }

    #[test]
    fn attachments_follow_format() {
        let base = tempfile::tempdir().unwrap();
        let pdf = write_attachments(base.path(), &[page(), page()], Format::Pdf, 75).unwrap();
        assert_eq!(pdf.len(), 1);
        assert!(pdf[0].ends_with("Scanned Document.pdf"));
        let jpgs = write_attachments(base.path(), &[page(), page()], Format::Jpeg, 75).unwrap();
        assert_eq!(jpgs.len(), 2);
        assert!(jpgs[1].ends_with("Scanned Document-2.jpg"));
        assert_ne!(pdf[0].parent(), jpgs[0].parent());
        prune(base.path(), SystemTime::now() + Duration::from_secs(2 * 24 * 3600));
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }

    struct MockEmail {
        tx: mpsc::Sender<(String, Vec<Vec<u8>>, Option<String>)>,
    }

    #[zbus::interface(name = "org.freedesktop.portal.Email")]
    impl MockEmail {
        fn compose_email(&self, parent_window: String, options: HashMap<String, OwnedValue>) -> OwnedObjectPath {
            let mut contents = Vec::new();
            if let Some(Value::Array(arr)) = options.get("attachment_fds").map(|v| &**v) {
                for v in arr.iter() {
                    if let Value::Fd(fd) = v {
                        let owned = fd.as_fd().try_clone_to_owned().unwrap();
                        let mut f = std::fs::File::from(owned);
                        let mut buf = Vec::new();
                        f.read_to_end(&mut buf).unwrap();
                        contents.push(buf);
                    }
                }
            }
            let subject = options.get("subject").and_then(|v| String::try_from(v.try_clone().ok()?).ok());
            let _ = self.tx.send((parent_window, contents, subject));
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/1_1/t").unwrap()
        }
    }

    /// Runs the portal call against a mock on a private dbus-daemon (no
    /// service activation, nothing reaches the real portal or a mail client).
    #[test]
    fn portal_call_on_private_bus() {
        let daemon = Path::new("/usr/bin/dbus-daemon");
        if !daemon.exists() {
            eprintln!("dbus-daemon not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let conf = tmp.path().join("bus.conf");
        std::fs::write(
            &conf,
            format!(
                "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \
                 \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\
                 <busconfig><type>session</type><listen>unix:dir={}</listen>\
                 <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
                 <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
                tmp.path().display()
            ),
        )
        .unwrap();
        let mut child = std::process::Command::new(daemon)
            .arg(format!("--config-file={}", conf.display()))
            .args(["--nofork", "--print-address=1"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        {
            use std::io::BufRead;
            let mut r = std::io::BufReader::new(child.stdout.take().unwrap());
            r.read_line(&mut line).unwrap();
        }
        let address = line.trim().to_string();
        let result = std::panic::catch_unwind(|| {
            let (tx, rx) = mpsc::channel();
            let _server = zbus::blocking::connection::Builder::address(address.as_str())
                .unwrap()
                .name(PORTAL_BUS)
                .unwrap()
                .serve_at(PORTAL_PATH, MockEmail { tx })
                .unwrap()
                .build()
                .unwrap();
            let client = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().build().unwrap();
            let files = write_attachments(tmp.path(), &[page(), page()], Format::Png, 75).unwrap();
            let handle = compose_via_portal(&client, &files, "").unwrap();
            assert!(handle.starts_with("/org/freedesktop/portal/desktop/request/"));
            let (parent, contents, subject) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(parent, "");
            assert_eq!(subject.as_deref(), Some(SUBJECT));
            assert_eq!(contents.len(), 2);
            for (c, f) in contents.iter().zip(&files) {
                assert!(c.starts_with(b"\x89PNG"));
                assert_eq!(c, &std::fs::read(f).unwrap());
            }
        });
        let _ = child.kill();
        let _ = child.wait();
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }
}
