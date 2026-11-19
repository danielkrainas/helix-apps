//! Preferences, History and bookmark import/export dialogs.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use webkit6::prelude::*;

use crate::app::App;
use crate::secrets;
use crate::session::RestorePolicy;
use crate::settings::{CookiePolicy, OVERVIEW};
use crate::store;
use crate::webapp;
use crate::window::Window;
use helix_app_common::i18n::{n_, tr, trc};
use helix_app_common::trf;
use helix_app_common::trnf;

/// Shared rebuild callback for the search-engine list.
type Refresh = Rc<dyn Fn()>;

fn parent(app: &App) -> Option<gtk::Window> {
    app.gtk.active_window()
}

fn toast(app: &App, text: &str) {
    match parent(app) {
        Some(w) => app.toast_for(&w, text),
        None => app.toast_for(&gtk::Label::new(None), text),
    }
}

// ---- preferences -----------------------------------------------------------

pub fn show_preferences(app: &Rc<App>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(tr("Preferences"));
    let s = app.settings.borrow().clone();

    // General.
    let general = adw::PreferencesPage::builder()
        .title(tr("General"))
        .icon_name("preferences-system-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Browsing"))
        .build();
    let homepage = adw::EntryRow::builder()
        .title(tr("Homepage (empty for the start page)"))
        .show_apply_button(true)
        .build();
    homepage.set_text(s.home_url().as_deref().unwrap_or(""));
    let weak = Rc::downgrade(app);
    homepage.connect_apply(move |row| {
        if let Some(app) = weak.upgrade() {
            let text = row.text().trim().to_string();
            let value = if text.is_empty() {
                OVERVIEW.to_string()
            } else {
                app.resolve_input(&text).unwrap_or(text)
            };
            app.update_settings(|s| s.homepage = value);
        }
    });
    group.add(&homepage);

    let restore = adw::ComboRow::builder()
        .title(tr("Restore Tabs on Startup"))
        .model(&gtk::StringList::new(&[
            tr("Always"),
            tr("After a Crash"),
            tr("Never"),
        ]))
        .build();
    restore.set_selected(match s.restore_policy() {
        RestorePolicy::Always => 0,
        RestorePolicy::Crashed => 1,
        RestorePolicy::Never => 2,
    });
    let weak = Rc::downgrade(app);
    restore.connect_selected_notify(move |row| {
        if let Some(app) = weak.upgrade() {
            let nick = RestorePolicy::NICKS[row.selected().min(2) as usize];
            app.update_settings(|s| s.restore_session = nick.into());
        }
    });
    group.add(&restore);

    let zoom = adw::SpinRow::with_range(30.0, 300.0, 10.0);
    zoom.set_title(tr("Default Zoom (%)"));
    zoom.set_value((s.default_zoom * 100.0).round());
    let weak = Rc::downgrade(app);
    zoom.connect_value_notify(move |row| {
        if let Some(app) = weak.upgrade() {
            let v = row.value() / 100.0;
            app.update_settings(|s| s.default_zoom = v);
        }
    });
    group.add(&zoom);
    general.add(&group);

    // Reader mode (org.gnome.Epiphany.reader).
    let reader_group = adw::PreferencesGroup::builder()
        .title(tr("Reader Mode"))
        .build();
    let (sans, serif) = (trc("font style", "Sans"), trc("font style", "Serif"));
    let font = adw::ComboRow::builder()
        .title(tr("Font Style"))
        .model(&gtk::StringList::new(&[sans.as_str(), serif.as_str()]))
        .build();
    font.set_selected(u32::from(s.reader_font == "serif"));
    let weak = Rc::downgrade(app);
    font.connect_selected_notify(move |row| {
        if let Some(app) = weak.upgrade() {
            let nick = if row.selected() == 1 { "serif" } else { "sans" };
            app.update_settings(|s| s.reader_font = nick.into());
        }
    });
    reader_group.add(&font);
    let colors = adw::ComboRow::builder()
        .title(tr("Color Scheme"))
        .subtitle(tr(
            "The dark scheme is always used when the system style is dark",
        ))
        .model(&gtk::StringList::new(&[tr("Light"), tr("Dark")]))
        .build();
    colors.set_selected(u32::from(s.reader_colors == "dark"));
    let weak = Rc::downgrade(app);
    colors.connect_selected_notify(move |row| {
        if let Some(app) = weak.upgrade() {
            let nick = if row.selected() == 1 { "dark" } else { "light" };
            app.update_settings(|s| s.reader_colors = nick.into());
        }
    });
    reader_group.add(&colors);
    general.add(&reader_group);

    // Search.
    let search_group = adw::PreferencesGroup::builder().title(tr("Search")).build();
    let engines_row = adw::ComboRow::builder()
        .title(tr("Default Search Engine"))
        .build();
    search_group.add(&engines_row);
    let list_group = adw::PreferencesGroup::builder()
        .title(tr("Search Engines"))
        .description(tr(
            "Custom engines use a URL template where %s is replaced by the search terms.",
        ))
        .build();
    let add_name = adw::EntryRow::builder()
        .title(tr("New engine name"))
        .build();
    let add_url = adw::EntryRow::builder()
        .title(tr("URL template, e.g. https://example.com/?q=%s"))
        .show_apply_button(true)
        .build();
    general.add(&search_group);
    general.add(&list_group);

    let engine_rows: Rc<std::cell::RefCell<Vec<adw::ActionRow>>> = Rc::default();
    // Self-reference for the delete buttons; released when the dialog closes.
    let cell: Rc<std::cell::RefCell<Option<Refresh>>> = Rc::default();
    let refresh: Rc<dyn Fn()> = {
        let weak = Rc::downgrade(app);
        let engines_row = engines_row.clone();
        let list_group = list_group.clone();
        let rows = engine_rows.clone();
        let add_name = add_name.clone();
        let add_url = add_url.clone();
        let me = Rc::downgrade(&cell);
        Rc::new(move || {
            let Some(app) = weak.upgrade() else { return };
            let s = app.settings.borrow().clone();
            let names: Vec<&str> = s.search_engines.iter().map(|e| e.name.as_str()).collect();
            engines_row.set_model(Some(&gtk::StringList::new(&names)));
            let idx = s
                .search_engines
                .iter()
                .position(|e| e.name == s.engine().name)
                .unwrap_or(0);
            engines_row.set_selected(idx as u32);
            for r in rows.borrow_mut().drain(..) {
                list_group.remove(&r);
            }
            list_group.remove(&add_name);
            list_group.remove(&add_url);
            for e in &s.search_engines {
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&e.name))
                    .subtitle(glib::markup_escape_text(&e.url))
                    .build();
                if s.search_engines.len() > 1 {
                    let del = gtk::Button::builder()
                        .icon_name("user-trash-symbolic")
                        .tooltip_text(tr("Remove"))
                        .valign(gtk::Align::Center)
                        .build()
                        .labelled(&trf!("Remove {}", e.name));
                    del.add_css_class("flat");
                    let name = e.name.clone();
                    let weak = Rc::downgrade(&app);
                    let me = me.clone();
                    del.connect_clicked(move |_| {
                        if let Some(app) = weak.upgrade() {
                            app.update_settings(|s| {
                                s.remove_engine(&name);
                            });
                        }
                        if let Some(f) = me.upgrade().and_then(|c| c.borrow().clone()) {
                            glib::idle_add_local_once(move || f());
                        }
                    });
                    row.add_suffix(&del);
                }
                list_group.add(&row);
                rows.borrow_mut().push(row);
            }
            list_group.add(&add_name);
            list_group.add(&add_url);
        })
    };
    cell.replace(Some(refresh.clone()));
    dialog.connect_closed(move |_| {
        cell.take();
    });
    refresh();
    let weak = Rc::downgrade(app);
    engines_row.connect_selected_notify(move |row| {
        let Some(app) = weak.upgrade() else { return };
        let Some(item) = row.selected_item().and_downcast::<gtk::StringObject>() else {
            return;
        };
        let name = item.string().to_string();
        if app.settings.borrow().default_search_engine != name {
            app.update_settings(|s| s.default_search_engine = name);
        }
    });
    let weak = Rc::downgrade(app);
    let r = refresh.clone();
    let name_row = add_name.clone();
    add_url.connect_apply(move |url_row| {
        let Some(app) = weak.upgrade() else { return };
        let mut ok = false;
        let (name, url) = (name_row.text().to_string(), url_row.text().to_string());
        app.update_settings(|s| ok = s.add_engine(&name, &url));
        if ok {
            name_row.set_text("");
            url_row.set_text("");
            let r = r.clone();
            glib::idle_add_local_once(move || r());
        } else {
            let problem = app
                .settings
                .borrow()
                .engine_problem(&name, &url)
                .unwrap_or(tr("This search engine cannot be added"));
            let bad_name = name.trim().is_empty() || problem.contains("name");
            if bad_name {
                helix_app_common::set_input_error(&name_row, None, Some(problem));
            } else {
                helix_app_common::set_input_error(url_row, None, Some(problem));
            }
            if let Some(d) = url_row
                .ancestor(adw::PreferencesDialog::static_type())
                .and_downcast::<adw::PreferencesDialog>()
            {
                d.add_toast(adw::Toast::new(problem));
            }
        }
    });
    helix_app_common::clear_input_error_on_edit(&add_url, None);
    helix_app_common::clear_input_error_on_edit(&add_name, None);

    // Privacy.
    let privacy = adw::PreferencesPage::builder()
        .title(tr("Privacy"))
        .icon_name("security-high-symbolic")
        .build();
    let content = adw::PreferencesGroup::builder()
        .title(tr("Web Content"))
        .build();
    let js = adw::SwitchRow::builder()
        .title(tr("Enable JavaScript"))
        .active(s.javascript)
        .build();
    let weak = Rc::downgrade(app);
    js.connect_active_notify(move |r| {
        if let Some(app) = weak.upgrade() {
            let v = r.is_active();
            app.update_settings(|s| s.javascript = v);
        }
    });
    content.add(&js);
    let popups = adw::SwitchRow::builder()
        .title(tr("Block Pop-up Windows"))
        .subtitle(tr("Windows opened by pages without a click"))
        .active(!s.enable_popups)
        .build();
    let weak = Rc::downgrade(app);
    popups.connect_active_notify(move |r| {
        if let Some(app) = weak.upgrade() {
            let v = !r.is_active();
            app.update_settings(|s| s.enable_popups = v);
        }
    });
    content.add(&popups);
    let ask = adw::SwitchRow::builder()
        .title(tr("Ask Before Downloading"))
        .subtitle(tr("Files a page saves without a click"))
        .active(s.ask_before_downloads)
        .build();
    let weak = Rc::downgrade(app);
    ask.connect_active_notify(move |r| {
        if let Some(app) = weak.upgrade() {
            let v = r.is_active();
            app.update_settings(|s| s.ask_before_downloads = v);
        }
    });
    content.add(&ask);
    let cookies = adw::ComboRow::builder()
        .title(tr("Cookies"))
        .model(&gtk::StringList::new(&[
            tr("Always Accept"),
            tr("Block Third-Party Cookies"),
            tr("Block All Cookies"),
        ]))
        .build();
    cookies.set_selected(match s.cookie_policy() {
        CookiePolicy::Always => 0,
        CookiePolicy::NoThirdParty => 1,
        CookiePolicy::Never => 2,
    });
    let weak = Rc::downgrade(app);
    cookies.connect_selected_notify(move |row| {
        if let Some(app) = weak.upgrade() {
            let nick = CookiePolicy::NICKS[row.selected().min(2) as usize];
            app.update_settings(|s| s.cookies_policy = nick.into());
        }
    });
    content.add(&cookies);
    privacy.add(&content);

    let pw_group = adw::PreferencesGroup::builder()
        .title(tr("Passwords"))
        .description(tr("Passwords are kept in your keyring (Secret Service)."))
        .build();
    let remember = adw::SwitchRow::builder()
        .title(tr("Remember Passwords"))
        .subtitle(tr("Offer to save site logins and fill them in"))
        .active(s.remember_passwords)
        .build();
    let weak = Rc::downgrade(app);
    remember.connect_active_notify(move |r| {
        if let Some(app) = weak.upgrade() {
            let v = r.is_active();
            app.update_settings(|s| s.remember_passwords = v);
        }
    });
    pw_group.add(&remember);
    let saved = adw::ActionRow::builder()
        .title(tr("Saved Passwords…"))
        .activatable(true)
        .build();
    saved.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let weak = Rc::downgrade(app);
    saved.connect_activated(move |_| {
        if let Some(app) = weak.upgrade() {
            show_passwords(&app);
        }
    });
    pw_group.add(&saved);
    privacy.add(&pw_group);

    let data = adw::PreferencesGroup::builder()
        .title(tr("Browsing Data"))
        .build();
    let clear = adw::ActionRow::builder()
        .title(tr("Clear Browsing Data…"))
        .activatable(true)
        .build();
    clear.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    data.add(&clear);
    let perms = adw::ActionRow::builder()
        .title(tr("Reset All Site Permissions"))
        .subtitle(tr("Forget every allow/deny decision"))
        .activatable(true)
        .build();
    data.add(&perms);
    privacy.add(&data);
    let weak = Rc::downgrade(app);
    let dlg = dialog.clone();
    clear.connect_activated(move |_| {
        if let Some(app) = weak.upgrade() {
            show_clear_data(&app, &dlg);
        }
    });
    let weak = Rc::downgrade(app);
    let dlg = dialog.clone();
    perms.connect_activated(move |_| {
        if let Some(app) = weak.upgrade() {
            confirm_reset_permissions(&app, &dlg);
        }
    });

    dialog.add(&general);
    dialog.add(&privacy);
    if let Some(w) = parent(app) {
        dialog.present(Some(&w));
    }
}

/// Asks before forgetting every site permission decision; reports the
/// outcome in the Preferences dialog.
fn confirm_reset_permissions(app: &Rc<App>, prefs: &adw::PreferencesDialog) {
    if app.permissions.borrow().is_empty() {
        prefs.add_toast(adw::Toast::new(tr("No site permissions to reset")));
        return;
    }
    let d = adw::AlertDialog::new(
        Some(tr("Reset All Site Permissions?")),
        Some(tr("Every site will ask again before using your camera, location, notifications and other permissions.")),
    );
    d.add_responses(&[("cancel", tr("_Cancel")), ("reset", tr("_Reset"))]);
    d.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    let weak = Rc::downgrade(app);
    let p = prefs.clone();
    d.connect_response(Some("reset"), move |_, _| {
        if let Some(app) = weak.upgrade() {
            app.permissions.borrow_mut().clear();
            app.save_permissions();
            p.add_toast(adw::Toast::new(tr("Site permissions reset")));
        }
    });
    d.present(Some(prefs));
}

fn show_clear_data(app: &Rc<App>, over: &adw::PreferencesDialog) {
    let d = adw::AlertDialog::new(
        Some(tr("Clear Browsing Data?")),
        Some(tr("Data from all time will be removed.")),
    );
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let site = gtk::CheckButton::builder()
        .label(tr("Cookies and site data"))
        .active(true)
        .build();
    let cache = gtk::CheckButton::builder()
        .label(tr("Cached files"))
        .active(true)
        .build();
    let history = gtk::CheckButton::builder()
        .label(tr("Browsing history"))
        .active(false)
        .build();
    list.append(&site);
    list.append(&cache);
    list.append(&history);
    d.set_extra_child(Some(&list));
    d.add_responses(&[("cancel", tr("_Cancel")), ("clear", tr("C_lear"))]);
    d.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
    d.set_close_response("cancel");
    let weak = Rc::downgrade(app);
    let prefs = over.downgrade();
    d.connect_response(Some("clear"), move |_, _| {
        if let Some(app) = weak.upgrade() {
            let (prefs, app_w) = (prefs.clone(), Rc::downgrade(&app));
            app.clear_data(
                site.is_active(),
                cache.is_active(),
                history.is_active(),
                move |r| {
                    let text = match r {
                        Ok(()) => tr("Browsing data cleared").to_string(),
                        Err(e) => trf!("Could not clear browsing data: {e}", e = e),
                    };
                    match prefs.upgrade() {
                        Some(p) if p.parent().is_some() => p.add_toast(adw::Toast::new(&text)),
                        _ => {
                            if let Some(app) = app_w.upgrade() {
                                toast(&app, &text);
                            }
                        }
                    }
                },
            );
        }
    });
    d.present(Some(over));
}

// ---- history ---------------------------------------------------------------

pub fn show_history(app: &Rc<App>) {
    let dialog = adw::Dialog::builder()
        .title(tr("History"))
        .content_width(640)
        .content_height(560)
        .build();
    let header = adw::HeaderBar::new();
    let clear = gtk::Button::with_label(tr("Clear All…"));
    clear.add_css_class("destructive-action");
    header.pack_start(&clear);
    let search = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search history"))
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    helix_app_common::set_accessible_label(&search, tr("Search History"));
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("navigation-sidebar");
    // Day of each row (by URL, which is unique in history), for headers.
    let days: Rc<RefCell<HashMap<String, String>>> = Rc::default();
    {
        let days = days.clone();
        list.set_header_func(move |row, before| {
            let days = days.borrow();
            let day = days.get(row.widget_name().as_str());
            let prev = before.and_then(|b| days.get(b.widget_name().as_str()));
            match day.filter(|d| Some(*d) != prev) {
                Some(d) => {
                    let l = gtk::Label::builder()
                        .label(d)
                        .xalign(0.0)
                        .margin_start(12)
                        .margin_top(if before.is_some() { 12 } else { 6 })
                        .margin_bottom(6)
                        .build();
                    l.add_css_class("heading");
                    row.set_header(Some(&l));
                }
                None => row.set_header(None::<&gtk::Widget>),
            }
        });
    }
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    let status = adw::StatusPage::builder().vexpand(true).build();
    let stack = gtk::Stack::new();
    stack.add_named(&scroll, Some("list"));
    stack.add_named(&status, Some("empty"));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&search);
    content.append(&stack);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&content));
    dialog.set_child(Some(&tv));

    let fill: Rc<dyn Fn()> = {
        let weak = Rc::downgrade(app);
        let list = list.clone();
        let search = search.clone();
        let (status, stack, days) = (status.clone(), stack.clone(), days.clone());
        Rc::new(move || {
            let Some(app) = weak.upgrade() else { return };
            while let Some(r) = list.row_at_index(0) {
                list.remove(&r);
            }
            let query = search.text();
            let entries = app.history.borrow().recent(&query, 500);
            if entries.is_empty() {
                let (icon, title, description) = history_empty_state(&query);
                status.set_icon_name(Some(icon));
                status.set_title(title);
                status.set_description(Some(description));
                stack.set_visible_child_name("empty");
                if !query.is_empty() {
                    helix_app_common::announce_settled(&status, title);
                }
                return;
            }
            stack.set_visible_child_name("list");
            if !query.is_empty() {
                helix_app_common::announce_settled(
                    &list,
                    &crate::window::result_count(entries.len()),
                );
            }
            let today = glib::DateTime::now_local()
                .ok()
                .map(|d| days_from_civil(d.year(), d.month(), d.day_of_month()));
            let mut day_of = days.borrow_mut();
            day_of.clear();
            for e in &entries {
                let visit = glib::DateTime::from_unix_local(e.last_visit).ok();
                let heading = match (&visit, today) {
                    (Some(v), Some(today)) => {
                        let ago = today - days_from_civil(v.year(), v.month(), v.day_of_month());
                        match day_heading(ago) {
                            DayHeading::Today => tr("Today").to_string(),
                            DayHeading::Yesterday => tr("Yesterday").to_string(),
                            DayHeading::Weekday => {
                                v.format("%A").map(|s| s.to_string()).unwrap_or_default()
                            }
                            DayHeading::Date => {
                                v.format("%x").map(|s| s.to_string()).unwrap_or_default()
                            }
                        }
                    }
                    _ => String::new(),
                };
                day_of.insert(e.url.clone(), heading);
            }
            drop(day_of);
            for e in entries {
                let row_box = gtk::Box::builder()
                    .spacing(10)
                    .margin_start(6)
                    .margin_end(6)
                    .margin_top(4)
                    .margin_bottom(4)
                    .build();
                let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
                text.set_hexpand(true);
                let title = if e.title.is_empty() {
                    e.url.clone()
                } else {
                    e.title.clone()
                };
                text.append(
                    &gtk::Label::builder()
                        .label(&title)
                        .xalign(0.0)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .build(),
                );
                // The day is in the group header; rows show the time.
                let when = glib::DateTime::from_unix_local(e.last_visit)
                    .ok()
                    .and_then(|d| d.format("%X").ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let sub = gtk::Label::builder()
                    .label(format!("{when} · {}", e.url))
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build();
                sub.add_css_class("dim-label");
                sub.add_css_class("caption");
                text.append(&sub);
                row_box.append(&text);
                let del = gtk::Button::builder()
                    .icon_name("edit-delete-symbolic")
                    .tooltip_text(tr("Remove"))
                    .valign(gtk::Align::Center)
                    .build()
                    .labelled(&trf!("Remove {title}", title = title));
                helix_app_common::focus::set_part(&del, "remove");
                del.add_css_class("flat");
                row_box.append(&del);
                let row = gtk::ListBoxRow::builder()
                    .child(&row_box)
                    .name(e.url.as_str())
                    .build();
                // Headers are not focusable: say the day with the row.
                if let Some(day) = days.borrow().get(&e.url).filter(|d| !d.is_empty()) {
                    row.update_property(&[gtk::accessible::Property::Description(day)]);
                }
                let weak = Rc::downgrade(&app);
                let url = e.url.clone();
                let r = row.clone();
                let l = list.clone();
                let search = search.clone();
                del.connect_clicked(move |_| {
                    if let Some(app) = weak.upgrade() {
                        if app.history.borrow_mut().remove(&url) {
                            app.schedule_save(crate::app::Saved::History);
                        }
                    }
                    // Keep focus on the next entry's Remove button.
                    helix_app_common::focus::remove_row_keeping_focus(
                        &l,
                        &r,
                        Some("remove"),
                        Some(search.upcast_ref()),
                    );
                });
                list.append(&row);
            }
        })
    };
    fill();
    let f = fill.clone();
    search.connect_search_changed(move |_| f());
    let weak = Rc::downgrade(app);
    let dlg = dialog.clone();
    list.connect_row_activated(move |_, row| {
        let url = row.widget_name().to_string();
        let Some(app) = weak.upgrade() else { return };
        if url.is_empty() {
            return;
        }
        let w = app
            .active_window(false)
            .unwrap_or_else(|| app.new_window(false));
        w.open_uri(&url, true);
        w.win.present();
        dlg.close();
    });
    let weak = Rc::downgrade(app);
    let f = fill.clone();
    let dlg = dialog.clone();
    clear.connect_clicked(move |_| {
        let confirm = adw::AlertDialog::new(
            Some(tr("Clear All History?")),
            Some(tr("All pages you visited will be removed from history.")),
        );
        confirm.add_responses(&[("cancel", tr("_Cancel")), ("clear", tr("C_lear"))]);
        confirm.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        confirm.set_close_response("cancel");
        let weak = weak.clone();
        let f = f.clone();
        confirm.connect_response(Some("clear"), move |_, _| {
            if let Some(app) = weak.upgrade() {
                // History only: clearing is immediate and cannot fail.
                app.clear_data(false, false, true, |_| {});
            }
            f();
        });
        confirm.present(Some(&dlg));
    });
    if let Some(w) = parent(app) {
        dialog.present(Some(&w));
        dialog.set_focus(Some(&search));
    }
}

/// Icon, title and description of the history dialog with nothing to
/// list: no history at all, or none matching `query`.
fn history_empty_state(query: &str) -> (&'static str, &'static str, &'static str) {
    if query.trim().is_empty() {
        (
            "document-open-recent-symbolic",
            tr("No History"),
            tr("Pages you visit will appear here"),
        )
    } else {
        (
            "edit-find-symbolic",
            tr("No Results Found"),
            tr("Try a different search"),
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DayHeading {
    Today,
    Yesterday,
    /// Within the last week: the weekday's name.
    Weekday,
    /// Older (or in the future, for a changed clock): the date.
    Date,
}

/// How a history group `days_ago` days old is headed.
fn day_heading(days_ago: i64) -> DayHeading {
    match days_ago {
        0 => DayHeading::Today,
        1 => DayHeading::Yesterday,
        2..=6 => DayHeading::Weekday,
        _ => DayHeading::Date,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date, so calendar days
/// can be subtracted across month and year ends.
fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let (y, m, d) = (i64::from(year), i64::from(month), i64::from(day));
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---- bookmarks import / export --------------------------------------------

fn html_filter() -> gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(tr("HTML Bookmark Files")));
    filter.add_mime_type("text/html");
    filter.add_suffix("html");
    filter.add_suffix("htm");
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&filter);
    store
}

pub fn import_bookmarks(app: &Rc<App>) {
    let dialog = gtk::FileDialog::builder()
        .title(tr("Import Bookmarks"))
        .filters(&html_filter())
        .modal(true)
        .build();
    let weak = Rc::downgrade(app);
    dialog.open(
        parent(app).as_ref(),
        None::<&gio::Cancellable>,
        move |res| {
            let Ok(file) = res else { return };
            let weak = weak.clone();
            file.load_contents_async(None::<&gio::Cancellable>, move |res| {
                let Some(app) = weak.upgrade() else { return };
                match res {
                    Ok((bytes, _)) => {
                        let html = String::from_utf8_lossy(&bytes);
                        let n = app.bookmarks.borrow_mut().import_html(&html, store::now());
                        app.bookmarks_changed();
                        toast(
                            &app,
                            &trnf!("Imported {n} bookmark", "Imported {n} bookmarks", n),
                        );
                    }
                    Err(e) => toast(&app, &trf!("Could not read file: {e}", e = e)),
                }
            });
        },
    );
}

pub fn export_bookmarks(app: &Rc<App>) {
    let dialog = gtk::FileDialog::builder()
        .title(tr("Export Bookmarks"))
        .filters(&html_filter())
        .initial_name("bookmarks.html")
        .modal(true)
        .build();
    let weak = Rc::downgrade(app);
    dialog.save(
        parent(app).as_ref(),
        None::<&gio::Cancellable>,
        move |res| {
            let Ok(file) = res else { return };
            let Some(app) = weak.upgrade() else { return };
            let html = app.bookmarks.borrow().export_html();
            let weak = weak.clone();
            file.replace_contents_async(
                html.into_bytes(),
                None,
                false,
                gio::FileCreateFlags::REPLACE_DESTINATION,
                None::<&gio::Cancellable>,
                move |res| {
                    if let (Err((_, e)), Some(app)) = (res, weak.upgrade()) {
                        toast(&app, &trf!("Could not export bookmarks: {e}", e = e));
                    }
                },
            );
        },
    );
}

// ---- saved passwords --------------------------------------------------------

/// Lists the Helix logins in the keyring, with per-entry removal. Passwords
/// themselves are never shown.
pub fn show_passwords(app: &Rc<App>) {
    let dialog = adw::Dialog::builder()
        .title(tr("Passwords"))
        .content_width(460)
        .content_height(520)
        .build();
    let group = adw::PreferencesGroup::new();
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let status = adw::StatusPage::builder()
        .icon_name("dialog-password-symbolic")
        .title(tr("Loading…"))
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&status, Some("status"));
    stack.add_named(&page, Some("list"));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&stack));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&toasts));
    dialog.set_child(Some(&view));
    if let Some(w) = parent(app) {
        dialog.present(Some(&w));
    }
    load_passwords(group, stack, status, toasts);
}

fn show_no_passwords(stack: &gtk::Stack, status: &adw::StatusPage) {
    status.set_title(tr("No Saved Passwords"));
    status.set_description(Some(tr("Logins you save while browsing appear here.")));
    stack.set_visible_child_name("status");
}

/// Asks before removing a saved login (the keyring cannot give it back),
/// then removes it, reporting the outcome in the Passwords dialog.
struct PasswordList {
    group: adw::PreferencesGroup,
    stack: gtk::Stack,
    status: adw::StatusPage,
    toasts: adw::ToastOverlay,
    rows: Cell<usize>,
}

impl PasswordList {
    fn confirm_remove(
        self: &Rc<Self>,
        row: &adw::ActionRow,
        button: &gtk::Button,
        origin: String,
        user: String,
        host: String,
    ) {
        let who = if user.is_empty() {
            trf!("the saved password for {host}", host = host)
        } else {
            trf!(
                "the saved password for {user} on {host}",
                user = user,
                host = host
            )
        };
        let d = adw::AlertDialog::new(
            Some(tr("Remove Password?")),
            Some(&trf!(
                "This removes {who} from your keyring. It cannot be undone.",
                who = who
            )),
        );
        d.add_responses(&[("cancel", tr("_Cancel")), ("remove", tr("_Remove"))]);
        d.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        d.set_default_response(Some("cancel"));
        d.set_close_response("cancel");
        let (me, row, b) = (self.clone(), row.clone(), button.clone());
        d.connect_response(Some("remove"), move |_, _| {
            me.remove(&row, &b, origin.clone(), user.clone(), host.clone());
        });
        d.present(Some(button));
    }

    fn remove(
        self: &Rc<Self>,
        row: &adw::ActionRow,
        button: &gtk::Button,
        origin: String,
        user: String,
        host: String,
    ) {
        button.set_sensitive(false);
        let (me, row, b) = (self.clone(), row.clone(), button.clone());
        glib::spawn_future_local(async move {
            let r = gio::spawn_blocking(move || {
                secrets::Client::session_bus().and_then(|c| c.delete(&origin, &user))
            })
            .await;
            match r {
                Ok(Ok(())) => {
                    me.group.remove(&row);
                    me.rows.set(me.rows.get().saturating_sub(1));
                    me.toasts.add_toast(adw::Toast::new(&trf!(
                        "Password for {host} removed",
                        host = host
                    )));
                    if me.rows.get() == 0 {
                        show_no_passwords(&me.stack, &me.status);
                    }
                }
                Ok(Err(e)) => {
                    b.set_sensitive(true);
                    me.toasts.add_toast(adw::Toast::new(&trf!(
                        "Could not remove the password: {e}",
                        e = e
                    )));
                }
                Err(_) => {
                    b.set_sensitive(true);
                    me.toasts
                        .add_toast(adw::Toast::new(tr("Could not remove the password")));
                }
            }
        });
    }
}

fn load_passwords(
    group: adw::PreferencesGroup,
    stack: gtk::Stack,
    status: adw::StatusPage,
    toasts: adw::ToastOverlay,
) {
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(|| secrets::Client::session_bus().and_then(|c| c.all_logins()))
            .await;
        let logins = match r {
            Ok(Ok(l)) => l,
            Ok(Err(e)) => {
                status.set_title(tr("Keyring Unavailable"));
                status.set_description(Some(&glib::markup_escape_text(&e.to_string())));
                stack.set_visible_child_name("status");
                return;
            }
            Err(_) => return,
        };
        if logins.is_empty() {
            show_no_passwords(&stack, &status);
            return;
        }
        let list = Rc::new(PasswordList {
            group: group.clone(),
            stack: stack.clone(),
            status: status.clone(),
            toasts,
            rows: Cell::new(logins.len()),
        });
        for l in logins {
            let host = l
                .origin
                .split_once("://")
                .map(|(_, h)| h)
                .unwrap_or(&l.origin)
                .to_string();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&host))
                .subtitle(glib::markup_escape_text(if l.username.is_empty() {
                    tr("(no user name)")
                } else {
                    &l.username
                }))
                .build();
            let del = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Remove"))
                .valign(gtk::Align::Center)
                .build()
                .labelled(&remove_password_name(&host, &l.username));
            del.add_css_class("flat");
            let (list, r2) = (list.clone(), row.clone());
            del.connect_clicked(move |b| {
                list.confirm_remove(&r2, b, l.origin.clone(), l.username.clone(), host.clone());
            });
            row.add_suffix(&del);
            group.add(&row);
        }
        stack.set_visible_child_name("list");
    });
}

// ---- web applications -------------------------------------------------------

fn message(over: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_response("ok", "_OK");
    d.present(Some(over));
}

/// PNG data for the launcher icon: the site's favicon, else the app icon.
fn launcher_icon(favicon: Option<gdk::Texture>) -> Option<Vec<u8>> {
    if let Some(t) = favicon {
        return Some(t.save_to_png_bytes().to_vec());
    }
    let display = gdk::Display::default()?;
    let icon = gtk::IconTheme::for_display(&display).lookup_icon(
        crate::app::APP_ID,
        &["web-browser"],
        128,
        1,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );
    let file = icon.file()?;
    let (bytes, _) = file.load_contents(gio::Cancellable::NONE).ok()?;
    Some(bytes.to_vec())
}

/// "Install Site as Web Application…" via the DynamicLauncher portal.
pub fn install_web_app(w: &Rc<Window>) {
    let Some(tab) = w.current() else { return };
    let uri = tab.uri();
    if !(uri.starts_with("https://") || uri.starts_with("http://")) {
        return;
    }
    let host = crate::urlinput::host_of(&uri).unwrap_or_default();
    let title = Some(tab.title())
        .filter(|t| !t.is_empty() && *t != uri)
        .unwrap_or(host);
    let win = w.win.clone();
    let Some(icon) = launcher_icon(tab.view.favicon()) else {
        message(
            &win,
            tr("Could Not Install Web Application"),
            tr("No icon is available for this site."),
        );
        return;
    };
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "helix-web".into());
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(move || webapp::install(&title, &uri, &icon, &exe)).await;
        match r {
            Ok(Ok((_, name))) => message(
                &win,
                tr("Web Application Installed"),
                &trf!(
                    "“{name}” can now be started from your applications.",
                    name = name
                ),
            ),
            Ok(Err(webapp::Error::Cancelled)) => {}
            Ok(Err(e)) => message(
                &win,
                tr("Could Not Install Web Application"),
                &e.to_string(),
            ),
            Err(_) => {}
        }
    });
}

/// Removes this web app's launcher (web-app mode only) and quits.
pub fn uninstall_web_app(app: &Rc<App>) {
    let Some(wa) = app.web_app.clone() else {
        return;
    };
    let Some(w) = parent(app) else { return };
    let d = adw::AlertDialog::new(
        Some(tr("Uninstall Web Application?")),
        Some(tr(
            "The launcher is removed. Its saved site data stays on disk.",
        )),
    );
    d.add_responses(&[("cancel", tr("_Cancel")), ("uninstall", tr("_Uninstall"))]);
    d.set_response_appearance("uninstall", adw::ResponseAppearance::Destructive);
    d.set_close_response("cancel");
    let gtk_app = app.gtk.clone();
    let w2 = w.clone();
    d.connect_response(Some("uninstall"), move |_, _| {
        let wa = wa.clone();
        let gtk_app = gtk_app.clone();
        let w2 = w2.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || webapp::uninstall(&wa)).await {
                Ok(Ok(())) => gtk_app.quit(),
                Ok(Err(e)) => message(&w2, tr("Could Not Uninstall"), &e.to_string()),
                Err(_) => {}
            }
        });
    });
    d.present(Some(&w));
}

// ---- web-app preferences ---------------------------------------------------

/// Accessible name of a saved password's Remove button:
/// "Remove password for jane on example.org".
fn remove_password_name(host: &str, user: &str) -> String {
    if user.is_empty() {
        trf!("Remove password for {host}", host = host)
    } else {
        trf!(
            "Remove password for {user} on {host}",
            user = user,
            host = host
        )
    }
}

/// Refills the extra-allowed-sites list from the app's web-app prefs.
fn fill_allowed_sites(list: &gtk::ListBox, app: &Rc<App>) {
    list.remove_all();
    let sites = app.web_app_prefs.borrow().additional_urls.clone();
    if sites.is_empty() {
        let row = adw::ActionRow::builder()
            .title(tr("No additional sites"))
            .activatable(false)
            .build();
        row.add_css_class("dim-label");
        list.append(&row);
    }
    for site in sites {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&site))
            .activatable(false)
            .build();
        let del = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(tr("Remove"))
            .valign(gtk::Align::Center)
            .build()
            .labelled(&trf!("Remove {site}", site = site));
        del.add_css_class("flat");
        let weak = Rc::downgrade(app);
        let list2 = list.clone();
        del.connect_clicked(move |_| {
            let Some(app) = weak.upgrade() else { return };
            app.web_app_prefs.borrow_mut().remove(&site);
            app.save_web_app_prefs();
            let list2 = list2.clone();
            // Not while the clicked button's row is being destroyed.
            glib::idle_add_local_once(move || fill_allowed_sites(&list2, &app));
        });
        row.add_suffix(&del);
        list.append(&row);
    }
}

/// Web Application Settings (web-app mode only): the extra allowed sites
/// whose links stay inside the web app.
pub fn show_web_app_preferences(app: &Rc<App>) {
    let Some(wa) = app.web_app.clone() else {
        return;
    };
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(tr("Web Application Settings"));
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Additional Allowed Sites"))
        .description(glib::markup_escape_text(&trf!(
            "Links to these sites open in this web application instead of the browser, \
             like links within {host}. Write *.example.com to include all subdomains.",
            host = crate::urlinput::host_of(&wa.url).unwrap_or_else(|| wa.url.clone())
        )))
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("boxed-list");
    group.add(&list);
    let add_group = adw::PreferencesGroup::new();
    let add = adw::EntryRow::builder()
        .title(tr("Add site, e.g. accounts.example.com"))
        .show_apply_button(true)
        .build();
    add_group.add(&add);
    page.add(&group);
    page.add(&add_group);
    dialog.add(&page);
    fill_allowed_sites(&list, app);

    helix_app_common::clear_input_error_on_edit(&add, None);
    let weak = Rc::downgrade(app);
    add.connect_apply(move |row| {
        let Some(app) = weak.upgrade() else { return };
        let added = app.web_app_prefs.borrow_mut().add(&row.text());
        match added {
            Some(_) => {
                app.save_web_app_prefs();
                row.set_text("");
                fill_allowed_sites(&list, &app);
            }
            None => {
                const PROBLEM: &str = n_("Enter a site address, such as example.org");
                helix_app_common::set_input_error(row, None, Some(tr(PROBLEM)));
                if let Some(d) = row
                    .ancestor(adw::PreferencesDialog::static_type())
                    .and_downcast::<adw::PreferencesDialog>()
                {
                    d.add_toast(adw::Toast::new(tr(PROBLEM)));
                }
            }
        }
    });
    dialog.present(parent(app).as_ref());
}
#[cfg(test)]
mod tests {
    use super::{
        day_heading, days_from_civil, history_empty_state, remove_password_name, DayHeading,
    };

    #[test]
    fn history_groups_by_calendar_day() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(
            days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 28),
            2
        );
        assert_eq!(
            days_from_civil(2026, 1, 1) - days_from_civil(2025, 12, 31),
            1
        );
        assert_eq!(day_heading(0), DayHeading::Today);
        assert_eq!(day_heading(1), DayHeading::Yesterday);
        assert_eq!(day_heading(6), DayHeading::Weekday);
        assert_eq!(day_heading(7), DayHeading::Date);
        assert_eq!(day_heading(-1), DayHeading::Date);
        assert_eq!(history_empty_state("").1, "No History");
        assert_eq!(history_empty_state("x").1, "No Results Found");
    }

    #[test]
    fn password_remove_names_its_login() {
        assert_eq!(
            remove_password_name("example.org", "jane"),
            "Remove password for jane on example.org"
        );
        assert_eq!(
            remove_password_name("example.org", ""),
            "Remove password for example.org"
        );
    }
}
