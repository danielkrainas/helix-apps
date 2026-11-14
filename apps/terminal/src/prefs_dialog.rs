//! Preferences dialog.

use adw::prelude::*;
use gtk::pango;
use std::rc::Rc;

use crate::app::App;
use crate::palette::Theme;
use crate::transparency::Transparency;

pub fn show(app: &Rc<App>, parent: &impl IsA<gtk::Widget>) {
    let prefs = &app.prefs;
    let dialog = adw::PreferencesDialog::builder()
        .title("Preferences")
        .build();
    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    // Appearance.
    let appearance = adw::PreferencesGroup::builder().title("Appearance").build();
    let style = adw::ComboRow::builder()
        .title("Style")
        .model(&gtk::StringList::new(&["Follow System", "Light", "Dark"]))
        .selected(app.theme().combo_index())
        .build();
    let a = app.clone();
    style.connect_selected_notify(move |row| {
        let wanted = Theme::from_combo_index(row.selected());
        // Keeps "hacker" as long as the row stays on "Dark".
        if wanted.combo_index() != a.theme().combo_index() {
            a.set_theme(wanted);
        }
    });
    appearance.add(&style);

    let use_system = adw::SwitchRow::builder()
        .title("Use System Font")
        .active(prefs.get("use-system-font", true))
        .build();
    let font_button = gtk::FontDialogButton::builder()
        .dialog(&gtk::FontDialog::builder().title("Terminal Font").build())
        .level(gtk::FontLevel::Font)
        .use_font(true)
        .valign(gtk::Align::Center)
        .build();
    font_button.set_font_desc(&pango::FontDescription::from_string(
        &prefs.get("custom-font", "Monospace 11".to_string()),
    ));
    let font_row = adw::ActionRow::builder().title("Custom Font").build();
    font_row.add_suffix(&font_button);
    font_row.set_activatable_widget(Some(&font_button));
    font_row.set_sensitive(!use_system.is_active());
    let a = app.clone();
    let fr = font_row.clone();
    use_system.connect_active_notify(move |row| {
        a.prefs.set("use-system-font", row.is_active());
        fr.set_sensitive(!row.is_active());
        a.refresh_all();
    });
    let a = app.clone();
    font_button.connect_font_desc_notify(move |b| {
        if let Some(desc) = b.font_desc() {
            a.prefs.set("custom-font", desc.to_string());
            a.refresh_all();
        }
    });
    appearance.add(&use_system);
    appearance.add(&font_row);

    // Transparency (org.helix.Terminal, GNOME Terminal key names).
    let current = app.transparency();
    let transparent = adw::SwitchRow::builder()
        .title("Transparent Background")
        .active(current.enabled)
        .build();
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_value(current.percent as f64);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_digits(0);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    scale.set_format_value_func(|_, v| format!("{v:.0} %"));
    helix_app_common::set_accessible_label(&scale, "Transparency");
    let amount_row = adw::ActionRow::builder().title("Transparency").build();
    amount_row.add_suffix(&scale);
    amount_row.set_sensitive(current.enabled);
    let a = app.clone();
    let ar = amount_row.clone();
    transparent.connect_active_notify(move |row| {
        Transparency::set_enabled(&a.helix_prefs, row.is_active());
        ar.set_sensitive(row.is_active());
        a.refresh_all();
    });
    let a = app.clone();
    scale.connect_value_changed(move |s| {
        let percent = s.value().round() as i32;
        if percent != a.transparency().percent {
            Transparency::set_percent(&a.helix_prefs, percent);
            a.refresh_all();
        }
    });
    appearance.add(&transparent);
    appearance.add(&amount_row);
    page.add(&appearance);

    // Behaviour.
    let behaviour = adw::PreferencesGroup::builder().title("Behaviour").build();
    // org.gnome.Console has no separate "unlimited" key: a negative
    // `scrollback-lines` means unlimited.
    let stored: i64 = prefs.get("scrollback-lines", 10000i64);
    let unlimited = adw::SwitchRow::builder()
        .title("Unlimited Scrollback")
        .subtitle("Keep all terminal output in memory")
        .active(stored < 0)
        .build();
    let lines = adw::SpinRow::with_range(0.0, 1_000_000.0, 1000.0);
    lines.set_title("Scrollback Lines");
    lines.set_value(if stored < 0 { 10000.0 } else { stored as f64 });
    lines.set_sensitive(!unlimited.is_active());
    let a = app.clone();
    let lr = lines.clone();
    unlimited.connect_active_notify(move |row| {
        let v = if row.is_active() {
            -1
        } else {
            lr.value() as i64
        };
        a.prefs.set("scrollback-lines", v);
        lr.set_sensitive(!row.is_active());
        a.refresh_all();
    });
    let a = app.clone();
    let ur = unlimited.clone();
    lines.connect_value_notify(move |row| {
        if !ur.is_active() {
            a.prefs.set("scrollback-lines", row.value() as i64);
            a.refresh_all();
        }
    });
    behaviour.add(&unlimited);
    behaviour.add(&lines);

    for (key, title, default) in [
        ("audible-bell", "Audible Bell", true),
        ("visual-bell", "Visual Bell", true),
        ("restore-window-size", "Restore Window Size", true),
    ] {
        let row = adw::SwitchRow::builder()
            .title(title)
            .active(prefs.get(key, default))
            .build();
        let a = app.clone();
        row.connect_active_notify(move |row| {
            a.prefs.set(key, row.is_active());
            a.refresh_all();
        });
        behaviour.add(&row);
    }
    page.add(&behaviour);

    dialog.add(&page);
    dialog.present(Some(parent));
}
