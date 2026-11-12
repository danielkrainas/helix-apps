//! Rows bound to GSettings keys through `Prefs`.

use adw::prelude::*;
use gtk::glib;
use helix_app_common::Prefs;
use std::cell::Cell;
use std::rc::Rc;

use crate::values::{self, Choice};

pub const UNSUPPORTED: &str = "Not supported by this system";

/// Whether `key` is stored in an installed schema (vs. memory only).
pub fn has_key(prefs: &Prefs, key: &str) -> bool {
    prefs.gsettings().and_then(|s| s.settings_schema()).is_some_and(|s| s.has_key(key))
}

/// Calls `f` whenever `key` changes in GSettings (no-op for memory keys).
pub fn watch(prefs: &Prefs, key: &str, f: impl Fn() + 'static) {
    if has_key(prefs, key) {
        if let Some(s) = prefs.gsettings() {
            s.connect_changed(Some(key), move |_, _| f());
        }
    }
}

fn mark(row: &impl IsA<adw::ActionRow>, prefs: &Prefs, key: &str) {
    if !has_key(prefs, key) {
        row.as_ref().set_subtitle(UNSUPPORTED);
    }
}

pub fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(glib::markup_escape_text(title)).build()
}

/// Switch bound to a boolean key; `invert` shows the negated value.
pub fn switch(prefs: &Prefs, key: &'static str, default: bool, invert: bool, title: &str, subtitle: &str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().use_markup(false).title(title).build();
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    mark(&row, prefs, key);
    row.set_active(prefs.get::<bool>(key, default) != invert);
    row.connect_active_notify(glib::clone!(
        #[strong]
        prefs,
        move |r| {
            let v = r.is_active() != invert;
            if prefs.get::<bool>(key, default) != v {
                prefs.set(key, v);
            }
        }
    ));
    let weak = row.downgrade();
    let p = prefs.clone();
    watch(prefs, key, move || {
        if let Some(r) = weak.upgrade() {
            r.set_active(p.get::<bool>(key, default) != invert);
        }
    });
    row
}

fn string_list(labels: impl IntoIterator<Item = String>) -> gtk::StringList {
    let list = gtk::StringList::new(&[]);
    for l in labels {
        list.append(&l);
    }
    list
}

/// Switch mapping a string/enum key to two nicks (`on`, `off`).
pub fn nick_switch(prefs: &Prefs, key: &'static str, (on, off): (&'static str, &'static str), default: &'static str, title: &str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().use_markup(false).title(title).build();
    row.set_active(prefs.get_enum_nick(key, default) != off);
    row.connect_active_notify(glib::clone!(
        #[strong]
        prefs,
        move |r| {
            let nick = if r.is_active() { on } else { off };
            if prefs.get_enum_nick(key, default) != nick {
                prefs.set_enum_nick(key, nick);
            }
        }
    ));
    let (weak, p) = (row.downgrade(), prefs.clone());
    watch(prefs, key, move || {
        if let Some(r) = weak.upgrade() {
            r.set_active(p.get_enum_nick(key, default) != off);
        }
    });
    row
}

/// Combo over enum/string nicks.
pub fn nick_combo(prefs: &Prefs, key: &'static str, default: &'static str, choices: &'static [(&'static str, &'static str)], title: &str) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&string_list(choices.iter().map(|c| c.1.to_string())))
        .build();
    mark(&row, prefs, key);
    let index = move |p: &Prefs| {
        let nick = p.get_enum_nick(key, default);
        choices.iter().position(|c| c.0 == nick).unwrap_or(0) as u32
    };
    row.set_selected(index(prefs));
    row.connect_selected_notify(glib::clone!(
        #[strong]
        prefs,
        move |r| {
            if let Some(c) = choices.get(r.selected() as usize) {
                if prefs.get_enum_nick(key, default) != c.0 {
                    prefs.set_enum_nick(key, c.0);
                }
            }
        }
    ));
    let weak = row.downgrade();
    let p = prefs.clone();
    watch(prefs, key, move || {
        if let Some(r) = weak.upgrade() {
            r.set_selected(index(&p));
        }
    });
    row
}

/// Numeric GVariant type of a key.
#[derive(Clone, Copy, Debug)]
pub enum Num {
    U32,
    I32,
    F64,
}

pub fn get_num(prefs: &Prefs, key: &str, kind: Num, default: f64) -> f64 {
    match kind {
        Num::U32 => prefs.get::<u32>(key, default as u32) as f64,
        Num::I32 => prefs.get::<i32>(key, default as i32) as f64,
        Num::F64 => prefs.get::<f64>(key, default),
    }
}

pub fn set_num(prefs: &Prefs, key: &str, kind: Num, v: f64) {
    if (get_num(prefs, key, kind, f64::NAN) - v).abs() < 1e-9 {
        return;
    }
    match kind {
        Num::U32 => prefs.set(key, v.max(0.0).round() as u32),
        Num::I32 => prefs.set(key, v.round() as i32),
        Num::F64 => prefs.set(key, v),
    }
}

/// Combo over numeric presets; a non-preset stored value is shown as an
/// extra entry.
pub fn preset_combo(
    prefs: &Prefs,
    key: &'static str,
    kind: Num,
    default: i64,
    presets: fn() -> Vec<Choice<i64>>,
    custom_label: fn(i64) -> String,
    title: &str,
) -> adw::ComboRow {
    let row = adw::ComboRow::builder().use_markup(false).title(title).build();
    mark(&row, prefs, key);
    let choices: Rc<std::cell::RefCell<Vec<Choice<i64>>>> = Rc::default();
    let updating = Rc::new(Cell::new(false));
    let load = {
        let row = row.downgrade();
        let choices = choices.clone();
        let updating = updating.clone();
        let prefs = prefs.clone();
        move || {
            let Some(row) = row.upgrade() else { return };
            let current = get_num(&prefs, key, kind, default as f64) as i64;
            let (list, idx) = values::with_current(presets(), current, custom_label);
            updating.set(true);
            row.set_model(Some(&string_list(list.iter().map(|c| c.label.clone()))));
            row.set_selected(idx as u32);
            updating.set(false);
            *choices.borrow_mut() = list;
        }
    };
    load();
    row.connect_selected_notify(glib::clone!(
        #[strong]
        prefs,
        #[strong]
        choices,
        #[strong]
        updating,
        move |r| {
            if updating.get() {
                return;
            }
            if let Some(c) = choices.borrow().get(r.selected() as usize) {
                set_num(&prefs, key, kind, c.value as f64);
            }
        }
    ));
    watch(prefs, key, load);
    row
}

pub fn seconds_label(v: i64) -> String {
    values::format_duration(v.max(0) as u32)
}

pub fn days_label(v: i64) -> String {
    if v < 0 {
        "Forever".into()
    } else if v == 1 {
        "1 day".into()
    } else {
        format!("{v} days")
    }
}

/// Row with a horizontal slider. `to_ui`/`from_ui` convert between the
/// stored value and the slider position.
#[allow(clippy::too_many_arguments)]
pub fn scale_row(
    prefs: &Prefs,
    key: &'static str,
    kind: Num,
    default: f64,
    (min, max, step): (f64, f64, f64),
    to_ui: fn(f64) -> f64,
    from_ui: fn(f64) -> f64,
    title: &str,
    marks: Option<(&str, &str)>,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder().use_markup(false).title(title).build();
    mark(&row, prefs, key);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    scale.set_draw_value(false);
    scale.set_width_request(220);
    scale.update_property(&[gtk::accessible::Property::Label(title)]);
    if let Some((lo, hi)) = marks {
        scale.add_mark(min, gtk::PositionType::Bottom, Some(lo));
        scale.add_mark(max, gtk::PositionType::Bottom, Some(hi));
    }
    scale.set_value(to_ui(get_num(prefs, key, kind, default)));
    scale.connect_value_changed(glib::clone!(
        #[strong]
        prefs,
        move |s| set_num(&prefs, key, kind, from_ui(s.value()))
    ));
    let weak = scale.downgrade();
    let p = prefs.clone();
    watch(prefs, key, move || {
        if let Some(s) = weak.upgrade() {
            let v = to_ui(get_num(&p, key, kind, default));
            if (s.value() - v).abs() > 1e-6 {
                s.set_value(v);
            }
        }
    });
    row.add_suffix(&scale);
    row
}

pub fn identity(v: f64) -> f64 {
    v
}

/// Read-only value label for info rows (selectable).
pub fn value_label(value: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(value)
        .selectable(true)
        .wrap(true)
        .xalign(1.0)
        .justify(gtk::Justification::Right)
        .css_classes(["dim-label"])
        .build()
}

/// A scrolled preferences page.
pub fn page() -> adw::PreferencesPage {
    adw::PreferencesPage::new()
}
