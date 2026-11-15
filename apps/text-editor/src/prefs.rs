//! Preferences dialog.

use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

use crate::app::App;
use helix_app_common::i18n::tr;

fn switch_row(
    app: &Rc<App>,
    group: &adw::PreferencesGroup,
    title: &str,
    key: &'static str,
    value: bool,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(value).build();
    let w = Rc::downgrade(app);
    row.connect_active_notify(move |r| {
        if let Some(app) = w.upgrade() {
            app.set_pref(key, r.is_active());
        }
    });
    group.add(&row);
    row
}

fn spin_row(
    app: &Rc<App>,
    group: &adw::PreferencesGroup,
    title: &str,
    key: &'static str,
    value: u32,
    min: f64,
    max: f64,
) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, 1.0);
    row.set_title(title);
    row.set_value(value as f64);
    let w = Rc::downgrade(app);
    row.connect_value_notify(move |r| {
        if let Some(app) = w.upgrade() {
            app.set_pref(key, r.value() as u32);
        }
    });
    group.add(&row);
    row
}

pub fn show(app: &Rc<App>, parent: &impl IsA<gtk::Widget>) {
    let s = app.settings.borrow().clone();
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(tr("Preferences"));

    // Appearance.
    let page = adw::PreferencesPage::builder()
        .title(tr("Appearance"))
        .icon_name("applications-graphics-symbolic")
        .build();
    let style = adw::PreferencesGroup::builder().title(tr("Style")).build();
    let variants = [
        ("follow", tr("Follow System")),
        ("light", tr("Light")),
        ("dark", tr("Dark")),
    ];
    let combo = adw::ComboRow::builder()
        .title(tr("Color Scheme"))
        .model(&gtk::StringList::new(&variants.map(|v| v.1)))
        .selected(
            variants
                .iter()
                .position(|v| v.0 == s.style_variant)
                .unwrap_or(0) as u32,
        )
        .build();
    let w = Rc::downgrade(app);
    combo.connect_selected_notify(move |c| {
        if let Some(app) = w.upgrade() {
            app.set_pref_nick(
                "style-variant",
                variants[c.selected() as usize % variants.len()].0,
            );
        }
    });
    style.add(&combo);
    page.add(&style);

    let font = adw::PreferencesGroup::builder().title(tr("Font")).build();
    let system = switch_row(
        app,
        &font,
        tr("Use System Font"),
        "use-system-font",
        s.use_system_font,
    );
    let font_btn = gtk::FontDialogButton::new(Some(
        gtk::FontDialog::builder().title(tr("Editor Font")).build(),
    ));
    font_btn.set_font_desc(&gtk::pango::FontDescription::from_string(&s.custom_font));
    font_btn.set_valign(gtk::Align::Center);
    let w = Rc::downgrade(app);
    font_btn.connect_font_desc_notify(move |b| {
        if let (Some(app), Some(desc)) = (w.upgrade(), b.font_desc()) {
            app.set_pref("custom-font", desc.to_str().to_string());
        }
    });
    let font_row = adw::ActionRow::builder().title(tr("Custom Font")).build();
    font_row.add_suffix(&font_btn);
    font_row.set_activatable_widget(Some(&font_btn));
    system
        .bind_property("active", &font_row, "sensitive")
        .invert_boolean()
        .sync_create()
        .build();
    font.add(&font_row);
    page.add(&font);

    let display = adw::PreferencesGroup::builder()
        .title(tr("Display"))
        .build();
    switch_row(
        app,
        &display,
        tr("Show Line Numbers"),
        "show-line-numbers",
        s.show_line_numbers,
    );
    switch_row(
        app,
        &display,
        tr("Highlight Current Line"),
        "highlight-current-line",
        s.highlight_current_line,
    );
    let margin = switch_row(
        app,
        &display,
        tr("Show Right Margin"),
        "show-right-margin",
        s.show_right_margin,
    );
    let margin_pos = spin_row(
        app,
        &display,
        tr("Right Margin Position"),
        "right-margin-position",
        s.right_margin_position,
        1.0,
        1000.0,
    );
    margin
        .bind_property("active", &margin_pos, "sensitive")
        .sync_create()
        .build();
    switch_row(app, &display, tr("Wrap Text"), "wrap-text", s.wrap_text);
    page.add(&display);
    dialog.add(&page);

    // Editor.
    let page = adw::PreferencesPage::builder()
        .title(tr("Editor"))
        .icon_name("document-edit-symbolic")
        .build();
    let indent = adw::PreferencesGroup::builder()
        .title(tr("Indentation"))
        .build();
    spin_row(
        app,
        &indent,
        tr("Tab Width"),
        "tab-width",
        s.tab_width,
        1.0,
        32.0,
    );
    let spaces = adw::SwitchRow::builder()
        .title(tr("Insert Spaces Instead of Tabs"))
        .active(s.indent_spaces)
        .build();
    let w = Rc::downgrade(app);
    spaces.connect_active_notify(move |r| {
        if let Some(app) = w.upgrade() {
            app.set_pref_nick("indent-style", if r.is_active() { "space" } else { "tab" });
        }
    });
    indent.add(&spaces);
    switch_row(
        app,
        &indent,
        tr("Automatic Indentation"),
        "auto-indent",
        s.auto_indent,
    );
    page.add(&indent);

    let session = adw::PreferencesGroup::builder()
        .title(tr("Session"))
        .build();
    let restore = switch_row(
        app,
        &session,
        tr("Restore Session"),
        "restore-session",
        s.restore_session,
    );
    restore.set_subtitle(tr(
        "Reopen documents and unsaved changes from the previous session",
    ));
    page.add(&session);

    if !app.prefs.is_persistent() {
        let note = adw::PreferencesGroup::builder()
            .description(glib::markup_escape_text(
                "The org.gnome.TextEditor settings schema is not installed; changes last until the editor quits.",
            ))
            .build();
        page.add(&note);
    }
    dialog.add(&page);
    dialog.present(Some(parent));
}
