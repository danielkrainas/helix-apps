//! `org.freedesktop.FileManager1` service.

use adw::prelude::*;
use gtk::gio;
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
            let Some((uris, _startup)) = params.get::<(Vec<String>, String)>() else {
                invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Expected (as, s)");
                return;
            };
            let files: Vec<gio::File> = uris
                .iter()
                .map(|u| gio::File::for_commandline_arg(u.as_str()))
                .collect();
            match method {
                "ShowFolders" => {
                    for f in files {
                        super::open_locations(&shared, vec![f], false, false);
                    }
                }
                "ShowItems" => super::open_locations(&shared, files, true, false),
                "ShowItemProperties" => super::open_locations(&shared, files, true, true),
                _ => {
                    invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unknown method");
                    return;
                }
            }
            invocation.return_value(None);
        })
        .build();
    if reg.is_err() {
        return;
    }
    // Claim the well-known name only if nobody else has it.
    // Ownership is kept for the app's lifetime (never unowned).
    let _ = gio::bus_own_name_on_connection(&conn, NAME, gio::BusNameOwnerFlags::NONE, |_, _| {}, |_, _| {});
}
