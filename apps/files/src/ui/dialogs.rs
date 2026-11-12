//! Dialogs: confirmation, naming, conflicts, Open With, preferences.

use adw::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

use super::Window;
use crate::naming;
use crate::ops::{Choice, Conflict, Resolution};

/// Destructive confirmation; `on_confirm` runs if accepted.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    accept: &str,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .default_response("cancel")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("accept", accept)]);
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Destructive);
    dialog.connect_response(Some("accept"), move |_, _| on_confirm());
    dialog.present(Some(parent));
}

/// Asks for a file name. `dir` (if known) is used to reject existing names.
pub fn name_dialog(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    accept: &str,
    initial: &str,
    is_dir: bool,
    dir: Option<gio::File>,
    on_accept: impl Fn(String) + 'static,
) {
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .width_chars(30)
        .build();
    let error = gtk::Label::builder().xalign(0.0).wrap(true).build();
    error.add_css_class("error");
    error.add_css_class("caption");
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&entry);
    b.append(&error);
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .extra_child(&b)
        .default_response("accept")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("accept", accept)]);
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Suggested);

    let initial = initial.to_string();
    let validate = {
        let dialog = dialog.clone();
        let error = error.clone();
        let initial = initial.clone();
        move |text: &str| {
            let msg = match naming::validate_name(text) {
                Err(_) if text.is_empty() => Some(String::new()),
                Err(e) => Some(e.to_string()),
                Ok(()) if text == initial => Some(String::new()),
                Ok(()) => match &dir {
                    Some(d) if d.child(text).query_exists(None::<&gio::Cancellable>) => Some(
                        if is_dir {
                            "A folder with that name already exists."
                        } else {
                            "A file with that name already exists."
                        }
                        .to_string(),
                    ),
                    _ => None,
                },
            };
            dialog.set_response_enabled("accept", msg.is_none());
            error.set_text(msg.as_deref().unwrap_or(""));
            error.set_visible(msg.as_ref().is_some_and(|m| !m.is_empty()));
            if text.starts_with('.') && msg.is_none() {
                error.set_text("Names starting with “.” are hidden.");
                error.remove_css_class("error");
                error.set_visible(true);
            } else {
                error.add_css_class("error");
            }
        }
    };
    validate(&initial);
    let v = validate.clone();
    entry.connect_changed(move |e| v(&e.text()));
    let e = entry.clone();
    dialog.connect_response(Some("accept"), move |_, _| on_accept(e.text().trim_end().to_string()));
    dialog.present(Some(parent));
    // Select the stem (not the extension) for renames.
    let (start, end) = naming::rename_selection(&initial, is_dir);
    glib::idle_add_local_once(move || {
        entry.grab_focus();
        entry.select_region(start, end);
    });
}

fn describe(info: Option<&gio::FileInfo>) -> String {
    let Some(i) = info else { return String::new() };
    let size = if i.file_type() == gio::FileType::Directory {
        String::new()
    } else {
        crate::fmt::size(i.size().max(0) as u64)
    };
    let date = i
        .modification_date_time()
        .and_then(|d| {
            glib::DateTime::now_local()
                .ok()
                .map(|now| crate::fmt::date(&d, &now, true))
        })
        .unwrap_or_default();
    match (size.is_empty(), date.is_empty()) {
        (false, false) => format!("{size}, last modified {date}"),
        (true, false) => format!("Last modified {date}"),
        (false, true) => size,
        _ => String::new(),
    }
}

/// Asks how to resolve a name conflict.
pub async fn conflict(parent: &adw::ApplicationWindow, c: &Conflict) -> Resolution {
    let attrs = "standard::type,standard::size,time::modified";
    let flags = gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS;
    let src = c
        .source
        .query_info_future(attrs, flags, glib::Priority::DEFAULT)
        .await
        .ok();
    let dst = c
        .dest
        .query_info_future(attrs, flags, glib::Priority::DEFAULT)
        .await
        .ok();
    let name = crate::ops::display_name(&c.dest);
    let folder = c
        .dest
        .parent()
        .map(|p| crate::ops::display_name(&p))
        .unwrap_or_default();
    let merge = c.source_is_dir && c.dest_is_dir;
    let heading = if merge {
        format!("Merge Folder “{name}”?")
    } else {
        format!("Replace “{name}”?")
    };
    let mut body = if merge {
        format!("A folder with the same name already exists in “{folder}”. Merging will ask before replacing any files in the folder that conflict with the files being copied.")
    } else {
        format!(
            "Another file with the same name already exists in “{folder}”. Replacing it will overwrite its content."
        )
    };
    let (o, n) = (describe(dst.as_ref()), describe(src.as_ref()));
    if !o.is_empty() {
        body.push_str(&format!("\n\nOriginal: {o}"));
    }
    if !n.is_empty() {
        body.push_str(&format!("\nReplace with: {n}"));
    }
    let all = gtk::CheckButton::with_label("Apply this action to all conflicts");
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .extra_child(&all)
        .default_response("replace")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[
        ("cancel", "_Cancel"),
        ("skip", "_Skip"),
        ("rename", "Keep _Both"),
        ("replace", if merge { "_Merge" } else { "_Replace" }),
    ]);
    dialog.set_response_appearance(
        "replace",
        if merge {
            adw::ResponseAppearance::Suggested
        } else {
            adw::ResponseAppearance::Destructive
        },
    );
    let response = dialog.choose_future(parent).await;
    let choice = match response.as_str() {
        "skip" => Choice::Skip,
        "rename" => Choice::KeepBoth,
        "replace" => Choice::Replace,
        _ => Choice::Cancel,
    };
    Resolution {
        choice,
        apply_to_all: all.is_active(),
    }
}

/// "Open With" chooser for the selected files.
pub fn open_with(win: &Rc<Window>, infos: Vec<gio::FileInfo>) {
    let files: Vec<gio::File> = infos.iter().filter_map(super::tab::info_target).collect();
    let Some(first) = infos.first() else { return };
    let ct = first
        .content_type()
        .or_else(|| first.attribute_string("standard::fast-content-type"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "application/octet-stream".into());
    let desc = gio::content_type_get_description(&ct);

    let mut apps: Vec<gio::AppInfo> = gio::AppInfo::recommended_for_type(&ct);
    for a in gio::AppInfo::all_for_type(&ct).into_iter().chain(gio::AppInfo::all()) {
        if a.should_show() && !apps.iter().any(|x| x.id() == a.id()) {
            apps.push(a);
        }
    }
    let default = gio::AppInfo::default_for_type(&ct, false);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    let recommended = gio::AppInfo::recommended_for_type(&ct).len();
    for (i, app) in apps.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(app.display_name().as_str())
            .activatable(true)
            .build();
        if let Some(icon) = app.icon() {
            let img = gtk::Image::from_gicon(&icon);
            img.set_pixel_size(32);
            row.add_prefix(&img);
        }
        if default.as_ref().is_some_and(|d| d.id() == app.id()) {
            row.set_subtitle("Default");
        } else if i >= recommended {
            row.set_subtitle("Other application");
        }
        list.append(&row);
    }
    if let Some(r) = list.row_at_index(0) {
        list.select_row(Some(&r));
    }
    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(320)
        .build();
    let always = gtk::CheckButton::with_label(&format!("Always use for “{desc}” files"));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(&scroll);
    content.append(&always);

    let header = adw::HeaderBar::new();
    let open_btn = gtk::Button::with_mnemonic("_Open");
    open_btn.add_css_class("suggested-action");
    header.pack_end(&open_btn);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title("Open With")
        .content_width(420)
        .content_height(520)
        .child(&tv)
        .build();

    let apps = Rc::new(apps);
    let launch = {
        let dialog = dialog.clone();
        let list = list.clone();
        let win = Rc::downgrade(win);
        let apps = apps.clone();
        move || {
            let Some(row) = list.selected_row() else { return };
            let Some(app) = apps.get(row.index() as usize) else {
                return;
            };
            if always.is_active() {
                let _ = app.set_as_default_for_type(&ct);
            }
            if let Some(w) = win.upgrade() {
                let ctx = gtk::prelude::WidgetExt::display(&w.window).app_launch_context();
                if let Err(e) = app.launch(&files, Some(&ctx)) {
                    w.toast(&format!("Could not open: {}", e.message()));
                } else {
                    for f in &files {
                        gtk::RecentManager::default().add_item(&f.uri());
                    }
                }
            }
            dialog.close();
        }
    };
    let l = Rc::new(launch);
    let l2 = l.clone();
    open_btn.connect_clicked(move |_| l2());
    list.connect_row_activated(move |_, _| l());
    dialog.present(Some(&win.window));
}

/// Preferences dialog bound to the Nautilus GSettings keys.
pub fn preferences(win: &Rc<Window>) {
    let shared = win.shared.clone();
    let dialog = adw::PreferencesDialog::new();
    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let combo = |title: &str,
                 subtitle: &str,
                 options: &[(&'static str, &'static str)],
                 current: &str,
                 apply: Box<dyn Fn(&str)>| {
        let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&labels))
            .build();
        if !subtitle.is_empty() {
            row.set_subtitle(subtitle);
        }
        row.set_selected(options.iter().position(|(n, _)| *n == current).unwrap_or(0) as u32);
        let opts: Vec<&'static str> = options.iter().map(|(n, _)| *n).collect();
        row.connect_selected_notify(move |r| {
            if let Some(n) = opts.get(r.selected() as usize) {
                apply(n);
            }
        });
        row
    };
    let switch = |title: &str, subtitle: &str, active: bool, apply: Box<dyn Fn(bool)>| {
        let row = adw::SwitchRow::builder().title(title).active(active).build();
        if !subtitle.is_empty() {
            row.set_subtitle(subtitle);
        }
        row.connect_active_notify(move |r| apply(r.is_active()));
        row
    };
    let enum_setter = |key: &'static str| -> Box<dyn Fn(&str)> {
        let s = shared.clone();
        Box::new(move |nick: &str| {
            s.prefs.set_enum_nick(key, nick);
            s.settings_changed();
        })
    };
    let bool_setter = |prefs: helix_app_common::Prefs, key: &'static str| -> Box<dyn Fn(bool)> {
        let s = shared.clone();
        Box::new(move |v: bool| {
            prefs.set(key, v);
            s.settings_changed();
        })
    };
    let p = &shared.prefs;

    let general = adw::PreferencesGroup::builder().title("General").build();
    general.add(&switch(
        "Sort Folders Before Files",
        "",
        shared.chooser_prefs.get("sort-directories-first", true),
        bool_setter(shared.chooser_prefs.clone(), "sort-directories-first"),
    ));
    general.add(&combo(
        "Open Action",
        "",
        &[("double", "Double Click"), ("single", "Single Click")],
        &p.get_enum_nick("click-policy", "double"),
        enum_setter("click-policy"),
    ));
    general.add(&switch(
        "Show Hidden Files",
        "",
        p.get("show-hidden-files", false),
        bool_setter(p.clone(), "show-hidden-files"),
    ));
    general.add(&switch(
        "Always Show the Location Entry",
        "Instead of the path bar",
        p.get("always-use-location-entry", false),
        bool_setter(p.clone(), "always-use-location-entry"),
    ));
    page.add(&general);

    let views = adw::PreferencesGroup::builder().title("Views").build();
    views.add(&combo(
        "Default View",
        "",
        &[("icon-view", "Grid"), ("list-view", "List")],
        &p.get_enum_nick("default-folder-viewer", "icon-view"),
        enum_setter("default-folder-viewer"),
    ));
    views.add(&combo(
        "Show Thumbnails",
        "",
        &[
            ("local-only", "On This Computer Only"),
            ("always", "All Files"),
            ("never", "Never"),
        ],
        &p.get_enum_nick("show-image-thumbnails", "local-only"),
        enum_setter("show-image-thumbnails"),
    ));
    views.add(&combo(
        "Date and Time Format",
        "",
        &[("simple", "Simple"), ("detailed", "Detailed")],
        &p.get_enum_nick("date-time-format", "simple"),
        enum_setter("date-time-format"),
    ));
    page.add(&views);

    let menu = adw::PreferencesGroup::builder().title("Context Menu").build();
    menu.add(&switch(
        "Create Link",
        "Show an action to create symbolic links",
        p.get("show-create-link", false),
        bool_setter(p.clone(), "show-create-link"),
    ));
    menu.add(&switch(
        "Delete Permanently",
        "Show an action to bypass the Trash",
        p.get("show-delete-permanently", false),
        bool_setter(p.clone(), "show-delete-permanently"),
    ));
    page.add(&menu);

    let search = adw::PreferencesGroup::builder().title("Search").build();
    search.add(&combo(
        "Search in Subfolders",
        "",
        &[
            ("local-only", "On This Computer Only"),
            ("always", "All Locations"),
            ("never", "Never"),
        ],
        &p.get_enum_nick("recursive-search", "local-only"),
        enum_setter("recursive-search"),
    ));
    page.add(&search);

    dialog.add(&page);
    dialog.present(Some(&win.window));
}
