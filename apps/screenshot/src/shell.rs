//! Captures through the shell's `org.gnome.Shell.Screenshot` D-Bus API.
//!
//! The shell answers only callers that own `org.gnome.Screenshot`, so each
//! capture opens its own session-bus connection, claims that name, makes
//! every call on that connection, and closes it afterwards. Everything here
//! blocks; the UI runs it with `gio::spawn_blocking`.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use zbus::blocking::Connection;

use crate::naming;

const SHELL_NAME: &str = "org.gnome.Shell";
const SHELL_PATH: &str = "/org/gnome/Shell/Screenshot";
const SHELL_IFACE: &str = "org.gnome.Shell.Screenshot";
const PICKER_IFACE: &str = "org.helix.ScreenshotWindowPicker";
/// The name the shell requires its callers to own.
pub const CALLER_NAME: &str = "org.gnome.Screenshot";

const NO_SHELL: &str =
    "The desktop shell does not offer screenshots. Screenshot needs Helix or GNOME Shell.";

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Screen,
    Window,
    Area,
}

/// One capture.
#[derive(Debug, Clone)]
pub struct Request {
    pub mode: Mode,
    pub delay: Duration,
    /// Draw the pointer into the image (Screen and Window only).
    pub pointer: bool,
    /// Include the window frame (Window only).
    pub frame: bool,
    /// In Window mode, let the user click the window (when the shell offers
    /// the Helix picker) instead of capturing the focused one.
    pub pick_window: bool,
    /// Pause after the app's window was hidden and after the selection,
    /// so that neither shows in the image.
    pub settle: Duration,
}

impl Request {
    pub const SETTLE: Duration = Duration::from_millis(300);

    pub fn new(mode: Mode, delay_secs: u32, pointer: bool) -> Self {
        Request {
            mode,
            delay: Duration::from_secs(delay_secs.into()),
            pointer,
            frame: true,
            pick_window: false,
            settle: Self::SETTLE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// The user cancelled the shell's selection; not an error to report.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Cancelled => f.write_str("Cancelled"),
            CaptureError::Failed(reason) => f.write_str(reason),
        }
    }
}

type Result<T> = std::result::Result<T, CaptureError>;

/// Takes a screenshot on a new session-bus connection and returns the PNG
/// bytes.
pub fn take(req: &Request) -> Result<Vec<u8>> {
    let conn = Connection::session()
        .map_err(|e| CaptureError::Failed(format!("Could not connect to the session bus: {e}")))?;
    take_on(&conn, req)
}

/// Takes a screenshot on `conn`, owning [`CALLER_NAME`] for the duration.
pub fn take_on(conn: &Connection, req: &Request) -> Result<Vec<u8>> {
    // Never queue: a queued request does not own the name, and the shell
    // would refuse every call.
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    match conn.request_name_with_flags(CALLER_NAME, flags) {
        Ok(
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner,
        ) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => {
            return Err(CaptureError::Failed(
                "Another screenshot is already being taken.".into(),
            ))
        }
        Err(e) => {
            return Err(CaptureError::Failed(format!(
                "Could not register as a screenshot tool on the session bus: {e}"
            )))
        }
    }
    let result = capture(conn, req);
    let _ = conn.release_name(CALLER_NAME);
    result
}

fn capture(conn: &Connection, req: &Request) -> Result<Vec<u8>> {
    std::thread::sleep(req.settle);
    let area = if req.mode == Mode::Area {
        let area = select_area(conn)?;
        std::thread::sleep(req.settle);
        Some(area)
    } else {
        None
    };
    std::thread::sleep(req.delay);

    let target = naming::temp_capture_path();
    let target_str = target.to_string_lossy().into_owned();
    let flash = true;
    let reply = match (req.mode, area) {
        (Mode::Area, Some((x, y, w, h))) => call(
            conn,
            SHELL_IFACE,
            "ScreenshotArea",
            &(x, y, w, h, flash, &target_str),
        ),
        (Mode::Window, _) if req.pick_window && req.delay.is_zero() => {
            match call(
                conn,
                PICKER_IFACE,
                "PickWindow",
                &(req.pointer, flash, &target_str),
            ) {
                Err(e) if is_unknown_method(&e) => call(
                    conn,
                    SHELL_IFACE,
                    "ScreenshotWindow",
                    &(req.frame, req.pointer, flash, &target_str),
                ),
                other => other,
            }
        }
        (Mode::Window, _) => call(
            conn,
            SHELL_IFACE,
            "ScreenshotWindow",
            &(req.frame, req.pointer, flash, &target_str),
        ),
        _ => call(
            conn,
            SHELL_IFACE,
            "Screenshot",
            &(req.pointer, flash, &target_str),
        ),
    };
    let (success, used): (bool, String) = reply
        .and_then(|m| m.body().deserialize())
        .map_err(describe)?;
    if !success {
        let _ = std::fs::remove_file(&target);
        let mut reason = String::from("The shell could not take the screenshot.");
        if req.mode == Mode::Window {
            reason.push_str(" Is a window focused?");
        }
        return Err(CaptureError::Failed(reason));
    }
    let used = if used.is_empty() {
        target.clone()
    } else {
        used.into()
    };
    let bytes = read_capture(&used);
    let _ = std::fs::remove_file(&used);
    if used != target {
        let _ = std::fs::remove_file(&target);
    }
    bytes
}

fn read_capture(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).map_err(|e| {
        CaptureError::Failed(format!(
            "Could not read the screenshot from {}: {e}",
            path.display()
        ))
    })?;
    if !is_png(&bytes) {
        return Err(CaptureError::Failed(format!(
            "The shell wrote something other than a PNG image to {}.",
            path.display()
        )));
    }
    Ok(bytes)
}

pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

fn call<B>(conn: &Connection, iface: &str, method: &str, body: &B) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(SHELL_NAME), SHELL_PATH, Some(iface), method, body)
}

/// The user drags out a rectangle; `Cancelled` when they cancel.
fn select_area(conn: &Connection) -> Result<(i32, i32, i32, i32)> {
    let (x, y, w, h): (i32, i32, i32, i32) = call(conn, SHELL_IFACE, "SelectArea", &())
        .and_then(|m| m.body().deserialize())
        .map_err(describe)?;
    if w <= 0 || h <= 0 {
        return Err(CaptureError::Cancelled);
    }
    Ok((x, y, w, h))
}

fn is_unknown_method(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.UnknownMethod"
                | "org.freedesktop.DBus.Error.UnknownInterface"
                | "org.freedesktop.DBus.Error.UnknownObject"
        ),
        _ => false,
    }
}

/// Turns a D-Bus failure into what the user is told.
fn describe(e: zbus::Error) -> CaptureError {
    match &e {
        zbus::Error::MethodError(name, detail, _) => {
            let detail = detail.as_deref().unwrap_or("");
            match name.as_str() {
                "org.freedesktop.DBus.Error.Failed"
                    if detail.to_ascii_lowercase().contains("cancel") =>
                {
                    CaptureError::Cancelled
                }
                "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner" => {
                    CaptureError::Failed(NO_SHELL.into())
                }
                "org.freedesktop.DBus.Error.UnknownMethod"
                | "org.freedesktop.DBus.Error.UnknownInterface"
                | "org.freedesktop.DBus.Error.UnknownObject" => CaptureError::Failed(format!(
                    "The desktop shell does not support this kind of screenshot ({detail})."
                )),
                "org.freedesktop.DBus.Error.AccessDenied" => CaptureError::Failed(format!(
                    "The desktop shell refused the screenshot: {detail}"
                )),
                _ if !detail.is_empty() => CaptureError::Failed(detail.to_string()),
                _ => CaptureError::Failed(format!("The shell reported an error ({name}).")),
            }
        }
        _ => CaptureError::Failed(format!("D-Bus error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    //! Runs against a fake shell on a private `dbus-daemon`, never the
    //! session bus.

    use super::*;
    use std::io::BufRead;
    use std::sync::{Arc, Mutex};

    /// A 1×1 PNG.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[derive(Default)]
    struct State {
        calls: Vec<String>,
        cancel_select: bool,
        fail_capture: bool,
        no_picker: bool,
    }

    struct FakeShell(Arc<Mutex<State>>);

    async fn caller_allowed(conn: &zbus::Connection, hdr: &zbus::message::Header<'_>) -> bool {
        let Some(sender) = hdr.sender() else {
            return false;
        };
        let Ok(proxy) = zbus::fdo::DBusProxy::new(conn).await else {
            return false;
        };
        let name = zbus::names::BusName::try_from(CALLER_NAME).unwrap();
        matches!(proxy.get_name_owner(name).await, Ok(owner) if owner.as_str() == sender.as_str())
    }

    impl FakeShell {
        fn write(&self, kind: &str, filename: &str) -> (bool, String) {
            let mut st = self.0.lock().unwrap();
            st.calls.push(format!("{kind} {filename}"));
            if st.fail_capture {
                return (false, String::new());
            }
            std::fs::write(filename, PNG).unwrap();
            (true, filename.to_string())
        }
    }

    #[zbus::interface(name = "org.gnome.Shell.Screenshot")]
    impl FakeShell {
        async fn screenshot(
            &self,
            include_cursor: bool,
            _flash: bool,
            filename: String,
            #[zbus(header)] hdr: zbus::message::Header<'_>,
            #[zbus(connection)] conn: &zbus::Connection,
        ) -> zbus::fdo::Result<(bool, String)> {
            if !caller_allowed(conn, &hdr).await {
                return Err(zbus::fdo::Error::AccessDenied("not allowed".into()));
            }
            Ok(self.write(&format!("Screenshot cursor={include_cursor}"), &filename))
        }

        async fn screenshot_window(
            &self,
            include_frame: bool,
            include_cursor: bool,
            _flash: bool,
            filename: String,
            #[zbus(header)] hdr: zbus::message::Header<'_>,
            #[zbus(connection)] conn: &zbus::Connection,
        ) -> zbus::fdo::Result<(bool, String)> {
            if !caller_allowed(conn, &hdr).await {
                return Err(zbus::fdo::Error::AccessDenied("not allowed".into()));
            }
            Ok(self.write(
                &format!("ScreenshotWindow frame={include_frame} cursor={include_cursor}"),
                &filename,
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn screenshot_area(
            &self,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            _flash: bool,
            filename: String,
            #[zbus(header)] hdr: zbus::message::Header<'_>,
            #[zbus(connection)] conn: &zbus::Connection,
        ) -> zbus::fdo::Result<(bool, String)> {
            if !caller_allowed(conn, &hdr).await {
                return Err(zbus::fdo::Error::AccessDenied("not allowed".into()));
            }
            Ok(self.write(
                &format!("ScreenshotArea {x},{y} {width}x{height}"),
                &filename,
            ))
        }

        async fn select_area(
            &self,
            #[zbus(header)] hdr: zbus::message::Header<'_>,
            #[zbus(connection)] conn: &zbus::Connection,
        ) -> zbus::fdo::Result<(i32, i32, i32, i32)> {
            if !caller_allowed(conn, &hdr).await {
                return Err(zbus::fdo::Error::AccessDenied("not allowed".into()));
            }
            let mut st = self.0.lock().unwrap();
            st.calls.push("SelectArea".into());
            if st.cancel_select {
                return Err(zbus::fdo::Error::Failed("Operation was cancelled".into()));
            }
            Ok((10, 20, 30, 40))
        }
    }

    struct FakePicker(Arc<Mutex<State>>);

    #[zbus::interface(name = "org.helix.ScreenshotWindowPicker")]
    impl FakePicker {
        async fn pick_window(
            &self,
            include_cursor: bool,
            _flash: bool,
            filename: String,
        ) -> zbus::fdo::Result<(bool, String)> {
            let mut st = self.0.lock().unwrap();
            if st.no_picker {
                return Err(zbus::fdo::Error::UnknownMethod("no picker".into()));
            }
            st.calls
                .push(format!("PickWindow cursor={include_cursor} {filename}"));
            std::fs::write(&filename, PNG).unwrap();
            Ok((true, filename))
        }
    }

    struct Bus {
        daemon: std::process::Child,
        address: String,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    fn private_bus() -> Option<Bus> {
        let mut daemon = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let mut line = String::new();
        std::io::BufReader::new(daemon.stdout.take()?)
            .read_line(&mut line)
            .ok()?;
        let address = line.trim().to_string();
        if address.is_empty() {
            let _ = daemon.kill();
            return None;
        }
        Some(Bus { daemon, address })
    }

    fn connect(bus: &Bus) -> Connection {
        zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .unwrap()
    }

    fn serve(bus: &Bus, state: &Arc<Mutex<State>>) -> Connection {
        zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(SHELL_NAME)
            .unwrap()
            .serve_at(SHELL_PATH, FakeShell(state.clone()))
            .unwrap()
            .serve_at(SHELL_PATH, FakePicker(state.clone()))
            .unwrap()
            .build()
            .unwrap()
    }

    fn quick(mode: Mode) -> Request {
        Request {
            settle: Duration::ZERO,
            ..Request::new(mode, 0, false)
        }
    }

    #[test]
    fn captures_through_a_fake_shell() {
        let Some(bus) = private_bus() else {
            eprintln!("dbus-daemon unavailable; skipping");
            return;
        };
        let state = Arc::new(Mutex::new(State::default()));
        let _shell = serve(&bus, &state);
        let app = connect(&bus);

        // Screen, with pointer.
        let req = Request {
            pointer: true,
            ..quick(Mode::Screen)
        };
        assert_eq!(take_on(&app, &req).unwrap(), PNG);
        // Window: the focused window, frame per request.
        let req = Request {
            frame: false,
            ..quick(Mode::Window)
        };
        assert_eq!(take_on(&app, &req).unwrap(), PNG);
        // Window with the picker.
        let req = Request {
            pick_window: true,
            ..quick(Mode::Window)
        };
        assert_eq!(take_on(&app, &req).unwrap(), PNG);
        // Area: select, then capture that rectangle.
        assert_eq!(take_on(&app, &quick(Mode::Area)).unwrap(), PNG);

        let calls = std::mem::take(&mut state.lock().unwrap().calls);
        let kinds: Vec<&str> = calls
            .iter()
            .map(|c| c.rsplit_once(' ').map_or(c.as_str(), |(k, _)| k))
            .collect();
        assert_eq!(
            kinds,
            [
                "Screenshot cursor=true",
                "ScreenshotWindow frame=false cursor=false",
                "PickWindow cursor=false",
                "SelectArea",
                "ScreenshotArea 10,20 30x40",
            ],
            "{calls:?}"
        );
        // The temporary files are gone again.
        for c in calls.iter().filter(|c| c.starts_with("Screenshot")) {
            let path = c.rsplit_once(' ').unwrap().1;
            assert!(path.contains("helix-screenshot-"), "{path}");
            assert!(!Path::new(path).exists(), "{path} left behind");
        }

        // Without the picker, Window falls back to the focused window.
        state.lock().unwrap().no_picker = true;
        let req = Request {
            pick_window: true,
            ..quick(Mode::Window)
        };
        assert_eq!(take_on(&app, &req).unwrap(), PNG);
        let calls = std::mem::take(&mut state.lock().unwrap().calls);
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].starts_with("ScreenshotWindow frame=true"),
            "{calls:?}"
        );

        // A cancelled selection is reported as such, without a capture.
        state.lock().unwrap().cancel_select = true;
        assert_eq!(
            take_on(&app, &quick(Mode::Area)),
            Err(CaptureError::Cancelled)
        );
        assert_eq!(state.lock().unwrap().calls, ["SelectArea"]);

        // success = false becomes an error with a reason.
        state.lock().unwrap().fail_capture = true;
        match take_on(&app, &quick(Mode::Window)) {
            Err(CaptureError::Failed(reason)) => assert!(reason.contains("window"), "{reason}"),
            other => panic!("{other:?}"),
        }

        // The name is released after each capture.
        let dbus = zbus::blocking::fdo::DBusProxy::new(&app).unwrap();
        let name = zbus::names::BusName::try_from(CALLER_NAME).unwrap();
        assert!(!dbus.name_has_owner(name).unwrap());
    }

    #[test]
    fn name_taken_and_no_shell() {
        let Some(bus) = private_bus() else {
            eprintln!("dbus-daemon unavailable; skipping");
            return;
        };
        let app = connect(&bus);
        // No shell on the bus.
        assert_eq!(
            take_on(&app, &quick(Mode::Screen)),
            Err(CaptureError::Failed(NO_SHELL.into()))
        );
        // Another tool owns the caller name.
        let other = connect(&bus);
        other.request_name(CALLER_NAME).unwrap();
        match take_on(&app, &quick(Mode::Screen)) {
            Err(CaptureError::Failed(reason)) => assert!(reason.contains("Another"), "{reason}"),
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn png_signature() {
        assert!(is_png(PNG));
        assert!(!is_png(b"GIF89a"));
        assert!(!is_png(b""));
    }
}
