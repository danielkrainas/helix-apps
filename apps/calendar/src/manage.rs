//! "Manage Calendars" dialog.

use adw::prelude::*;
use gtk::{gdk, glib};
use helix_app_common::Labelled;
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
    for s in sources
        .iter()
        .filter(|s| s.is(SourceKind::Calendar) && s.enabled())
    {
        groups
            .entry(source::account_label(s, &sources))
            .or_default()
            .push(s);
    }
    for (account, mut cals) in groups {
        cals.sort_by_key(|s| s.display_name().to_lowercase());
        let group = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(&account).as_str())
            .build();
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
    let used: Vec<String> = sources
        .iter()
        .filter_map(|s| s.color().map(str::to_string))
        .collect();
    let color =
        gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    color.set_rgba(&rgba(colors::next_free(&used)));
    color.set_valign(gtk::Align::Center);
    color.set_tooltip_text(Some("Calendar Colour"));
    helix_app_common::set_accessible_label(&color, "New Calendar Colour");
    let add = gtk::Button::builder()
        .label("Add")
        .css_classes(["suggested-action"])
        .valign(gtk::Align::Center)
        .sensitive(false)
        .build();
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
    page.add(&url_group(ui, &dialog, &used));

    dialog.add(&page);
    dialog.present(Some(&ui.window));
}

/// "New Calendar from URL": subscribes to an iCalendar feed through EDS'
/// `webcal` backend (read-only, refreshed by EDS).
fn url_group(
    ui: &Rc<Ui>,
    dialog: &adw::PreferencesDialog,
    used: &[String],
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("New Calendar from URL")
        .description("Subscribe to a published calendar (.ics). It is read-only and refreshed automatically.")
        .build();
    let url = adw::EntryRow::builder()
        .title("Calendar Address (https:// or webcal://)")
        .input_purpose(gtk::InputPurpose::Url)
        .build();
    let name = adw::EntryRow::builder().title("Calendar Name").build();
    let color =
        gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    color.set_rgba(&rgba(colors::next_free(used)));
    color.set_valign(gtk::Align::Center);
    color.set_tooltip_text(Some("Calendar Colour"));
    helix_app_common::set_accessible_label(&color, "Subscribed Calendar Colour");
    name.add_suffix(&color);
    let error = gtk::Label::builder()
        .css_classes(["error", "caption"])
        .xalign(0.0)
        .wrap(true)
        .margin_top(6)
        .visible(false)
        .build();
    let add = gtk::Button::builder()
        .label("Subscribe")
        .css_classes(["suggested-action", "pill"])
        .halign(gtk::Align::End)
        .margin_top(12)
        .sensitive(false)
        .build();
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.append(&url);
    list.append(&name);
    bx.append(&list);
    bx.append(&error);
    bx.append(&add);
    group.add(&bx);

    let clear_error = {
        let (error, url) = (error.clone(), url.clone());
        move || {
            error.set_visible(false);
            url.remove_css_class("error");
        }
    };
    {
        let (add, clear) = (add.clone(), clear_error.clone());
        url.connect_changed(move |e| {
            add.set_sensitive(!e.text().trim().is_empty());
            clear();
        });
    }
    let submit = {
        let (ui, dialog) = (ui.clone(), dialog.clone());
        let (url, name, color, error) = (url.clone(), name.clone(), color.clone(), error.clone());
        move || {
            let parsed = match source::parse_webcal_url(&url.text()) {
                Ok(u) => u,
                Err(e) => {
                    error.set_label(&e.to_string());
                    error.set_visible(true);
                    url.add_css_class("error");
                    url.grab_focus();
                    return;
                }
            };
            let typed = name.text().trim().to_string();
            let n = if typed.is_empty() {
                manage_names::name_from_url(&parsed)
            } else {
                typed
            };
            let data = source::new_webcal_calendar(&n, &hex(&color.rgba()), &parsed);
            ui.backend.send(Cmd::CreateSource {
                uid: source::new_uid(),
                data,
            });
            dialog.add_toast(adw::Toast::new(&format!("Subscribed to “{n}”")));
            url.set_text("");
            name.set_text("");
            clear_error();
        }
    };
    let s = submit.clone();
    add.connect_clicked(move |_| s());
    let s = submit.clone();
    url.connect_entry_activated(move |_| s());
    name.connect_entry_activated(move |_| submit());
    group
}

/// Pure helpers for the manage dialog.
pub mod manage_names {
    use helix_eds::source::WebcalUrl;

    /// A default calendar name for a feed: the file name without `.ics`
    /// (`_`/`-` become spaces), else the host.
    pub fn name_from_url(url: &WebcalUrl) -> String {
        let file = url.path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
        let file = file
            .strip_suffix(".ics")
            .or_else(|| file.strip_suffix(".ICS"))
            .unwrap_or(file);
        let decoded = percent_decode(file).replace(['_', '-'], " ");
        let decoded = decoded.trim();
        if decoded.is_empty() {
            url.host.clone()
        } else {
            decoded.to_string()
        }
    }

    fn percent_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                let hex = |c: u8| (c as char).to_digit(16);
                if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use helix_eds::source::parse_webcal_url;

        #[test]
        fn names() {
            let n = |u: &str| name_from_url(&parse_webcal_url(u).unwrap());
            assert_eq!(
                n("webcal://example.org/feeds/public_holidays.ics"),
                "public holidays"
            );
            assert_eq!(
                n("https://example.org/My%20Team-Calendar.ics?x=1"),
                "My Team Calendar"
            );
            assert_eq!(n("https://cal.example.org/"), "cal.example.org");
            assert_eq!(n("https://example.org/a/%zz"), "%zz");
            assert_eq!(n("https://example.org/x%4"), "x%4");
        }
    }
}

fn calendar_row(ui: &Rc<Ui>, dialog: &adw::PreferencesDialog, s: &Source) -> gtk::Widget {
    let backend = s
        .backend_name(SourceKind::Calendar)
        .unwrap_or("")
        .to_string();
    let color =
        gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    color.set_rgba(&rgba(&colors::normalize(s.color())));
    color.set_valign(gtk::Align::Center);
    color.set_sensitive(s.writable);
    color.set_tooltip_text(Some("Calendar Colour"));
    let display = s.display_name();
    helix_app_common::set_accessible_label(&color, &format!("Colour of {display}"));
    let shown = gtk::Switch::builder()
        .active(s.selected())
        .valign(gtk::Align::Center)
        .sensitive(s.writable)
        .build();
    shown.set_tooltip_text(Some("Show in Calendar"));
    helix_app_common::set_accessible_label(&shown, &format!("Show {display}"));

    let row: gtk::Widget = if s.writable && backend == "local" {
        let r = adw::EntryRow::builder()
            .title("Name")
            .text(s.display_name())
            .show_apply_button(true)
            .build();
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
            .subtitle(glib::markup_escape_text(&match source::webcal_url(s) {
                Some(url) if backend == "webcal" => url,
                _ => backend.clone(),
            }))
            .subtitle_selectable(backend == "webcal")
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
        .build()
        .labelled(&format!("Remove Calendar {}", s.display_name()));
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
