//! A private `dbus-daemon` for tests, with **no activatable services**.
//!
//! The standard session configuration (`dbus-run-session` without
//! `--config-file`, or `dbus-daemon --session`) loads the system's
//! session service directories, so a test that calls a name nobody owns
//! starts the real service on its "private" bus: the permission store,
//! gvfs, a portal, a keyring. Buses from here load no service files at
//! all, or only the directories a test passes for its own fakes, so a
//! call to a missing service fails at once with `ServiceUnknown`.
//!
//! Every test that needs its own bus starts it with [`TestBus::start`];
//! scripts use `tools/testing/session.conf` (see `tools/testing/`).
//! `tools/testing/check-test-buses.sh` fails when a test starts a bus
//! some other way. Not for use in the apps.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The well-known name of the bus itself; the only name a test bus may
/// list as activatable.
pub const BUS_NAME: &str = "org.freedesktop.DBus";

/// A private bus; the daemon is stopped and its directory removed when
/// dropped.
pub struct TestBus {
    daemon: Child,
    address: String,
    dir: PathBuf,
}

impl Drop for TestBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The bus configuration: a session-type bus listening in `listen_dir`
/// that anyone may use, loading service files only from `service_dirs`.
pub fn config(listen_dir: &Path, service_dirs: &[&Path]) -> String {
    let mut dirs = String::new();
    for dir in service_dirs {
        dirs.push_str(&format!("  <servicedir>{}</servicedir>\n", xml_escape(dir)));
    }
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={listen}</listen>
  <auth>EXTERNAL</auth>
{dirs}  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        listen = xml_escape(listen_dir),
    )
}

fn xml_escape(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The system's `dbus-daemon`. `/usr/bin` first: a `dbus-daemon` earlier
/// on `PATH` (say, a Conda environment's) may read other defaults.
fn daemon_binary() -> PathBuf {
    for dir in ["/usr/bin", "/bin"] {
        let path = Path::new(dir).join("dbus-daemon");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("dbus-daemon")
}

impl TestBus {
    /// A new private bus that can start nothing. `None` (with a note on
    /// stderr) when `dbus-daemon` is not installed; the caller then skips
    /// its test.
    pub fn start() -> Option<TestBus> {
        Self::with_services(&[])
    }

    /// A new private bus that can start only the services described by
    /// the `.service` files in `service_dirs` (a test's own fakes).
    pub fn with_services(service_dirs: &[&Path]) -> Option<TestBus> {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "helix-test-bus-{}-{n}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).ok()?;
        let conf = dir.join("bus.conf");
        std::fs::write(&conf, config(&dir, service_dirs)).ok()?;
        let daemon = Command::new(daemon_binary())
            .arg(format!("--config-file={}", conf.display()))
            .args(["--nofork", "--print-address=1"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut daemon) = daemon else {
            eprintln!("dbus-daemon unavailable; skipping");
            let _ = std::fs::remove_dir_all(&dir);
            return None;
        };
        let mut line = String::new();
        let read = daemon
            .stdout
            .take()
            .map(|out| BufReader::new(out).read_line(&mut line));
        let address = line.trim().to_string();
        let bus = TestBus {
            daemon,
            address,
            dir,
        };
        if !matches!(read, Some(Ok(n)) if n > 0) || bus.address.is_empty() {
            eprintln!("dbus-daemon did not start; skipping");
            return None;
        }
        Some(bus)
    }

    /// The bus's address, for `DBUS_SESSION_BUS_ADDRESS` or a connection
    /// builder.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// A scratch directory that lives as long as the bus.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// The names the bus at `address` could start, besides [`BUS_NAME`].
/// Empty is the only right answer for a test bus.
pub fn activatable_names(address: &str) -> Result<Vec<String>, String> {
    let conn = gio::DBusConnection::for_address_sync(
        address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.to_string())?;
    let reply = conn
        .call_sync(
            Some(BUS_NAME),
            "/org/freedesktop/DBus",
            BUS_NAME,
            "ListActivatableNames",
            None,
            Some(glib::VariantTy::new("(as)").unwrap()),
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string());
    let _ = conn.close_sync(gio::Cancellable::NONE);
    let (names,): (Vec<String>,) = reply?.get().ok_or("unexpected reply")?;
    Ok(names.into_iter().filter(|n| n != BUS_NAME).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test bus lists nothing to start, and asking it to start a real
    /// service fails instead of starting it.
    #[test]
    fn test_bus_activates_nothing() {
        let Some(bus) = TestBus::start() else { return };
        assert_eq!(
            activatable_names(bus.address()).unwrap(),
            Vec::<String>::new()
        );

        use gio::prelude::*;
        let conn = gio::DBusConnection::for_address_sync(
            bus.address(),
            gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
                | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
            None,
            gio::Cancellable::NONE,
        )
        .unwrap();
        let err = conn
            .call_sync(
                Some(BUS_NAME),
                "/org/freedesktop/DBus",
                BUS_NAME,
                "StartServiceByName",
                Some(&("org.freedesktop.impl.portal.PermissionStore", 0u32).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                5000,
                gio::Cancellable::NONE,
            )
            .unwrap_err();
        assert!(
            gio::DBusError::remote_error(&err).as_deref()
                == Some("org.freedesktop.DBus.Error.ServiceUnknown"),
            "{err}"
        );
        let _ = conn.close_sync(gio::Cancellable::NONE);
    }

    /// Only the test's own service files are loaded.
    #[test]
    fn test_bus_activates_only_its_fakes() {
        let services =
            std::env::temp_dir().join(format!("helix-test-bus-services-{}", std::process::id()));
        std::fs::create_dir_all(&services).unwrap();
        std::fs::write(
            services.join("org.helix.TestFake.service"),
            "[D-BUS Service]\nName=org.helix.TestFake\nExec=/bin/false\n",
        )
        .unwrap();
        let bus = TestBus::with_services(&[&services]);
        let names = bus.as_ref().map(|b| activatable_names(b.address()));
        let _ = std::fs::remove_dir_all(&services);
        let Some(names) = names else { return };
        assert_eq!(names.unwrap(), vec!["org.helix.TestFake".to_string()]);
    }

    /// The bus the whole test run is on (`tools/testing/run-tests.sh`,
    /// CI) must not be able to start real services either. Checked only
    /// when that script says so: a developer's plain `cargo test` may run
    /// on their real session bus, which these tests never use.
    #[test]
    fn test_run_bus_activates_nothing() {
        if std::env::var_os("HELIX_TEST_BUS_CHECK").is_none() {
            eprintln!("HELIX_TEST_BUS_CHECK unset; not checking the session bus");
            return;
        }
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .expect("HELIX_TEST_BUS_CHECK set but no DBUS_SESSION_BUS_ADDRESS");
        assert_eq!(
            activatable_names(&address).unwrap(),
            Vec::<String>::new(),
            "the test run's session bus can start real services; \
             run the tests through tools/testing/run-tests.sh"
        );
    }
}
