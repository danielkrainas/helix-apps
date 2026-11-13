//! One-shot current-location lookup through GeoClue 2 on the system bus.
//! Blocking; call from a worker thread.

use std::time::{Duration, Instant};
use zbus::blocking::{proxy::Builder, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

const BUS_NAME: &str = "org.freedesktop.GeoClue2";
/// GeoClue accuracy level "exact".
const ACCURACY_EXACT: u32 = 8;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub accuracy: f64,
    pub description: String,
}

fn proxy<'a>(conn: &Connection, path: &'a str, iface: &'a str) -> zbus::Result<Proxy<'a>> {
    Builder::<Proxy>::new(conn)
        .destination(BUS_NAME)?
        .path(path)?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
}

fn friendly(e: zbus::Error) -> String {
    match &e {
        zbus::Error::MethodError(name, msg, _) => {
            let name = name.as_str();
            if name.ends_with("AccessDenied") {
                "Location access was denied".to_string()
            } else if name.ends_with("ServiceUnknown") || name.ends_with("NameHasNoOwner") {
                "The location service (GeoClue) is not available".to_string()
            } else {
                msg.clone().unwrap_or_else(|| name.to_string())
            }
        }
        _ => format!("Location unavailable: {e}"),
    }
}

/// Asks GeoClue for the current position, waiting up to 30 seconds.
pub fn locate(desktop_id: &str) -> Result<Fix, String> {
    let conn = Connection::system().map_err(|e| format!("Cannot reach the system bus: {e}"))?;
    let manager = proxy(&conn, "/org/freedesktop/GeoClue2/Manager", "org.freedesktop.GeoClue2.Manager")
        .map_err(friendly)?;
    let client_path: OwnedObjectPath = manager.call("GetClient", &()).map_err(friendly)?;
    let client = proxy(&conn, client_path.as_str(), "org.freedesktop.GeoClue2.Client").map_err(friendly)?;
    client.set_property("DesktopId", desktop_id).map_err(|e| friendly(e.into()))?;
    client
        .set_property("RequestedAccuracyLevel", ACCURACY_EXACT)
        .map_err(|e| friendly(e.into()))?;
    client.call_method("Start", &()).map_err(friendly)?;

    let result = (|| {
        let start = Instant::now();
        loop {
            let loc: OwnedObjectPath = client.get_property("Location").map_err(friendly)?;
            if loc.as_str() != "/" {
                let l = proxy(&conn, loc.as_str(), "org.freedesktop.GeoClue2.Location").map_err(friendly)?;
                let lat: f64 = l.get_property("Latitude").map_err(friendly)?;
                let lon: f64 = l.get_property("Longitude").map_err(friendly)?;
                let accuracy: f64 = l.get_property("Accuracy").unwrap_or(f64::NAN);
                let description: String = l.get_property("Description").unwrap_or_default();
                return Ok(Fix { lat, lon, accuracy, description });
            }
            if start.elapsed() > TIMEOUT {
                return Err("Timed out waiting for a location fix".to_string());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    })();
    let _ = client.call_method("Stop", &());
    result
}
