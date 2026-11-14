//! XDG Camera portal (`org.freedesktop.portal.Camera`).

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU32, Ordering};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const BUS_NAME: &str = "org.freedesktop.portal.Desktop";
const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
const CAMERA_IFACE: &str = "org.freedesktop.portal.Camera";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// Result of asking the portal for camera access.
#[derive(Debug)]
pub enum Access {
    Granted(OwnedFd),
    Denied,
    NoCamera,
}

/// Whether the app runs inside a sandbox (or the portal is forced on).
pub fn should_use_portal() -> bool {
    match std::env::var("HELIX_CAMERA_PORTAL").ok().as_deref() {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => std::path::Path::new("/.flatpak-info").exists(),
    }
}

/// A fresh, valid `handle_token` (letters, digits and `_` only).
pub fn handle_token() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "helix_camera_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Options for `AccessCamera`.
pub fn access_options(token: &str) -> HashMap<&'static str, Value<'_>> {
    let mut opts = HashMap::new();
    opts.insert("handle_token", Value::from(token));
    opts
}

/// Options for `OpenPipeWireRemote` (currently none are defined).
pub fn remote_options() -> HashMap<&'static str, Value<'static>> {
    HashMap::new()
}

/// The request object path the portal will use for `token` (as specified by
/// the `org.freedesktop.portal.Request` documentation).
pub fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{OBJECT_PATH}/request/{sender}/{token}")
}

/// Maps a `Response` code to an outcome.
pub fn response_granted(code: u32) -> bool {
    code == 0
}

/// Blocking: requests access and opens the PipeWire remote. Call from a
/// worker thread only.
pub fn access_camera() -> zbus::Result<Access> {
    let conn = zbus::blocking::Connection::session()?;
    let camera = zbus::blocking::Proxy::new(&conn, BUS_NAME, OBJECT_PATH, CAMERA_IFACE)?;
    if let Ok(false) = camera.get_property::<bool>("IsCameraPresent") {
        return Ok(Access::NoCamera);
    }
    let token = handle_token();
    let unique = conn
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let predicted = request_path(&unique, &token);
    let request = zbus::blocking::Proxy::new(&conn, BUS_NAME, predicted.clone(), REQUEST_IFACE)?;
    let mut responses = request.receive_signal("Response")?;

    let reply = camera.call_method("AccessCamera", &(access_options(&token),))?;
    let handle: OwnedObjectPath = reply.body().deserialize()?;
    let msg = if handle.as_str() == predicted {
        responses.next()
    } else {
        // Very old portals return a different handle.
        let request = zbus::blocking::Proxy::new(&conn, BUS_NAME, handle.as_str(), REQUEST_IFACE)?;
        let mut other = request.receive_signal("Response")?;
        other.next()
    };
    let Some(msg) = msg else {
        return Ok(Access::Denied);
    };
    let (code, _results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    if !response_granted(code) {
        return Ok(Access::Denied);
    }
    let reply = camera.call_method("OpenPipeWireRemote", &(remote_options(),))?;
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize()?;
    Ok(Access::Granted(fd.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_path_from_unique_name() {
        assert_eq!(
            request_path(":1.42", "helix_camera_1_0"),
            "/org/freedesktop/portal/desktop/request/1_42/helix_camera_1_0"
        );
    }

    #[test]
    fn tokens_are_unique_and_valid() {
        let a = handle_token();
        let b = handle_token();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn access_options_carry_token() {
        let opts = access_options("tok_1");
        assert_eq!(opts.len(), 1);
        assert_eq!(opts["handle_token"], Value::from("tok_1"));
        assert!(remote_options().is_empty());
        // Serialisable as a{sv}.
        let v = Value::from(opts);
        assert_eq!(v.value_signature().to_string(), "a{sv}");
    }

    #[test]
    fn response_codes() {
        assert!(response_granted(0));
        assert!(!response_granted(1));
        assert!(!response_granted(2));
    }
}
