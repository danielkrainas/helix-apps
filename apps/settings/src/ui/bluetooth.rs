//! Bluetooth panel: BlueZ's adapter and devices.

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use super::widgets::{self, InfoRow};
use super::{follow, spawn_worker, Ctx, Panel};
use crate::bluez::{self, Device};
use crate::bus;
use crate::rfkill;
use helix_app_common::i18n::tr;
use helix_app_common::trf;

pub struct BluetoothPanel;

/// What a reading found.
struct Reading {
    bluez: bus::Result<bluez::State>,
    /// Soft and hard blocks of the Bluetooth radios.
    blocked: (bool, bool),
}

fn read() -> Reading {
    let switches = rfkill::read(std::path::Path::new(rfkill::SYSFS));
    Reading {
        bluez: bus::system().and_then(|c| bluez::read(&c)),
        blocked: rfkill::blocked(&switches, "bluetooth"),
    }
}

/// The open details dialog of one device.
struct Details {
    dialog: adw::Dialog,
    path: String,
    connection: adw::SwitchRow,
    kind: InfoRow,
    battery: InfoRow,
}

struct State {
    ctx: Rc<Ctx>,
    stack: gtk::Stack,
    power: adw::SwitchRow,
    name: adw::EntryRow,
    paired: adw::PreferencesGroup,
    nearby: adw::PreferencesGroup,
    files: adw::PreferencesGroup,
    paired_rows: RefCell<Vec<gtk::Widget>>,
    nearby_rows: RefCell<Vec<gtk::Widget>>,
    /// The devices as the rows show them.
    shown: RefCell<Vec<Device>>,
    current: RefCell<bluez::State>,
    soft_blocked: Cell<bool>,
    /// Programmatic changes don't write back.
    quiet: Cell<bool>,
    /// The page is on screen: the adapter is visible and searching.
    open: Cell<bool>,
    made_visible: Cell<bool>,
    searching: Cell<bool>,
    /// Devices being paired.
    pairing: RefCell<HashSet<String>>,
    details: RefCell<Option<Details>>,
}

impl Panel for BluetoothPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let stack = gtk::Stack::new();
        stack.add_named(&widgets::loading(tr("Loading Bluetooth…")), Some("loading"));
        stack.add_named(
            &widgets::status(
                "bluetooth-disabled-symbolic",
                tr("Bluetooth Is Not Available"),
                tr("The Bluetooth service is not running."),
            ),
            Some("missing"),
        );
        stack.add_named(
            &widgets::status(
                "bluetooth-disabled-symbolic",
                tr("No Bluetooth Adapter Found"),
                tr("Plug in a Bluetooth adapter to use Bluetooth."),
            ),
            Some("no-adapter"),
        );
        stack.add_named(
            &widgets::status(
                "bluetooth-disabled-symbolic",
                tr("Bluetooth Is Turned Off by a Hardware Switch"),
                tr("Turn it on with the switch or key on the computer."),
            ),
            Some("hard-blocked"),
        );

        let page = widgets::page();
        let top = widgets::group("");
        let power = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Bluetooth"))
            .build();
        top.add(&power);
        let name = adw::EntryRow::builder()
            .use_markup(false)
            .title(tr("Visible Name"))
            .show_apply_button(true)
            .build();
        top.add(&name);
        page.add(&top);
        let paired = widgets::group(tr("Devices"));
        page.add(&paired);
        let nearby = widgets::group(tr("Nearby Devices"));
        let spinner = gtk::Spinner::builder().spinning(true).build();
        helix_app_common::set_accessible_label(&spinner, tr("Searching for Devices"));
        nearby.set_header_suffix(Some(&spinner));
        page.add(&nearby);

        let files = widgets::group(tr("Received Files"));
        let downloads = glib::user_special_dir(glib::UserDirectory::Downloads)
            .unwrap_or_else(|| glib::home_dir().join("Downloads"));
        let (row, open) = widgets::action_row(
            tr("Received Files Folder"),
            &trf!(
                "Files sent to this computer are saved in {folder}",
                folder = downloads.display()
            ),
            tr("Open"),
            None,
        );
        {
            let ctx = Rc::downgrade(ctx);
            open.connect_clicked(move |_| {
                let Some(ctx) = ctx.upgrade() else { return };
                let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&downloads)));
                let c = Rc::downgrade(&ctx);
                launcher.launch(Some(&ctx.window), gio::Cancellable::NONE, move |r| {
                    if r.is_err() {
                        if let Some(ctx) = c.upgrade() {
                            ctx.toast(tr("Could not open the folder"));
                        }
                    }
                });
            });
        }
        files.add(&row);
        page.add(&files);
        stack.add_named(&page, Some("list"));
        stack.set_visible_child_name("loading");

        let state = Rc::new(State {
            ctx: ctx.clone(),
            stack: stack.clone(),
            power: power.clone(),
            name: name.clone(),
            paired,
            nearby,
            files,
            paired_rows: RefCell::default(),
            nearby_rows: RefCell::default(),
            shown: RefCell::default(),
            current: RefCell::default(),
            soft_blocked: Cell::new(false),
            quiet: Cell::new(false),
            open: Cell::new(false),
            made_visible: Cell::new(false),
            searching: Cell::new(false),
            pairing: RefCell::default(),
            details: RefCell::default(),
        });

        let weak = Rc::downgrade(&state);
        power.connect_active_notify(move |sw| {
            let Some(state) = weak.upgrade() else { return };
            if !state.quiet.get() {
                turn(&state, sw.is_active());
            }
        });
        let weak = Rc::downgrade(&state);
        name.connect_apply(move |e| {
            let Some(state) = weak.upgrade() else { return };
            let Some(adapter) = state.current.borrow().adapter().map(|a| a.path.clone()) else {
                return;
            };
            let text = e.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            let weak = Rc::downgrade(&state);
            spawn_worker(
                move || bus::system().and_then(|c| bluez::set_name(&c, &adapter, &text)),
                move |r| {
                    done(&weak, r, |e| {
                        trf!("Could not rename the adapter: {e}", e = e)
                    })
                },
            );
        });

        // Visible and searching while the page is on screen.
        let weak = Rc::downgrade(&state);
        stack.connect_map(move |_| {
            if let Some(state) = weak.upgrade() {
                state.open.set(true);
                be_visible(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        stack.connect_unmap(move |_| {
            if let Some(state) = weak.upgrade() {
                state.open.set(false);
                be_visible(&state);
            }
        });

        refresh(&state);
        follow(
            &state,
            Duration::from_millis(250),
            |tell| {
                if let Ok(conn) = bus::system() {
                    bluez::watch(&conn, tell);
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

/// A call's error as a toast (made by `failed`), then a fresh reading.
fn done(weak: &std::rc::Weak<State>, r: bus::Result<()>, failed: impl Fn(&str) -> String) {
    let Some(state) = weak.upgrade() else { return };
    if let Err(e) = r {
        if e != bus::Error::Denied {
            state.ctx.toast(&failed(&e.to_string()));
        }
    }
    refresh(&state);
}

fn show(state: &Rc<State>, r: Reading) {
    let (soft, hard) = r.blocked;
    state.soft_blocked.set(soft);
    let s = match r.bluez {
        Ok(s) => s,
        Err(e) => {
            if e != bus::Error::Missing {
                eprintln!("helix-settings-app: bluez: {e}");
            }
            state.stack.set_visible_child_name("missing");
            close_details(state);
            return;
        }
    };
    let Some(adapter) = s.adapter().cloned() else {
        state
            .stack
            .set_visible_child_name(if hard { "hard-blocked" } else { "no-adapter" });
        close_details(state);
        *state.current.borrow_mut() = s;
        return;
    };
    if hard && !adapter.powered {
        state.stack.set_visible_child_name("hard-blocked");
        close_details(state);
        *state.current.borrow_mut() = s;
        return;
    }
    state.stack.set_visible_child_name("list");
    state.quiet.set(true);
    state.power.set_active(adapter.powered);
    state.quiet.set(false);
    state.power.set_subtitle(&if !adapter.powered {
        tr("Turn on to connect devices and receive files").to_string()
    } else if adapter.discoverable {
        trf!(
            "Visible to nearby devices as “{name}” while this page is open",
            name = adapter.name
        )
    } else {
        String::new()
    });
    if !state.name.has_focus() && state.name.text() != adapter.name {
        state.name.set_text(&adapter.name);
    }
    state.name.set_sensitive(adapter.powered);
    for g in [&state.paired, &state.nearby, &state.files] {
        g.set_visible(adapter.powered);
    }
    if let Some(spinner) = state.nearby.header_suffix() {
        spinner.set_visible(adapter.discovering);
    }

    let devices: Vec<Device> = s.devices().cloned().collect();
    if *state.shown.borrow() != devices {
        rebuild(state, &devices);
        *state.shown.borrow_mut() = devices;
    }
    *state.current.borrow_mut() = s;
    update_details(state);
    be_visible(state);
}

fn device_row(state: &Rc<State>, d: &Device) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(d.title())
        .activatable(true)
        .build();
    row.add_prefix(
        &gtk::Image::builder()
            .icon_name(bluez::icon_name(&d.icon))
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build(),
    );
    let path = d.path.clone();
    let weak = Rc::downgrade(state);
    if d.paired {
        row.set_subtitle(&d.status());
        row.add_suffix(
            &gtk::Image::builder()
                .icon_name("go-next-symbolic")
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build(),
        );
        row.connect_activated(move |_| {
            if let Some(state) = weak.upgrade() {
                open_details(&state, &path);
            }
        });
    } else if state.pairing.borrow().contains(&d.path) {
        row.set_subtitle(tr("Pairing…"));
        let spinner = gtk::Spinner::builder().spinning(true).build();
        helix_app_common::set_accessible_label(&spinner, tr("Pairing"));
        row.add_suffix(&spinner);
        row.set_activatable(false);
    } else {
        row.set_subtitle(tr("Select to pair"));
        let name = d.title().to_string();
        row.connect_activated(move |_| {
            if let Some(state) = weak.upgrade() {
                pair(&state, &path, &name);
            }
        });
    }
    row
}

fn rebuild(state: &Rc<State>, devices: &[Device]) {
    let paired: Vec<gtk::Widget> = devices
        .iter()
        .filter(|d| d.paired)
        .map(|d| device_row(state, d).upcast())
        .collect();
    // Nameless nearby devices are left out: nobody can tell them apart.
    let nearby: Vec<gtk::Widget> = devices
        .iter()
        .filter(|d| !d.paired && !d.name.is_empty())
        .map(|d| device_row(state, d).upcast())
        .collect();
    state.paired.set_description(if paired.is_empty() {
        Some(tr("No devices are paired yet."))
    } else {
        None
    });
    state.nearby.set_description(if nearby.is_empty() {
        Some(tr("Searching for devices…"))
    } else {
        Some(tr("Make sure the device is in pairing mode."))
    });
    widgets::replace_rows(&state.paired, &state.paired_rows, paired);
    widgets::replace_rows(&state.nearby, &state.nearby_rows, nearby);
}

/// Makes the adapter visible and searching while the page is open, and
/// undoes it when the page goes away.
fn be_visible(state: &Rc<State>) {
    let current = state.current.borrow();
    let Some(adapter) = current.adapter() else {
        return;
    };
    let path = adapter.path.clone();
    let want = state.open.get() && adapter.powered;
    let make_visible = want && !adapter.discoverable && !state.made_visible.get();
    let hide = !want && state.made_visible.get();
    let search = want && !state.searching.get();
    let stop = !want && state.searching.get();
    if make_visible {
        state.made_visible.set(true);
    }
    if hide {
        state.made_visible.set(false);
    }
    if search {
        state.searching.set(true);
    }
    if stop {
        state.searching.set(false);
    }
    if !(make_visible || hide || search || stop) {
        return;
    }
    spawn_worker(
        move || {
            let Ok(c) = bus::system() else { return };
            if make_visible || hide {
                let _ = bluez::set_discoverable(&c, &path, make_visible);
            }
            if search || stop {
                let _ = bluez::discover(&c, &path, search);
            }
        },
        |_| {},
    );
}

fn turn(state: &Rc<State>, on: bool) {
    let Some(adapter) = state.current.borrow().adapter().map(|a| a.path.clone()) else {
        return;
    };
    let unblock = on && state.soft_blocked.get();
    let weak = Rc::downgrade(state);
    spawn_worker(
        move || {
            if unblock {
                // Lift the software block first (the user turned it on).
                rfkill::unblock(std::path::Path::new(rfkill::DEVICE), rfkill::TYPE_BLUETOOTH)
                    .map_err(|e| bus::Error::Other(e.to_string()))?;
                std::thread::sleep(Duration::from_millis(300));
            }
            bus::system().and_then(|c| bluez::set_powered(&c, &adapter, on))
        },
        move |r| {
            done(&weak, r, |e| {
                if on {
                    trf!("Could not turn Bluetooth on: {e}", e = e)
                } else {
                    trf!("Could not turn Bluetooth off: {e}", e = e)
                }
            })
        },
    );
}

fn pair(state: &Rc<State>, path: &str, name: &str) {
    state.pairing.borrow_mut().insert(path.to_string());
    // Show "Pairing…".
    let devices = state.shown.borrow().clone();
    rebuild(state, &devices);
    let weak = Rc::downgrade(state);
    let (path, name) = (path.to_string(), name.to_string());
    let p = path.clone();
    spawn_worker(
        move || bus::system().and_then(|c| bluez::pair(&c, &p)),
        move |r| {
            let Some(state) = weak.upgrade() else { return };
            state.pairing.borrow_mut().remove(&path);
            match r {
                Ok(()) => helix_app_common::announce(
                    &state.paired,
                    &trf!("Paired with {name}", name = name),
                    helix_app_common::Urgency::Polite,
                ),
                Err(bus::Error::Denied) => state
                    .ctx
                    .toast(&trf!("Pairing with {name} was cancelled", name = name)),
                Err(e) => {
                    state
                        .ctx
                        .toast(&trf!("Could not pair with {name}: {e}", name = name, e = e))
                }
            }
            let devices = state.shown.borrow().clone();
            rebuild(&state, &devices);
            refresh(&state);
        },
    );
}

fn device(state: &State, path: &str) -> Option<Device> {
    state
        .current
        .borrow()
        .devices
        .iter()
        .find(|d| d.path == path)
        .cloned()
}

fn open_details(state: &Rc<State>, path: &str) {
    let Some(d) = device(state, path) else { return };
    let page = widgets::page();
    let group = widgets::group("");
    let connection = adw::SwitchRow::builder()
        .use_markup(false)
        .title(tr("Connection"))
        .build();
    group.add(&connection);
    let kind = InfoRow::new(tr("Type"), "");
    group.add(&kind.row);
    let address = InfoRow::new(tr("Address"), &d.address);
    group.add(&address.row);
    let battery = InfoRow::new(tr("Battery"), "");
    group.add(&battery.row);
    page.add(&group);

    let actions = widgets::group("");
    let (send_row, send) = widgets::action_row(
        tr("Send Files"),
        tr("Choose files to send to this device"),
        tr("Send Files…"),
        None,
    );
    actions.add(&send_row);
    let (forget_row, forget) = widgets::action_row(
        tr("Forget Device"),
        tr("It will have to be paired again to be used"),
        tr("Forget…"),
        Some("destructive-action"),
    );
    actions.add(&forget_row);
    page.add(&actions);

    let dialog = widgets::dialog(d.title(), &page);
    dialog.set_content_height(480);

    let weak = Rc::downgrade(state);
    let p = d.path.clone();
    connection.connect_active_notify(move |sw| {
        let Some(state) = weak.upgrade() else { return };
        if state.quiet.get() {
            return;
        }
        let on = sw.is_active();
        let path = p.clone();
        sw.set_sensitive(false);
        let weak = Rc::downgrade(&state);
        spawn_worker(
            move || bus::system().and_then(|c| bluez::connect(&c, &path, on)),
            move |r| {
                if let Some(state) = weak.upgrade() {
                    if let Some(d) = state.details.borrow().as_ref() {
                        d.connection.set_sensitive(true);
                    }
                }
                done(&weak, r, |e| {
                    if on {
                        trf!("Could not connect: {e}", e = e)
                    } else {
                        trf!("Could not disconnect: {e}", e = e)
                    }
                })
            },
        );
    });
    let weak = Rc::downgrade(state);
    let address_text = d.address.clone();
    send.connect_clicked(move |_| {
        let address = address_text.clone();
        let weak = weak.clone();
        spawn_worker(
            move || bus::session().and_then(|c| bluez::send_files(&c, &address)),
            move |r| {
                let Some(state) = weak.upgrade() else { return };
                match r {
                    Ok(()) => {}
                    Err(bus::Error::Missing) => {
                        state.ctx.toast(tr("Sending files needs the Helix shell"))
                    }
                    Err(e) => state.ctx.toast(&trf!("Could not send files: {e}", e = e)),
                }
            },
        );
    });
    let weak = Rc::downgrade(state);
    let (p, title) = (d.path.clone(), d.title().to_string());
    forget.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            confirm_forget(&state, &p, &title);
        }
    });

    let weak = Rc::downgrade(state);
    dialog.connect_closed(move |_| {
        if let Some(state) = weak.upgrade() {
            state.details.borrow_mut().take();
        }
    });
    *state.details.borrow_mut() = Some(Details {
        dialog: dialog.clone(),
        path: d.path.clone(),
        connection,
        kind,
        battery,
    });
    update_details(state);
    dialog.present(Some(&state.ctx.window));
}

fn update_details(state: &Rc<State>) {
    let details = state.details.borrow();
    let Some(det) = details.as_ref() else { return };
    let Some(d) = device(state, &det.path).filter(|d| d.paired) else {
        let dialog = det.dialog.clone();
        drop(details);
        dialog.close();
        return;
    };
    det.dialog.set_title(d.title());
    state.quiet.set(true);
    det.connection.set_active(d.connected);
    state.quiet.set(false);
    det.connection.set_subtitle(&d.status());
    det.kind.set(bluez::kind_label(&d.icon));
    det.battery.row.set_visible(d.battery.is_some());
    if let Some(b) = d.battery {
        det.battery.set(&format!("{b} %"));
    }
}

fn close_details(state: &Rc<State>) {
    let dialog = state.details.borrow().as_ref().map(|d| d.dialog.clone());
    if let Some(d) = dialog {
        d.close();
    }
}

fn confirm_forget(state: &Rc<State>, path: &str, name: &str) {
    let alert = adw::AlertDialog::new(
        Some(&trf!("Forget {name}?", name = name)),
        Some(tr("It will have to be paired again before it can be used.")),
    );
    alert.add_response("cancel", tr("_Cancel"));
    alert.add_response("forget", tr("_Forget"));
    alert.set_response_appearance("forget", adw::ResponseAppearance::Destructive);
    alert.set_default_response(Some("cancel"));
    alert.set_close_response("cancel");
    let weak = Rc::downgrade(state);
    let path = path.to_string();
    alert.connect_response(None, move |_, response| {
        if response != "forget" {
            return;
        }
        let Some(state) = weak.upgrade() else { return };
        let Some(adapter) = state.current.borrow().adapter().map(|a| a.path.clone()) else {
            return;
        };
        close_details(&state);
        let path = path.clone();
        let weak = Rc::downgrade(&state);
        spawn_worker(
            move || bus::system().and_then(|c| bluez::forget(&c, &adapter, &path)),
            move |r| {
                done(&weak, r, |e| {
                    trf!("Could not forget the device: {e}", e = e)
                })
            },
        );
    });
    let parent = state
        .details
        .borrow()
        .as_ref()
        .map(|d| d.dialog.clone().upcast::<gtk::Widget>())
        .unwrap_or_else(|| state.ctx.window.clone().upcast());
    alert.present(Some(&parent));
}
