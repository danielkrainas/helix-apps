//! "Manage Calendars" dialog.

use adw::prelude::*;
use gtk::{gdk, glib};
use helix_eds::{source, Source, SourceKind};
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::backend::Cmd;
use crate::colors;
use crate::ui::Ui;

fn rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::BLUE)
}

fn hex(c: &gdk::RGBA) -> String {
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    colors::to_hex((to(c.red()), to(c.green()), to(c.blue())))
}

pub fn open(ui: &Rc<Ui>) {
    let dialog = adw::PreferencesDialog::builder().title("Calendars").build();
    let page = adw::PreferencesPage::new();
    let sources = ui.sources();

    // Group calendars by account.
    let mut groups: BTreeMap<String, Vec<&Source>> = BTreeMap::new();
    for s in sources.iter().filter(|s| s.is(SourceKind::Calendar) && s.enabled()) {
        groups.entry(source::account_label(s, &sources)).or_default().push(s);
    }
    for (account, mut cals) in groups {
        cals.sort_by_key(|s| s.display_name().to_lowercase());
        let group = adw::PreferencesGroup::builder().title(glib::markup_escape_text(&account).as_str()).build();
        for s in cals {
            group.add(&calendar_row(ui, &dialog, s));
        }
        page.add(&group);
    }

    // New local calendar.
    let new_group = adw::PreferencesGroup::builder()
        .title("New Calendar")
        .description("Calendars from online accounts (Google, Nextcloud, Microsoft…) are added in Settings → Online Accounts.")
        .build();
    let name = adw::EntryRow::builder().title("Calendar Name").build();
    let used: Vec<String> = sources.iter().filter_map(|s| s.color().map(str::to_string)).collect();
    let color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    color.set_rgba(&rgba(colors::next_free(&used)));
    color.set_valign(gtk::Align::Center);
    let add = gtk::Button::builder().label("Add").css_classes(["suggested-action"]).valign(gtk::Align::Center).sensitive(false).build();
    name.add_suffix(&color);
    name.add_suffix(&add);
    let a = add.clone();
    name.connect_changed(move |e| a.set_sensitive(!e.text().trim().is_empty()));
    let create = {
        let ui = ui.clone();
        let name = name.clone();
        let color = color.clone();
        let dialog = dialog.clone();
        move || {
            let n = name.text().trim().to_string();
            if n.is_empty() {
                return;
            }
            let uid = source::new_uid();
            let data = source::new_local_calendar(&n, &hex(&color.rgba()));
            ui.backend.send(Cmd::CreateSource { uid, data });
            dialog.add_toast(adw::Toast::new(&format!("Calendar “{n}” created")));
            name.set_text("");
        }
    };
    let c = create.clone();
    add.connect_clicked(move |_| c());
    name.connect_entry_activated(move |_| create());
    new_group.add(&name);
    page.add(&new_group);

    dialog.add(&page);
    dialog.present(Some(&ui.window));
}

fn calendar_row(ui: &Rc<Ui>, dialog: &adw::PreferencesDialog, s: &Source) -> gtk::Widget {
    let backend = s.backend_name(SourceKind::Calendar).unwrap_or("").to_string();
    let color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    color.set_rgba(&rgba(&colors::normalize(s.color())));
    color.set_valign(gtk::Align::Center);
    color.set_sensitive(s.writable);
    let shown = gtk::Switch::builder().active(s.selected()).valign(gtk::Align::Center).sensitive(s.writable).build();
    shown.set_tooltip_text(Some("Show in Calendar"));

    let row: gtk::Widget = if s.writable && backend == "local" {
        let r = adw::EntryRow::builder().title("Name").text(s.display_name()).show_apply_button(true).build();
        r.add_prefix(&color);
        r.add_suffix(&shown);
        let (ui2, src) = (ui.clone(), s.clone());
        r.connect_apply(move |e| {
            let n = e.text().trim().to_string();
            if !n.is_empty() {
                let mut s = src.clone();
                s.set_display_name(&n);
                ui2.backend.send(Cmd::WriteSource(Box::new(s)));
            }
        });
        if s.removable {
            r.add_suffix(&remove_button(ui, dialog, s));
        }
        r.upcast()
    } else {
        let r = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&s.display_name()).as_str())
            .subtitle(if backend.is_empty() { String::new() } else { backend.clone() })
            .build();
        r.add_prefix(&color);
        r.add_suffix(&shown);
        if s.removable {
            r.add_suffix(&remove_button(ui, dialog, s));
        }
        r.upcast()
    };

    let (ui2, src) = (ui.clone(), s.clone());
    color.connect_rgba_notify(move |b| {
        let mut s = src.clone();
        s.set_color(&hex(&b.rgba()));
        ui2.backend.send(Cmd::WriteSource(Box::new(s)));
    });
    let (ui2, src) = (ui.clone(), s.clone());
    shown.connect_active_notify(move |sw| {
        let mut s = src.clone();
        s.set_selected(sw.is_active());
        ui2.backend.send(Cmd::WriteSource(Box::new(s)));
    });
    row
}

fn remove_button(ui: &Rc<Ui>, dialog: &adw::PreferencesDialog, s: &Source) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Remove Calendar")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    let (ui, src, dialog) = (ui.clone(), s.clone(), dialog.clone());
    b.connect_clicked(move |b| {
        let alert = adw::AlertDialog::new(
            Some(&format!("Remove “{}”?", src.display_name())),
            Some("The calendar and all its events will be deleted. This cannot be undone."),
        );
        alert.add_response("cancel", "_Cancel");
        alert.add_response("remove", "_Remove");
        alert.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        alert.set_close_response("cancel");
        let (ui2, src2, dialog2, b2) = (ui.clone(), src.clone(), dialog.clone(), b.clone());
        alert.connect_response(Some("remove"), move |_, _| {
            ui2.backend.send(Cmd::RemoveSource(Box::new(src2.clone())));
            if let Some(row) = b2.ancestor(adw::PreferencesRow::static_type()) {
                row.set_visible(false);
            }
            dialog2.add_toast(adw::Toast::new("Calendar removed"));
        });
        alert.present(Some(&dialog));
    });
    b
}
