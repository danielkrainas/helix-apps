//! Browser window: header bar, address entry, tabs, find bar, popovers.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use webkit6::prelude::*;

use crate::app::{self, App};
use crate::bookmarks::{self, Bookmark};
use crate::session::{TabState, WindowState};
use crate::settings;
use crate::store;
use crate::tab::{display_uri, Change, Security, Tab};
use crate::urlinput;

const MAX_CLOSED: usize = 10;
const MAX_SUGGESTIONS: usize = 8;

const CSS: &str = "
.permission-bar, .tls-bar { border-bottom: 1px solid alpha(currentColor, .15); }
.permission-bar { background: alpha(@accent_bg_color, .18); }
.tls-bar { background: alpha(@error_bg_color, .22); }
.link-preview { background: @popover_bg_color; color: @popover_fg_color; padding: 3px 8px; margin: 4px;
  border-radius: 6px; font-size: smaller; box-shadow: 0 1px 3px alpha(black, .3); }
.private-indicator { font-weight: bold; padding: 2px 8px; border-radius: 99px;
  background: alpha(@accent_bg_color, .25); }
.suggestion-url { font-size: smaller; }
";

pub fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

pub struct Window {
    pub this: Weak<Window>,
    app: Weak<App>,
    pub private: bool,
    pub session: webkit6::NetworkSession,
    pub win: adw::ApplicationWindow,
    pub tab_view: adw::TabView,
    overview: adw::TabOverview,
    toolbar: adw::ToolbarView,
    entry: gtk::Entry,
    back: gtk::Button,
    forward: gtk::Button,
    reload: gtk::Button,
    home: gtk::Button,
    reader: gtk::Button,
    /// Title shown instead of the address entry in web-app mode.
    app_title: Option<adw::WindowTitle>,
    suggest_pop: gtk::Popover,
    suggest_list: gtk::ListBox,
    suggestions: RefCell<Vec<String>>,
    /// Set while the program (not the user) changes the entry text.
    setting_entry: Cell<bool>,
    /// The user has typed since the entry was last synced.
    editing: Cell<bool>,
    find_bar: gtk::SearchBar,
    find_entry: gtk::SearchEntry,
    find_label: gtk::Label,
    dl_button: gtk::MenuButton,
    dl_list: gtk::ListBox,
    bm_search: gtk::SearchEntry,
    bm_list: gtk::ListBox,
    zoom_label: gtk::Label,
    closed: RefCell<Vec<TabState>>,
    info_pop: gtk::Popover,
    bm_pop: gtk::Popover,
    bm_title: gtk::Entry,
    bm_tags: gtk::Entry,
    bm_editing: RefCell<Option<String>>,
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build()
}

fn web_app_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let zoom = gio::Menu::new();
    let item = gio::MenuItem::new(None, None);
    item.set_attribute_value("custom", Some(&"zoom".to_variant()));
    zoom.append_item(&item);
    menu.append_section(None, &zoom);
    let s1 = gio::Menu::new();
    s1.append(Some("_Find…"), Some("win.find"));
    s1.append(Some("_Print…"), Some("win.print"));
    s1.append(Some("_Fullscreen"), Some("win.fullscreen"));
    s1.append(Some("Open in _Browser"), Some("win.open-in-browser"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some("_Uninstall Web Application…"), Some("app.uninstall-web-app"));
    s2.append(Some("_About Web"), Some("app.about"));
    menu.append_section(None, &s2);
    menu
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some("_New Tab"), Some("win.new-tab"));
    s1.append(Some("New _Window"), Some("app.new-window"));
    s1.append(Some("New _Private Window"), Some("app.new-private-window"));
    menu.append_section(None, &s1);
    let zoom = gio::Menu::new();
    let item = gio::MenuItem::new(None, None);
    item.set_attribute_value("custom", Some(&"zoom".to_variant()));
    zoom.append_item(&item);
    menu.append_section(None, &zoom);
    let s2 = gio::Menu::new();
    s2.append(Some("_Reopen Closed Tab"), Some("win.reopen-tab"));
    s2.append(Some("_Find…"), Some("win.find"));
    s2.append(Some("_Print…"), Some("win.print"));
    s2.append(Some("_Fullscreen"), Some("win.fullscreen"));
    s2.append(Some("Install Site as _Web Application…"), Some("win.install-web-app"));
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    s3.append(Some("_History"), Some("app.history"));
    s3.append(Some("_Passwords"), Some("app.passwords"));
    s3.append(Some("_Import Bookmarks…"), Some("app.import-bookmarks"));
    s3.append(Some("_Export Bookmarks…"), Some("app.export-bookmarks"));
    menu.append_section(None, &s3);
    let s4 = gio::Menu::new();
    s4.append(Some("_Preferences"), Some("app.preferences"));
    s4.append(Some("_About Web"), Some("app.about"));
    menu.append_section(None, &s4);
    menu
}

fn two_line_row(title: &str, subtitle: &str, icon: &str) -> (gtk::Box, gtk::Box) {
    let row = gtk::Box::builder()
        .spacing(10)
        .margin_start(8)
        .margin_end(8)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    row.append(&gtk::Image::from_icon_name(icon));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let t = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    text.append(&t);
    if !subtitle.is_empty() {
        let s = gtk::Label::builder()
            .label(subtitle)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        s.add_css_class("dim-label");
        s.add_css_class("suggestion-url");
        text.append(&s);
    }
    row.append(&text);
    (row, text)
}

impl Window {
    pub fn new(app: &Rc<App>, private: bool, session: webkit6::NetworkSession) -> Rc<Window> {
        let win = adw::ApplicationWindow::builder()
            .application(&app.gtk)
            .default_width(1200)
            .default_height(800)
            .title(if private { "Private Browsing" } else { "Web" })
            .build();

        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder()
            .view(&tab_view)
            .autohide(true)
            .build();

        // Header: navigation.
        let back = icon_button("go-previous-symbolic", "Back", "win.back");
        let forward = icon_button("go-next-symbolic", "Forward", "win.forward");
        let reload = icon_button("view-refresh-symbolic", "Reload", "win.reload");
        let home = icon_button("go-home-symbolic", "Home", "win.home");
        let reader = icon_button("format-justify-left-symbolic", "Reader View", "win.reader");
        reader.set_visible(false);
        let web_app = app.web_app.is_some();
        let header = adw::HeaderBar::new();
        header.pack_start(&back);
        header.pack_start(&forward);
        header.pack_start(&reload);
        header.pack_start(&home);

        // Address entry.
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .input_purpose(gtk::InputPurpose::Url)
            .placeholder_text("Search or enter address")
            .primary_icon_name("web-browser-symbolic")
            .primary_icon_activatable(true)
            .primary_icon_tooltip_text("Site information")
            .secondary_icon_name("non-starred-symbolic")
            .secondary_icon_activatable(true)
            .secondary_icon_tooltip_text("Bookmark this page")
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(760)
            .tightening_threshold(560)
            .child(&entry)
            .build();
        clamp.set_hexpand(true);
        let app_title = web_app.then(|| adw::WindowTitle::new("", ""));
        match &app_title {
            Some(t) => header.set_title_widget(Some(t)),
            None => header.set_title_widget(Some(&clamp)),
        }

        // Header: end.
        let zoom_label = gtk::Label::new(Some("100%"));
        let zoom_box = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::Center)
            .hexpand(true)
            .build();
        let zout = icon_button("zoom-out-symbolic", "Zoom Out", "win.zoom-out");
        let zin = icon_button("zoom-in-symbolic", "Zoom In", "win.zoom-in");
        let zreset = gtk::Button::builder()
            .child(&zoom_label)
            .action_name("win.zoom-reset")
            .tooltip_text("Reset Zoom")
            .build();
        for b in [&zout, &zreset, &zin] {
            b.add_css_class("flat");
        }
        zoom_box.append(&zout);
        zoom_box.append(&zreset);
        zoom_box.append(&zin);
        let menu_pop = gtk::PopoverMenu::from_model(Some(&if web_app {
            web_app_menu()
        } else {
            main_menu()
        }));
        menu_pop.add_child(&zoom_box, "zoom");
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .popover(&menu_pop)
            .primary(true)
            .build();
        header.pack_end(&menu_button);
        let tab_button = adw::TabButton::builder()
            .view(&tab_view)
            .action_name("win.overview")
            .visible(!web_app)
            .build();
        header.pack_end(&tab_button);

        let dl_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        dl_list.add_css_class("boxed-list");
        let dl_clear = gtk::Button::with_label("Clear Finished");
        let dl_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let dl_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .min_content_width(360)
            .child(&dl_list)
            .build();
        dl_box.append(&dl_scroll);
        dl_box.append(&dl_clear);
        let dl_pop = gtk::Popover::builder().child(&dl_box).build();
        let dl_button = gtk::MenuButton::builder()
            .icon_name("folder-download-symbolic")
            .tooltip_text("Downloads")
            .popover(&dl_pop)
            .visible(false)
            .build();
        header.pack_end(&dl_button);

        let bm_search = gtk::SearchEntry::builder()
            .placeholder_text("Search bookmarks")
            .build();
        let bm_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        bm_list.add_css_class("navigation-sidebar");
        let bm_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(480)
            .min_content_width(360)
            .child(&bm_list)
            .build();
        let bm_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        bm_box.append(&bm_search);
        bm_box.append(&bm_scroll);
        let bm_list_pop = gtk::Popover::builder().child(&bm_box).build();
        let bm_button = gtk::MenuButton::builder()
            .icon_name("user-bookmarks-symbolic")
            .tooltip_text("Bookmarks")
            .popover(&bm_list_pop)
            .visible(!web_app)
            .build();
        header.pack_end(&bm_button);
        header.pack_end(&reader);

        if private {
            let pill = gtk::Label::new(Some("Private"));
            pill.add_css_class("private-indicator");
            pill.set_tooltip_text(Some(
                "Private browsing: history and site data are not saved",
            ));
            header.pack_start(&pill);
        }

        // Find bar.
        let find_entry = gtk::SearchEntry::builder()
            .placeholder_text("Find in page")
            .width_chars(28)
            .build();
        let find_prev = icon_button("go-up-symbolic", "Previous Match", "win.find-prev");
        let find_next = icon_button("go-down-symbolic", "Next Match", "win.find-next");
        let find_label = gtk::Label::new(None);
        find_label.add_css_class("dim-label");
        let find_box = gtk::Box::builder().spacing(6).build();
        find_box.append(&find_entry);
        find_box.append(&find_prev);
        find_box.append(&find_next);
        find_box.append(&find_label);
        let find_bar = gtk::SearchBar::builder()
            .child(&find_box)
            .show_close_button(true)
            .build();
        find_bar.connect_entry(&find_entry);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&find_bar);
        tab_view.set_vexpand(true);
        content.append(&tab_view);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        toolbar.set_content(Some(&content));

        let overview = adw::TabOverview::builder()
            .view(&tab_view)
            .child(&toolbar)
            .enable_new_tab(true)
            .build();
        win.set_content(Some(&overview));

        // Suggestions popover.
        let suggest_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        suggest_list.add_css_class("navigation-sidebar");
        suggest_list.set_can_focus(false);
        let suggest_pop = gtk::Popover::builder()
            .child(&suggest_list)
            .autohide(false)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::Start)
            .can_focus(false)
            .build();
        suggest_pop.set_parent(&entry);

        // Site info and bookmark-edit popovers, anchored to entry icons.
        let info_pop = gtk::Popover::new();
        info_pop.set_parent(&entry);
        let bm_title = gtk::Entry::builder().width_chars(28).build();
        let bm_tags = gtk::Entry::builder()
            .placeholder_text("Tags, comma separated")
            .build();
        let bm_remove = gtk::Button::with_label("Remove Bookmark");
        bm_remove.add_css_class("destructive-action");
        let bm_edit_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let heading = gtk::Label::builder().label("Bookmark").xalign(0.0).build();
        heading.add_css_class("heading");
        bm_edit_box.append(&heading);
        bm_edit_box.append(&bm_title);
        bm_edit_box.append(&bm_tags);
        bm_edit_box.append(&bm_remove);
        let bm_pop = gtk::Popover::builder().child(&bm_edit_box).build();
        bm_pop.set_parent(&entry);

        let w = Rc::new_cyclic(|this| Window {
            this: this.clone(),
            app: Rc::downgrade(app),
            private,
            session,
            win,
            tab_view,
            overview,
            toolbar,
            entry,
            back,
            forward,
            reload,
            home,
            reader,
            app_title,
            suggest_pop,
            suggest_list,
            suggestions: RefCell::default(),
            setting_entry: Cell::new(false),
            editing: Cell::new(false),
            find_bar,
            find_entry,
            find_label,
            dl_button,
            dl_list,
            bm_search,
            bm_list,
            zoom_label,
            closed: RefCell::default(),
            info_pop,
            bm_pop,
            bm_title,
            bm_tags,
            bm_editing: RefCell::default(),
        });
        w.install_actions();
        w.connect_signals(&dl_clear, &bm_list_pop, &bm_remove);
        // Popovers parented to the entry must be unparented with it.
        let pops = [w.suggest_pop.clone(), w.info_pop.clone(), w.bm_pop.clone()];
        w.entry.connect_destroy(move |_| {
            for p in &pops {
                p.unparent();
            }
        });
        w.sync();
        w
    }

    fn app(&self) -> Option<Rc<App>> {
        self.app.upgrade()
    }

    /// Runs `f` with this window if it is still alive.
    fn weak_fn<A>(&self, f: impl Fn(&Rc<Window>, A) + 'static) -> impl Fn(A) + 'static {
        let weak = self.this.clone();
        move |a| {
            if let Some(w) = weak.upgrade() {
                f(&w, a)
            }
        }
    }

    fn install_actions(&self) {
        let add = |name: &str, f: fn(&Rc<Window>)| {
            let a = gio::SimpleAction::new(name, None);
            let cb = self.weak_fn(move |w, ()| f(w));
            a.connect_activate(move |_, _| cb(()));
            self.win.add_action(&a);
        };
        add("new-tab", |w| {
            w.open_home(true);
        });
        add("close-tab", |w| {
            if let Some(p) = w.tab_view.selected_page() {
                w.tab_view.close_page(&p);
            }
        });
        add("reopen-tab", |w| w.reopen_closed());
        add("location", |w| {
            w.entry.grab_focus();
            w.entry.select_region(0, -1);
        });
        add("back", |w| w.with_view(|v| v.go_back()));
        add("forward", |w| w.with_view(|v| v.go_forward()));
        add("reload", |w| {
            if let Some(t) = w.current() {
                if t.view.is_loading() {
                    t.view.stop_loading();
                } else if t.pending_uri.borrow().is_some() {
                    t.realize_lazy();
                } else {
                    t.view.reload();
                }
            }
        });
        add("reload-bypass", |w| {
            w.with_view(|v| v.reload_bypass_cache())
        });
        add("home", |w| {
            if let (Some(app), Some(t)) = (w.app(), w.current()) {
                t.load(&app.home_uri());
            }
        });
        add("bookmark", |w| w.bookmark_current());
        add("overview", |w| w.overview.set_open(!w.overview.is_open()));
        add("find", |w| {
            w.find_bar.set_search_mode(true);
            w.find_entry.grab_focus();
        });
        add("find-next", |w| w.find_step(true));
        add("find-prev", |w| w.find_step(false));
        add("print", |w| {
            w.with_view(|v| {
                let op = webkit6::PrintOperation::new(v);
                op.run_dialog(Some(&w.win));
            })
        });
        add("zoom-in", |w| w.zoom(1));
        add("zoom-out", |w| w.zoom(-1));
        add("zoom-reset", |w| w.zoom(0));
        add("reader", |w| {
            if let Some(t) = w.current() {
                t.toggle_reader();
            }
        });
        add("install-web-app", crate::dialogs::install_web_app);
        add("open-in-browser", |w| {
            if let Some(t) = w.current() {
                crate::tab::open_externally(&w.win, &t.uri());
            }
        });
        add("fullscreen", |w| {
            w.win.set_fullscreened(!w.win.is_fullscreen())
        });
    }

    fn connect_signals(
        &self,
        dl_clear: &gtk::Button,
        bm_list_pop: &gtk::Popover,
        bm_remove: &gtk::Button,
    ) {
        // Window lifecycle.
        let cb = self.weak_fn(|w, ()| {
            if let Some(app) = w.app() {
                app.window_closed(w);
            }
        });
        self.win.connect_close_request(move |_| {
            cb(());
            glib::Propagation::Proceed
        });
        let cb = self.weak_fn(|w, fs: bool| {
            w.toolbar.set_reveal_top_bars(!fs);
            if fs {
                w.find_bar.set_search_mode(false);
            }
        });
        self.win
            .connect_fullscreened_notify(move |win| cb(win.is_fullscreen()));

        // Tab view.
        let cb = self.weak_fn(|w, ()| w.selection_changed());
        self.tab_view.connect_selected_page_notify(move |_| cb(()));
        let cb = self.weak_fn(|w, page: adw::TabPage| w.page_closing(&page));
        self.tab_view.connect_close_page(move |_, page| {
            cb(page.clone());
            glib::Propagation::Proceed
        });
        let cb = self.weak_fn(|w, ()| {
            if w.tab_view.n_pages() == 0 {
                w.win.close();
            } else if let Some(app) = w.app() {
                app.schedule_session_save();
            }
        });
        self.tab_view.connect_page_detached(move |_, _, _| cb(()));
        let cb = self.weak_fn(|w, page: adw::TabPage| {
            if let Some(t) = w.tab_for_page(&page) {
                w.refresh_page(&t);
            }
            if let Some(app) = w.app() {
                app.schedule_session_save();
            }
        });
        self.tab_view
            .connect_page_attached(move |_, page, _| cb(page.clone()));
        let cb = self.weak_fn(|w, ()| {
            if let Some(app) = w.app() {
                app.schedule_session_save();
            }
        });
        self.tab_view.connect_page_reordered(move |_, _, _| cb(()));
        let weak = self.this.clone();
        self.tab_view.connect_create_window(move |_| {
            let w = weak.upgrade()?;
            let app = w.app()?;
            let nw = app.new_window(w.private);
            nw.win.present();
            Some(nw.tab_view.clone())
        });
        let cb = self.weak_fn(|w, page: adw::TabPage| {
            if let Some(t) = w.tab_for_page(&page) {
                t.view.set_is_muted(!t.view.is_muted());
            }
        });
        self.tab_view
            .connect_indicator_activated(move |_, page| cb(page.clone()));
        let weak = self.this.clone();
        self.overview.connect_create_tab(move |o| {
            match weak.upgrade() {
                Some(w) => {
                    let t = w.open_home(true);
                    w.tab_view.page(&t.root)
                }
                // Unreachable in practice: the overview lives in the window.
                None => o
                    .view()
                    .expect("tab view")
                    .append(&gtk::Box::new(gtk::Orientation::Vertical, 0)),
            }
        });

        // Address entry.
        let cb = self.weak_fn(|w, ()| w.entry_activated(false));
        self.entry.connect_activate(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| {
            if w.setting_entry.get() || !w.entry_focused() {
                return;
            }
            w.editing.set(true);
            w.update_suggestions();
        });
        self.entry.connect_changed(move |_| cb(()));
        let cb = self.weak_fn(|w, pos: gtk::EntryIconPosition| match pos {
            gtk::EntryIconPosition::Primary => w.show_site_info(),
            _ => w.bookmark_current(),
        });
        self.entry.connect_icon_press(move |_, pos| cb(pos));

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.this.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            w.entry_key(key, mods)
        });
        self.entry.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let cb = self.weak_fn(|w, ()| {
            let weak = w.this.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                if let Some(w) = weak.upgrade() {
                    if !w.entry_focused() {
                        w.suggest_pop.popdown();
                        w.editing.set(false);
                        w.sync_entry();
                    }
                }
            });
        });
        focus.connect_leave(move |_| cb(()));
        self.entry.add_controller(focus);

        let cb = self.weak_fn(|w, index: i32| w.activate_suggestion(index as usize, false));
        self.suggest_list
            .connect_row_activated(move |_, row| cb(row.index()));

        // Find bar.
        let cb = self.weak_fn(|w, ()| w.find_changed());
        self.find_entry.connect_search_changed(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.find_step(true));
        self.find_entry.connect_activate(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.find_step(false));
        self.find_entry.connect_previous_match(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.find_step(true));
        self.find_entry.connect_next_match(move |_| cb(()));
        let cb = self.weak_fn(|w, on: bool| {
            if !on {
                w.with_view(|v| {
                    if let Some(fc) = v.find_controller() {
                        fc.search_finish();
                    }
                });
                w.find_label.set_text("");
                w.with_view(|v| {
                    v.grab_focus();
                });
            }
        });
        self.find_bar
            .connect_search_mode_enabled_notify(move |b| cb(b.is_search_mode()));

        // Downloads.
        let cb = self.weak_fn(|w, ()| {
            let mut child = w.dl_list.first_child();
            while let Some(row) = child {
                child = row.next_sibling();
                if row.widget_name() == "finished" {
                    w.dl_list.remove(&row);
                }
            }
            if w.dl_list.first_child().is_none() {
                w.dl_button.popdown();
                w.dl_button.set_visible(false);
            }
        });
        dl_clear.connect_clicked(move |_| cb(()));

        // Bookmarks list.
        let cb = self.weak_fn(|w, ()| w.fill_bookmarks_list());
        bm_list_pop.connect_show(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| w.fill_bookmarks_list());
        self.bm_search.connect_search_changed(move |_| cb(()));
        let pop = bm_list_pop.clone();
        let cb = self.weak_fn(move |w, row: gtk::ListBoxRow| {
            let url = row.widget_name().to_string();
            pop.popdown();
            if let Some(t) = w.current() {
                t.load(&url);
            }
        });
        self.bm_list
            .connect_row_activated(move |_, row| cb(row.clone()));

        // Bookmark edit popover.
        let cb = self.weak_fn(|w, ()| w.bookmark_popover_closed());
        self.bm_pop.connect_closed(move |_| cb(()));
        let cb = self.weak_fn(|w, ()| {
            if let (Some(app), Some(url)) = (w.app(), w.bm_editing.take()) {
                app.bookmarks.borrow_mut().remove(&url);
                app.bookmarks_changed();
            }
            w.bm_pop.popdown();
        });
        bm_remove.connect_clicked(move |_| cb(()));
        let pop = self.bm_pop.clone();
        self.bm_title.connect_activate(move |_| pop.popdown());
        let pop = self.bm_pop.clone();
        self.bm_tags.connect_activate(move |_| pop.popdown());
    }

    // ---- tabs -------------------------------------------------------------

    pub fn contains(&self, tab: &Tab) -> bool {
        tab.root
            .ancestor(adw::TabView::static_type())
            .is_some_and(|a| a == *self.tab_view.upcast_ref::<gtk::Widget>())
    }

    /// The entry's focus lives on its internal text child.
    fn entry_focused(&self) -> bool {
        self.entry
            .state_flags()
            .contains(gtk::StateFlags::FOCUS_WITHIN)
    }

    pub fn current(&self) -> Option<Rc<Tab>> {
        let page = self.tab_view.selected_page()?;
        self.tab_for_page(&page)
    }

    fn tab_for_page(&self, page: &adw::TabPage) -> Option<Rc<Tab>> {
        let child = page.child();
        let app = self.app()?;
        let tabs = app.tabs.borrow();
        tabs.iter()
            .find(|t| t.root.upcast_ref::<gtk::Widget>() == &child)
            .cloned()
    }

    fn page_of(&self, tab: &Tab) -> Option<adw::TabPage> {
        self.contains(tab).then(|| self.tab_view.page(&tab.root))
    }

    fn with_view(&self, f: impl FnOnce(&webkit6::WebView)) {
        if let Some(t) = self.current() {
            f(&t.view);
        }
    }

    fn new_tab_widget(&self) -> Option<Rc<Tab>> {
        let app = self.app()?;
        Some(app.create_tab(self.private, &self.session))
    }

    fn setup_page(&self, tab: &Rc<Tab>) {
        self.refresh_page(tab);
    }

    /// Appends a tab that opens the homepage.
    pub fn open_home(&self, select: bool) -> Rc<Tab> {
        let home = self
            .app()
            .map(|a| a.home_uri())
            .unwrap_or_else(|| settings::OVERVIEW.into());
        let tab = self.open_uri(&home, select);
        if select && home == settings::OVERVIEW {
            self.entry.grab_focus();
        }
        tab
    }

    pub fn open_uri(&self, uri: &str, select: bool) -> Rc<Tab> {
        let tab = self.new_tab_widget().expect("app alive");
        let page = self.tab_view.append(&tab.root);
        self.setup_page(&tab);
        tab.load(uri);
        if select {
            self.tab_view.set_selected_page(&page);
        }
        tab
    }

    /// Creates an empty tab right after `from`.
    pub fn new_tab_after(&self, from: &Tab) -> Rc<Tab> {
        let tab = self.new_tab_widget().expect("app alive");
        self.insert_tab_after(from, &tab);
        tab
    }

    pub fn insert_tab_after(&self, from: &Tab, tab: &Rc<Tab>) {
        let parent = self.page_of(from);
        self.tab_view.add_page(&tab.root, parent.as_ref());
        self.setup_page(tab);
    }

    pub fn select(&self, tab: &Tab) {
        if let Some(p) = self.page_of(tab) {
            self.tab_view.set_selected_page(&p);
        }
    }

    pub fn close_tab(&self, tab: &Tab) {
        if let Some(p) = self.page_of(tab) {
            self.tab_view.close_page(&p);
        }
    }

    fn page_closing(&self, page: &adw::TabPage) {
        let Some(t) = self.tab_for_page(page) else {
            return;
        };
        t.deny_all_permissions();
        if let Some(state) = app::tab_state(&t) {
            let mut closed = self.closed.borrow_mut();
            closed.push(state);
            if closed.len() > MAX_CLOSED {
                closed.remove(0);
            }
        }
        if let Some(app) = self.app() {
            app.forget_tab(&t);
        }
    }

    fn reopen_closed(&self) {
        let last = self.closed.borrow_mut().pop();
        if let Some(state) = last {
            let uri = if state.uri == settings::OVERVIEW {
                crate::tab::START_URI.to_string()
            } else {
                state.uri
            };
            self.open_uri(&uri, true);
        }
    }

    pub fn restore(&self, ws: &WindowState) {
        let mut selected = None;
        for (i, ts) in ws.tabs.iter().enumerate() {
            let Some(tab) = self.new_tab_widget() else {
                return;
            };
            let page = self.tab_view.append(&tab.root);
            if i == ws.selected {
                tab.load(&ts.uri);
                selected = Some(page);
            } else {
                tab.set_lazy(&ts.uri, &ts.title);
            }
            self.setup_page(&tab);
        }
        if let Some(p) = selected {
            self.tab_view.set_selected_page(&p);
        }
    }

    pub fn session_state(&self) -> WindowState {
        let mut tabs = Vec::new();
        let mut selected = 0;
        let current = self.tab_view.selected_page();
        for i in 0..self.tab_view.n_pages() {
            let page = self.tab_view.nth_page(i);
            let Some(t) = self.tab_for_page(&page) else {
                continue;
            };
            if let Some(state) = app::tab_state(&t) {
                if Some(&page) == current.as_ref() {
                    selected = tabs.len();
                }
                tabs.push(state);
            }
        }
        WindowState { tabs, selected }
    }

    fn selection_changed(&self) {
        if let Some(t) = self.current() {
            t.realize_lazy();
            if self.find_bar.is_search_mode() {
                self.find_changed();
            }
        }
        self.editing.set(false);
        self.sync();
        if let Some(app) = self.app() {
            app.schedule_session_save();
        }
    }

    pub fn set_page_fullscreen(&self, on: bool) {
        if on {
            self.find_bar.set_search_mode(false);
        }
    }

    // ---- state sync -------------------------------------------------------

    fn refresh_page(&self, tab: &Tab) {
        let Some(page) = self.page_of(tab) else {
            return;
        };
        page.set_title(&tab.title());
        page.set_tooltip(&glib::markup_escape_text(&display_uri(&tab.uri())));
        page.set_loading(tab.view.is_loading());
        match tab.view.favicon() {
            Some(tex) => page.set_icon(Some(&tex)),
            None => page.set_icon(None::<&gio::Icon>),
        }
        if tab.view.is_playing_audio() || tab.view.is_muted() {
            let icon = if tab.view.is_muted() {
                "audio-volume-muted-symbolic"
            } else {
                "audio-volume-high-symbolic"
            };
            page.set_indicator_icon(Some(&gio::ThemedIcon::new(icon)));
            page.set_indicator_tooltip(if tab.view.is_muted() {
                "Unmute Tab"
            } else {
                "Mute Tab"
            });
            page.set_indicator_activatable(true);
        } else {
            page.set_indicator_icon(None::<&gio::Icon>);
        }
    }

    pub fn tab_changed(&self, tab: &Rc<Tab>, what: Change) {
        if matches!(what, Change::Title | Change::Uri | Change::Loading) {
            self.refresh_page(tab);
        }
        let is_current = self.current().is_some_and(|c| Rc::ptr_eq(&c, tab));
        if !is_current {
            return;
        }
        match what {
            Change::Progress => self.sync_progress(),
            Change::Zoom => self.sync_zoom(),
            Change::Title => self.sync_title(),
            Change::Reader => self.sync_reader(),
            _ => self.sync(),
        }
    }

    pub fn sync(&self) {
        let tab = self.current();
        self.back
            .set_sensitive(tab.as_ref().is_some_and(|t| t.view.can_go_back()));
        self.forward
            .set_sensitive(tab.as_ref().is_some_and(|t| t.view.can_go_forward()));
        let loading = tab.as_ref().is_some_and(|t| t.view.is_loading());
        self.reload.set_icon_name(if loading {
            "process-stop-symbolic"
        } else {
            "view-refresh-symbolic"
        });
        self.reload
            .set_tooltip_text(Some(if loading { "Stop" } else { "Reload" }));
        let has_home = self
            .app()
            .is_some_and(|a| a.settings.borrow().home_url().is_some() && a.web_app.is_none());
        self.home.set_visible(has_home);
        self.sync_reader();
        let web_page = tab.as_ref().is_some_and(|t| {
            let u = t.uri();
            u.starts_with("https://") || u.starts_with("http://")
        });
        if let Some(a) = self.win.lookup_action("install-web-app").and_downcast::<gio::SimpleAction>() {
            a.set_enabled(web_page && !self.private);
        }
        self.sync_entry();
        self.sync_progress();
        self.sync_zoom();
        self.sync_title();
        self.sync_bookmark_state();
    }

    fn sync_reader(&self) {
        let (available, active) = self
            .current()
            .map(|t| (t.reader_available(), t.reader_active.get()))
            .unwrap_or((false, false));
        self.reader.set_visible(available);
        self.reader.set_tooltip_text(Some(if active {
            "Leave Reader View"
        } else {
            "Reader View"
        }));
        if active {
            self.reader.add_css_class("suggested-action");
        } else {
            self.reader.remove_css_class("suggested-action");
        }
    }

    fn sync_title(&self) {
        let title = self.current().map(|t| t.title()).unwrap_or_default();
        if let (Some(t), Some(app)) = (&self.app_title, self.app()) {
            t.set_title(&title);
            t.set_subtitle(
                &app.web_app
                    .as_ref()
                    .and_then(|w| urlinput::host_of(&w.url))
                    .unwrap_or_default(),
            );
            self.win.set_title(Some(&title));
            return;
        }
        let suffix = if self.private {
            "Private Browsing"
        } else {
            "Web"
        };
        if title.is_empty() {
            self.win.set_title(Some(suffix));
        } else {
            self.win.set_title(Some(&format!("{title} — {suffix}")));
        }
    }

    fn set_entry_text(&self, text: &str) {
        self.setting_entry.set(true);
        self.entry.set_text(text);
        self.setting_entry.set(false);
    }

    fn sync_entry(&self) {
        let Some(t) = self.current() else {
            self.set_entry_text("");
            return;
        };
        if !self.editing.get() {
            self.set_entry_text(&display_uri(&t.uri()));
        }
        let (icon, tip) = match t.security.get() {
            _ if t.is_start_page() => ("web-browser-symbolic", "Start page"),
            Security::Secure => ("channel-secure-symbolic", "Secure connection"),
            Security::Insecure => ("channel-insecure-symbolic", "Connection is not secure"),
            Security::Broken => ("channel-insecure-symbolic", "Certificate problems"),
            Security::Local => ("text-x-generic-symbolic", "Local page"),
        };
        self.entry
            .set_icon_from_icon_name(gtk::EntryIconPosition::Primary, Some(icon));
        self.entry
            .set_icon_tooltip_text(gtk::EntryIconPosition::Primary, Some(tip));
    }

    fn sync_progress(&self) {
        let p = self
            .current()
            .filter(|t| t.view.is_loading())
            .map(|t| t.view.estimated_load_progress())
            .unwrap_or(0.0);
        self.entry
            .set_progress_fraction(if p >= 1.0 { 0.0 } else { p });
    }

    fn sync_zoom(&self) {
        let z = self.current().map(|t| t.view.zoom_level()).unwrap_or(1.0);
        self.zoom_label.set_text(&format!("{:.0}%", z * 100.0));
    }

    pub fn sync_bookmark_state(&self) {
        let Some(app) = self.app() else { return };
        let uri = self.current().map(|t| t.uri()).unwrap_or_default();
        let bookmarkable =
            !uri.is_empty() && !uri.starts_with("helix-about:") && uri != "about:blank";
        let marked = bookmarkable && app.bookmarks.borrow().contains(&uri);
        self.entry.set_icon_from_icon_name(
            gtk::EntryIconPosition::Secondary,
            bookmarkable.then_some(if marked {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            }),
        );
        self.entry.set_icon_tooltip_text(
            gtk::EntryIconPosition::Secondary,
            Some(if marked {
                "Edit bookmark"
            } else {
                "Bookmark this page"
            }),
        );
    }

    fn zoom(&self, dir: i32) {
        let default = self
            .app()
            .map(|a| a.settings.borrow().default_zoom)
            .unwrap_or(1.0);
        self.with_view(|v| {
            let z = if dir == 0 {
                default
            } else {
                settings::zoom_step(v.zoom_level(), dir)
            };
            v.set_zoom_level(z);
        });
    }

    // ---- address entry ----------------------------------------------------

    fn entry_key(&self, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let visible = self.suggest_pop.is_visible();
        match key {
            gdk::Key::Down | gdk::Key::Up if visible => {
                let n = self.suggestions.borrow().len() as i32;
                if n == 0 {
                    return glib::Propagation::Stop;
                }
                let cur = self
                    .suggest_list
                    .selected_row()
                    .map(|r| r.index())
                    .unwrap_or(-1);
                let next = if key == gdk::Key::Down {
                    (cur + 1).min(n - 1)
                } else {
                    (cur - 1).max(-1)
                };
                match self.suggest_list.row_at_index(next) {
                    Some(row) if next >= 0 => {
                        self.suggest_list.select_row(Some(&row));
                        if let Some(url) = self.suggestions.borrow().get(next as usize) {
                            self.set_entry_text(url);
                            self.entry.set_position(-1);
                        }
                    }
                    _ => self.suggest_list.unselect_all(),
                }
                glib::Propagation::Stop
            }
            gdk::Key::Escape => {
                self.suggest_pop.popdown();
                self.editing.set(false);
                self.sync_entry();
                self.with_view(|v| {
                    v.grab_focus();
                });
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
                if mods
                    .intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::CONTROL_MASK) =>
            {
                self.entry_activated(true);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn entry_activated(&self, new_tab: bool) {
        let selected = self
            .suggest_list
            .selected_row()
            .filter(|_| self.suggest_pop.is_visible());
        if let Some(row) = selected {
            self.activate_suggestion(row.index() as usize, new_tab);
            return;
        }
        let Some(app) = self.app() else { return };
        let text = self.entry.text();
        let Some(uri) = app.resolve_input(&text) else {
            return;
        };
        self.navigate(&uri, new_tab);
    }

    fn activate_suggestion(&self, index: usize, new_tab: bool) {
        let url = self.suggestions.borrow().get(index).cloned();
        if let Some(url) = url {
            self.navigate(&url, new_tab);
        }
    }

    fn navigate(&self, uri: &str, new_tab: bool) {
        self.suggest_pop.popdown();
        self.editing.set(false);
        match self.current().filter(|_| !new_tab) {
            Some(t) => t.load(uri),
            None => {
                self.open_uri(uri, true);
            }
        }
        self.sync_entry();
        self.with_view(|v| {
            v.grab_focus();
        });
    }

    fn update_suggestions(&self) {
        let Some(app) = self.app() else { return };
        let text = self.entry.text().to_string();
        while let Some(row) = self.suggest_list.row_at_index(0) {
            self.suggest_list.remove(&row);
        }
        let mut urls: Vec<String> = Vec::new();
        let query = text.trim();
        if query.is_empty() {
            self.suggestions.replace(urls);
            self.suggest_pop.popdown();
            return;
        }
        let home = glib::home_dir().to_string_lossy().into_owned();
        let engine = app.settings.borrow().engine();
        let mut rows: Vec<(String, String, &'static str, String)> = Vec::new();
        match urlinput::classify(query, &home) {
            Some(urlinput::Input::Search(q)) => rows.push((
                format!("Search {} for “{}”", engine.name, q),
                String::new(),
                "system-search-symbolic",
                app.resolve_input(query).unwrap_or_else(|| engine.search_url(&q)),
            )),
            Some(urlinput::Input::Url(u)) => {
                rows.push((u.clone(), String::new(), "web-browser-symbolic", u))
            }
            None => {}
        }
        for b in app.bookmarks.borrow().search(query).into_iter().take(3) {
            let title = if b.title.is_empty() {
                b.url.clone()
            } else {
                b.title.clone()
            };
            rows.push((title, b.url.clone(), "starred-symbolic", b.url));
        }
        if !self.private || app.history.borrow().len() > 0 {
            for e in app
                .history
                .borrow()
                .suggest(query, MAX_SUGGESTIONS, store::now())
            {
                let title = if e.title.is_empty() {
                    e.url.clone()
                } else {
                    e.title.clone()
                };
                rows.push((title, e.url.clone(), "document-open-recent-symbolic", e.url));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for (title, subtitle, icon, url) in rows {
            if urls.len() > MAX_SUGGESTIONS || !seen.insert(url.clone()) {
                continue;
            }
            let (content, _) = two_line_row(&title, &subtitle, icon);
            let row = gtk::ListBoxRow::builder()
                .child(&content)
                .focusable(false)
                .build();
            self.suggest_list.append(&row);
            urls.push(url);
        }
        self.suggestions.replace(urls);
        let width = self.entry.width().max(320);
        self.suggest_list.set_size_request(width - 24, -1);
        self.suggest_pop.popup();
        // Keep typing in the entry.
        self.entry.grab_focus_without_selecting();
    }

    // ---- site info & bookmarks -------------------------------------------

    fn anchor(&self, pop: &gtk::Popover, pos: gtk::EntryIconPosition) {
        let rect = self.entry.icon_area(pos);
        pop.set_pointing_to(Some(&rect));
    }

    fn show_site_info(&self) {
        let (Some(app), Some(t)) = (self.app(), self.current()) else {
            return;
        };
        let uri = t.uri();
        let vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .width_request(300)
            .build();
        let host = urlinput::host_of(&uri).unwrap_or_else(|| display_uri(&uri));
        let heading = gtk::Label::builder()
            .label(if host.is_empty() {
                "Start Page".to_string()
            } else {
                host
            })
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        heading.add_css_class("heading");
        vbox.append(&heading);
        let status = match t.security.get() {
            _ if t.is_start_page() => "This is a page built into Web.",
            Security::Secure => "Your connection to this site is secure.",
            Security::Insecure => "Your connection to this site is not encrypted. Do not enter passwords or other sensitive information.",
            Security::Broken => "This site's security certificate has problems. The connection may not be private.",
            Security::Local => "This page is stored on your computer.",
        };
        vbox.append(
            &gtk::Label::builder()
                .label(status)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(40)
                .build(),
        );
        if let Some((cert, flags)) = t.view.tls_info() {
            let mut lines = Vec::new();
            if let Some(s) = cert.subject_name() {
                lines.push(format!("Issued to: {s}"));
            }
            if let Some(s) = cert.issuer_name() {
                lines.push(format!("Issued by: {s}"));
            }
            if let Some(d) = cert.not_valid_after().and_then(|d| d.format("%x").ok()) {
                lines.push(format!("Expires: {d}"));
            }
            if !flags.is_empty() {
                lines.push(format!("Problems: {flags:?}"));
            }
            let l = gtk::Label::builder()
                .label(lines.join("\n"))
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .build();
            l.add_css_class("dim-label");
            l.add_css_class("caption");
            vbox.append(&l);
        }
        if let Some(origin) = urlinput::origin_of(&uri) {
            let perms = app.permissions.borrow().for_origin(&origin);
            if !perms.is_empty() {
                let h = gtk::Label::builder()
                    .label("Permissions")
                    .xalign(0.0)
                    .build();
                h.add_css_class("heading");
                vbox.append(&h);
                for (kind, allowed) in perms {
                    let row = gtk::Box::builder().spacing(6).build();
                    let l = gtk::Label::builder()
                        .label(format!(
                            "{}: {}",
                            kind.label(),
                            if allowed { "Allowed" } else { "Denied" }
                        ))
                        .xalign(0.0)
                        .hexpand(true)
                        .build();
                    let reset = gtk::Button::with_label("Reset");
                    reset.add_css_class("flat");
                    let weak_app = Rc::downgrade(&app);
                    let origin = origin.clone();
                    let row_c = row.clone();
                    reset.connect_clicked(move |_| {
                        if let Some(app) = weak_app.upgrade() {
                            app.permissions.borrow_mut().reset(&origin, kind);
                            app.save_permissions();
                        }
                        row_c.set_visible(false);
                    });
                    row.append(&l);
                    row.append(&reset);
                    vbox.append(&row);
                }
            }
        }
        self.info_pop.set_child(Some(&vbox));
        self.anchor(&self.info_pop, gtk::EntryIconPosition::Primary);
        self.info_pop.popup();
    }

    fn bookmark_current(&self) {
        let (Some(app), Some(t)) = (self.app(), self.current()) else {
            return;
        };
        let uri = t.uri();
        if uri.is_empty() || t.is_start_page() {
            return;
        }
        let existing = app.bookmarks.borrow().get(&uri).cloned();
        let bm = match existing {
            Some(b) => b,
            None => {
                let b = Bookmark {
                    url: uri.clone(),
                    title: t.title(),
                    tags: Vec::new(),
                    added: store::now(),
                };
                app.bookmarks.borrow_mut().add(b.clone());
                app.bookmarks_changed();
                b
            }
        };
        self.bm_title.set_text(&bm.title);
        self.bm_tags.set_text(&bm.tags.join(", "));
        self.bm_editing.replace(Some(uri));
        self.anchor(&self.bm_pop, gtk::EntryIconPosition::Secondary);
        self.bm_pop.popup();
        self.bm_title.grab_focus();
    }

    fn bookmark_popover_closed(&self) {
        let Some(url) = self.bm_editing.take() else {
            return;
        };
        let Some(app) = self.app() else { return };
        let title = self.bm_title.text().to_string();
        let tags = bookmarks::parse_tags(&self.bm_tags.text());
        if app.bookmarks.borrow_mut().update(&url, &title, &tags) {
            app.bookmarks_changed();
        }
    }

    fn fill_bookmarks_list(&self) {
        let Some(app) = self.app() else { return };
        while let Some(row) = self.bm_list.row_at_index(0) {
            self.bm_list.remove(&row);
        }
        let items = app.bookmarks.borrow().search(&self.bm_search.text());
        if items.is_empty() {
            let l = gtk::Label::builder()
                .label("No bookmarks")
                .margin_top(12)
                .margin_bottom(12)
                .build();
            l.add_css_class("dim-label");
            let row = gtk::ListBoxRow::builder()
                .child(&l)
                .activatable(false)
                .selectable(false)
                .build();
            self.bm_list.append(&row);
            return;
        }
        for b in items {
            let title = if b.title.is_empty() {
                b.url.clone()
            } else {
                b.title.clone()
            };
            let sub = if b.tags.is_empty() {
                b.url.clone()
            } else {
                format!("{} · {}", b.tags.join(", "), b.url)
            };
            let (content, _) = two_line_row(&title, &sub, "starred-symbolic");
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove Bookmark")
                .valign(gtk::Align::Center)
                .build();
            remove.add_css_class("flat");
            content.append(&remove);
            let row = gtk::ListBoxRow::builder()
                .child(&content)
                .name(b.url.as_str())
                .build();
            let url = b.url.clone();
            let cb = self.weak_fn(move |w, ()| {
                if let Some(app) = w.app() {
                    app.bookmarks.borrow_mut().remove(&url);
                    app.bookmarks_changed();
                }
                w.fill_bookmarks_list();
            });
            remove.connect_clicked(move |_| cb(()));
            self.bm_list.append(&row);
        }
    }

    // ---- find -------------------------------------------------------------

    fn find_options() -> u32 {
        (webkit6::FindOptions::CASE_INSENSITIVE | webkit6::FindOptions::WRAP_AROUND).bits()
    }

    fn find_changed(&self) {
        let text = self.find_entry.text().to_string();
        self.with_view(|v| {
            let Some(fc) = v.find_controller() else {
                return;
            };
            if text.is_empty() {
                fc.search_finish();
            } else {
                fc.count_matches(&text, Self::find_options(), u32::MAX);
                fc.search(&text, Self::find_options(), u32::MAX);
            }
        });
        if text.is_empty() {
            self.find_label.set_text("");
        }
    }

    fn find_step(&self, forward: bool) {
        if !self.find_bar.is_search_mode() {
            self.find_bar.set_search_mode(true);
            self.find_entry.grab_focus();
            return;
        }
        self.with_view(|v| {
            if let Some(fc) = v.find_controller() {
                if forward {
                    fc.search_next();
                } else {
                    fc.search_previous();
                }
            }
        });
    }

    pub fn find_result(&self, tab: &Rc<Tab>, matches: Option<u32>) {
        if !self.current().is_some_and(|c| Rc::ptr_eq(&c, tab)) || self.find_entry.text().is_empty()
        {
            return;
        }
        let text = match matches {
            Some(0) => "Not found".to_string(),
            Some(1) => "1 match".to_string(),
            Some(n) => format!("{n} matches"),
            None => String::new(),
        };
        self.find_label.set_text(&text);
    }

    // ---- downloads --------------------------------------------------------

    pub fn add_download(&self, d: &webkit6::Download, path: &Path) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let row_box = gtk::Box::builder()
            .spacing(10)
            .margin_start(10)
            .margin_end(6)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        row_box.append(&gtk::Image::from_icon_name("folder-download-symbolic"));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        let name_label = gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        let progress = gtk::ProgressBar::new();
        let status = gtk::Label::builder().label("Starting…").xalign(0.0).build();
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        text.append(&name_label);
        text.append(&progress);
        text.append(&status);
        row_box.append(&text);
        let cancel = gtk::Button::builder()
            .icon_name("process-stop-symbolic")
            .tooltip_text("Cancel")
            .valign(gtk::Align::Center)
            .build();
        let open = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Open")
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let folder = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Open Folder")
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        for b in [&cancel, &open, &folder] {
            b.add_css_class("flat");
            row_box.append(b);
        }
        let row = gtk::ListBoxRow::builder()
            .child(&row_box)
            .activatable(false)
            .build();
        self.dl_list.prepend(&row);
        self.dl_button.set_visible(true);
        self.dl_button.popup();

        let dc = d.clone();
        cancel.connect_clicked(move |_| dc.cancel());
        let file = gio::File::for_path(path);
        let win = self.win.clone();
        let f = file.clone();
        open.connect_clicked(move |_| {
            gtk::FileLauncher::new(Some(&f)).launch(Some(&win), None::<&gio::Cancellable>, |r| {
                if let Err(e) = r {
                    eprintln!("helix-web: could not open download: {e}");
                }
            });
        });
        let win = self.win.clone();
        folder.connect_clicked(move |_| {
            gtk::FileLauncher::new(Some(&file)).open_containing_folder(
                Some(&win),
                None::<&gio::Cancellable>,
                |r| {
                    if let Err(e) = r {
                        eprintln!("helix-web: could not open folder: {e}");
                    }
                },
            );
        });

        let (p, s) = (progress.clone(), status.clone());
        d.connect_received_data(move |d, _| {
            p.set_fraction(d.estimated_progress());
            let got = d.received_data_length();
            let total = d.response().map(|r| r.content_length()).unwrap_or(0);
            let text = if total > 0 {
                format!("{} of {}", glib::format_size(got), glib::format_size(total))
            } else {
                glib::format_size(got).to_string()
            };
            s.set_text(&text);
        });
        let failed = Rc::new(Cell::new(false));
        let (p, s, c, o, fo, r, fl) = (
            progress.clone(),
            status.clone(),
            cancel.clone(),
            open.clone(),
            folder.clone(),
            row.clone(),
            failed.clone(),
        );
        d.connect_finished(move |_| {
            r.set_widget_name("finished");
            c.set_visible(false);
            if fl.get() {
                return;
            }
            p.set_fraction(1.0);
            s.set_text("Finished");
            o.set_visible(true);
            fo.set_visible(true);
        });
        let (s, c, r) = (status, cancel, row);
        d.connect_failed(move |_, err| {
            failed.set(true);
            r.set_widget_name("finished");
            c.set_visible(false);
            if err.matches(webkit6::DownloadError::CancelledByUser) {
                s.set_text("Cancelled");
            } else {
                s.set_text(&format!("Failed: {}", err.message()));
            }
        });
    }
}
