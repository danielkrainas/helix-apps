//! Running in the background: the XDG Background portal
//! (`org.freedesktop.portal.Background.RequestBackground`) asks for
//! permission to keep running without a window and to be started at login
//! with [`AUTOSTART_COMMANDLINE`], so enabled alarms survive closing the
//! window and logging out. Blocking; call from a worker thread.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const BACKGROUND_IFACE: &str = "org.freedesktop.portal.Background";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const REGISTRY_IFACE: &str = "org.freedesktop.host.portal.Registry";

/// The command the session starts at login: no window, just alarms.
pub const AUTOSTART_COMMANDLINE: [&str; 2] = ["helix-clocks", "--hidden"];

/// What the portal granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grant {
    pub background: bool,
    pub autostart: bool,
}

pub fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{token}")
}

/// `RequestBackground` options.
pub fn options(token: &str, autostart: bool) -> HashMap<&'static str, Value<'static>> {
    HashMap::from([
        ("handle_token", Value::from(token.to_string())),
        (
            "reason",
            Value::from("Ring alarms when Clocks is closed".to_string()),
        ),
        ("autostart", Value::from(autostart)),
        (
            "commandline",
            Value::from(
                AUTOSTART_COMMANDLINE
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>(),
            ),
        ),
        ("dbus-activatable", Value::from(false)),
    ])
}

/// Interprets the `Response` signal.
pub fn grant(code: u32, results: &HashMap<String, OwnedValue>) -> Grant {
    let flag = |k: &str| {
        results
            .get(k)
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false)
    };
    if code == 0 {
        Grant {
            background: flag("background"),
            autostart: flag("autostart"),
        }
    } else {
        Grant::default()
    }
}

/// Requests background permission (and autostart when `autostart`; pass
/// `false` to remove the autostart entry once no alarm is enabled).
pub fn request(app_id: &str, autostart: bool) -> Result<Grant, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    // Unsandboxed apps identify themselves to newer portals; older portals
    // lack the interface, which is fine.
    let _ = conn.call_method(
        Some(PORTAL_BUS),
        PORTAL_PATH,
        Some(REGISTRY_IFACE),
        "Register",
        &(app_id, HashMap::<&str, Value>::new()),
    );
    let unique = conn
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default();
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let token = format!(
        "helix_clocks_bg_{}_{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let request = zbus::blocking::Proxy::new(
        &conn,
        PORTAL_BUS,
        request_path(&unique, &token),
        REQUEST_IFACE,
    )
    .map_err(|e| e.to_string())?;
    let mut responses = request
        .receive_signal("Response")
        .map_err(|e| e.to_string())?;
    conn.call_method(
        Some(PORTAL_BUS),
        PORTAL_PATH,
        Some(BACKGROUND_IFACE),
        "RequestBackground",
        &("", options(&token, autostart)),
    )
    .map_err(|e| e.to_string())?;
    let msg = responses
        .next()
        .ok_or_else(|| "The portal closed the request".to_string())?;
    let (code, results): (u32, HashMap<String, OwnedValue>) =
        msg.body().deserialize().map_err(|e| e.to_string())?;
    Ok(grant(code, &results))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_options() {
        let o = options("tok", true);
        assert_eq!(o["handle_token"], Value::from("tok"));
        assert_eq!(o["autostart"], Value::from(true));
        assert_eq!(o["dbus-activatable"], Value::from(false));
        let cmd: Vec<String> = o["commandline"].clone().try_into().unwrap();
        assert_eq!(cmd, vec!["helix-clocks", "--hidden"]);
        assert_eq!(options("t", false)["autostart"], Value::from(false));
    }

    #[test]
    fn grants() {
        let mut r = HashMap::new();
        r.insert("background".to_string(), OwnedValue::from(true));
        r.insert("autostart".to_string(), OwnedValue::from(false));
        assert_eq!(
            grant(0, &r),
            Grant {
                background: true,
                autostart: false
            }
        );
        assert_eq!(grant(1, &r), Grant::default());
        assert_eq!(
            request_path(":1.5", "x"),
            "/org/freedesktop/portal/desktop/request/1_5/x"
        );
    }

    #[test]
    fn shipped_autostart_file_matches_commandline() {
        let text = include_str!("../data/org.helix.Clocks-autostart.desktop");
        assert!(text
            .lines()
            .any(|l| l == format!("Exec={}", AUTOSTART_COMMANDLINE.join(" "))));
    }
}
