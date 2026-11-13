//! One-shot current-location lookup through the XDG Location portal
//! (`org.freedesktop.portal.Location` on the session bus). Helix apps do not
//! talk to GeoClue directly: the portal applies the desktop's permission
//! store and works the same inside and outside a sandbox.
//!
//! Blocking; call [`locate`] from a worker thread.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

pub const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub const LOCATION_IFACE: &str = "org.freedesktop.portal.Location";
pub const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
pub const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

/// Portal accuracy enum: 0 none, 1 country, 2 city, 3 neighborhood,
/// 4 street, 5 exact.
pub const ACCURACY_EXACT: u32 = 5;
pub const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    /// Metres; NaN when unknown.
    pub accuracy: f64,
    pub description: String,
}

type Dict = HashMap<String, OwnedValue>;

/// The portal's object path for a sender-unique handle token.
pub fn portal_path(kind: &str, unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/{kind}/{sender}/{token}")
}

fn token(prefix: &str) -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("helix_maps_{prefix}_{}_{n}", std::process::id())
}

/// Extracts a fix from a `LocationUpdated` dictionary.
pub fn fix_from_dict(d: &Dict) -> Option<Fix> {
    let f = |k: &str| d.get(k).and_then(|v| f64::try_from(v).ok());
    let (lat, lon) = (f("Latitude")?, f("Longitude")?);
    if !(lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
        return None;
    }
    let description = d.get("Description").and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
    Some(Fix { lat, lon, accuracy: f("Accuracy").unwrap_or(f64::NAN), description })
}

/// Maps a D-Bus error name to a user-facing message.
pub fn friendly_error(name: &str, detail: Option<&str>) -> String {
    const ABSENT: [&str; 5] = ["ServiceUnknown", "NameHasNoOwner", "UnknownInterface", "UnknownMethod", "UnknownObject"];
    if ABSENT.iter().any(|s| name.ends_with(s)) {
        "The location portal is not available".to_string()
    } else if name.ends_with("AccessDenied") || name.ends_with("NotAllowed") {
        "Location access was denied".to_string()
    } else {
        detail.map(str::to_string).unwrap_or_else(|| name.to_string())
    }
}

fn friendly(e: zbus::Error) -> String {
    match &e {
        zbus::Error::MethodError(name, msg, _) => friendly_error(name.as_str(), msg.as_deref()),
        zbus::Error::FDO(fdo) => {
            use zbus::DBusError;
            friendly_error(fdo.name().as_str(), fdo.description())
        }
        _ => format!("Location unavailable: {e}"),
    }
}

enum Event {
    Session(OwnedObjectPath),
    Done(Result<Fix, String>),
}

fn close_session(conn: &Connection, session: &OwnedObjectPath) {
    let _ = conn.call_method(Some(PORTAL_BUS), session.as_str(), Some(SESSION_IFACE), "Close", &());
}

fn run(conn: &Connection, accuracy: u32, events: &mpsc::Sender<Event>) -> Result<Fix, String> {
    let unique = conn.unique_name().map(|n| n.to_string()).ok_or("Not connected to a message bus")?;
    let location = Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, LOCATION_IFACE).map_err(friendly)?;
    let mut updates = location.receive_signal("LocationUpdated").map_err(friendly)?;

    let session_token = token("session");
    let opts: HashMap<&str, Value> = HashMap::from([
        ("session_handle_token", Value::from(session_token.as_str())),
        ("accuracy", Value::from(accuracy)),
        ("distance-threshold", Value::from(0u32)),
        ("time-threshold", Value::from(0u32)),
    ]);
    let session: OwnedObjectPath = location.call("CreateSession", &(opts,)).map_err(friendly)?;
    let _ = events.send(Event::Session(session.clone()));

    let request_token = token("request");
    let request =
        Proxy::new(conn, PORTAL_BUS, portal_path("request", &unique, &request_token), REQUEST_IFACE).map_err(friendly)?;
    let mut responses = request.receive_signal("Response").map_err(friendly)?;
    let opts: HashMap<&str, Value> = HashMap::from([("handle_token", Value::from(request_token.as_str()))]);
    location.call::<_, _, OwnedObjectPath>("Start", &(&session, "", opts)).map_err(friendly)?;

    let msg = responses.next().ok_or("The location portal closed the request")?;
    let (code, _): (u32, Dict) = msg.body().deserialize().map_err(|e| e.to_string())?;
    match code {
        0 => {}
        1 => return Err("Location access was denied".into()),
        _ => return Err("The location request was cancelled".into()),
    }
    for msg in updates.by_ref() {
        let Ok((path, data)) = msg.body().deserialize::<(OwnedObjectPath, Dict)>() else { continue };
        if path != session {
            continue;
        }
        if let Some(fix) = fix_from_dict(&data) {
            return Ok(fix);
        }
    }
    Err("No location received".into())
}

/// Asks the portal on `conn` for one position fix, waiting up to `timeout`.
/// The portal session is always closed afterwards (also on timeout) so the
/// location service stops tracking.
pub fn locate_on(conn: Connection, accuracy: u32, timeout: Duration) -> Result<Fix, String> {
    let (tx, rx) = mpsc::channel();
    let worker_conn = conn.clone();
    std::thread::spawn(move || {
        let result = run(&worker_conn, accuracy, &tx);
        let _ = tx.send(Event::Done(result));
    });
    let deadline = Instant::now() + timeout;
    let mut session = None;
    let result = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Event::Session(s)) => session = Some(s),
            Ok(Event::Done(r)) => break r,
            // A timed-out worker stays blocked on the signal stream; closing
            // the session still stops the location service.
            Err(_) => break Err("Timed out waiting for a location fix".into()),
        }
    };
    if let Some(s) = session {
        close_session(&conn, &s);
    }
    result
}

/// Current location via the session bus portal.
pub fn locate() -> Result<Fix, String> {
    let conn = Connection::session().map_err(|e| format!("Cannot reach the session bus: {e}"))?;
    locate_on(conn, ACCURACY_EXACT, TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_tokens() {
        assert_eq!(portal_path("request", ":1.7", "t"), "/org/freedesktop/portal/desktop/request/1_7/t");
        assert_ne!(token("a"), token("a"));
        assert!(token("x").chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn error_names() {
        assert_eq!(
            friendly_error("org.freedesktop.DBus.Error.ServiceUnknown", None),
            "The location portal is not available"
        );
        assert_eq!(friendly_error("org.freedesktop.portal.Error.NotAllowed", Some("x")), "Location access was denied");
        assert_eq!(friendly_error("org.example.Other", Some("boom")), "boom");
    }

    #[test]
    fn dict_parsing() {
        let mut d = Dict::new();
        d.insert("Latitude".into(), OwnedValue::from(52.5f64));
        assert!(fix_from_dict(&d).is_none());
        d.insert("Longitude".into(), OwnedValue::from(13.4f64));
        let f = fix_from_dict(&d).unwrap();
        assert!(f.accuracy.is_nan());
        d.insert("Accuracy".into(), OwnedValue::from(25.0f64));
        d.insert("Description".into(), OwnedValue::try_from(Value::from("Berlin")).unwrap());
        assert_eq!(
            fix_from_dict(&d),
            Some(Fix { lat: 52.5, lon: 13.4, accuracy: 25.0, description: "Berlin".into() })
        );
        d.insert("Latitude".into(), OwnedValue::from(95.0f64));
        assert!(fix_from_dict(&d).is_none());
    }
}

/// A mock `org.freedesktop.portal.Desktop` Location portal on a private
/// `dbus-daemon` started just for the test: the user's session bus and
/// real location services are never touched.
#[cfg(test)]
mod mock_portal {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use zbus::interface;
    use zbus::message::Header;

    #[derive(Default)]
    struct State {
        /// Response code for `Start` (0 granted, 1 denied).
        response: u32,
        /// Whether to send a fix after a granted `Start`.
        send_fix: bool,
        accuracy_requested: Option<u32>,
        sessions: Vec<String>,
        closed: Vec<String>,
    }
    type Shared = Arc<Mutex<State>>;

    struct Location(Shared);
    struct Session(Shared, String);

    #[interface(name = "org.freedesktop.portal.Location")]
    impl Location {
        async fn create_session(
            &self,
            options: HashMap<String, OwnedValue>,
            #[zbus(header)] hdr: Header<'_>,
            #[zbus(object_server)] server: &zbus::ObjectServer,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
            let token = options
                .get("session_handle_token")
                .and_then(|v| String::try_from(v.clone()).ok())
                .ok_or_else(|| zbus::fdo::Error::InvalidArgs("session_handle_token".into()))?;
            let path = portal_path("session", &sender, &token);
            {
                let mut s = self.0.lock().unwrap();
                s.accuracy_requested = options.get("accuracy").and_then(|v| u32::try_from(v).ok());
                s.sessions.push(path.clone());
            }
            server.at(path.as_str(), Session(self.0.clone(), path.clone())).await?;
            Ok(OwnedObjectPath::try_from(path).unwrap())
        }

        async fn start(
            &self,
            session: OwnedObjectPath,
            _parent_window: &str,
            options: HashMap<String, OwnedValue>,
            #[zbus(header)] hdr: Header<'_>,
            #[zbus(connection)] conn: &zbus::Connection,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
            let token = options
                .get("handle_token")
                .and_then(|v| String::try_from(v.clone()).ok())
                .unwrap_or_else(|| "t".into());
            let request = portal_path("request", &sender, &token);
            let (code, send_fix) = {
                let s = self.0.lock().unwrap();
                (s.response, s.send_fix)
            };
            let empty: HashMap<&str, Value> = HashMap::new();
            conn.emit_signal(Some(sender.as_str()), request.as_str(), REQUEST_IFACE, "Response", &(code, empty))
                .await?;
            if code == 0 && send_fix {
                // An update for a different session must be ignored.
                let other = OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/x/other").unwrap();
                let bogus: HashMap<&str, Value> =
                    HashMap::from([("Latitude", Value::from(1.0)), ("Longitude", Value::from(1.0))]);
                conn.emit_signal(Some(sender.as_str()), PORTAL_PATH, LOCATION_IFACE, "LocationUpdated", &(other, bogus))
                    .await?;
                let fix: HashMap<&str, Value> = HashMap::from([
                    ("Latitude", Value::from(48.8584)),
                    ("Longitude", Value::from(2.2945)),
                    ("Accuracy", Value::from(12.0)),
                    ("Altitude", Value::from(35.0)),
                    ("Description", Value::from("Mock fix")),
                ]);
                conn.emit_signal(Some(sender.as_str()), PORTAL_PATH, LOCATION_IFACE, "LocationUpdated", &(session, fix))
                    .await?;
            }
            Ok(OwnedObjectPath::try_from(request).unwrap())
        }
    }

    #[interface(name = "org.freedesktop.portal.Session")]
    impl Session {
        async fn close(&self) {
            self.0.lock().unwrap().closed.push(self.1.clone());
        }
    }

    /// A private message bus: no service activation, killed on drop.
    struct Bus {
        child: Child,
        address: String,
        dir: std::path::PathBuf,
    }

    impl Bus {
        fn start() -> Option<Bus> {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            let dir = std::env::temp_dir().join(format!("helix-maps-bus-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).ok()?;
            let config = dir.join("bus.conf");
            std::fs::write(
                &config,
                format!(
                    r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>"#,
                    dir.display()
                ),
            )
            .ok()?;
            let mut child = Command::new("/usr/bin/dbus-daemon")
                .arg(format!("--config-file={}", config.display()))
                .arg("--nofork")
                .arg("--print-address=1")
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("DBUS_SESSION_BUS_ADDRESS")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut line = String::new();
            BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
            let address = line.trim().to_string();
            if address.is_empty() {
                let _ = child.kill();
                return None;
            }
            Some(Bus { child, address, dir })
        }

        fn connect(&self) -> Connection {
            zbus::blocking::connection::Builder::address(self.address.as_str()).unwrap().build().unwrap()
        }

        fn serve(&self, state: Shared) -> Connection {
            zbus::blocking::connection::Builder::address(self.address.as_str())
                .unwrap()
                .name(PORTAL_BUS)
                .unwrap()
                .serve_at(PORTAL_PATH, Location(state))
                .unwrap()
                .build()
                .unwrap()
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn bus() -> Option<Bus> {
        let b = Bus::start();
        if b.is_none() {
            eprintln!("skipping: /usr/bin/dbus-daemon unavailable");
        }
        b
    }

    #[test]
    fn granted_fix() {
        let Some(bus) = bus() else { return };
        let state: Shared = Arc::new(Mutex::new(State { send_fix: true, ..Default::default() }));
        let _server = bus.serve(state.clone());
        let fix = locate_on(bus.connect(), ACCURACY_EXACT, Duration::from_secs(10)).unwrap();
        assert_eq!(fix, Fix { lat: 48.8584, lon: 2.2945, accuracy: 12.0, description: "Mock fix".into() });
        let s = state.lock().unwrap();
        assert_eq!(s.accuracy_requested, Some(ACCURACY_EXACT));
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.closed, s.sessions, "session is closed after the fix");
    }

    #[test]
    fn denied() {
        let Some(bus) = bus() else { return };
        let state: Shared = Arc::new(Mutex::new(State { response: 1, send_fix: true, ..Default::default() }));
        let _server = bus.serve(state.clone());
        let err = locate_on(bus.connect(), ACCURACY_EXACT, Duration::from_secs(10)).unwrap_err();
        assert_eq!(err, "Location access was denied");
        let s = state.lock().unwrap();
        assert_eq!(s.closed, s.sessions);
    }

    #[test]
    fn portal_absent() {
        let Some(bus) = bus() else { return };
        let err = locate_on(bus.connect(), ACCURACY_EXACT, Duration::from_secs(10)).unwrap_err();
        assert_eq!(err, "The location portal is not available");
    }

    #[test]
    fn no_fix_times_out() {
        let Some(bus) = bus() else { return };
        let state: Shared = Arc::new(Mutex::new(State::default()));
        let _server = bus.serve(state.clone());
        let started = Instant::now();
        let err = locate_on(bus.connect(), ACCURACY_EXACT, Duration::from_millis(700)).unwrap_err();
        assert!(err.contains("Timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let s = state.lock().unwrap();
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.closed, s.sessions, "session is closed on timeout");
    }
}
