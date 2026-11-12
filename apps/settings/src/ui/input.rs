//! Mouse & Touchpad and Keyboard panels.

use adw::prelude::*;
use gtk::glib;
use helix_app_common::Prefs;
use std::cell::Cell;
use std::rc::Rc;

use super::widgets::{self, identity, scale_row, switch, watch, Num};
use super::{Ctx, Panel};
use crate::values;
use crate::xkb::{self, XkbInfo};

const MOUSE: &str = "org.gnome.desktop.peripherals.mouse";
const TOUCHPAD: &str = "org.gnome.desktop.peripherals.touchpad";
const KEYBOARD: &str = "org.gnome.desktop.peripherals.keyboard";
const INPUT_SOURCES: &str = "org.gnome.desktop.input-sources";

pub struct MousePanel;
pub struct KeyboardPanel;

/// Two-choice combo writing a boolean (`choices.1` = true).
fn bool_combo(prefs: &Prefs, key: &'static str, default: bool, title: &str, choices: (&str, &str)) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&[choices.0, choices.1]))
        .build();
    row.set_selected(prefs.get::<bool>(key, default) as u32);
    row.connect_selected_notify(glib::clone!(
        #[strong]
        prefs,
        move |r| {
            let v = r.selected() == 1;
            if prefs.get::<bool>(key, default) != v {
                prefs.set(key, v);
            }
        }
    ));
    let (weak, p) = (row.downgrade(), prefs.clone());
    watch(prefs, key, move || {
        if let Some(r) = weak.upgrade() {
            r.set_selected(p.get::<bool>(key, default) as u32);
        }
    });
    row
}

impl Panel for MousePanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let mouse = ctx.prefs(MOUSE);
        let tp = ctx.prefs(TOUCHPAD);
        let page = widgets::page();

        let general = widgets::group("General");
        let primary = bool_combo(&mouse, "left-handed", false, "Primary Button", ("Left", "Right"));
        primary.set_subtitle("Order of physical buttons on mice and touchpads");
        general.add(&primary);
        page.add(&general);

        let m = widgets::group("Mouse");
        m.add(&scale_row(&mouse, "speed", Num::F64, 0.0, (-1.0, 1.0, 0.05), identity, identity, "Pointer Speed", Some(("Slow", "Fast"))));
        let accel = widgets::nick_switch(&mouse, "accel-profile", ("default", "flat"), "default", "Mouse Acceleration");
        accel.set_subtitle("Pointer speed increases with movement speed");
        m.add(&accel);
        m.add(&switch(&mouse, "natural-scroll", false, false, "Natural Scrolling", "Scrolling moves the content, not the view"));
        page.add(&m);

        let t = widgets::group("Touchpad");
        let enabled = widgets::nick_switch(&tp, "send-events", ("enabled", "disabled"), "enabled", "Touchpad");
        t.add(&enabled);
        let rows: Vec<gtk::Widget> = vec![
            switch(&tp, "disable-while-typing", true, false, "Disable Touchpad While Typing", "").upcast(),
            scale_row(&tp, "speed", Num::F64, 0.0, (-1.0, 1.0, 0.05), identity, identity, "Pointer Speed", Some(("Slow", "Fast"))).upcast(),
            widgets::nick_combo(&tp, "click-method", "fingers", &[("fingers", "Two Finger Push"), ("areas", "Corner Push")], "Secondary Click").upcast(),
            switch(&tp, "tap-to-click", true, false, "Tap to Click", "Tap the touchpad to click").upcast(),
            switch(&tp, "natural-scroll", true, false, "Natural Scrolling", "Scrolling moves the content, not the view").upcast(),
        ];
        // Two-finger and edge scrolling are mutually exclusive.
        let two = switch(&tp, "two-finger-scrolling-enabled", true, false, "Two-Finger Scrolling", "");
        let edge = switch(&tp, "edge-scrolling-enabled", false, false, "Edge Scrolling", "");
        two.connect_active_notify(glib::clone!(
            #[weak]
            edge,
            move |r| {
                if r.is_active() {
                    edge.set_active(false);
                }
            }
        ));
        edge.connect_active_notify(glib::clone!(
            #[weak]
            two,
            move |r| {
                if r.is_active() {
                    two.set_active(false);
                }
            }
        ));
        let mut all = rows;
        all.insert(4, two.upcast());
        all.insert(5, edge.upcast());
        for r in &all {
            t.add(r);
            enabled.bind_property("active", r, "sensitive").sync_create().build();
        }
        page.add(&t);
        page.upcast()
    }
}

impl Panel for KeyboardPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let page = widgets::page();
        page.add(&input_sources_group(ctx));
        let switching = widgets::group("Input Source Switching");
        let sources = ctx.prefs(INPUT_SOURCES);
        switching.add(&switch(&sources, "per-window", false, false, "Switch Input Sources Individually for Each Window", ""));
        page.add(&switching);

        let kb = ctx.prefs(KEYBOARD);
        let typing = widgets::group("Typing");
        let repeat = switch(&kb, "repeat", true, false, "Repeat Keys", "Key presses repeat when a key is held down");
        typing.add(&repeat);
        let delay = scale_row(&kb, "delay", Num::U32, 500.0, (100.0, 2000.0, 10.0), identity, identity, "Delay", Some(("Short", "Long")));
        let speed = scale_row(
            &kb,
            "repeat-interval",
            Num::U32,
            30.0,
            (0.0, 1.0, 0.01),
            |ms| values::interval_to_speed(ms as u32),
            |p| values::speed_to_interval(p) as f64,
            "Speed",
            Some(("Slow", "Fast")),
        );
        for r in [&delay, &speed] {
            repeat.bind_property("active", r, "sensitive").sync_create().build();
            typing.add(r);
        }
        page.add(&typing);
        page.upcast()
    }
}

type Sources = Vec<(String, String)>;

fn read_sources(prefs: &Prefs) -> Sources {
    prefs.get::<Sources>("sources", Vec::new())
}

struct SourcesUi {
    prefs: Prefs,
    info: Rc<XkbInfo>,
    group: adw::PreferencesGroup,
    rows: std::cell::RefCell<Vec<adw::ActionRow>>,
    writing: Cell<bool>,
}

impl SourcesUi {
    fn write(self: &Rc<Self>, sources: Sources) {
        self.writing.set(true);
        self.prefs.set("sources", sources);
        self.writing.set(false);
        self.rebuild();
    }

    fn rebuild(self: &Rc<Self>) {
        for r in self.rows.borrow_mut().drain(..) {
            self.group.remove(&r);
        }
        let sources = read_sources(&self.prefs);
        let n = sources.len();
        if n == 0 {
            let row = adw::ActionRow::builder().title("No input sources").subtitle("The keyboard uses the system default layout").build();
            self.group.add(&row);
            self.rows.borrow_mut().push(row);
        }
        for (i, (kind, id)) in sources.iter().enumerate() {
            let row = adw::ActionRow::builder().use_markup(false).title(xkb::source_name(&self.info, kind, id)).build();
            if kind == "xkb" {
                row.set_subtitle(id);
            }
            for (icon, tip, delta, enabled) in [
                ("go-up-symbolic", "Move Up", -1isize, i > 0),
                ("go-down-symbolic", "Move Down", 1, i + 1 < n),
            ] {
                let b = gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).css_classes(["flat"]).sensitive(enabled).build();
                let weak = Rc::downgrade(self);
                b.connect_clicked(move |_| {
                    if let Some(this) = weak.upgrade() {
                        let mut s = read_sources(&this.prefs);
                        if xkb::move_item(&mut s, i, delta) {
                            this.write(s);
                        }
                    }
                });
                row.add_suffix(&b);
            }
            let rm = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Remove").valign(gtk::Align::Center).css_classes(["flat"]).build();
            let weak = Rc::downgrade(self);
            rm.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    let mut s = read_sources(&this.prefs);
                    if i < s.len() {
                        s.remove(i);
                        this.write(s);
                    }
                }
            });
            row.add_suffix(&rm);
            self.group.add(&row);
            self.rows.borrow_mut().push(row);
        }
    }
}

fn input_sources_group(ctx: &Rc<Ctx>) -> adw::PreferencesGroup {
    let prefs = ctx.prefs(INPUT_SOURCES);
    let group = widgets::group("Input Sources");
    group.set_description(Some("Includes keyboard layouts and input methods"));
    let add = gtk::Button::builder().label("Add Input Source…").css_classes(["flat"]).build();
    group.set_header_suffix(Some(&add));
    let ui = Rc::new(SourcesUi {
        prefs: prefs.clone(),
        info: Rc::new(XkbInfo::load()),
        group: group.clone(),
        rows: Default::default(),
        writing: Cell::new(false),
    });
    ui.rebuild();
    let weak = Rc::downgrade(&ui);
    watch(&prefs, "sources", move || {
        if let Some(ui) = weak.upgrade() {
            if !ui.writing.get() {
                ui.rebuild();
            }
        }
    });
    let window = ctx.window.clone();
    add.connect_clicked(move |_| add_source_dialog(&window, &ui));

    group
}

fn add_source_dialog(parent: &adw::ApplicationWindow, ui: &Rc<SourcesUi>) {
    let dialog = adw::Dialog::builder().title("Add Input Source").content_width(420).content_height(520).build();
    let search = gtk::SearchEntry::builder().placeholder_text("Search languages and layouts").margin_start(12).margin_end(12).margin_top(6).margin_bottom(6).build();
    let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    let scrolled = gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).child(&list).build();
    let fill = {
        let (list, info) = (list.clone(), ui.info.clone());
        move |q: &str| {
            list.remove_all();
            for (id, desc) in xkb::filter(&info, q).into_iter().take(400) {
                let row = adw::ActionRow::builder().use_markup(false).title(desc.as_str()).subtitle(id.as_str()).activatable(true).build();
                row.set_widget_name(id);
                list.append(&row);
            }
        }
    };
    fill("");
    search.connect_search_changed(move |e| fill(&e.text()));
    let weak = Rc::downgrade(ui);
    let dlg = dialog.downgrade();
    list.connect_row_activated(move |_, row| {
        let id = row.widget_name().to_string();
        if let Some(ui) = weak.upgrade() {
            let mut s = read_sources(&ui.prefs);
            let entry = ("xkb".to_string(), id);
            if !s.contains(&entry) {
                s.push(entry);
                ui.write(s);
            }
        }
        if let Some(d) = dlg.upgrade() {
            d.close();
        }
    });
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bx.append(&search);
    bx.append(&scrolled);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&bx));
    dialog.set_child(Some(&view));
    dialog.present(Some(parent));
    search.grab_focus();
}
