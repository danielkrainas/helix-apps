//! Current position through the XDG Location portal, falling back to
//! GeoClue2 on the system bus. Blocking; [`locate`] runs the lookup on a
//! worker thread and gives up after a timeout.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use helix_app_common::i18n::tr;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const LOCATION_IFACE: &str = "org.freedesktop.portal.Location";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";
const GEOCLUE_BUS: &str = "org.freedesktop.GeoClue2";
const GEOCLUE_MANAGER: &str = "/org/freedesktop/GeoClue2/Manager";

/// Portal accuracy `City` (enum: 0 none … 2 city … 5 exact).
const PORTAL_ACCURACY_CITY: u32 = 2;
/// GeoClue `GCLUE_ACCURACY_LEVEL_CITY`.
const GEOCLUE_ACCURACY_CITY: u32 = 4;

/// A position fix in degrees; accuracy in metres when known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: Option<f64>,
}

type Dict = HashMap<String, OwnedValue>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The portal's object path for a sender-unique token.
pub fn portal_path(kind: &str, unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/{kind}/{sender}/{token}")
}

fn token(prefix: &str) -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}_{}_{n}", std::process::id())
}

/// Extracts a fix from a portal `LocationUpdated` dictionary.
pub fn fix_from_portal(d: &Dict) -> Option<Fix> {
    let f = |k: &str| d.get(k).and_then(|v| f64::try_from(v).ok());
    let (latitude, longitude) = (f("Latitude")?, f("Longitude")?);
    valid(latitude, longitude).then_some(Fix {
        latitude,
        longitude,
        accuracy: f("Accuracy"),
    })
}

fn valid(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
}

fn via_portal() -> Result<Fix, String> {
    let conn = zbus::blocking::Connection::session().map_err(err)?;
    let unique = conn
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let location =
        zbus::blocking::Proxy::new(&conn, PORTAL_BUS, PORTAL_PATH, LOCATION_IFACE).map_err(err)?;
    let mut updates = location.receive_signal("LocationUpdated").map_err(err)?;
    let session_token = token("helix_session");
    let opts: HashMap<&str, Value> = HashMap::from([
        ("session_handle_token", Value::from(session_token.clone())),
        ("accuracy", Value::from(PORTAL_ACCURACY_CITY)),
    ]);
    let session: OwnedObjectPath = location.call("CreateSession", &(opts,)).map_err(err)?;
    let close = || {
        let _ = conn.call_method(
            Some(PORTAL_BUS),
            session.as_str(),
            Some(SESSION_IFACE),
            "Close",
            &(),
        );
    };
    let req_token = token("helix_location");
    let request = zbus::blocking::Proxy::new(
        &conn,
        PORTAL_BUS,
        portal_path("request", &unique, &req_token),
        REQUEST_IFACE,
    )
    .map_err(err)?;
    let mut responses = request.receive_signal("Response").map_err(err)?;
    let opts: HashMap<&str, Value> =
        HashMap::from([("handle_token", Value::from(req_token.clone()))]);
    if let Err(e) = location.call::<_, _, OwnedObjectPath>("Start", &(&session, "", opts)) {
        close();
        return Err(err(e));
    }
    let msg = responses
        .next()
        .ok_or(tr("The location portal closed the request"))?;
    let (code, _): (u32, Dict) = msg.body().deserialize().map_err(err)?;
    if code != 0 {
        close();
        return Err(tr("Location access was denied").into());
    }
    for msg in updates.by_ref() {
        let Ok((path, data)) = msg.body().deserialize::<(OwnedObjectPath, Dict)>() else {
            continue;
        };
        if path != session {
            continue;
        }
        if let Some(fix) = fix_from_portal(&data) {
            close();
            return Ok(fix);
        }
    }
    close();
    Err(tr("No location received").into())
}

fn via_geoclue(desktop_id: &str) -> Result<Fix, String> {
    let conn = zbus::blocking::Connection::system().map_err(err)?;
    let manager = zbus::blocking::Proxy::new(
        &conn,
        GEOCLUE_BUS,
        GEOCLUE_MANAGER,
        "org.freedesktop.GeoClue2.Manager",
    )
    .map_err(err)?;
    let client_path: OwnedObjectPath = manager.call("GetClient", &()).map_err(err)?;
    let client = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy>::new(&conn)
        .destination(GEOCLUE_BUS)
        .map_err(err)?
        .path(client_path.clone())
        .map_err(err)?
        .interface("org.freedesktop.GeoClue2.Client")
        .map_err(err)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .map_err(err)?;
    client.set_property("DesktopId", desktop_id).map_err(err)?;
    client
        .set_property("RequestedAccuracyLevel", GEOCLUE_ACCURACY_CITY)
        .map_err(err)?;
    let mut updates = client.receive_signal("LocationUpdated").map_err(err)?;
    client.call::<_, _, ()>("Start", &()).map_err(err)?;
    let result = (|| {
        for msg in updates.by_ref() {
            let Ok((_old, new)) = msg
                .body()
                .deserialize::<(OwnedObjectPath, OwnedObjectPath)>()
            else {
                continue;
            };
            let loc = zbus::blocking::Proxy::new(
                &conn,
                GEOCLUE_BUS,
                new,
                "org.freedesktop.GeoClue2.Location",
            )
            .map_err(err)?;
            let lat: f64 = loc.get_property("Latitude").map_err(err)?;
            let lon: f64 = loc.get_property("Longitude").map_err(err)?;
            let acc: Option<f64> = loc.get_property("Accuracy").ok();
            if valid(lat, lon) {
                return Ok(Fix {
                    latitude: lat,
                    longitude: lon,
                    accuracy: acc,
                });
            }
        }
        Err(tr("No location received").to_string())
    })();
    let _ = client.call::<_, _, ()>("Stop", &());
    let _ = manager.call::<_, _, ()>("DeleteClient", &(&client_path,));
    result
}

/// Looks up the current position: Location portal first, then GeoClue2.
/// Returns an error after `timeout`.
pub fn locate(desktop_id: &str, timeout: Duration) -> Result<Fix, String> {
    let (tx, rx) = mpsc::channel();
    let id = desktop_id.to_string();
    std::thread::spawn(move || {
        let r = via_portal().or_else(|portal| {
            via_geoclue(&id).map_err(|gc| format!("portal: {portal}; GeoClue: {gc}"))
        });
        let _ = tx.send(r);
    });
    rx.recv_timeout(timeout)
        .unwrap_or_else(|_| Err(tr("Timed out waiting for a location").into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(
            portal_path("session", ":1.7", "t"),
            "/org/freedesktop/portal/desktop/session/1_7/t"
        );
    }

    #[test]
    fn portal_dict() {
        let mut d = Dict::new();
        d.insert("Latitude".into(), OwnedValue::from(52.5f64));
        assert!(fix_from_portal(&d).is_none());
        d.insert("Longitude".into(), OwnedValue::from(13.4f64));
        d.insert("Accuracy".into(), OwnedValue::from(1000.0f64));
        assert_eq!(
            fix_from_portal(&d),
            Some(Fix {
                latitude: 52.5,
                longitude: 13.4,
                accuracy: Some(1000.0)
            })
        );
        d.insert("Latitude".into(), OwnedValue::from(95.0f64));
        assert!(fix_from_portal(&d).is_none());
    }
}
