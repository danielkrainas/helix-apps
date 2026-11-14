//! GTK user interface.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::category::Category;
use crate::format;
use crate::journal::{self, Boot, Entry, QueryArgs};
use crate::live::{self, Live, Placement};
use crate::search::{Search, SearchField};
use crate::worker;

pub const APP_ID: &str = "org.helix.Logs";
const SCHEMA: &str = "org.gnome.Logs";
const BATCH: usize = 200;
const SORT_DESC: &str = "descending-time";
const SORT_ASC: &str = "ascending-time";

struct State {
    prefs: Prefs,
    uid: u32,
    boots: Vec<Boot>,
    category: Category,
    search_text: String,
    search_field: SearchField,
    descending: bool,
    handle: Option<worker::Handle>,
    /// Live `journalctl --follow` of the current boot.
    follow: Option<worker::Follow>,
    live: Live,
    generation: u64,
    loading: bool,
    done: bool,
}

struct Ui {
    window: adw::ApplicationWindow,
    store: gio::ListStore,
    list: gtk::ListView,
    scrolled: gtk::ScrolledWindow,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    spinner: gtk::Spinner,
    banner: adw::Banner,
    nav: adw::NavigationView,
    list_page: adw::NavigationPage,
    split: adw::NavigationSplitView,
    boot_dropdown: gtk::DropDown,
    toasts: adw::ToastOverlay,
}

struct App {
    ui: Ui,
    state: RefCell<State>,
    /// Set while the boot model is replaced programmatically.
    suppress_boot_change: Cell<bool>,
}

#[derive(Serialize, Deserialize, Default)]
struct WindowState {
    width: i32,
    height: i32,
    maximized: bool,
}

fn window_state_path() -> std::path::PathBuf {
    helix_app_common::config_dir("helix-logs").join("window.json")
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(
        &app,
        "Logs",
        "View detailed event logs for the system",
    );
    app.connect_startup(|_| load_css());
    app.connect_activate(|app| {
        let window = build(app);
        window.present();
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".log-row { padding: 4px 6px; }
         .log-time { font-feature-settings: \"tnum\"; min-width: 7em; }
         .log-field-name { font-size: smaller; }
         .log-field-value { font-family: monospace; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let prefs = Prefs::new(SCHEMA);
    let descending = prefs.get_enum_nick("sort-order", SORT_DESC) != SORT_ASC;
    let ws: WindowState = std::fs::read(window_state_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Logs")
        .default_width(if ws.width > 300 { ws.width } else { 1000 })
        .default_height(if ws.height > 200 { ws.height } else { 680 })
        .width_request(360)
        .height_request(294)
        .build();
    if ws.maximized {
        window.maximize();
    }

    // ---- Sidebar -------------------------------------------------------
    let sidebar_list = gtk::ListBox::new();
    sidebar_list.add_css_class("navigation-sidebar");
    for c in Category::ALL {
        let l = label(c.label(), &[]);
        l.set_xalign(0.0);
        l.set_margin_top(6);
        l.set_margin_bottom(6);
        l.set_margin_start(6);
        sidebar_list.append(&l);
    }
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&sidebar_list)
        .build();
    let sidebar_tv = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    sidebar_tv.add_top_bar(&sidebar_header);
    sidebar_tv.set_content(Some(&sidebar_scroll));
    let sidebar_page = adw::NavigationPage::new(&sidebar_tv, "Logs");

    // ---- Event list ----------------------------------------------------
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("log-row");
        let time = label("", &["log-time", "dim-label", "numeric"]);
        time.set_xalign(0.0);
        let msg = label("", &["log-message"]);
        msg.set_xalign(0.0);
        msg.set_hexpand(true);
        msg.set_ellipsize(gtk::pango::EllipsizeMode::End);
        msg.set_single_line_mode(true);
        let proc_ = label("", &["dim-label"]);
        proc_.set_xalign(1.0);
        proc_.set_ellipsize(gtk::pango::EllipsizeMode::End);
        proc_.set_max_width_chars(22);
        proc_.set_width_chars(10);
        row.append(&time);
        row.append(&msg);
        row.append(&proc_);
        item.set_child(Some(&row));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(obj) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let Some(row) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let entry = obj.borrow::<Entry>();
        let time = row.first_child().and_downcast::<gtk::Label>();
        let msg = time
            .as_ref()
            .and_then(|t| t.next_sibling())
            .and_downcast::<gtk::Label>();
        let proc_ = msg
            .as_ref()
            .and_then(|m| m.next_sibling())
            .and_downcast::<gtk::Label>();
        let (Some(time), Some(msg), Some(proc_)) = (time, msg, proc_) else {
            return;
        };
        let tz = glib::TimeZone::local();
        let now =
            glib::DateTime::now(&tz).unwrap_or_else(|_| glib::DateTime::now_utc().expect("clock"));
        time.set_text(&format::row_time(entry.timestamp_usec(), &now, &tz));
        msg.set_text(&format::one_line(entry.message()));
        proc_.set_text(entry.process());
        msg.remove_css_class("error");
        msg.remove_css_class("warning");
        match entry.priority() {
            0..=3 => msg.add_css_class("error"),
            4 => msg.add_css_class("warning"),
            _ => {}
        }
        row.set_tooltip_text(Some(&format!(
            "{} · {}",
            journal::priority_name(entry.priority()),
            format::full_time(entry.timestamp_usec(), &tz)
        )));
    });
    let selection = gtk::NoSelection::new(Some(store.clone()));
    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.set_single_click_activate(true);
    list.add_css_class("rich-list");
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();

    let empty = adw::StatusPage::builder()
        .icon_name("edit-find-symbolic")
        .title("No Events")
        .build();
    let loading_page = gtk::Spinner::builder()
        .spinning(true)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .width_request(32)
        .height_request(32)
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&scrolled, Some("list"));
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&loading_page, Some("loading"));
    stack.set_visible_child_name("loading");

    // Header.
    let header = adw::HeaderBar::new();
    let boot_dropdown = gtk::DropDown::from_strings(&["Current Boot"]);
    boot_dropdown.set_tooltip_text(Some("Boot"));
    header.set_title_widget(Some(&boot_dropdown));
    let search_button = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Search")
        .build()
        .labelled("Search");
    header.pack_start(&search_button);
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some("_Export…"), Some("win.export"));
    s1.append(Some("_Refresh"), Some("win.refresh"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(
        Some("_Newest First"),
        Some(&format!("win.sort-order::{SORT_DESC}")),
    );
    s2.append(
        Some("_Oldest First"),
        Some(&format!("win.sort-order::{SORT_ASC}")),
    );
    menu.append_section(Some("Sort"), &s2);
    let s3 = gio::Menu::new();
    s3.append(Some("_About Logs"), Some("app.about"));
    menu.append_section(None, &s3);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build()
        .labelled("Main Menu");
    header.pack_end(&menu_button);
    let spinner = gtk::Spinner::new();
    header.pack_end(&spinner);

    // Search bar.
    let search_entry = gtk::SearchEntry::builder()
        .hexpand(true)
        .placeholder_text("Search events (FIELD=value restricts to a journal field)")
        .build();
    let field_dropdown = gtk::DropDown::from_strings(&SearchField::ALL.map(|f| f.label()));
    field_dropdown.set_tooltip_text(Some("Search in"));
    let search_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    search_box.append(&search_entry);
    search_box.append(&field_dropdown);
    let search_clamp = adw::Clamp::builder()
        .maximum_size(600)
        .child(&search_box)
        .build();
    let search_bar = gtk::SearchBar::builder()
        .child(&search_clamp)
        .show_close_button(false)
        .build();
    search_bar.connect_entry(&search_entry);
    search_bar.set_key_capture_widget(Some(&window));
    search_button
        .bind_property("active", &search_bar, "search-mode-enabled")
        .bidirectional()
        .sync_create()
        .build();

    let banner = adw::Banner::builder()
        .title("Only your own logs are shown. Join the “systemd-journal” group to see system logs.")
        .button_label("Don't Show Again")
        .build();

    let list_tv = adw::ToolbarView::new();
    list_tv.add_top_bar(&header);
    list_tv.add_top_bar(&search_bar);
    list_tv.add_top_bar(&banner);
    list_tv.set_content(Some(&stack));
    let list_page = adw::NavigationPage::with_tag(&list_tv, "Important", "events");

    let nav = adw::NavigationView::new();
    nav.add(&list_page);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));
    let content_page = adw::NavigationPage::new(&toasts, "Events");

    let split = adw::NavigationSplitView::new();
    split.set_sidebar(Some(&sidebar_page));
    split.set_content(Some(&content_page));
    split.set_min_sidebar_width(180.0);
    split.set_max_sidebar_width(240.0);
    window.set_content(Some(&split));
    if let Ok(cond) = adw::BreakpointCondition::parse("max-width: 600sp") {
        let bp = adw::Breakpoint::new(cond);
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);
    }

    let app_state = Rc::new(App {
        ui: Ui {
            window: window.clone(),
            store,
            list: list.clone(),
            scrolled: scrolled.clone(),
            stack,
            empty,
            spinner,
            banner: banner.clone(),
            nav: nav.clone(),
            list_page,
            split: split.clone(),
            boot_dropdown: boot_dropdown.clone(),
            toasts,
        },
        state: RefCell::new(State {
            prefs: prefs.clone(),
            uid: worker::current_uid(),
            boots: vec![Boot {
                index: 0,
                id: String::new(),
                first: None,
                last: None,
                range_text: None,
            }],
            category: Category::Important,
            search_text: String::new(),
            search_field: SearchField::All,
            descending,
            handle: None,
            follow: None,
            live: Live::default(),
            generation: 0,
            loading: false,
            done: false,
        }),
        suppress_boot_change: Cell::new(false),
    });

    // Permission banner.
    if !prefs.get("ignore-warning", false) && !worker::system_logs_readable() {
        banner.set_revealed(true);
    }
    {
        let prefs = prefs.clone();
        banner.connect_button_clicked(move |b| {
            prefs.set("ignore-warning", true);
            b.set_revealed(false);
        });
    }

    // ---- Signals -------------------------------------------------------
    {
        let a = app_state.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let Some(cat) = Category::ALL.get(row.index() as usize).copied() else {
                return;
            };
            a.ui.list_page.set_title(cat.label());
            a.ui.split.set_show_content(true);
            if a.state.borrow().category != cat {
                a.state.borrow_mut().category = cat;
                a.ui.nav.pop_to_page(&a.ui.list_page);
                reload(&a);
            }
        });
    }
    sidebar_list.select_row(sidebar_list.row_at_index(0).as_ref());

    {
        let a = app_state.clone();
        boot_dropdown.connect_selected_notify(move |_| {
            if !a.suppress_boot_change.get() {
                a.ui.nav.pop_to_page(&a.ui.list_page);
                reload(&a);
            }
        });
    }
    {
        let a = app_state.clone();
        search_entry.connect_search_changed(move |e| {
            let text = e.text().to_string();
            if a.state.borrow().search_text != text {
                a.state.borrow_mut().search_text = text;
                reload(&a);
            }
        });
    }
    {
        let a = app_state.clone();
        field_dropdown.connect_selected_notify(move |d| {
            let f = SearchField::ALL
                .get(d.selected() as usize)
                .copied()
                .unwrap_or_default();
            a.state.borrow_mut().search_field = f;
            if !a.state.borrow().search_text.trim().is_empty() {
                reload(&a);
            }
        });
    }
    {
        let a = app_state.clone();
        search_bar.connect_search_mode_enabled_notify(move |bar| {
            if !bar.is_search_mode() && !a.state.borrow().search_text.is_empty() {
                a.state.borrow_mut().search_text.clear();
                reload(&a);
            }
        });
    }
    {
        let a = app_state.clone();
        list.connect_activate(move |_, pos| {
            let Some(obj) = a.ui.store.item(pos).and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let entry = obj.borrow::<Entry>().clone();
            let page = detail_page(&entry, &a.ui.toasts);
            a.ui.nav.push(&page);
        });
    }
    let adj = scrolled.vadjustment();
    {
        let a = app_state.clone();
        adj.connect_value_changed(move |_| maybe_load_more(&a));
    }
    {
        let a = app_state.clone();
        adj.connect_changed(move |_| maybe_load_more(&a));
    }

    // ---- Actions -------------------------------------------------------
    let export = gio::SimpleAction::new("export", None);
    {
        let a = app_state.clone();
        export.connect_activate(move |_, _| export_view(&a));
    }
    let refresh = gio::SimpleAction::new("refresh", None);
    {
        let a = app_state.clone();
        refresh.connect_activate(move |_, _| reload(&a));
    }
    let sort = gio::SimpleAction::new_stateful(
        "sort-order",
        Some(glib::VariantTy::STRING),
        &(if descending { SORT_DESC } else { SORT_ASC }).to_variant(),
    );
    {
        let a = app_state.clone();
        sort.connect_activate(move |action, param| {
            let Some(nick) = param.and_then(|p| p.get::<String>()) else {
                return;
            };
            action.set_state(&nick.to_variant());
            let desc = nick != SORT_ASC;
            if a.state.borrow().descending != desc {
                a.state.borrow_mut().descending = desc;
                a.state
                    .borrow()
                    .prefs
                    .set_enum_nick("sort-order", if desc { SORT_DESC } else { SORT_ASC });
                reload(&a);
            }
        });
    }
    let search_action = gio::SimpleAction::new("search", None);
    {
        let bar = search_bar.clone();
        let entry = search_entry.clone();
        search_action.connect_activate(move |_, _| {
            bar.set_search_mode(true);
            entry.grab_focus();
        });
    }
    window.add_action(&export);
    window.add_action(&refresh);
    window.add_action(&sort);
    window.add_action(&search_action);
    app.set_accels_for_action("win.export", &["<Control>s"]);
    app.set_accels_for_action("win.refresh", &["F5", "<Control>r"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);

    window.connect_close_request(|w| {
        let (width, height) = w.default_size();
        let ws = WindowState {
            width,
            height,
            maximized: w.is_maximized(),
        };
        if let Ok(json) = serde_json::to_vec(&ws) {
            let _ = std::fs::write(window_state_path(), json);
        }
        glib::Propagation::Proceed
    });
    {
        let a = app_state.clone();
        window.connect_destroy(move |_| {
            let mut st = a.state.borrow_mut();
            st.handle = None;
            st.follow = None;
        });
    }

    load_boots(&app_state);
    reload(&app_state);
    window
}

/// Lists boots on a worker thread and fills the boot selector.
fn load_boots(a: &Rc<App>) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(worker::list_boots());
    });
    let a = a.clone();
    glib::spawn_future_local(async move {
        let Ok(boots) = rx.recv().await else { return };
        let tz = glib::TimeZone::local();
        let labels: Vec<String> = boots.iter().map(|b| format::boot_label(b, &tz)).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let current = boots.iter().position(|b| b.index == 0).unwrap_or(0);
        a.state.borrow_mut().boots = boots;
        a.suppress_boot_change.set(true);
        a.ui.boot_dropdown
            .set_model(Some(&gtk::StringList::new(&refs)));
        a.ui.boot_dropdown.set_selected(current as u32);
        a.suppress_boot_change.set(false);
    });
}

/// Id of the selected boot (`None` = current boot by `--boot=0`) and
/// whether it is the current boot, the only one that gets new entries.
fn selected_boot(a: &App) -> (Option<String>, bool) {
    let st = a.state.borrow();
    match st.boots.get(a.ui.boot_dropdown.selected() as usize) {
        Some(b) => ((!b.id.is_empty()).then(|| b.id.clone()), b.index == 0),
        None => (None, true),
    }
}

/// Inserts live entries at the top (newest-first view) or the end
/// (oldest-first view). The view scrolls to them only if it was showing
/// that edge; otherwise the list view keeps its position (it is anchored
/// to the items in view, not to an offset).
fn insert_live(a: &Rc<App>, entries: Vec<Entry>, at_top: bool) {
    if entries.is_empty() {
        return;
    }
    let adj = a.ui.scrolled.vadjustment();
    let descending = a.state.borrow().descending;
    let follow_edge = a.ui.store.n_items() == 0
        || live::at_live_edge(descending, adj.value(), adj.page_size(), adj.upper());
    let objs: Vec<glib::BoxedAnyObject> =
        entries.into_iter().map(glib::BoxedAnyObject::new).collect();
    if at_top {
        a.ui.store.splice(0, 0, &objs);
    } else {
        a.ui.store.extend_from_slice(&objs);
    }
    a.ui.stack.set_visible_child_name("list");
    if follow_edge {
        let pos = if at_top {
            0
        } else {
            a.ui.store.n_items().saturating_sub(1)
        };
        a.ui.list.scroll_to(pos, gtk::ListScrollFlags::NONE, None);
    }
}

/// Restarts the query for the current view.
fn reload(a: &Rc<App>) {
    let (boot, is_current_boot) = selected_boot(a);
    let (rx, generation) = {
        let mut st = a.state.borrow_mut();
        st.generation += 1;
        st.handle = None; // stops the previous worker
        st.follow = None; // and its journalctl --follow
        st.live.clear();
        st.loading = true;
        st.done = false;
        let mut args = QueryArgs {
            boot,
            reverse: st.descending,
            ..Default::default()
        };
        st.category.prefilter(&mut args, st.uid);
        let query = worker::Query {
            args,
            category: st.category,
            search: Search::new(&st.search_text, st.search_field),
            uid: st.uid,
        };
        let (handle, follow, rx) = worker::start(query, is_current_boot);
        handle.request(BATCH);
        st.handle = Some(handle);
        st.follow = follow;
        (rx, st.generation)
    };
    a.ui.store.remove_all();
    a.ui.scrolled.vadjustment().set_value(0.0);
    a.ui.stack.set_visible_child_name("loading");
    a.ui.spinner.set_spinning(true);

    let a = a.clone();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            if a.state.borrow().generation != generation {
                break;
            }
            match msg {
                worker::Msg::Batch(entries) => {
                    let entries = a.state.borrow_mut().live.loaded(entries);
                    let objs: Vec<glib::BoxedAnyObject> =
                        entries.into_iter().map(glib::BoxedAnyObject::new).collect();
                    a.ui.store.extend_from_slice(&objs);
                    a.state.borrow_mut().loading = false;
                    a.ui.spinner.set_spinning(false);
                    if a.ui.store.n_items() > 0 {
                        a.ui.stack.set_visible_child_name("list");
                    }
                    maybe_load_more(&a);
                }
                worker::Msg::Done => {
                    {
                        let mut st = a.state.borrow_mut();
                        st.done = true;
                        st.loading = false;
                        st.handle = None;
                    }
                    a.ui.spinner.set_spinning(false);
                    let deferred = a.state.borrow_mut().live.finish();
                    insert_live(&a, deferred, false);
                    if a.ui.store.n_items() == 0 {
                        let searching = {
                            let st = a.state.borrow();
                            !Search::new(&st.search_text, st.search_field).is_empty()
                        };
                        if a.ui.stack.visible_child_name().as_deref() != Some("empty") || !searching
                        {
                            a.ui.empty.set_icon_name(Some(if searching {
                                "edit-find-symbolic"
                            } else {
                                "document-open-recent-symbolic"
                            }));
                            a.ui.empty.set_title(if searching {
                                "No Results"
                            } else {
                                "No Events"
                            });
                            a.ui.empty.set_description(None);
                        }
                        a.ui.stack.set_visible_child_name("empty");
                        if searching {
                            helix_app_common::announce_settled(&a.ui.empty, "No results");
                        }
                    }
                    // Keep listening: live entries may still arrive.
                }
                worker::Msg::Live(entries) => {
                    let (descending, done) = {
                        let st = a.state.borrow();
                        (st.descending, st.done)
                    };
                    let placement = a
                        .state
                        .borrow_mut()
                        .live
                        .followed(entries, descending, done);
                    match placement {
                        Placement::Prepend(v) => insert_live(&a, v, true),
                        Placement::Append(v) => insert_live(&a, v, false),
                        Placement::Deferred => {}
                    }
                }
                worker::Msg::LimitedAccess => {
                    if !a.state.borrow().prefs.get("ignore-warning", false) {
                        a.ui.banner.set_revealed(true);
                    }
                }
                worker::Msg::Error(e) => {
                    a.ui.empty.set_icon_name(Some("dialog-error-symbolic"));
                    a.ui.empty.set_title("Unable to Read Logs");
                    a.ui.empty
                        .set_description(Some(&glib::markup_escape_text(&e)));
                    a.ui.stack.set_visible_child_name("empty");
                    helix_app_common::announce(
                        &a.ui.empty,
                        &format!("Unable to read logs. {e}"),
                        helix_app_common::Urgency::Assertive,
                    );
                }
            }
        }
    });
}

/// Requests the next batch when the view is close to its end.
fn maybe_load_more(a: &Rc<App>) {
    let adj = a.ui.scrolled.vadjustment();
    let near_end = adj.upper() - (adj.value() + adj.page_size()) < adj.page_size() * 1.5;
    if !near_end {
        return;
    }
    let mut st = a.state.borrow_mut();
    if st.loading || st.done {
        return;
    }
    let Some(h) = &st.handle else { return };
    h.request(BATCH);
    st.loading = true;
    a.ui.spinner.set_spinning(true);
}

fn export_view(a: &Rc<App>) {
    let dialog = gtk::FileDialog::builder()
        .title("Export Logs")
        .initial_name("logs.txt")
        .modal(true)
        .build();
    let a2 = a.clone();
    dialog.save(Some(&a.ui.window), None::<&gio::Cancellable>, move |res| {
        let Ok(file) = res else { return };
        let tz = glib::TimeZone::local();
        let entries: Vec<Entry> = a2
            .ui
            .store
            .iter::<glib::BoxedAnyObject>()
            .map_while(Result::ok)
            .map(|o| o.borrow::<Entry>().clone())
            .collect();
        let text = format::export_text(&entries, &tz);
        let toasts = a2.ui.toasts.clone();
        glib::spawn_future_local(async move {
            let res = file
                .replace_contents_future(
                    text.into_bytes(),
                    None,
                    false,
                    gio::FileCreateFlags::REPLACE_DESTINATION,
                )
                .await;
            let msg = match res {
                Ok(_) => "Logs exported".to_string(),
                Err((_, e)) => format!("Export failed: {e}"),
            };
            toasts.add_toast(adw::Toast::new(&msg));
        });
    });
}

fn detail_page(entry: &Entry, toasts: &adw::ToastOverlay) -> adw::NavigationPage {
    let tz = glib::TimeZone::local();
    let process = entry.process();
    let title = if process.is_empty() { "Event" } else { process };
    let time = format::full_time(entry.timestamp_usec(), &tz);

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(title, &time)));
    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy Event")
        .build()
        .labelled("Copy Event");
    {
        let text = entry.to_export_fields();
        let toasts = toasts.clone();
        copy.connect_clicked(move |b| {
            b.clipboard().set_text(&text);
            toasts.add_toast(adw::Toast::new("Copied to clipboard"));
        });
    }
    header.pack_end(&copy);

    let listbox = gtk::ListBox::new();
    listbox.set_selection_mode(gtk::SelectionMode::None);
    listbox.add_css_class("boxed-list");
    let add_row = |name: &str, value: &str| {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.set_margin_top(8);
        b.set_margin_bottom(8);
        b.set_margin_start(12);
        b.set_margin_end(12);
        let n = label(name, &["dim-label", "log-field-name"]);
        n.set_xalign(0.0);
        let v = label(value, &["log-field-value"]);
        v.set_xalign(0.0);
        v.set_wrap(true);
        v.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        v.set_selectable(true);
        b.append(&n);
        b.append(&v);
        let row = gtk::ListBoxRow::builder()
            .child(&b)
            .activatable(false)
            .build();
        listbox.append(&row);
    };
    let mut time_added = false;
    for (k, v) in entry.display_fields() {
        if k == "PRIORITY" {
            let p = entry.priority();
            add_row(k, &format!("{v} ({})", journal::priority_name(p)));
        } else {
            add_row(k, v);
        }
        if !time_added && k != "MESSAGE" {
            add_row("Time", &time);
            time_added = true;
        }
    }
    if !time_added {
        add_row("Time", &time);
    }

    let clamp = adw::Clamp::builder()
        .maximum_size(900)
        .child(&listbox)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&scrolled));
    adw::NavigationPage::with_tag(&tv, title, "detail")
}
