//! `org.freedesktop.FileManager1` service.

use adw::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

use super::Shared;

const XML: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItems">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItemProperties">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
  </interface>
</node>"#;

const NAME: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";

pub fn register(shared: &Rc<Shared>) {
    let Some(conn) = shared.app.dbus_connection() else {
        return;
    };
    let Ok(node) = gio::DBusNodeInfo::for_xml(XML) else {
        return;
    };
    let Some(iface) = node.lookup_interface(NAME) else {
        return;
    };
    let weak = Rc::downgrade(shared);
    let reg = conn
        .register_object(PATH, &iface)
        .method_call(move |_, _, _, _, method, params, invocation| {
            let Some(shared) = weak.upgrade() else {
                invocation.return_value(None);
                return;
            };
            let Some((uris, startup)) = params.get::<(Vec<String>, String)>() else {
                invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Expected (as, s)");
                return;
            };
            if !matches!(method, "ShowFolders" | "ShowItems" | "ShowItemProperties") {
                invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unknown method");
                return;
            }
            // An empty list is a no-op (never an unasked-for home window).
            if !uris.is_empty() {
                let files: Vec<gio::File> = uris
                    .iter()
                    .map(|u| gio::File::for_commandline_arg(u.as_str()))
                    .collect();
                let startup = Some(startup);
                match method {
                    "ShowFolders" => {
                        for f in files {
                            super::open_locations(&shared, vec![f], false, false, startup.clone());
                        }
                    }
                    "ShowItems" => super::open_locations(&shared, files, true, false, startup),
                    _ => super::open_locations(&shared, files, true, true, startup),
                }
            }
            invocation.return_value(None);
        })
        .build();
    if reg.is_err() {
        return;
    }
    // Claim the well-known name only if nobody else has it: no REPLACE, so a
    // running Nautilus (or whichever file manager packaging made the
    // default) keeps it, and we stay queued in case it goes away.
    // ALLOW_REPLACEMENT lets a file manager that is configured to REPLACE
    // take over. Ownership is kept for the app's lifetime.
    //
    // A plain RequestName call is used rather than
    // `gio::bus_own_name_on_connection`, whose gio-rs closure wrapper
    // panics (aborting) when the bus connection closes and GIO reports the
    // name as lost with a NULL connection.
    conn.call(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "RequestName",
        Some(&(NAME, REQUEST_FLAGS).to_variant()),
        Some(glib::VariantTy::new("(u)").unwrap()),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        |res| match res.map(|v| v.get::<(u32,)>().map(|(r,)| r)) {
            Ok(Some(1 | 4)) => eprintln!("helix-files: acquired {NAME}"),
            Ok(Some(2 | 3)) => {
                eprintln!("helix-files: {NAME} is owned by another file manager; not replacing it (queued)")
            }
            Ok(r) => eprintln!("helix-files: RequestName({NAME}) returned {r:?}"),
            Err(e) => eprintln!("helix-files: could not request {NAME}: {e}"),
        },
    );
}

/// `DBUS_NAME_FLAG_ALLOW_REPLACEMENT`; deliberately without
/// `DBUS_NAME_FLAG_REPLACE_EXISTING` (2) or `DO_NOT_QUEUE` (4).
const REQUEST_FLAGS: u32 = 1;
