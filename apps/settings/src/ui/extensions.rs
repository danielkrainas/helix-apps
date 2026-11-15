//! Extensions panel: Helix shell extensions through `org.helix.Extensions1`.

use adw::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use super::widgets::{self, switch};
use super::{spawn_worker, Ctx, Panel};
use crate::extensions::{self as ext, Ext, Failure, Kind, Setting, Val};

const SCHEMA: &str = "org.helix.extensions";
/// Changes closer together than this are read once.
const SETTLE: Duration = Duration::from_millis(300);

pub struct ExtensionsPanel;

/// One setting's row.
enum SettingRow {
    Switch(adw::SwitchRow),
    Spin(adw::SpinRow),
    Combo(adw::ComboRow, Vec<String>),
    Entry(adw::EntryRow),
}

impl SettingRow {
    fn widget(&self) -> gtk::Widget {
        match self {
            SettingRow::Switch(r) => r.clone().upcast(),
            SettingRow::Spin(r) => r.clone().upcast(),
            SettingRow::Combo(r, _) => r.clone().upcast(),
            SettingRow::Entry(r) => r.clone().upcast(),
        }
    }

    /// Show `value` (the stored one).
    fn show(&self, value: &Val) {
        match (self, value) {
            (SettingRow::Switch(r), Val::Bool(b)) => r.set_active(*b),
            (SettingRow::Spin(r), Val::Int(i)) => r.set_value(*i as f64),
            (SettingRow::Spin(r), Val::Double(d)) => r.set_value(*d),
            (SettingRow::Combo(r, choices), Val::Text(t)) => {
                if let Some(at) = choices.iter().position(|c| c == t) {
                    r.set_selected(at as u32);
                }
            }
            (SettingRow::Entry(r), Val::Text(t)) => r.set_text(t),
            (SettingRow::Entry(r), Val::List(l)) => r.set_text(&l.join(", ")),
            _ => {}
        }
    }
}

/// One extension's rows.
struct ExtRow {
    expander: adw::ExpanderRow,
    switch: Option<gtk::Switch>,
    description: adw::ActionRow,
    access: adw::ActionRow,
    problem: adw::ActionRow,
    restart: adw::ActionRow,
    settings: Vec<SettingRow>,
    /// Command rows' key labels, by command id.
    commands: Vec<(String, gtk::ShortcutLabel)>,
}

/// What the rows are made of: rebuilt when it changes.
type Shape = Vec<(String, Vec<String>, Vec<String>)>;

struct State {
    ctx: Rc<Ctx>,
    stack: gtk::Stack,
    group: adw::PreferencesGroup,
    rows: RefCell<HashMap<String, ExtRow>>,
    shape: RefCell<Shape>,
    /// The list as last read.
    exts: RefCell<HashMap<String, Ext>>,
    /// Programmatic changes don't write back.
    quiet: Cell<bool>,
    pending: Cell<bool>,
}

impl Panel for ExtensionsPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let stack = gtk::Stack::new();
        stack.add_named(&loading_page(), Some("loading"));
        let missing = adw::StatusPage::builder()
            .icon_name("application-x-addon-symbolic")
            .title("Extensions Are Not Available")
            .description("They need the Helix shell.")
            .build();
        stack.add_named(&missing, Some("missing"));

        let page = widgets::page();
        let general = widgets::group("General");
        let prefs = ctx.prefs(SCHEMA);
        general.add(&switch(
            &prefs,
            "disable-all",
            false,
            true,
            "Use Extensions",
            "Panel items, quick settings and more from extensions",
        ));
        page.add(&general);
        let group = widgets::group("Installed");
        page.add(&group);
        stack.add_named(&page, Some("list"));
        // Until the first read answers; "missing" only when it fails.
        stack.set_visible_child_name("loading");

        let state = Rc::new(State {
            ctx: ctx.clone(),
            stack: stack.clone(),
            group,
            rows: RefCell::default(),
            shape: RefCell::default(),
            exts: RefCell::default(),
            quiet: Cell::new(false),
            pending: Cell::new(false),
        });
        refresh(&state);

        // Follow the shell: its Changed signal, and it coming or going.
        let (tx, rx) = async_channel::unbounded::<()>();
        std::thread::spawn(move || ext::watch(move || tx.send_blocking(()).is_ok()));
        let weak = Rc::downgrade(&state);
        glib::spawn_future_local(async move {
            while rx.recv().await.is_ok() {
                let Some(state) = weak.upgrade() else { break };
                if !state.pending.replace(true) {
                    let weak = Rc::downgrade(&state);
                    glib::timeout_add_local_once(SETTLE, move || {
                        if let Some(state) = weak.upgrade() {
                            state.pending.set(false);
                            refresh(&state);
                        }
                    });
                }
            }
        });
        stack.connect_destroy(move |_| {
            let _ = &state;
        });
        stack.upcast()
    }
}

/// A spinner with a visible label, shown while the list is first read.
fn loading_page() -> gtk::Widget {
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .width_request(32)
        .height_request(32)
        .build();
    let label = gtk::Label::builder()
        .label("Loading Extensions…")
        .css_classes(["dim-label"])
        .build();
    helix_app_common::label_widget(&spinner, &label);
    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build();
    bx.append(&spinner);
    bx.append(&label);
    bx.upcast()
}

/// Read the list (and every extension's settings) again.
fn refresh(state: &Rc<State>) {
    let weak = Rc::downgrade(state);
    spawn_worker(
        || {
            ext::list().map(|list| {
                list.into_iter()
                    .map(|e| {
                        let s = ext::settings(&e.id);
                        (e, s)
                    })
                    .collect::<Vec<_>>()
            })
        },
        move |found| {
            if let Some(state) = weak.upgrade() {
                show(&state, found);
            }
        },
    );
}

fn show(state: &Rc<State>, found: Option<Vec<(Ext, Vec<Setting>)>>) {
    let Some(found) = found else {
        state.stack.set_visible_child_name("missing");
        return;
    };
    state.stack.set_visible_child_name("list");
    let shape: Shape = found
        .iter()
        .map(|(e, s)| {
            (
                e.id.clone(),
                s.iter()
                    .map(|s| format!("{}:{:?}:{:?}", s.key, s.kind, s.choices))
                    .collect(),
                e.commands.iter().map(|c| c.0.clone()).collect(),
            )
        })
        .collect();
    if *state.shape.borrow() != shape {
        for row in state.rows.borrow_mut().drain().map(|(_, r)| r) {
            state.group.remove(&row.expander);
        }
        for (e, settings) in &found {
            let row = build_row(state, e, settings);
            state.group.add(&row.expander);
            state.rows.borrow_mut().insert(e.id.clone(), row);
        }
        *state.shape.borrow_mut() = shape;
    }
    if found.is_empty() {
        state
            .group
            .set_description(Some("No extensions are installed."));
    } else {
        state.group.set_description(None);
    }
    state.quiet.set(true);
    for (e, settings) in &found {
        if let Some(row) = state.rows.borrow().get(&e.id) {
            update_row(row, e, settings);
        }
    }
    state.quiet.set(false);
    *state.exts.borrow_mut() = found.into_iter().map(|(e, _)| (e.id.clone(), e)).collect();
}

fn update_row(row: &ExtRow, e: &Ext, settings: &[Setting]) {
    row.expander.set_title(&e.name);
    row.expander.set_subtitle(&e.subtitle());
    if let Some(sw) = &row.switch {
        sw.set_active(e.on());
        sw.set_sensitive(e.state != "invalid" || e.on());
    }
    row.description.set_title(&e.description);
    row.description.set_visible(!e.description.is_empty());
    let (title, below) = e.access();
    row.access.set_title(&title);
    row.access.set_subtitle(&below);
    if e.sandboxed {
        row.access.remove_css_class("warning");
    } else {
        row.access.add_css_class("warning");
    }
    row.problem.set_visible(e.troubled());
    row.problem.set_subtitle(&e.error);
    row.restart
        .set_visible(matches!(e.state.as_str(), "running" | "crashed"));
    for (row, setting) in row.settings.iter().zip(settings) {
        row.show(&setting.value);
    }
    for (id, label) in &row.commands {
        let keys = e
            .commands
            .iter()
            .find(|c| &c.0 == id)
            .map(|c| c.2.clone())
            .unwrap_or_default();
        label.set_accelerator(&keys);
    }
}

fn build_row(state: &Rc<State>, e: &Ext, settings: &[Setting]) -> ExtRow {
    let expander = adw::ExpanderRow::builder()
        .use_markup(false)
        .title(&e.name)
        .build();
    let id = e.id.clone();
    let switch = (!e.development()).then(|| {
        let sw = gtk::Switch::builder().valign(gtk::Align::Center).build();
        helix_app_common::set_accessible_label(&sw, &format!("Use {}", e.name));
        let weak = Rc::downgrade(state);
        let id = id.clone();
        sw.connect_active_notify(move |sw| {
            let Some(state) = weak.upgrade() else { return };
            if !state.quiet.get() {
                turn(&state, sw, &id, sw.is_active());
            }
        });
        expander.add_suffix(&sw);
        sw
    });

    let description = adw::ActionRow::builder()
        .use_markup(false)
        .title_lines(0)
        .css_classes(["dim-label"])
        .build();
    expander.add_row(&description);
    let access = adw::ActionRow::builder()
        .use_markup(false)
        .subtitle_lines(0)
        .build();
    expander.add_row(&access);

    let problem = adw::ActionRow::builder()
        .use_markup(false)
        .title("Problem")
        .subtitle_lines(0)
        .build();
    let log_button = gtk::Button::builder()
        .label("Show Log")
        .valign(gtk::Align::Center)
        .build();
    helix_app_common::set_accessible_label(&log_button, &format!("Show Log for {}", e.name));
    {
        let weak = Rc::downgrade(state);
        let (id, name) = (id.clone(), e.name.clone());
        log_button.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                show_log(&state, &id, &name);
            }
        });
    }
    problem.add_suffix(&log_button);
    expander.add_row(&problem);

    let restart = adw::ActionRow::builder().title("Restart").build();
    let restart_button = gtk::Button::builder()
        .label("Restart")
        .valign(gtk::Align::Center)
        .build();
    helix_app_common::set_accessible_label(&restart_button, &format!("Restart {}", e.name));
    {
        let weak = Rc::downgrade(state);
        let id = id.clone();
        restart_button.connect_clicked(move |_| {
            let id = id.clone();
            let weak = weak.clone();
            spawn_worker(move || ext::restart(&id), move |r| report(&weak, r));
        });
    }
    restart.add_suffix(&restart_button);
    restart.set_activatable_widget(Some(&restart_button));
    expander.add_row(&restart);

    let settings: Vec<SettingRow> = settings
        .iter()
        .map(|s| setting_row(state, &id, s))
        .collect();
    for row in &settings {
        expander.add_row(&row.widget());
    }

    let mut commands = Vec::new();
    for (command, title, _) in &e.commands {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(title)
            .build();
        let keys = gtk::ShortcutLabel::builder()
            .disabled_text("None")
            .valign(gtk::Align::Center)
            .build();
        let change = gtk::Button::builder()
            .label("Change…")
            .valign(gtk::Align::Center)
            .build();
        helix_app_common::set_accessible_label(
            &change,
            &format!("Change Shortcut for {title} ({})", e.name),
        );
        let weak = Rc::downgrade(state);
        let (full, title) = (format!("{id}/{command}"), title.clone());
        change.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                capture_shortcut(&state, &full, &title);
            }
        });
        row.add_suffix(&keys);
        row.add_suffix(&change);
        expander.add_row(&row);
        commands.push((command.clone(), keys));
    }
    ExtRow {
        expander,
        switch,
        description,
        access,
        problem,
        restart,
        settings,
        commands,
    }
}

/// The switch was flipped by the user.
fn turn(state: &Rc<State>, sw: &gtk::Switch, id: &str, on: bool) {
    sw.set_sensitive(false);
    let name = state
        .exts
        .borrow()
        .get(id)
        .map(|e| e.name.clone())
        .unwrap_or_else(|| id.to_owned());
    let weak = Rc::downgrade(state);
    let sw = sw.downgrade();
    let id = id.to_owned();
    spawn_worker(
        move || {
            if on {
                ext::enable(&id)
            } else {
                ext::disable(&id)
            }
        },
        move |result| {
            let Some(state) = weak.upgrade() else { return };
            if let Some(sw) = sw.upgrade() {
                sw.set_sensitive(true);
                if result.is_err() {
                    state.quiet.set(true);
                    sw.set_active(!on);
                    state.quiet.set(false);
                }
            }
            match result {
                Ok(()) => {}
                Err(Failure::Denied) => state.ctx.toast(&format!("{name} was not turned on")),
                Err(Failure::Other(msg)) => state.ctx.toast(&msg),
            }
            refresh(&state);
        },
    );
}

/// A call's error as a toast, then the list again.
fn report(weak: &std::rc::Weak<State>, result: Result<(), Failure>) {
    let Some(state) = weak.upgrade() else { return };
    if let Err(Failure::Other(msg)) = result {
        state.ctx.toast(&msg);
    }
    refresh(&state);
}

/// Write a setting; a refusal shows the stored value again.
fn write(state: &Rc<State>, id: &str, key: &str, value: Val) {
    if state.quiet.get() {
        return;
    }
    let (id, key) = (id.to_owned(), key.to_owned());
    let weak = Rc::downgrade(state);
    spawn_worker(
        move || ext::set_setting(&id, &key, &value),
        move |r| report(&weak, r),
    );
}

fn setting_row(state: &Rc<State>, id: &str, s: &Setting) -> SettingRow {
    let weak = Rc::downgrade(state);
    let (id, key) = (id.to_owned(), s.key.clone());
    let send = move |value: Val| {
        if let Some(state) = weak.upgrade() {
            write(&state, &id, &key, value);
        }
    };
    let row = match s.kind {
        Kind::Bool => {
            let r = adw::SwitchRow::builder()
                .use_markup(false)
                .title(&s.label)
                .build();
            r.connect_active_notify(move |r| send(Val::Bool(r.is_active())));
            SettingRow::Switch(r)
        }
        Kind::Int | Kind::Double => {
            let (lo, hi) = s.range();
            let r = adw::SpinRow::with_range(lo, hi, s.step());
            r.set_use_markup(false);
            r.set_title(&s.label);
            let int = s.kind == Kind::Int;
            r.set_digits(if int { 0 } else { 2 });
            r.connect_value_notify(move |r| {
                send(if int {
                    Val::Int(r.value().round() as i64)
                } else {
                    Val::Double(r.value())
                })
            });
            SettingRow::Spin(r)
        }
        Kind::Enum => {
            let model =
                gtk::StringList::new(&s.choices.iter().map(String::as_str).collect::<Vec<_>>());
            let r = adw::ComboRow::builder()
                .use_markup(false)
                .title(&s.label)
                .model(&model)
                .build();
            let choices = s.choices.clone();
            r.connect_selected_notify(move |r| {
                if let Some(c) = choices.get(r.selected() as usize) {
                    send(Val::Text(c.clone()));
                }
            });
            SettingRow::Combo(r, s.choices.clone())
        }
        Kind::Text | Kind::List => {
            let r = adw::EntryRow::builder()
                .use_markup(false)
                .title(&s.label)
                .show_apply_button(true)
                .build();
            let list = s.kind == Kind::List;
            r.connect_apply(move |r| {
                let text = r.text().to_string();
                send(if list {
                    Val::List(ext::split_list(&text))
                } else {
                    Val::Text(text)
                });
            });
            if !s.help.is_empty() {
                r.set_tooltip_text(Some(&s.help));
            }
            SettingRow::Entry(r)
        }
    };
    if !s.help.is_empty() {
        match &row {
            SettingRow::Switch(r) => r.set_subtitle(&s.help),
            SettingRow::Spin(r) => r.set_subtitle(&s.help),
            SettingRow::Combo(r, _) => r.set_subtitle(&s.help),
            SettingRow::Entry(_) => {}
        }
    }
    state.quiet.set(true);
    row.show(&s.value);
    state.quiet.set(false);
    row
}

/// "Show Log": the extension's last log lines.
fn show_log(state: &Rc<State>, id: &str, name: &str) {
    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scrolled));
    let dialog = adw::Dialog::builder()
        .title(format!("{name} Log"))
        .content_width(640)
        .content_height(420)
        .child(&toolbar)
        .build();
    dialog.present(Some(&state.ctx.window));
    let id = id.to_owned();
    let view = view.downgrade();
    spawn_worker(
        move || ext::log(&id),
        move |lines| {
            if let Some(view) = view.upgrade() {
                let text = if lines.is_empty() {
                    "Nothing was logged.".to_owned()
                } else {
                    lines.join("\n")
                };
                view.buffer().set_text(&text);
                let end = view.buffer().end_iter();
                view.buffer().place_cursor(&end);
                view.scroll_to_iter(&mut view.buffer().end_iter(), 0.0, false, 0.0, 1.0);
            }
        },
    );
}

/// Shows why keys were refused in the capture dialog and speaks it at
/// once (focus stays on the dialog, so nothing else would).
fn show_capture_error(status: &gtk::Label, message: &str) {
    status.set_label(message);
    status.set_visible(true);
    helix_app_common::announce(status, message, helix_app_common::Urgency::Assertive);
}

/// "Change…": wait for a key combination for `command`.
fn capture_shortcut(state: &Rc<State>, command: &str, title: &str) {
    let heading = gtk::Label::builder()
        .label(format!("Press the new keys for {title}"))
        .wrap(true)
        .css_classes(["title-4"])
        .build();
    let hint = gtk::Label::builder()
        .label("Escape cancels, Backspace removes the key.")
        .css_classes(["dim-label"])
        .build();
    let status = gtk::Label::builder()
        .wrap(true)
        .css_classes(["error"])
        .visible(false)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    content.append(&heading);
    content.append(&hint);
    content.append(&status);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title("Change Key")
        .content_width(420)
        .child(&toolbar)
        .build();

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(state);
    let command = command.to_owned();
    let d = dialog.downgrade();
    let status_weak = status.downgrade();
    keys.connect_key_pressed(move |_, keyval, _, mods| {
        let wanted = mods
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK
                | gdk::ModifierType::SHIFT_MASK);
        let accelerator = match keyval {
            gdk::Key::Escape if wanted.is_empty() => {
                if let Some(d) = d.upgrade() {
                    d.close();
                }
                return glib::Propagation::Stop;
            }
            gdk::Key::BackSpace if wanted.is_empty() => String::new(),
            _ => {
                let name = keyval.name().map(|n| n.to_string()).unwrap_or_default();
                let ctrl_alt_super = wanted.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                );
                if !ext::shortcut_allowed(&name, ctrl_alt_super) {
                    // A plain key is refused; say so (a modifier alone just
                    // starts a combination).
                    if !ext::is_modifier_key(&name) {
                        if let Some(status) = status_weak.upgrade() {
                            show_capture_error(&status, "Use Ctrl, Alt or Super with this key");
                        }
                    }
                    return glib::Propagation::Stop;
                }
                gtk::accelerator_name(keyval.to_lower(), wanted).to_string()
            }
        };
        let (command, weak, d, status) =
            (command.clone(), weak.clone(), d.clone(), status.downgrade());
        spawn_worker(
            move || ext::bind(&command, &accelerator),
            move |result| match result {
                Ok(()) => {
                    if let Some(d) = d.upgrade() {
                        d.close();
                    }
                    if let Some(state) = weak.upgrade() {
                        refresh(&state);
                    }
                }
                Err(Failure::Denied) | Err(Failure::Other(_)) if status.upgrade().is_none() => {}
                Err(err) => {
                    let msg = match err {
                        Failure::Other(m) => m,
                        Failure::Denied => "Not allowed".to_owned(),
                    };
                    if let Some(status) = status.upgrade() {
                        show_capture_error(&status, &format!("Could not use these keys: {msg}"));
                    }
                }
            },
        );
        glib::Propagation::Stop
    });
    dialog.add_controller(keys);
    dialog.present(Some(&state.ctx.window));
}
