//! Preferences, History and bookmark import/export dialogs.

use adw::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

use crate::app::App;
use crate::session::RestorePolicy;
use crate::settings::{CookiePolicy, OVERVIEW};
use crate::store;

/// Shared rebuild callback for the search-engine list.
type Refresh = Rc<dyn Fn()>;

fn parent(app: &App) -> Option<gtk::Window> {
    app.gtk.active_window()
}

fn toast(app: &App, text: &str) {
    // Windows have no toast overlay; use a simple alert for feedback.
    if let Some(w) = parent(app) {
        let d = adw::AlertDialog::new(Some(text), None);
        d.add_response("ok", "_OK");
        d.present(Some(&w));
    }
}

// ---- preferences -----------------------------------------------------------

pub fn show_preferences(app: &Rc<App>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Preferences");
    let s = app.settings.borrow().clone();

    // General.
    let general = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder().title("Browsing").build();
    let homepage = adw::EntryRow::builder()
        .title("Homepage (empty for the start page)")
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
        .title("Restore Tabs on Startup")
        .model(&gtk::StringList::new(&["Always", "After a Crash", "Never"]))
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
    zoom.set_title("Default Zoom (%)");
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

    // Search.
    let search_group = adw::PreferencesGroup::builder().title("Search").build();
    let engines_row = adw::ComboRow::builder()
        .title("Default Search Engine")
        .build();
    search_group.add(&engines_row);
    let list_group = adw::PreferencesGroup::builder()
        .title("Search Engines")
        .description("Custom engines use a URL template where %s is replaced by the search terms.")
        .build();
    let add_name = adw::EntryRow::builder().title("New engine name").build();
    let add_url = adw::EntryRow::builder()
        .title("URL template, e.g. https://example.com/?q=%s")
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
                        .tooltip_text("Remove")
                        .valign(gtk::Align::Center)
                        .build();
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
            url_row.add_css_class("error");
        }
    });

    // Privacy.
    let privacy = adw::PreferencesPage::builder()
        .title("Privacy")
        .icon_name("security-high-symbolic")
        .build();
    let content = adw::PreferencesGroup::builder()
        .title("Web Content")
        .build();
    let js = adw::SwitchRow::builder()
        .title("Enable JavaScript")
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
        .title("Block Pop-up Windows")
        .subtitle("Windows opened by pages without a click")
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
    let cookies = adw::ComboRow::builder()
        .title("Cookies")
        .model(&gtk::StringList::new(&[
            "Always Accept",
            "Block Third-Party Cookies",
            "Block All Cookies",
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

    let data = adw::PreferencesGroup::builder()
        .title("Browsing Data")
        .build();
    let clear = adw::ActionRow::builder()
        .title("Clear Browsing Data…")
        .activatable(true)
        .build();
    clear.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    data.add(&clear);
    let perms = adw::ActionRow::builder()
        .title("Reset All Site Permissions")
        .subtitle("Forget every allow/deny decision")
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
    perms.connect_activated(move |_| {
        if let Some(app) = weak.upgrade() {
            app.permissions.borrow_mut().clear();
            app.save_permissions();
        }
    });

    dialog.add(&general);
    dialog.add(&privacy);
    if let Some(w) = parent(app) {
        dialog.present(Some(&w));
    }
}

fn show_clear_data(app: &Rc<App>, over: &impl IsA<gtk::Widget>) {
    let d = adw::AlertDialog::new(
        Some("Clear Browsing Data?"),
        Some("Data from all time will be removed."),
    );
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let site = gtk::CheckButton::builder()
        .label("Cookies and site data")
        .active(true)
        .build();
    let cache = gtk::CheckButton::builder()
        .label("Cached files")
        .active(true)
        .build();
    let history = gtk::CheckButton::builder()
        .label("Browsing history")
        .active(false)
        .build();
    list.append(&site);
    list.append(&cache);
    list.append(&history);
    d.set_extra_child(Some(&list));
    d.add_responses(&[("cancel", "_Cancel"), ("clear", "C_lear")]);
    d.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
    d.set_close_response("cancel");
    let weak = Rc::downgrade(app);
    d.connect_response(Some("clear"), move |_, _| {
        if let Some(app) = weak.upgrade() {
            app.clear_data(site.is_active(), cache.is_active(), history.is_active());
        }
    });
    d.present(Some(over));
}

// ---- history ---------------------------------------------------------------

pub fn show_history(app: &Rc<App>) {
    let dialog = adw::Dialog::builder()
        .title("History")
        .content_width(640)
        .content_height(560)
        .build();
    let header = adw::HeaderBar::new();
    let clear = gtk::Button::with_label("Clear All…");
    clear.add_css_class("destructive-action");
    header.pack_start(&clear);
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search history")
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("navigation-sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&search);
    content.append(&scroll);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&content));
    dialog.set_child(Some(&tv));

    let fill: Rc<dyn Fn()> = {
        let weak = Rc::downgrade(app);
        let list = list.clone();
        let search = search.clone();
        Rc::new(move || {
            let Some(app) = weak.upgrade() else { return };
            while let Some(r) = list.row_at_index(0) {
                list.remove(&r);
            }
            let entries = app.history.borrow().recent(&search.text(), 500);
            if entries.is_empty() {
                let l = gtk::Label::builder()
                    .label("No history")
                    .margin_top(24)
                    .build();
                l.add_css_class("dim-label");
                list.append(
                    &gtk::ListBoxRow::builder()
                        .child(&l)
                        .activatable(false)
                        .build(),
                );
                return;
            }
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
                let when = glib::DateTime::from_unix_local(e.last_visit)
                    .ok()
                    .and_then(|d| d.format("%x %X").ok())
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
                    .tooltip_text("Remove")
                    .valign(gtk::Align::Center)
                    .build();
                del.add_css_class("flat");
                row_box.append(&del);
                let row = gtk::ListBoxRow::builder()
                    .child(&row_box)
                    .name(e.url.as_str())
                    .build();
                let weak = Rc::downgrade(&app);
                let url = e.url.clone();
                let r = row.clone();
                let l = list.clone();
                del.connect_clicked(move |_| {
                    if let Some(app) = weak.upgrade() {
                        if app.history.borrow_mut().remove(&url) {
                            app.schedule_save(crate::app::Saved::History);
                        }
                    }
                    l.remove(&r);
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
            Some("Clear All History?"),
            Some("All pages you visited will be removed from history."),
        );
        confirm.add_responses(&[("cancel", "_Cancel"), ("clear", "C_lear")]);
        confirm.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        confirm.set_close_response("cancel");
        let weak = weak.clone();
        let f = f.clone();
        confirm.connect_response(Some("clear"), move |_, _| {
            if let Some(app) = weak.upgrade() {
                app.clear_data(false, false, true);
            }
            f();
        });
        confirm.present(Some(&dlg));
    });
    if let Some(w) = parent(app) {
        dialog.present(Some(&w));
    }
}

// ---- bookmarks import / export --------------------------------------------

fn html_filter() -> gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("HTML Bookmark Files"));
    filter.add_mime_type("text/html");
    filter.add_suffix("html");
    filter.add_suffix("htm");
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&filter);
    store
}

pub fn import_bookmarks(app: &Rc<App>) {
    let dialog = gtk::FileDialog::builder()
        .title("Import Bookmarks")
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
                            &format!("Imported {n} bookmark{}", if n == 1 { "" } else { "s" }),
                        );
                    }
                    Err(e) => toast(&app, &format!("Could not read file: {e}")),
                }
            });
        },
    );
}

pub fn export_bookmarks(app: &Rc<App>) {
    let dialog = gtk::FileDialog::builder()
        .title("Export Bookmarks")
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
                        toast(&app, &format!("Could not export bookmarks: {e}"));
                    }
                },
            );
        },
    );
}
