//! Displays panel: monitors through `org.gnome.Mutter.DisplayConfig`
//! and Night Light (`org.gnome.settings-daemon.plugins.color`).

use adw::prelude::*;
use gtk::{gdk, glib};
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use super::widgets::{self, switch, watch, Num};
use super::{follow, spawn_worker, Ctx, Panel};
use crate::bus;
use crate::displays::{self, Arrangement, Config};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;

pub struct DisplaysPanel;

const COLOR: &str = "org.gnome.settings-daemon.plugins.color";

const ARRANGEMENTS: [(Arrangement, &str); 3] = [
    (Arrangement::Join, n_("Join Displays")),
    (Arrangement::Mirror, n_("Mirror")),
    (Arrangement::Single, n_("Single Display")),
];

/// Orientations offered: (transform, label).
const ORIENTATIONS: [(u32, &str); 4] = [
    (0, n_("Landscape")),
    (1, n_("Portrait Right")),
    (3, n_("Portrait Left")),
    (2, n_("Landscape (Flipped)")),
];

struct State {
    ctx: Rc<Ctx>,
    stack: gtk::Stack,
    arrangement_group: adw::PreferencesGroup,
    mode: adw::ComboRow,
    single: adw::ComboRow,
    area: gtk::DrawingArea,
    primary: adw::ComboRow,
    monitor_group: adw::PreferencesGroup,
    which: adw::ComboRow,
    orientation: adw::ComboRow,
    resolution: adw::ComboRow,
    rate: adw::ComboRow,
    scale: adw::ComboRow,
    underscan: adw::SwitchRow,
    apply_bar: gtk::Revealer,
    /// As the service reported it.
    current: RefCell<Config>,
    /// As the user is changing it.
    pending: RefCell<Config>,
    /// The logical monitor being set up (and highlighted).
    selected: Cell<usize>,
    /// Connectors shown in the "which" and "single" rows.
    connectors: RefCell<Vec<String>>,
    /// Choices behind the resolution, rate and scale rows.
    resolutions: RefCell<Vec<(i32, i32)>>,
    rates: RefCell<Vec<String>>,
    scales: RefCell<Vec<f64>>,
    quiet: Cell<bool>,
    /// Dragging: the logical monitor, its start and the pixels per unit.
    drag: Cell<Option<(usize, i32, i32, f64)>>,
}

fn combo(title: &str) -> adw::ComboRow {
    adw::ComboRow::builder()
        .use_markup(false)
        .title(title)
        .build()
}

fn strings(items: impl IntoIterator<Item = String>) -> gtk::StringList {
    let l = gtk::StringList::new(&[]);
    for i in items {
        l.append(&i);
    }
    l
}

impl Panel for DisplaysPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let stack = gtk::Stack::new();
        stack.add_named(&widgets::loading(tr("Loading Displays…")), Some("loading"));
        let page = widgets::page();

        let arrangement_group = widgets::group(tr("Arrangement"));
        let mode = combo(tr("Display Mode"));
        mode.set_model(Some(&strings(
            ARRANGEMENTS.iter().map(|a| tr(a.1).to_string()),
        )));
        arrangement_group.add(&mode);
        let single = combo(tr("Display"));
        helix_app_common::set_accessible_label(&single, tr("Display to Use"));
        arrangement_group.add(&single);
        let area = gtk::DrawingArea::builder()
            .content_height(200)
            .hexpand(true)
            .focusable(true)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        helix_app_common::set_accessible_label(&area, tr("Display Arrangement"));
        area.set_tooltip_text(Some(tr(
            "Drag displays to match how they stand on your desk; with the keyboard, move the selected display with the arrow keys",
        )));
        let frame = gtk::Frame::builder()
            .child(&area)
            .css_classes(["view"])
            .build();
        arrangement_group.add(&frame);
        let primary = combo(tr("Primary Display"));
        primary.set_subtitle(tr("Holds the top bar and new windows"));
        arrangement_group.add(&primary);
        page.add(&arrangement_group);

        let monitor_group = widgets::group(tr("Display"));
        let which = combo(tr("Display"));
        helix_app_common::set_accessible_label(&which, tr("Display to Set Up"));
        monitor_group.add(&which);
        let orientation = combo(tr("Orientation"));
        orientation.set_model(Some(&strings(
            ORIENTATIONS.iter().map(|o| tr(o.1).to_string()),
        )));
        monitor_group.add(&orientation);
        let resolution = combo(tr("Resolution"));
        monitor_group.add(&resolution);
        let rate = combo(tr("Refresh Rate"));
        monitor_group.add(&rate);
        let scale = combo(tr("Scale"));
        scale.set_subtitle(tr("Makes everything larger or smaller"));
        monitor_group.add(&scale);
        let underscan = adw::SwitchRow::builder()
            .use_markup(false)
            .title(tr("Adjust for TV"))
            .subtitle(tr("Shrinks the picture for TVs that cut off its edges"))
            .build();
        monitor_group.add(&underscan);
        page.add(&monitor_group);

        page.add(&night_light_group(&ctx.prefs(COLOR)));

        // "Apply" / "Reset" under the page while something changed.
        let bar = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .halign(gtk::Align::Center)
            .build();
        let label = gtk::Label::new(Some(tr("The display settings changed.")));
        let reset = gtk::Button::with_mnemonic(tr("_Reset"));
        let apply = gtk::Button::builder()
            .label(tr("_Apply"))
            .use_underline(true)
            .css_classes(["suggested-action"])
            .build();
        bar.append(&label);
        bar.append(&reset);
        bar.append(&apply);
        let apply_bar = gtk::Revealer::builder()
            .child(&bar)
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.set_vexpand(true);
        content.append(&page);
        content.append(&apply_bar);
        stack.add_named(&content, Some("page"));
        stack.add_named(
            &widgets::status(
                "video-display-symbolic",
                tr("Displays Are Not Available"),
                tr("The display service is not running."),
            ),
            Some("missing"),
        );
        stack.set_visible_child_name("loading");

        let state = Rc::new(State {
            ctx: ctx.clone(),
            stack: stack.clone(),
            arrangement_group,
            mode,
            single,
            area,
            primary,
            monitor_group,
            which,
            orientation,
            resolution,
            rate,
            scale,
            underscan,
            apply_bar,
            current: RefCell::default(),
            pending: RefCell::default(),
            selected: Cell::new(0),
            connectors: RefCell::default(),
            resolutions: RefCell::default(),
            rates: RefCell::default(),
            scales: RefCell::default(),
            quiet: Cell::new(false),
            drag: Cell::new(None),
        });
        connect(&state);
        let weak = Rc::downgrade(&state);
        apply.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                apply_pending(&state);
            }
        });
        let weak = Rc::downgrade(&state);
        reset.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                let c = state.current.borrow().clone();
                *state.pending.borrow_mut() = c;
                sync(&state);
            }
        });
        refresh(&state);
        follow(
            &state,
            Duration::from_millis(200),
            |tell| {
                if let Ok(conn) = bus::session() {
                    displays::watch(&conn, tell);
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
    spawn_worker(
        || {
            let c = bus::session()?;
            Ok::<_, bus::Error>((displays::read(&c)?, displays::allowed(&c)))
        },
        move |r| {
            let Some(state) = weak.upgrade() else { return };
            match r {
                Ok((c, allowed)) => {
                    state.monitor_group.set_sensitive(allowed);
                    state.arrangement_group.set_sensitive(allowed);
                    state.monitor_group.set_description(if allowed {
                        None
                    } else {
                        Some(tr("Changing the displays is not allowed in this session."))
                    });
                    state.stack.set_visible_child_name("page");
                    // A change from elsewhere replaces what was being edited.
                    *state.pending.borrow_mut() = c.clone();
                    *state.current.borrow_mut() = c;
                    sync(&state);
                }
                Err(e) => {
                    if e != bus::Error::Missing {
                        eprintln!("helix-settings-app: DisplayConfig: {e}");
                    }
                    state.stack.set_visible_child_name("missing");
                }
            }
        },
    );
}

fn apply_pending(state: &Rc<State>) {
    let config = state.pending.borrow().clone();
    let weak = Rc::downgrade(state);
    spawn_worker(
        move || {
            let c = bus::session()?;
            // Checked first, so nothing changes when it would not work.
            displays::apply(&c, &config, displays::VERIFY)?;
            displays::apply(&c, &config, displays::PERSISTENT)
        },
        move |r| {
            let Some(state) = weak.upgrade() else { return };
            if let Err(e) = r {
                if e != bus::Error::Denied {
                    state
                        .ctx
                        .toast(&trf!("Could not change the displays: {e}", e = e));
                }
            }
            refresh(&state);
        },
    );
}

/// The connector of the first monitor of logical monitor `i`.
fn connector(c: &Config, i: usize) -> Option<String> {
    c.logical
        .get(i)?
        .outputs
        .first()
        .map(|o| o.connector.clone())
}

fn monitor_label(c: &Config, connector: &str) -> String {
    let index = c
        .monitors
        .iter()
        .position(|m| m.connector == connector)
        .unwrap_or(0)
        + 1;
    let name = c
        .monitor(connector)
        .map(|m| m.name.clone())
        .unwrap_or_default();
    format!("{index}. {name}")
}

/// Shows the pending configuration.
fn sync(state: &Rc<State>) {
    state.quiet.set(true);
    let c = state.pending.borrow().clone();
    let several = c.monitors.len() > 1;
    let arrangement = c.arrangement();
    state.arrangement_group.set_visible(several);
    if let Some(i) = ARRANGEMENTS.iter().position(|a| a.0 == arrangement) {
        state.mode.set_selected(i as u32);
    }
    let all: Vec<String> = c.monitors.iter().map(|m| m.connector.clone()).collect();
    let labels: Vec<String> = all.iter().map(|m| monitor_label(&c, m)).collect();
    if *state.connectors.borrow() != all {
        state.single.set_model(Some(&strings(labels.clone())));
        *state.connectors.borrow_mut() = all.clone();
    }
    state.single.set_visible(arrangement == Arrangement::Single);
    if let Some(on) = connector(&c, 0) {
        if let Some(i) = all.iter().position(|m| *m == on) {
            state.single.set_selected(i as u32);
        }
    }
    let joined = arrangement == Arrangement::Join && c.logical.len() > 1;
    state
        .area
        .parent()
        .unwrap_or(state.area.clone().upcast())
        .set_visible(joined);
    state.primary.set_visible(joined);
    let logical_labels: Vec<String> = (0..c.logical.len())
        .filter_map(|i| connector(&c, i))
        .map(|m| monitor_label(&c, &m))
        .collect();
    state
        .primary
        .set_model(Some(&strings(logical_labels.clone())));
    if let Some(i) = c.logical.iter().position(|l| l.primary) {
        state.primary.set_selected(i as u32);
    }

    let sel = state.selected.get().min(c.logical.len().saturating_sub(1));
    state.selected.set(sel);
    state.which.set_model(Some(&strings(logical_labels)));
    state.which.set_selected(sel as u32);
    state.which.set_visible(c.logical.len() > 1);
    state.monitor_group.set_title(&if several
        && c.logical.len() == 1
        && arrangement == Arrangement::Mirror
    {
        tr("Mirrored Displays").to_string()
    } else if several && c.logical.len() == 1 {
        connector(&c, 0)
            .map(|m| monitor_label(&c, &m))
            .unwrap_or_default()
    } else if several {
        tr("Display").to_string()
    } else {
        connector(&c, 0)
            .and_then(|m| c.monitor(&m).map(|m| m.name.clone()))
            .unwrap_or_default()
    });

    if let (Some(l), Some(conn)) = (c.logical.get(sel), connector(&c, sel)) {
        let m = c.monitor(&conn).cloned();
        let out = l.outputs.first().cloned();
        if let (Some(m), Some(out)) = (m, out) {
            let mode = m.mode(&out.mode).cloned();
            if let Some(i) = ORIENTATIONS.iter().position(|o| o.0 == l.transform) {
                state.orientation.set_selected(i as u32);
            }
            // Mirrored monitors offer only the resolutions they share.
            let resolutions: Vec<(i32, i32)> = m
                .resolutions()
                .into_iter()
                .filter(|r| {
                    l.outputs.iter().all(|o| {
                        c.monitor(&o.connector)
                            .is_some_and(|om| om.resolutions().contains(r))
                    })
                })
                .collect();
            state.resolution.set_model(Some(&strings(
                resolutions
                    .iter()
                    .map(|(w, h)| displays::resolution_label(*w, *h)),
            )));
            if let Some(md) = &mode {
                if let Some(i) = resolutions.iter().position(|r| *r == (md.width, md.height)) {
                    state.resolution.set_selected(i as u32);
                }
                let rates = m.rates(md.width, md.height);
                state.rate.set_model(Some(&strings(
                    rates.iter().map(|r| displays::rate_label(r)),
                )));
                if let Some(i) = rates.iter().position(|r| r.id == md.id) {
                    state.rate.set_selected(i as u32);
                }
                state.rate.set_sensitive(rates.len() > 1);
                *state.rates.borrow_mut() = rates.iter().map(|r| r.id.clone()).collect();
            }
            *state.resolutions.borrow_mut() = resolutions;
            let scales = c.scales(sel);
            state.scale.set_model(Some(&strings(
                scales.iter().map(|s| displays::scale_label(*s)),
            )));
            if let Some(i) = scales.iter().position(|s| (s - l.scale).abs() < 1e-6) {
                state.scale.set_selected(i as u32);
            }
            state.scale.set_sensitive(scales.len() > 1);
            *state.scales.borrow_mut() = scales;
            state.underscan.set_visible(m.underscan.is_some());
            state.underscan.set_active(out.underscan);
        }
    }
    let changed = c.logical != state.current.borrow().logical;
    state.apply_bar.set_reveal_child(changed);
    state.area.queue_draw();
    state.quiet.set(false);
}

/// Changes the pending configuration with `f`, then shows it.
fn edit(state: &Rc<State>, f: impl FnOnce(&mut Config, usize)) {
    if state.quiet.get() {
        return;
    }
    let sel = state.selected.get();
    f(&mut state.pending.borrow_mut(), sel);
    sync(state);
}

fn connect(state: &Rc<State>) {
    let weak = Rc::downgrade(state);
    state.mode.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let Some(&(a, _)) = ARRANGEMENTS.get(r.selected() as usize) else {
            return;
        };
        let mut failed = false;
        edit(&state, |c, _| match a {
            Arrangement::Join => c.join(),
            Arrangement::Mirror => failed = !c.mirror(),
            Arrangement::Single => {
                let first = c
                    .monitors
                    .iter()
                    .find(|m| m.builtin)
                    .or(c.monitors.first())
                    .map(|m| m.connector.clone());
                if let Some(m) = first {
                    c.single(&m);
                }
            }
        });
        if failed {
            state
                .ctx
                .toast(tr("These displays have no resolution in common to mirror"));
            sync(&state);
        }
    });
    let weak = Rc::downgrade(state);
    state.single.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let m = state
            .connectors
            .borrow()
            .get(r.selected() as usize)
            .cloned();
        if let Some(m) = m {
            edit(&state, |c, _| c.single(&m));
        }
    });
    let weak = Rc::downgrade(state);
    state.primary.connect_selected_notify(move |r| {
        if let Some(state) = weak.upgrade() {
            edit(&state, |c, _| c.set_primary(r.selected() as usize));
        }
    });
    let weak = Rc::downgrade(state);
    state.which.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        if !state.quiet.get() {
            state.selected.set(r.selected() as usize);
            sync(&state);
        }
    });
    let weak = Rc::downgrade(state);
    state.orientation.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        if let Some(&(t, _)) = ORIENTATIONS.get(r.selected() as usize) {
            edit(&state, |c, i| c.set_transform(i, t));
        }
    });
    let weak = Rc::downgrade(state);
    state.resolution.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let Some((w, h)) = state
            .resolutions
            .borrow()
            .get(r.selected() as usize)
            .copied()
        else {
            return;
        };
        edit(&state, |c, i| {
            let Some(conn) = connector(c, i) else { return };
            let best = c
                .monitor(&conn)
                .and_then(|m| m.rates(w, h).first().map(|m| m.id.clone()));
            if let Some(id) = best {
                c.set_mode(i, &conn, &id);
            }
        });
    });
    let weak = Rc::downgrade(state);
    state.rate.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let Some(id) = state.rates.borrow().get(r.selected() as usize).cloned() else {
            return;
        };
        edit(&state, |c, i| {
            if let Some(conn) = connector(c, i) {
                c.set_mode(i, &conn, &id);
            }
        });
    });
    let weak = Rc::downgrade(state);
    state.scale.connect_selected_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let Some(s) = state.scales.borrow().get(r.selected() as usize).copied() else {
            return;
        };
        edit(&state, |c, i| c.set_scale(i, s));
    });
    let weak = Rc::downgrade(state);
    state.underscan.connect_active_notify(move |r| {
        let Some(state) = weak.upgrade() else { return };
        let on = r.is_active();
        edit(&state, |c, i| {
            if let Some(l) = c.logical.get_mut(i) {
                for o in &mut l.outputs {
                    o.underscan = on;
                }
            }
        });
    });
    arrangement_area(state);
}

/// Where logical monitors are drawn: (offset x, offset y, pixels per unit).
fn area_geometry(c: &Config, width: f64, height: f64) -> (f64, f64, f64) {
    let rects: Vec<(i32, i32, i32, i32)> = c
        .logical
        .iter()
        .map(|l| {
            let (w, h) = displays::logical_size(c, l);
            (l.x, l.y, w, h)
        })
        .collect();
    let max_x = rects.iter().map(|r| r.0 + r.2).max().unwrap_or(1).max(1) as f64;
    let max_y = rects.iter().map(|r| r.1 + r.3).max().unwrap_or(1).max(1) as f64;
    let k = ((width - 24.0) / max_x)
        .min((height - 24.0) / max_y)
        .max(0.001);
    ((width - max_x * k) / 2.0, (height - max_y * k) / 2.0, k)
}

fn arrangement_area(state: &Rc<State>) {
    let weak = Rc::downgrade(state);
    state.area.set_draw_func(move |area, cr, w, h| {
        let Some(state) = weak.upgrade() else { return };
        let c = state.pending.borrow();
        let (ox, oy, k) = area_geometry(&c, w as f64, h as f64);
        let fg = area.color();
        for (i, l) in c.logical.iter().enumerate() {
            let (lw, lh) = displays::logical_size(&c, l);
            let (x, y) = (ox + l.x as f64 * k + 2.0, oy + l.y as f64 * k + 2.0);
            let (rw, rh) = (lw as f64 * k - 4.0, lh as f64 * k - 4.0);
            let selected = i == state.selected.get();
            cr.set_source_rgba(
                fg.red() as f64,
                fg.green() as f64,
                fg.blue() as f64,
                if selected { 0.25 } else { 0.1 },
            );
            cr.rectangle(x, y, rw, rh);
            let _ = cr.fill_preserve();
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.8);
            cr.set_line_width(if selected { 3.0 } else { 1.0 });
            let _ = cr.stroke();
            if let Some(m) = connector(&c, i) {
                let text = monitor_label(&c, &m);
                cr.set_font_size(13.0);
                cr.move_to(x + 8.0, y + 20.0);
                let _ = cr.show_text(&text);
            }
        }
        let has_focus = area.has_focus();
        if has_focus {
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.5);
            cr.set_line_width(1.0);
            cr.set_dash(&[3.0, 3.0], 0.0);
            cr.rectangle(1.0, 1.0, w as f64 - 2.0, h as f64 - 2.0);
            let _ = cr.stroke();
        }
    });
    let drag = gtk::GestureDrag::new();
    let weak = Rc::downgrade(state);
    drag.connect_drag_begin(move |g, px, py| {
        let Some(state) = weak.upgrade() else { return };
        let c = state.pending.borrow().clone();
        let w = g.widget().map_or(1, |w| w.width()) as f64;
        let h = g.widget().map_or(1, |w| w.height()) as f64;
        let (ox, oy, k) = area_geometry(&c, w, h);
        let (ux, uy) = ((px - ox) / k, (py - oy) / k);
        let hit = c.logical.iter().position(|l| {
            let (lw, lh) = displays::logical_size(&c, l);
            ux >= l.x as f64 && ux < (l.x + lw) as f64 && uy >= l.y as f64 && uy < (l.y + lh) as f64
        });
        if let Some(i) = hit {
            state.selected.set(i);
            state.drag.set(Some((i, c.logical[i].x, c.logical[i].y, k)));
            sync(&state);
        }
    });
    let weak = Rc::downgrade(state);
    drag.connect_drag_update(move |_, dx, dy| {
        let Some(state) = weak.upgrade() else { return };
        if let Some((i, x, y, k)) = state.drag.get() {
            if let Some(l) = state.pending.borrow_mut().logical.get_mut(i) {
                l.x = x + (dx / k) as i32;
                l.y = y + (dy / k) as i32;
            }
            state.area.queue_draw();
        }
    });
    let weak = Rc::downgrade(state);
    drag.connect_drag_end(move |_, _, _| {
        let Some(state) = weak.upgrade() else { return };
        if let Some((i, ..)) = state.drag.take() {
            displays::snap(&mut state.pending.borrow_mut(), i);
            sync(&state);
            announce_place(&state, i);
        }
    });
    state.area.add_controller(drag);

    // Keyboard: Tab or Space picks the next display, arrows move it.
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(state);
    keys.connect_key_pressed(move |_, key, _, _| {
        let Some(state) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let n = state.pending.borrow().logical.len();
        if n < 2 {
            return glib::Propagation::Proceed;
        }
        let i = state.selected.get();
        let (dx, dy) = match key {
            gdk::Key::Left => (-1, 0),
            gdk::Key::Right => (1, 0),
            gdk::Key::Up => (0, -1),
            gdk::Key::Down => (0, 1),
            gdk::Key::space => {
                state.selected.set((i + 1) % n);
                sync(&state);
                announce_place(&state, (i + 1) % n);
                return glib::Propagation::Stop;
            }
            _ => return glib::Propagation::Proceed,
        };
        {
            let mut c = state.pending.borrow_mut();
            let (w, h) = displays::logical_size(&c, &c.logical[i]);
            // Jump past the neighbour: a whole display's width or height.
            let l = &mut c.logical[i];
            l.x += dx * w;
            l.y += dy * h;
            displays::snap(&mut c, i);
        }
        sync(&state);
        announce_place(&state, i);
        glib::Propagation::Stop
    });
    state.area.add_controller(keys);
    let click = gtk::GestureClick::new();
    let area = state.area.downgrade();
    click.connect_pressed(move |_, _, _, _| {
        if let Some(a) = area.upgrade() {
            a.grab_focus();
        }
    });
    state.area.add_controller(click);
    let area = state.area.downgrade();
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(move |_| {
        if let Some(a) = area.upgrade() {
            a.queue_draw();
        }
    });
    state.area.add_controller(focus);
}

/// Says where a display now is, for screen readers.
fn announce_place(state: &Rc<State>, i: usize) {
    let c = state.pending.borrow().clone();
    let Some(conn) = connector(&c, i) else { return };
    let l = &c.logical[i];
    let message = trf!(
        "{display} at {x}, {y}",
        display = monitor_label(&c, &conn),
        x = l.x,
        y = l.y
    );
    state
        .area
        .update_property(&[gtk::accessible::Property::Description(&message)]);
    helix_app_common::announce(&state.area, &message, helix_app_common::Urgency::Polite);
}

/// Hours as "20:30".
fn hours_label(h: f64) -> String {
    let minutes = (h * 60.0).round() as i64;
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

/// A row with hour and minute spin buttons for a `d` key in hours.
fn time_row(prefs: &Prefs, key: &'static str, default: f64, title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title)
        .build();
    let hours = gtk::SpinButton::with_range(0.0, 23.0, 1.0);
    let minutes = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
    for s in [&hours, &minutes] {
        s.set_valign(gtk::Align::Center);
        s.set_orientation(gtk::Orientation::Vertical);
        s.set_wrap(true);
    }
    helix_app_common::set_accessible_label(&hours, &trf!("{time}, Hours", time = title));
    helix_app_common::set_accessible_label(&minutes, &trf!("{time}, Minutes", time = title));
    minutes.connect_output(|s| {
        s.set_text(&format!("{:02}", s.value() as i64));
        glib::Propagation::Stop
    });
    let show = {
        let (p, hours, minutes) = (prefs.clone(), hours.clone(), minutes.clone());
        move || {
            let v = p.get::<f64>(key, default);
            let t = hours_label(v);
            let (h, m) = t.split_once(':').unwrap_or(("0", "0"));
            hours.set_value(h.parse().unwrap_or(0.0));
            minutes.set_value(m.parse().unwrap_or(0.0));
        }
    };
    show();
    let quiet = Rc::new(Cell::new(false));
    for s in [&hours, &minutes] {
        let (p, hours, minutes, quiet) =
            (prefs.clone(), hours.clone(), minutes.clone(), quiet.clone());
        s.connect_value_changed(move |_| {
            if quiet.get() {
                return;
            }
            let v = hours.value() + minutes.value() / 60.0;
            if (p.get::<f64>(key, default) - v).abs() > 1e-6 {
                p.set(key, v);
            }
        });
    }
    {
        let quiet = quiet.clone();
        watch(prefs, key, move || {
            quiet.set(true);
            show();
            quiet.set(false);
        });
    }
    let bx = gtk::Box::builder()
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    bx.append(&hours);
    bx.append(&gtk::Label::new(Some(":")));
    bx.append(&minutes);
    row.add_suffix(&bx);
    row
}

fn night_light_group(prefs: &Prefs) -> adw::PreferencesGroup {
    let group = widgets::group(tr("Night Light"));
    group.set_description(Some(tr(
        "Makes the screen warmer in the evening, which can help you sleep.",
    )));
    let enabled = switch(
        prefs,
        "night-light-enabled",
        false,
        false,
        tr("Night Light"),
        "",
    );
    group.add(&enabled);
    let schedule = combo(tr("Schedule"));
    schedule.set_model(Some(&strings([
        tr("Sunset to Sunrise").to_string(),
        tr("Manual Schedule").to_string(),
    ])));
    let auto = prefs.get::<bool>("night-light-schedule-automatic", true);
    schedule.set_selected(u32::from(!auto));
    {
        let p = prefs.clone();
        schedule.connect_selected_notify(move |r| {
            let auto = r.selected() == 0;
            if p.get::<bool>("night-light-schedule-automatic", true) != auto {
                p.set("night-light-schedule-automatic", auto);
            }
        });
    }
    group.add(&schedule);
    let from = time_row(prefs, "night-light-schedule-from", 20.0, tr("From"));
    let to = time_row(prefs, "night-light-schedule-to", 6.0, tr("To"));
    group.add(&from);
    group.add(&to);
    let temperature = widgets::scale_row(
        prefs,
        "night-light-temperature",
        Num::U32,
        2700.0,
        (1700.0, 4700.0, 100.0),
        widgets::identity,
        widgets::identity,
        tr("Color Temperature"),
        Some((tr("Warmer"), tr("Cooler"))),
    );
    group.add(&temperature);
    let sync = {
        let (p, schedule, from, to, temperature) = (
            prefs.clone(),
            schedule.clone(),
            from.clone(),
            to.clone(),
            temperature.clone(),
        );
        move || {
            let on = p.get::<bool>("night-light-enabled", false);
            let auto = p.get::<bool>("night-light-schedule-automatic", true);
            schedule.set_sensitive(on);
            schedule.set_selected(u32::from(!auto));
            from.set_visible(!auto);
            to.set_visible(!auto);
            from.set_sensitive(on);
            to.set_sensitive(on);
            temperature.set_sensitive(on);
        }
    };
    sync();
    for key in ["night-light-enabled", "night-light-schedule-automatic"] {
        let s = sync.clone();
        watch(prefs, key, s);
    }
    {
        let s = sync.clone();
        enabled.connect_active_notify(move |_| s());
    }
    schedule.connect_selected_notify(move |_| sync());
    group
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(hours_label(20.5), "20:30");
        assert_eq!(hours_label(6.0), "06:00");
        assert_eq!(hours_label(23.999), "00:00");
    }
}
