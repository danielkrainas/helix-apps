//! Setting the desktop background through the XDG Wallpaper portal
//! (`org.freedesktop.portal.Wallpaper.SetWallpaperURI`).

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const WALLPAPER_IFACE: &str = "org.freedesktop.portal.Wallpaper";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Set,
    Cancelled,
    Failed,
}

/// The portal's `Request` object path for a sender and handle token.
pub fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// Options passed to `SetWallpaperURI`.
pub fn options(token: &str) -> HashMap<&'static str, Value<'static>> {
    HashMap::from([
        ("handle_token", Value::from(token.to_string())),
        ("show-preview", Value::from(true)),
        ("set-on", Value::from("background".to_string())),
    ])
}

pub fn outcome(code: u32) -> Outcome {
    match code {
        0 => Outcome::Set,
        1 => Outcome::Cancelled,
        _ => Outcome::Failed,
    }
}

/// Asks the portal to set `uri` as the background and waits for the user's
/// answer. Blocking: call from a worker thread. `parent` is a portal parent
/// window identifier ("" if unknown).
pub fn set_wallpaper(uri: &str, parent: &str) -> Result<Outcome, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    let unique = conn
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default();
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let token = format!("helix_image_viewer_{}_{n}", std::process::id());
    let path = request_path(&unique, &token);
    // Subscribe before calling so the Response cannot be missed.
    let request = zbus::blocking::Proxy::new(&conn, PORTAL_BUS, path, REQUEST_IFACE)
        .map_err(|e| e.to_string())?;
    let mut responses = request
        .receive_signal("Response")
        .map_err(|e| e.to_string())?;
    conn.call_method(
        Some(PORTAL_BUS),
        PORTAL_PATH,
        Some(WALLPAPER_IFACE),
        "SetWallpaperURI",
        &(parent, uri, options(&token)),
    )
    .map_err(|e| e.to_string())?;
    let msg = responses
        .next()
        .ok_or_else(|| "The portal closed the request".to_string())?;
    let (code, _results): (u32, HashMap<String, OwnedValue>) =
        msg.body().deserialize().map_err(|e| e.to_string())?;
    Ok(outcome(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_path_escapes_sender() {
        assert_eq!(
            request_path(":1.42", "tok"),
            "/org/freedesktop/portal/desktop/request/1_42/tok"
        );
    }

    #[test]
    fn options_ask_for_preview_on_background() {
        let o = options("t1");
        assert_eq!(o["show-preview"], Value::from(true));
        assert_eq!(o["set-on"], Value::from("background"));
        assert_eq!(o["handle_token"], Value::from("t1"));
    }

    #[test]
    fn outcomes() {
        assert_eq!(outcome(0), Outcome::Set);
        assert_eq!(outcome(1), Outcome::Cancelled);
        assert_eq!(outcome(2), Outcome::Failed);
    }
}
