//! Wi-Fi panel: NetworkManager's Wi-Fi devices and networks.

use adw::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use super::netedit::{self, Editor};
use super::widgets::{self, InfoRow};
use super::{follow, spawn_worker, Ctx, Panel};
use crate::bluez;
use crate::bus;
use crate::nm::{self, Device, Network, Nm, Saved, Security};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

pub struct WifiPanel;

/// What one network row shows: name, strength in tens, connected, saved
/// connection, device state.
type Shape = (String, u8, bool, Option<String>, u32);

/// Seconds between scans while the page is on screen.
const SCAN_SECS: u32 = 20;

struct Reading {
    nm: bus::Result<Nm>,
    /// Paths of the Bluetooth adapters and whether each is on (for
    /// airplane mode).
    bluetooth: Vec<(String, bool)>,
}

fn read() -> Reading {
    let conn = bus::system();
    let nm = conn.clone().and_then(|c| nm::read(&c));
    let bluetooth = conn
        .and_then(|c| bluez::read(&c))
        .map(|s| {
            s.adapters
                .into_iter()
                .map(|a| (a.path, a.powered))
                .collect()
        })
        .unwrap_or_default();
    Reading { nm, bluetooth }
}

struct State {
    ctx: Rc<Ctx>,
    stack: gtk::Stack,
    wifi: adw::SwitchRow,
    airplane: adw::SwitchRow,
    adapter: adw::ComboRow,
    networks: adw::PreferencesGroup,
    network_rows: RefCell<Vec<gtk::Widget>>,
    hotspot: adw::PreferencesGroup,
    hotspot_name: InfoRow,
    hotspot_password: InfoRow,
    more: adw::PreferencesGroup,
    nm: RefCell<Nm>,
    bluetooth: RefCell<Vec<(String, bool)>>,
    /// Interfaces of the Wi-Fi devices, as the adapter row lists them.
    adapters: RefCell<Vec<String>>,
    /// What the network rows show (to rebuild only on change).
    shown: RefCell<Vec<Shape>>,
    quiet: Cell<bool>,
    open: Cell<bool>,
}

impl State {
    /// The Wi-Fi device the page shows.
    fn device(&self) -> Option<Device> {
        let nm = self.nm.borrow();
        let devices = nm.wifi_devices();
        let i = (self.adapter.selected() as usize).min(devices.len().saturating_sub(1));
        devices.get(i).map(|d| (*d).clone())
    }
}

impl Panel for WifiPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let stack = gtk::Stack::new();
        stack.add_named(&widgets::loading(tr("Loading Wi-Fi…")), Some("loading"));
        stack.add_named(
            &widgets::status(
                "network-wireless-disabled-symbolic",
                tr("Networking Is Not Available"),
                tr("NetworkManager is not running."),
            ),
            Some("missing"),
        );
        stack.add_named(
            &widgets::status(
                "network-wireless-disabled-symbolic",
                tr("No Wi-Fi Adapter Found"),
                tr("Make sure a Wi-Fi adapter is plugged in and turned on."),
            ),
            Some("no-device"),
        );

        let page = widgets::page();
        let top = widgets::group("");
        let wifi = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Wi-Fi"))
            .build();
        top.add(&wifi);
        let airplane = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Airplane Mode"))
            .subtitle(tr("Turns off Wi-Fi, Bluetooth and mobile broadband"))
            .build();
        top.add(&airplane);
        let adapter = adw::ComboRow::builder()
            .use_markup(false)
            .title(tr("Wi-Fi Adapter"))
            .visible(false)
            .build();
        top.add(&adapter);
        page.add(&top);

        let hotspot = widgets::group(tr("Hotspot"));
        hotspot.set_description(Some(tr(
            "Other devices can use this computer's connection through this network.",
        )));
        let hotspot_name = InfoRow::new(tr("Network Name"), "");
        let hotspot_password = InfoRow::new(tr("Password"), "");
        hotspot.add(&hotspot_name.row);
        hotspot.add(&hotspot_password.row);
        let (qr_row, qr) = widgets::action_row(tr("Share Hotspot"), "", tr("Show QR Code"), None);
        hotspot.add(&qr_row);
        let (off_row, off) = widgets::action_row(
            tr("Turn Off Hotspot"),
            "",
            tr("Turn Off"),
            Some("destructive-action"),
        );
        hotspot.add(&off_row);
        hotspot.set_visible(false);
        page.add(&hotspot);

        let networks = widgets::group(tr("Visible Networks"));
        page.add(&networks);

        let more = widgets::group("");
        let saved_row = widgets::subpage_row(
            tr("Saved Networks"),
            tr("Forget networks and change their settings"),
            "view-list-symbolic",
        );
        let hidden_row = widgets::subpage_row(
            tr("Connect to Hidden Network"),
            tr("A network that does not show its name"),
            "network-wireless-symbolic",
        );
        let hotspot_row = widgets::subpage_row(
            tr("Wi-Fi Hotspot"),
            tr("Share this computer's connection"),
            "network-wireless-hotspot-symbolic",
        );
        more.add(&saved_row);
        more.add(&hidden_row);
        more.add(&hotspot_row);
        page.add(&more);
        stack.add_named(&page, Some("list"));
        stack.set_visible_child_name("loading");

        let state = Rc::new(State {
            ctx: ctx.clone(),
            stack: stack.clone(),
            wifi: wifi.clone(),
            airplane: airplane.clone(),
            adapter: adapter.clone(),
            networks,
            network_rows: RefCell::default(),
            hotspot,
            hotspot_name,
            hotspot_password,
            more,
            nm: RefCell::default(),
            bluetooth: RefCell::default(),
            adapters: RefCell::default(),
            shown: RefCell::default(),
            quiet: Cell::new(false),
            open: Cell::new(false),
        });

        let weak = Rc::downgrade(&state);
        wifi.connect_active_notify(move |sw| {
            let Some(state) = weak.upgrade() else { return };
            if state.quiet.get() {
                return;
            }
            let on = sw.is_active();
            let weak = Rc::downgrade(&state);
            spawn_worker(
                move || bus::system().and_then(|c| nm::set_wireless(&c, on)),
                move |r| report(&weak, r, tr("Could not change Wi-Fi")),
            );
        });
        let weak = Rc::downgrade(&state);
        airplane.connect_active_notify(move |sw| {
            let Some(state) = weak.upgrade() else { return };
            if !state.quiet.get() {
                set_airplane(&state, sw.is_active());
            }
        });
        let weak = Rc::downgrade(&state);
        adapter.connect_selected_notify(move |_| {
            if let Some(state) = weak.upgrade() {
                if !state.quiet.get() {
                    state.shown.borrow_mut().clear();
                    let nm = state.nm.borrow().clone();
                    update(&state, &nm);
                }
            }
        });
        let weak = Rc::downgrade(&state);
        saved_row.connect_activated(move |_| {
            if let Some(state) = weak.upgrade() {
                saved_dialog(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        hidden_row.connect_activated(move |_| {
            if let Some(state) = weak.upgrade() {
                hidden_dialog(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        hotspot_row.connect_activated(move |_| {
            if let Some(state) = weak.upgrade() {
                hotspot_dialog(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        qr.connect_clicked(move |_| {
            let Some(state) = weak.upgrade() else { return };
            let saved = state.nm.borrow().hotspot().cloned();
            if let Some(s) = saved {
                share(&state, &s);
            }
        });
        let weak = Rc::downgrade(&state);
        off.connect_clicked(move |_| {
            let Some(state) = weak.upgrade() else { return };
            let Some(d) = state.device() else { return };
            let active = d.active.clone();
            let weak = Rc::downgrade(&state);
            spawn_worker(
                move || bus::system().and_then(|c| nm::deactivate(&c, &active)),
                move |r| report(&weak, r, tr("Could not turn the hotspot off")),
            );
        });

        // Scan while the page is on screen.
        let weak = Rc::downgrade(&state);
        stack.connect_map(move |_| {
            if let Some(state) = weak.upgrade() {
                state.open.set(true);
                scan(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        stack.connect_unmap(move |_| {
            if let Some(state) = weak.upgrade() {
                state.open.set(false);
            }
        });
        let weak = Rc::downgrade(&state);
        glib::timeout_add_seconds_local(SCAN_SECS, move || match weak.upgrade() {
            Some(state) => {
                if state.open.get() {
                    scan(&state);
                }
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });

        refresh(&state);
        follow(
            &state,
            Duration::from_millis(500),
            |tell| {
                if let Ok(conn) = bus::system() {
                    // NetworkManager and BlueZ (airplane mode).
                    bus::watch(&conn, &[nm::NM, bluez::BLUEZ], tell);
                }
            },
            refresh,
        );
        stack.connect_destroy(move |_| {
            let _ = &state;
        });
        stack.upcast()
    }
}

fn refresh(state: &Rc<State>) {
    let weak = Rc::downgrade(state);
    spawn_worker(read, move |r| {
        if let Some(state) = weak.upgrade() {
            show(&state, r);
        }
    });
}

fn scan(state: &Rc<State>) {
    let Some(d) = state.device() else { return };
    if !state.nm.borrow().wireless_enabled {
        return;
    }
    spawn_worker(
        move || {
            // NetworkManager refuses scans that come too soon; that is fine.
            let _ = bus::system().and_then(|c| nm::request_scan(&c, &d.path));
        },
        |_| {},
    );
}

/// A failed call as a toast (`what`: "Could not …"), then a fresh reading.
fn report(weak: &std::rc::Weak<State>, r: bus::Result<impl Sized>, what: &str) {
    let Some(state) = weak.upgrade() else { return };
    if let Err(e) = r {
        if e != bus::Error::Denied {
            state.ctx.toast(&format!("{what}: {e}"));
        }
    }
    refresh(&state);
}

fn airplane_on(nm: &Nm, bluetooth: &[(String, bool)]) -> bool {
    !nm.wireless_enabled && !bluetooth.iter().any(|(_, on)| *on)
}

fn show(state: &Rc<State>, r: Reading) {
    let nm = match r.nm {
        Ok(nm) => nm,
        Err(e) => {
            if e != bus::Error::Missing {
                eprintln!("helix-settings-app: NetworkManager: {e}");
            }
            state.stack.set_visible_child_name("missing");
            return;
        }
    };
    *state.bluetooth.borrow_mut() = r.bluetooth;
    let interfaces: Vec<String> = nm
        .wifi_devices()
        .iter()
        .map(|d| d.interface.clone())
        .collect();
    if interfaces.is_empty() {
        state.stack.set_visible_child_name("no-device");
        *state.nm.borrow_mut() = nm;
        return;
    }
    state.stack.set_visible_child_name("list");
    state.quiet.set(true);
    if *state.adapters.borrow() != interfaces {
        let model =
            gtk::StringList::new(&interfaces.iter().map(String::as_str).collect::<Vec<_>>());
        state.adapter.set_model(Some(&model));
        state.adapter.set_visible(interfaces.len() > 1);
        *state.adapters.borrow_mut() = interfaces;
    }
    state.wifi.set_active(nm.wireless_enabled);
    state
        .airplane
        .set_active(airplane_on(&nm, &state.bluetooth.borrow()));
    state.quiet.set(false);
    if !nm.wireless_hardware_enabled {
        state.wifi.set_sensitive(false);
        state
            .wifi
            .set_subtitle(tr("Turned off by a hardware switch"));
    } else {
        state.wifi.set_sensitive(true);
        state.wifi.set_subtitle("");
    }
    update(state, &nm);
    *state.nm.borrow_mut() = nm;
}

/// Shows the selected device's networks and hotspot.
fn update(state: &Rc<State>, nm: &Nm) {
    let devices = nm.wifi_devices();
    let i = (state.adapter.selected() as usize).min(devices.len().saturating_sub(1));
    let Some(dev) = devices.get(i).map(|d| (*d).clone()) else {
        return;
    };
    let on = nm.wireless_enabled && nm.wireless_hardware_enabled;
    let connection = nm.device_connection(&dev).cloned();
    let hotspot_on = connection.as_ref().is_some_and(|c| c.is_hotspot()) && dev.activated();
    state.hotspot.set_visible(hotspot_on);
    if let Some(c) = connection.as_ref().filter(|_| hotspot_on) {
        state.hotspot_name.set(&nm::ssid_text(&c.ssid()));
        state.hotspot_password.set("");
        let (path, weak) = (c.clone(), Rc::downgrade(state));
        spawn_worker(
            move || bus::system().and_then(|conn| nm::wifi_password(&conn, &path)),
            move |r| {
                if let (Ok(p), Some(state)) = (r, weak.upgrade()) {
                    state.hotspot_password.set(&p);
                }
            },
        );
    }
    state.networks.set_visible(!hotspot_on);
    state.more.set_visible(on);
    let networks = if on { nm.networks(&dev) } else { Vec::new() };
    let shape: Vec<Shape> = networks
        .iter()
        .map(|n| {
            (
                n.name.clone(),
                n.strength / 10,
                n.active,
                n.saved.clone(),
                dev.state,
            )
        })
        .collect();
    state.networks.set_description(if !on {
        Some(tr("Turn on Wi-Fi to see the networks around you."))
    } else if networks.is_empty() {
        Some(tr("No networks found."))
    } else {
        None
    });
    if *state.shown.borrow() == shape {
        return;
    }
    let rows: Vec<gtk::Widget> = networks
        .iter()
        .map(|n| network_row(state, &dev, n).upcast())
        .collect();
    // Keep the keyboard focus on the same network across the rebuild.
    let focused = state
        .network_rows
        .borrow()
        .iter()
        .position(|r| r.has_focus() || r.focus_child().is_some());
    widgets::replace_rows(&state.networks, &state.network_rows, rows);
    if let Some(i) = focused {
        let rows = state.network_rows.borrow();
        if let Some(r) = rows.get(i).or(rows.last()) {
            r.grab_focus();
        }
    }
    *state.shown.borrow_mut() = shape;
}

fn network_row(state: &Rc<State>, dev: &Device, n: &Network) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(&n.name)
        .activatable(true)
        .build();
    row.add_prefix(
        &gtk::Image::builder()
            .icon_name(nm::signal_icon(n.strength))
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build(),
    );
    let status = if n.active && dev.activated() {
        tr("Connected")
    } else if n.active && dev.connecting() {
        tr("Connecting…")
    } else if n.saved.is_some() {
        tr("Saved")
    } else {
        ""
    };
    row.set_subtitle(status);
    let secured = n.security != Security::None && n.security != Security::Owe;
    if secured {
        row.add_suffix(
            &gtk::Image::builder()
                .icon_name("system-lock-screen-symbolic")
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build(),
        );
    }
    // Read as "Home, Connected, Good signal, Secured".
    let mut description = nm::signal_text(n.strength).to_string();
    if secured {
        description = format!("{description}, {}", tr("Secured"));
    }
    row.update_property(&[gtk::accessible::Property::Description(&description)]);
    if let Some(saved) = &n.saved {
        let button = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text(tr("Network Details"))
            .build();
        helix_app_common::set_accessible_label(&button, &trf!("Details of {name}", name = n.name));
        let (weak, saved) = (Rc::downgrade(state), saved.clone());
        button.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                details(&state, &saved);
            }
        });
        row.add_suffix(&button);
    }
    let (weak, n, dev) = (Rc::downgrade(state), n.clone(), dev.clone());
    row.connect_activated(move |_| {
        let Some(state) = weak.upgrade() else { return };
        match &n.saved {
            Some(saved) if n.active => details(&state, saved),
            _ => join(&state, &dev, &n),
        }
    });
    row
}

/// Connects to a network the user chose.
fn join(state: &Rc<State>, dev: &Device, n: &Network) {
    let weak = Rc::downgrade(state);
    let (device, ap, name) = (dev.path.clone(), n.ap.clone(), n.name.clone());
    let failed = trf!("Could not connect to {name}", name = n.name);
    if let Some(saved) = n.saved.clone() {
        spawn_worker(
            move || bus::system().and_then(|c| nm::activate(&c, &saved, &device, &ap)),
            move |r| report(&weak, r, &failed),
        );
        return;
    }
    match n.security {
        Security::None | Security::Owe => spawn_worker(
            move || {
                bus::system()
                    .and_then(|c| nm::add_and_activate(&c, &nm::Settings::new(), &device, &ap))
            },
            move |r| report(&weak, r, &failed),
        ),
        Security::Enterprise => state.ctx.toast(&trf!(
            "{name} is an enterprise network; joining those is not supported here yet",
            name = name
        )),
        security => password_dialog(state, &name.clone(), move |state, password| {
            let settings =
                nm::wifi_settings(&name, &n_ssid(&name, &ap, state), security, password, false);
            let weak = Rc::downgrade(state);
            let (device, ap, failed) = (device.clone(), ap.clone(), failed.clone());
            spawn_worker(
                move || {
                    bus::system().and_then(|c| nm::add_and_activate(&c, &settings, &device, &ap))
                },
                move |r| report(&weak, r, &failed),
            );
        }),
    }
}

/// The network name bytes of an access point (names that are not UTF-8
/// must be sent as they are).
fn n_ssid(name: &str, ap: &str, state: &State) -> Vec<u8> {
    state
        .nm
        .borrow()
        .access_points
        .get(ap)
        .map(|a| a.ssid.clone())
        .unwrap_or_else(|| name.as_bytes().to_vec())
}

/// Asks for a network's password; `connect` gets it.
fn password_dialog(state: &Rc<State>, name: &str, connect: impl Fn(&Rc<State>, &str) + 'static) {
    let alert = adw::AlertDialog::new(
        Some(&trf!("Connect to {name}", name = name)),
        Some(tr("Enter the network's password.")),
    );
    let entry = adw::PasswordEntryRow::builder()
        .title(tr("Password"))
        .build();
    let list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.append(&entry);
    alert.set_extra_child(Some(&list));
    alert.add_response("cancel", tr("_Cancel"));
    alert.add_response("connect", tr("C_onnect"));
    alert.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
    alert.set_response_enabled("connect", false);
    alert.set_default_response(Some("connect"));
    alert.set_close_response("cancel");
    let security = state
        .nm
        .borrow()
        .access_points
        .values()
        .find(|a| a.name() == name)
        .map(|a| a.security())
        .unwrap_or(Security::Personal);
    {
        let alert = alert.downgrade();
        entry.connect_changed(move |e| {
            if let Some(a) = alert.upgrade() {
                a.set_response_enabled("connect", nm::password_ok(security, &e.text()));
            }
        });
    }
    {
        let alert = alert.downgrade();
        entry.connect_entry_activated(move |e| {
            if let Some(a) = alert.upgrade() {
                if nm::password_ok(security, &e.text()) {
                    a.emit_by_name::<()>("response", &[&"connect"]);
                    a.close();
                }
            }
        });
    }
    let weak = Rc::downgrade(state);
    alert.connect_response(None, move |_, response| {
        if response == "connect" {
            if let Some(state) = weak.upgrade() {
                connect(&state, &entry.text());
            }
        }
    });
    alert.present(Some(&state.ctx.window));
}

/// The details and settings of a saved network.
fn details(state: &Rc<State>, saved_path: &str) {
    let nm = state.nm.borrow().clone();
    let Some(saved) = nm.saved(saved_path).cloned() else {
        return;
    };
    let dev = state.device();
    let active = dev.as_ref().filter(|d| {
        nm.device_connection(d)
            .is_some_and(|c| c.path == saved.path)
            && d.activated()
    });
    let mut info = Vec::new();
    if let Some(d) = active {
        if let Some(ap) = nm.access_points.get(&d.active_ap) {
            info.push((
                tr("Signal Strength").to_string(),
                nm::signal_text(ap.strength).to_string(),
            ));
            info.push((
                tr("Security").to_string(),
                ap.security().label().to_string(),
            ));
            info.push((
                tr("Frequency").to_string(),
                if ap.frequency > 4000 {
                    "5 GHz"
                } else {
                    "2.4 GHz"
                }
                .to_string(),
            ));
        }
        if d.speed > 0 {
            info.push((
                tr("Link Speed").to_string(),
                trf!("{n} Mb/s", n = d.speed / 1000),
            ));
        }
        push_ip_info(&mut info, &nm, d);
    } else {
        info.push((
            tr("Security").to_string(),
            nm::saved_security(&saved).label().to_string(),
        ));
    }
    let share_group = widgets::group(tr("Share Network"));
    let (share_row, share_button) = widgets::action_row(
        tr("QR Code"),
        tr("Join from a phone by scanning the code"),
        tr("Show QR Code"),
        None,
    );
    share_group.add(&share_row);
    {
        let (weak, saved) = (Rc::downgrade(state), saved.clone());
        share_button.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                share(&state, &saved);
            }
        });
    }
    let weak = Rc::downgrade(state);
    netedit::open(
        &state.ctx,
        Editor {
            saved,
            info,
            extra: vec![share_group],
            reactivate: active.map(|d| d.path.clone()),
            rename: false,
            remove: helix_app_common::i18n::n_("Forget Network"),
        },
        move || {
            if let Some(state) = weak.upgrade() {
                refresh(&state);
            }
        },
    );
}

/// IP facts of an active device, for the details.
pub fn push_ip_info(info: &mut Vec<(String, String)>, nm: &Nm, d: &Device) {
    if let Some(ip) = nm.ip4.get(&d.ip4) {
        if !ip.addresses.is_empty() {
            info.push((tr("IPv4 Address").to_string(), ip.addresses.join(", ")));
        }
        if !ip.gateway.is_empty() {
            info.push((tr("Default Route").to_string(), ip.gateway.clone()));
        }
        if !ip.dns.is_empty() {
            info.push((tr("DNS").to_string(), ip.dns.join(", ")));
        }
    }
    if let Some(ip) = nm.ip6.get(&d.ip6) {
        if !ip.addresses.is_empty() {
            info.push((tr("IPv6 Address").to_string(), ip.addresses.join(", ")));
        }
        if !ip.dns.is_empty() {
            info.push((tr("IPv6 DNS").to_string(), ip.dns.join(", ")));
        }
    }
    if !d.mac().is_empty() {
        info.push((tr("Hardware Address").to_string(), d.mac().to_string()));
    }
}

/// A texture of a QR code for `text`, `scale` pixels per module.
fn qr_texture(text: &str, scale: usize) -> Option<gdk::Texture> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let quiet = 4;
    let side = (modules + 2 * quiet) * scale;
    let mut pixels = vec![255u8; side * side * 3];
    for (i, c) in colors.iter().enumerate() {
        if *c != qrcode::Color::Dark {
            continue;
        }
        let (mx, my) = (i % modules + quiet, i / modules + quiet);
        for y in my * scale..(my + 1) * scale {
            for x in mx * scale..(mx + 1) * scale {
                let at = (y * side + x) * 3;
                pixels[at..at + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }
    let bytes = glib::Bytes::from_owned(pixels);
    Some(
        gdk::MemoryTexture::new(
            side as i32,
            side as i32,
            gdk::MemoryFormat::R8g8b8,
            &bytes,
            side * 3,
        )
        .upcast(),
    )
}

/// Shows a saved network's QR code, with its name and password.
fn share(state: &Rc<State>, saved: &Saved) {
    let weak = Rc::downgrade(state);
    let saved = saved.clone();
    let s = saved.clone();
    spawn_worker(
        move || bus::system().and_then(|c| nm::wifi_password(&c, &s)),
        move |r| {
            let Some(state) = weak.upgrade() else { return };
            let password = match r {
                Ok(p) => p,
                Err(bus::Error::Denied) => return,
                Err(e) => {
                    state
                        .ctx
                        .toast(&trf!("Could not read the password: {e}", e = e));
                    return;
                }
            };
            let name = nm::ssid_text(&saved.ssid());
            let security = nm::saved_security(&saved);
            let page = widgets::page();
            let group = widgets::group("");
            if let Some(t) = qr_texture(&nm::qr_text(&name, security, &password), 6) {
                let picture = gtk::Picture::for_paintable(&t);
                picture.set_can_shrink(false);
                picture.set_halign(gtk::Align::Center);
                picture.set_margin_bottom(12);
                helix_app_common::set_accessible_label(
                    &picture,
                    &trf!("QR code for {name}", name = name),
                );
                group.add(&picture);
            }
            let rows = widgets::group("");
            rows.add(&InfoRow::new(tr("Network Name"), &name).row);
            if !password.is_empty() {
                rows.add(&InfoRow::new(tr("Password"), &password).row);
            }
            rows.add(&InfoRow::new(tr("Security"), security.label()).row);
            page.add(&group);
            page.add(&rows);
            let dialog = widgets::dialog(tr("Share Network"), &page);
            dialog.present(Some(&state.ctx.window));
        },
    );
}

fn saved_dialog(state: &Rc<State>) {
    let nm = state.nm.borrow().clone();
    let page = widgets::page();
    let group = widgets::group("");
    let saved = nm.saved_wifi();
    if saved.is_empty() {
        group.set_description(Some(tr("No networks are saved.")));
    }
    let dialog = widgets::dialog(tr("Saved Networks"), &page);
    dialog.set_content_height(520);
    for s in saved {
        let name = nm::ssid_text(&s.ssid());
        let row = widgets::subpage_row(&name, "", "network-wireless-symbolic");
        if nm.active_of(&s.path).is_some() {
            row.set_subtitle(tr("Connected"));
        }
        let (weak, path, d) = (Rc::downgrade(state), s.path.clone(), dialog.downgrade());
        row.connect_activated(move |_| {
            if let (Some(state), Some(d)) = (weak.upgrade(), d.upgrade()) {
                d.close();
                details(&state, &path);
            }
        });
        group.add(&row);
    }
    page.add(&group);
    dialog.present(Some(&state.ctx.window));
}

const HIDDEN_SECURITY: [Security; 3] = [Security::None, Security::Personal, Security::Wpa3];

fn hidden_dialog(state: &Rc<State>) {
    let Some(dev) = state.device() else { return };
    let page = widgets::page();
    let group = widgets::group("");
    let name = adw::EntryRow::builder()
        .use_markup(false)
        .title(tr("Network Name"))
        .build();
    let model = gtk::StringList::new(&[]);
    for s in HIDDEN_SECURITY {
        model.append(s.label());
    }
    let security = adw::ComboRow::builder()
        .use_markup(false)
        .title(tr("Security"))
        .model(&model)
        .selected(1)
        .build();
    let password = adw::PasswordEntryRow::builder()
        .title(tr("Password"))
        .build();
    group.add(&name);
    group.add(&security);
    group.add(&password);
    page.add(&group);
    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let connect = gtk::Button::builder()
        .label(tr("C_onnect"))
        .use_underline(true)
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    header.pack_end(&connect);
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(tr("Hidden Network"))
        .content_width(460)
        .child(&view)
        .build();
    let valid = {
        let (name, security, password) = (name.clone(), security.clone(), password.clone());
        move || {
            let s = HIDDEN_SECURITY[security.selected() as usize % HIDDEN_SECURITY.len()];
            !name.text().trim().is_empty()
                && (!s.needs_password() || nm::password_ok(s, &password.text()))
        }
    };
    let sync = {
        let (connect, password, security, valid) = (
            connect.clone(),
            password.clone(),
            security.clone(),
            valid.clone(),
        );
        move || {
            let s = HIDDEN_SECURITY[security.selected() as usize % HIDDEN_SECURITY.len()];
            password.set_visible(s.needs_password());
            connect.set_sensitive(valid());
        }
    };
    sync();
    {
        let sync = sync.clone();
        name.connect_changed(move |_| sync());
    }
    {
        let sync = sync.clone();
        password.connect_changed(move |_| sync());
    }
    security.connect_selected_notify(move |_| sync());
    let weak = Rc::downgrade(state);
    let d = dialog.downgrade();
    connect.connect_clicked(move |_| {
        let (Some(state), Some(d)) = (weak.upgrade(), d.upgrade()) else {
            return;
        };
        if !valid() {
            return;
        }
        let s = HIDDEN_SECURITY[security.selected() as usize % HIDDEN_SECURITY.len()];
        let n = name.text().trim().to_string();
        let settings = nm::wifi_settings(&n, n.as_bytes(), s, &password.text(), true);
        let (device, weak) = (dev.path.clone(), Rc::downgrade(&state));
        let failed = trf!("Could not connect to {name}", name = n);
        d.close();
        spawn_worker(
            move || bus::system().and_then(|c| nm::add_and_activate(&c, &settings, &device, "/")),
            move |r| report(&weak, r, &failed),
        );
    });
    dialog.present(Some(&state.ctx.window));
}

fn hotspot_dialog(state: &Rc<State>) {
    let Some(dev) = state.device() else { return };
    let nm = state.nm.borrow().clone();
    let existing = nm.hotspot().cloned();
    let page = widgets::page();
    let group = widgets::group("");
    let current = nm
        .device_connection(&dev)
        .filter(|c| !c.is_hotspot())
        .map(|c| c.id());
    group.set_description(Some(&match &current {
        Some(c) => trf!(
            "Other devices can use this computer's connection. Turning the hotspot on disconnects from {network}.",
            network = c
        ),
        None => tr("Other devices can use this computer's connection.").to_string(),
    }));
    let default_name = existing
        .as_ref()
        .map(|s| nm::ssid_text(&s.ssid()))
        .unwrap_or_else(|| glib::host_name().to_string());
    let name = adw::EntryRow::builder()
        .use_markup(false)
        .title(tr("Network Name"))
        .text(default_name)
        .build();
    let password = adw::PasswordEntryRow::builder()
        .title(tr("Password"))
        .text(nm::random_password())
        .build();
    group.add(&name);
    group.add(&password);
    page.add(&group);
    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let on = gtk::Button::builder()
        .label(tr("_Turn On"))
        .use_underline(true)
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&on);
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(tr("Wi-Fi Hotspot"))
        .content_width(460)
        .child(&view)
        .build();
    let sync = {
        let (on, name, password) = (on.clone(), name.clone(), password.clone());
        move || {
            on.set_sensitive(
                !name.text().trim().is_empty()
                    && nm::password_ok(Security::Personal, &password.text()),
            )
        }
    };
    {
        let sync = sync.clone();
        name.connect_changed(move |_| sync());
    }
    password.connect_changed(move |_| sync());
    let weak = Rc::downgrade(state);
    let d = dialog.downgrade();
    on.connect_clicked(move |_| {
        let (Some(state), Some(d)) = (weak.upgrade(), d.upgrade()) else {
            return;
        };
        let settings = nm::hotspot_settings(name.text().trim(), &password.text());
        let device = dev.path.clone();
        let existing = existing.as_ref().map(|s| s.path.clone());
        let weak = Rc::downgrade(&state);
        d.close();
        spawn_worker(
            move || {
                let c = bus::system()?;
                match existing {
                    Some(path) => {
                        nm::update(&c, &path, settings, &[])?;
                        nm::activate(&c, &path, &device, "/")
                    }
                    None => nm::add_and_activate(&c, &settings, &device, "/").map(drop),
                }
            },
            move |r| report(&weak, r, tr("Could not turn the hotspot on")),
        );
    });
    dialog.present(Some(&state.ctx.window));
}

fn set_airplane(state: &Rc<State>, on: bool) {
    let adapters: Vec<String> = state
        .bluetooth
        .borrow()
        .iter()
        .map(|(p, _)| p.clone())
        .collect();
    let weak = Rc::downgrade(state);
    spawn_worker(
        move || {
            let c = bus::system()?;
            nm::set_wireless(&c, !on)?;
            if on {
                // Mobile broadband may not be there.
                let _ = nm::set_wwan(&c, false);
            }
            for a in &adapters {
                let _ = bluez::set_powered(&c, a, !on);
            }
            Ok::<_, bus::Error>(())
        },
        move |r| report(&weak, r, tr("Could not change airplane mode")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airplane_mode() {
        let nm = Nm {
            wireless_enabled: false,
            ..Default::default()
        };
        assert!(airplane_on(&nm, &[]));
        assert!(airplane_on(&nm, &[("/a".into(), false)]));
        assert!(!airplane_on(&nm, &[("/a".into(), true)]));
        let wifi = Nm {
            wireless_enabled: true,
            ..Default::default()
        };
        assert!(!airplane_on(&wifi, &[]));
    }
}
