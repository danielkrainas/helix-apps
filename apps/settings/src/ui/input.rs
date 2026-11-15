//! Mouse & Touchpad and Keyboard panels.

use adw::prelude::*;
use gtk::glib;
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::picker::{self, Picker};
use super::widgets::{self, identity, scale_row, switch, watch, Num};
use super::{spawn_worker, Ctx, Panel};
use crate::values;
use crate::xkb::{self, XkbInfo};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;

const MOUSE: &str = "org.gnome.desktop.peripherals.mouse";
const TOUCHPAD: &str = "org.gnome.desktop.peripherals.touchpad";
const KEYBOARD: &str = "org.gnome.desktop.peripherals.keyboard";
const INPUT_SOURCES: &str = "org.gnome.desktop.input-sources";

pub struct MousePanel;
pub struct KeyboardPanel;

/// Two-choice combo writing a boolean (`choices.1` = true).
fn bool_combo(
    prefs: &Prefs,
    key: &'static str,
    default: bool,
    title: &str,
    choices: (&str, &str),
) -> adw::ComboRow {
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

        let general = widgets::group(tr("General"));
        let primary = bool_combo(
            &mouse,
            "left-handed",
            false,
            tr("Primary Button"),
            (tr("Left"), tr("Right")),
        );
        primary.set_subtitle(tr("Button order on mice and touchpads"));
        general.add(&primary);
        page.add(&general);

        let m = widgets::group(tr("Mouse"));
        m.add(&scale_row(
            &mouse,
            "speed",
            Num::F64,
            0.0,
            (-1.0, 1.0, 0.05),
            identity,
            identity,
            tr("Pointer Speed"),
            Some((tr("Slow"), tr("Fast"))),
        ));
        let accel = widgets::nick_switch(
            &mouse,
            "accel-profile",
            ("default", "flat"),
            "default",
            tr("Mouse Acceleration"),
        );
        accel.set_subtitle(tr("Pointer speed increases with movement speed"));
        m.add(&accel);
        m.add(&switch(
            &mouse,
            "natural-scroll",
            false,
            false,
            tr("Natural Scrolling"),
            tr("Scrolling moves the content, not the view"),
        ));
        page.add(&m);

        let t = widgets::group(tr("Touchpad"));
        let enabled = widgets::nick_switch(
            &tp,
            "send-events",
            ("enabled", "disabled"),
            "enabled",
            tr("Touchpad"),
        );
        t.add(&enabled);
        let rows: Vec<gtk::Widget> = vec![
            switch(
                &tp,
                "disable-while-typing",
                true,
                false,
                tr("Disable Touchpad While Typing"),
                "",
            )
            .upcast(),
            scale_row(
                &tp,
                "speed",
                Num::F64,
                0.0,
                (-1.0, 1.0, 0.05),
                identity,
                identity,
                tr("Pointer Speed"),
                Some((tr("Slow"), tr("Fast"))),
            )
            .upcast(),
            widgets::nick_combo(
                &tp,
                "click-method",
                "fingers",
                {
                    const CHOICES: &[(&str, &str)] = &[
                        ("fingers", n_("Two Finger Push")),
                        ("areas", n_("Corner Push")),
                    ];
                    CHOICES
                },
                tr("Secondary Click"),
            )
            .upcast(),
            switch(
                &tp,
                "tap-to-click",
                true,
                false,
                tr("Tap to Click"),
                tr("Tap the touchpad to click"),
            )
            .upcast(),
            switch(
                &tp,
                "natural-scroll",
                true,
                false,
                tr("Natural Scrolling"),
                tr("Scrolling moves the content, not the view"),
            )
            .upcast(),
        ];
        // Two-finger and edge scrolling are mutually exclusive.
        let two = switch(
            &tp,
            "two-finger-scrolling-enabled",
            true,
            false,
            tr("Two-Finger Scrolling"),
            "",
        );
        let edge = switch(
            &tp,
            "edge-scrolling-enabled",
            false,
            false,
            tr("Edge Scrolling"),
            "",
        );
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
            enabled
                .bind_property("active", r, "sensitive")
                .sync_create()
                .build();
        }
        page.add(&t);
        page.upcast()
    }
}

impl Panel for KeyboardPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let page = widgets::page();
        page.add(&input_sources_group(ctx));
        let switching = widgets::group(tr("Input Source Switching"));
        let sources = ctx.prefs(INPUT_SOURCES);
        switching.add(&switch(
            &sources,
            "per-window",
            false,
            false,
            tr("Switch Input Sources Individually for Each Window"),
            "",
        ));
        page.add(&switching);

        let kb = ctx.prefs(KEYBOARD);
        let typing = widgets::group(tr("Typing"));
        let repeat = switch(
            &kb,
            "repeat",
            true,
            false,
            tr("Repeat Keys"),
            tr("Key presses repeat when a key is held down"),
        );
        typing.add(&repeat);
        let delay = scale_row(
            &kb,
            "delay",
            Num::U32,
            500.0,
            (100.0, 2000.0, 10.0),
            identity,
            identity,
            tr("Delay"),
            Some((tr("Short"), tr("Long"))),
        );
        let speed = scale_row(
            &kb,
            "repeat-interval",
            Num::U32,
            30.0,
            (0.0, 1.0, 0.01),
            |ms| values::interval_to_speed(ms as u32),
            |p| values::speed_to_interval(p) as f64,
            tr("Speed"),
            Some((tr("Slow"), tr("Fast"))),
        );
        for r in [&delay, &speed] {
            repeat
                .bind_property("active", r, "sensitive")
                .sync_create()
                .build();
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
    /// "Add Input Source…": where focus goes when the last source is
    /// removed.
    add: gtk::Button,
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

    /// Rebuilds the rows, keeping keyboard focus on the moved source's
    /// button (so repeated Move Up/Down presses keep moving it), on a
    /// neighbour after Remove (or on "Add Input Source…" when none is
    /// left), and the scroll position.
    fn rebuild(self: &Rc<Self>) {
        let group = self.group.clone();
        let add = self.add.clone().upcast::<gtk::Widget>();
        helix_app_common::focus::rebuild_keeping_focus(&group, Some(&add), || self.rebuild_rows());
    }

    fn rebuild_rows(self: &Rc<Self>) {
        for r in self.rows.borrow_mut().drain(..) {
            self.group.remove(&r);
        }
        let sources = read_sources(&self.prefs);
        let n = sources.len();
        if n == 0 {
            let row = adw::ActionRow::builder()
                .title(tr("No input sources"))
                .subtitle(tr("The keyboard uses the system default layout"))
                .build();
            self.group.add(&row);
            self.rows.borrow_mut().push(row);
        }
        for (i, (kind, id)) in sources.iter().enumerate() {
            let name = xkb::source_name(&self.info, kind, id);
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&name)
                .build();
            if kind == "xkb" {
                row.set_subtitle(id);
            }
            helix_app_common::focus::set_key(&row, &format!("{kind}:{id}"));
            for (icon, tip, delta, enabled) in [
                ("go-up-symbolic", tr("Move Up"), -1isize, i > 0),
                ("go-down-symbolic", tr("Move Down"), 1, i + 1 < n),
            ] {
                let b = gtk::Button::builder()
                    .icon_name(icon)
                    .tooltip_text(tip)
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .sensitive(enabled)
                    .build()
                    .labelled(&row_button_name(tip, &name));
                helix_app_common::focus::set_part(&b, tip);
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
            let rm = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Remove"))
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build()
                .labelled(&row_button_name(tr("Remove"), &name));
            helix_app_common::focus::set_part(&rm, "remove");
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
    let group = widgets::group(tr("Input Sources"));
    group.set_description(Some(tr("Includes keyboard layouts and input methods")));
    let add = gtk::Button::builder()
        .label(tr("Add Input Source…"))
        .css_classes(["flat"])
        .build();
    group.set_header_suffix(Some(&add));
    let ui = Rc::new(SourcesUi {
        prefs: prefs.clone(),
        info: Rc::new(XkbInfo::load()),
        group: group.clone(),
        add: add.clone(),
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
    let picker = Picker::new(
        tr("Add Input Source"),
        tr("Search languages, layouts and input methods"),
        tr("Search Input Sources"),
        tr("Loading Input Sources…"),
    );
    let engines: Rc<RefCell<Option<Vec<xkb::IbusEngine>>>> = Rc::default();
    let fill = {
        let (picker, info, engines) = (Rc::downgrade(&picker), ui.info.clone(), engines.clone());
        move |q: &str| {
            let Some(picker) = picker.upgrade() else {
                return;
            };
            let engines = engines.borrow();
            let Some(engines) = engines.as_ref() else {
                return;
            };
            // Ids are "<type>\t<id>" so IBus names may contain anything.
            let mut all: Vec<picker::Choice> = xkb::filter(&info, q)
                .into_iter()
                .map(|(id, desc)| (format!("xkb\t{id}"), desc.clone(), id.clone()))
                .collect();
            all.extend(engines.iter().filter(|e| xkb::ibus_matches(e, q)).map(|e| {
                let lang = if e.language.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", e.language)
                };
                (
                    format!("ibus\t{}", e.name),
                    e.longname.clone(),
                    trf!("Input method (IBus){lang}", lang = lang),
                )
            }));
            let total = all.len();
            all.truncate(picker::MAX_ROWS);
            picker.set_choices(&all, total);
        }
    };
    {
        let (fill, engines, search) = (fill.clone(), engines.clone(), picker.search.clone());
        spawn_worker(xkb::load_ibus_engines, move |list| {
            *engines.borrow_mut() = Some(list);
            fill(&search.text());
        });
    }
    picker
        .search
        .connect_search_changed(move |e| fill(&e.text()));
    let weak = Rc::downgrade(ui);
    picker.connect_pick(move |choice| {
        let Some((kind, id)) = choice.split_once('\t') else {
            return;
        };
        if let Some(ui) = weak.upgrade() {
            let mut s = read_sources(&ui.prefs);
            let entry = (kind.to_string(), id.to_string());
            if !s.contains(&entry) {
                s.push(entry);
                ui.write(s);
            }
        }
    });
    picker.present(parent);
}

/// Accessible name of an input source's row button, naming the source:
/// "Move English (US) Up", "Remove English (US)".
fn row_button_name(action: &str, source: &str) -> String {
    match action.strip_prefix("Move ") {
        Some(direction) => trf!(
            "Move {source} {direction}",
            source = source,
            direction = direction
        ),
        None => format!("{action} {source}"),
    }
}

#[cfg(test)]
mod tests {
    use super::row_button_name;

    #[test]
    fn row_buttons_name_their_source() {
        assert_eq!(
            row_button_name("Move Up", "English (US)"),
            "Move English (US) Up"
        );
        assert_eq!(row_button_name("Move Down", "German"), "Move German Down");
        assert_eq!(row_button_name("Remove", "German"), "Remove German");
    }
}
