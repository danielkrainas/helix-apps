//! Terminal windows and tabs.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::a11y::{self, Urgency};
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;
use vte4::prelude::*;

use crate::app::{App, Launch};
use crate::paths;
use crate::process::{self, Tint};
use crate::search;
use helix_app_common::i18n::tr;
use helix_app_common::links;
use helix_app_common::nc_;
use helix_app_common::trf;

pub struct Tab {
    pub root: gtk::Box,
    pub term: vte4::Terminal,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    match_case: gtk::ToggleButton,
    use_regex: gtk::ToggleButton,
    banner: adw::Banner,
    /// Pid of the spawned shell/command while it runs.
    child_pid: Cell<Option<i32>>,
    /// Started from `-- COMMAND`: keep the tab when it exits.
    is_command: bool,
    /// Link under the pointer at the last button press.
    link: RefCell<Option<String>>,
    /// Whether `link` is an OSC 8 hyperlink: a target written by the
    /// program, not the text on screen.
    link_is_hyperlink: Cell<bool>,
    /// The window currently showing this tab.
    window: RefCell<Weak<Window>>,
}

impl Tab {
    fn window(&self) -> Option<Rc<Window>> {
        self.window.borrow().upgrade()
    }

    /// Makes the context menu's link the selected URL, or nothing.
    fn link_from_selection(&self) {
        let link = if self.term.has_selection() {
            self.term
                .text_selected(vte4::Format::Text)
                .and_then(|s| search::url_in_selection(&s))
        } else {
            None
        };
        *self.link.borrow_mut() = link;
        self.link_is_hyperlink.set(false);
        if let Some(w) = self.window() {
            w.update_link_actions();
        }
    }

    /// Pty foreground process group, if a child is running.
    fn foreground(&self) -> Option<i32> {
        self.child_pid.get()?;
        let pty = self.term.pty()?;
        process::foreground_pgrp(pty.fd().as_raw_fd())
    }

    /// Whether a program other than the shell is in the foreground.
    pub fn is_busy(&self) -> bool {
        let Some(pid) = self.child_pid.get() else {
            return false;
        };
        match self.term.pty() {
            Some(pty) => process::is_busy(pty.fd().as_raw_fd(), pid),
            None => false,
        }
    }

    fn busy_name(&self) -> String {
        self.foreground()
            .and_then(process::command_name)
            .unwrap_or_else(|| tr("A process").into())
    }

    /// Current working directory: OSC 7 if reported, else the shell's cwd.
    pub fn cwd(&self) -> Option<PathBuf> {
        let host = glib::host_name();
        self.term
            .current_directory_uri()
            .and_then(|u| paths::local_dir_from_uri(&u, &host))
            .or_else(|| self.child_pid.get().and_then(process::process_cwd))
    }

    pub fn title(&self) -> String {
        let t = self.term.window_title();
        paths::tab_title(t.as_deref(), self.cwd().as_deref(), &glib::home_dir())
    }

    /// Shows `error` on the find entry (red, invalid state, spoken), or
    /// clears it.
    fn set_search_error(&self, error: Option<&str>) {
        a11y::set_input_error(&self.search_entry, None, error);
    }

    /// Compiles the find text into the terminal's search regex. Returns
    /// whether there is something to search for; an invalid expression is
    /// reported on the entry. A valid one leaves the entry's state to the
    /// next search, so an unchanged "No matches" is not spoken again on
    /// every key press.
    fn update_search(&self) -> bool {
        let text = self.search_entry.text();
        if text.is_empty() {
            self.term.search_set_regex(None, 0);
            self.set_search_error(None);
            return false;
        }
        let pattern = search::search_pattern(&text, self.use_regex.is_active());
        match vte4::Regex::for_search(&pattern, search::search_flags(self.match_case.is_active())) {
            Ok(re) => {
                self.term.search_set_regex(Some(&re), 0);
                self.term.search_set_wrap_around(true);
                true
            }
            Err(_) => {
                self.term.search_set_regex(None, 0);
                self.set_search_error(Some(tr("Invalid regular expression")));
                false
            }
        }
    }

    /// Searches towards older output (`up`) or newer output.
    fn find(&self, up: bool) {
        if self.term.search_get_regex().is_none() && !self.update_search() {
            return;
        }
        let found = if up {
            self.term.search_find_previous()
        } else {
            self.term.search_find_next()
        };
        self.set_search_error((!found).then_some(tr("No matches")));
    }

    /// Shows `title` in the tab's banner and tells screen readers.
    fn show_banner(&self, title: &str, urgency: Urgency) {
        self.banner.set_title(title);
        self.banner.set_revealed(true);
        a11y::announce(&self.banner, title, urgency);
    }
}

type WindowAction = Box<dyn Fn(&Rc<Window>)>;

/// The main menu's Follow System / Light / Dark switcher: toggle buttons
/// for the `app.style` action, so exactly one is pressed.
fn style_switcher() -> gtk::Box {
    let row = gtk::Box::builder()
        .css_classes(["linked"])
        .homogeneous(true)
        .margin_start(6)
        .margin_end(6)
        .accessible_role(gtk::AccessibleRole::Group)
        .build();
    for (label, name, target) in [
        (
            tr(nc_!("style", "System")),
            tr("Follow System Style"),
            "auto",
        ),
        (tr("Light"), tr("Light Style"), "day"),
        (tr("Dark"), tr("Dark Style"), "night"),
    ] {
        let b = gtk::ToggleButton::builder()
            .label(label)
            .action_name("app.style")
            .action_target(&target.to_variant())
            .tooltip_text(name)
            .build();
        a11y::set_accessible_label(&b, name);
        row.append(&b);
    }
    a11y::set_accessible_label(&row, tr("Style"));
    row
}

/// The main menu's Zoom Out / level (resets) / Zoom In row.
fn zoom_row(level: &gtk::Button) -> gtk::Box {
    let zoom_out = a11y::icon_button("zoom-out-symbolic", tr("Zoom Out"));
    zoom_out.set_action_name(Some("win.zoom-out"));
    let zoom_in = a11y::icon_button("zoom-in-symbolic", tr("Zoom In"));
    zoom_in.set_action_name(Some("win.zoom-in"));
    for b in [&zoom_out, &zoom_in] {
        b.add_css_class("flat");
        b.add_css_class("circular");
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&zoom_out);
    row.append(level);
    row.append(&zoom_in);
    row
}

/// "125%" for a font scale of 1.25.
pub fn zoom_percent(scale: f64) -> String {
    format!("{}%", (scale * 100.0).round() as i64)
}

pub struct Window {
    this: Weak<Window>,
    app: Weak<App>,
    pub win: adw::ApplicationWindow,
    tab_view: adw::TabView,
    overview: adw::TabOverview,
    pub tabs: RefCell<Vec<Rc<Tab>>>,
    find_button: gtk::ToggleButton,
    /// Set while `find_button` is being updated from the find bar.
    syncing_find: Cell<bool>,
    title: adw::WindowTitle,
    tint_icon: gtk::Image,
    /// The words for the current tint ("Running as root", …), or empty.
    tint_status: RefCell<String>,
    tint: Cell<Tint>,
    /// Set once closing has been confirmed.
    closing: Cell<bool>,
    /// The main menu's zoom level, which also resets the zoom.
    zoom_level: gtk::Button,
}

impl Window {
    /// Shows the zoom level in the main menu.
    pub fn update_zoom(&self, scale: f64) {
        let text = zoom_percent(scale);
        self.zoom_level.set_label(&text);
        a11y::set_accessible_label(
            &self.zoom_level,
            &trf!("Reset Zoom, now {text}", text = text),
        );
    }

    pub fn new(app: &Rc<App>) -> Rc<Window> {
        let win = adw::ApplicationWindow::builder()
            .application(&app.gtk)
            .title(tr("Terminal"))
            .width_request(helix_app_common::MIN_WIDTH)
            .height_request(helix_app_common::MIN_HEIGHT)
            .build();
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder()
            .view(&tab_view)
            .autohide(true)
            .build();

        let header = adw::HeaderBar::new();
        // Title, plus an icon and subtitle that say (not only tint) when
        // the tab runs as root or on another host.
        let title = adw::WindowTitle::new(tr("Terminal"), "");
        win.bind_property("title", &title, "title")
            .sync_create()
            .build();
        let tint_icon = gtk::Image::builder()
            .accessible_role(gtk::AccessibleRole::Presentation)
            .visible(false)
            .build();
        let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        title_box.append(&tint_icon);
        title_box.append(&title);
        header.set_title_widget(Some(&title_box));
        let new_tab = gtk::Button::builder()
            .icon_name("tab-new-symbolic")
            .tooltip_text(tr("New Tab"))
            .action_name("win.new-tab")
            .build()
            .labelled(tr("New Tab"));
        header.pack_start(&new_tab);

        let menu = gio::Menu::new();
        // Style switcher and zoom row, as custom children (added below).
        let s0 = gio::Menu::new();
        for id in ["style", "zoom"] {
            let item = gio::MenuItem::new(None, None);
            item.set_attribute_value("custom", Some(&id.to_variant()));
            s0.append_item(&item);
        }
        menu.append_section(None, &s0);
        let s1 = gio::Menu::new();
        s1.append(Some(tr("_New Window")), Some("win.new-window"));
        s1.append(Some(tr("_Show All Tabs")), Some("win.tab-overview"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append(Some(tr("_Preferences")), Some("app.preferences"));
        s2.append(Some(tr("_Keyboard Shortcuts")), Some("win.show-shortcuts"));
        s2.append(Some(tr("_About Terminal")), Some("app.about"));
        menu.append_section(None, &s2);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text(tr("Main Menu"))
            .menu_model(&menu)
            .primary(true)
            .build()
            .labelled(tr("Main Menu"));
        let zoom_level = gtk::Button::builder()
            .label("100%")
            .action_name("win.zoom-reset")
            .tooltip_text(tr("Reset Zoom"))
            .hexpand(true)
            .css_classes(["flat"])
            .build();
        if let Some(pm) = menu_button.popover().and_downcast::<gtk::PopoverMenu>() {
            pm.add_child(&style_switcher(), "style");
            pm.add_child(&zoom_row(&zoom_level), "zoom");
        }
        header.pack_end(&menu_button);
        let tab_button = adw::TabButton::builder()
            .view(&tab_view)
            .action_name("overview.open")
            .build();
        header.pack_end(&tab_button);
        // Shows and hides the current tab's find bar; its pressed state
        // follows that bar.
        let find = a11y::icon_toggle_button("edit-find-symbolic", tr("Find"));
        header.pack_end(&find);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        toolbar.set_content(Some(&tab_view));

        let overview = adw::TabOverview::builder()
            .view(&tab_view)
            .child(&toolbar)
            .enable_new_tab(true)
            .build();
        win.set_content(Some(&overview));

        let w = Rc::new_cyclic(|this| Window {
            this: this.clone(),
            app: Rc::downgrade(app),
            win,
            tab_view,
            overview,
            tabs: RefCell::default(),
            find_button: find,
            syncing_find: Cell::new(false),
            title,
            tint_icon,
            tint_status: RefCell::default(),
            tint: Cell::new(Tint::None),
            closing: Cell::new(false),
            zoom_level,
        });
        app.apply_to_window(&w.win);
        w.install_actions();
        w.connect_signals();
        let wk = Rc::downgrade(&w);
        w.find_button.connect_toggled(move |b| {
            let Some(w) = wk.upgrade() else { return };
            let open = w
                .current_tab()
                .is_some_and(|t| t.search_bar.is_search_mode());
            if !w.syncing_find.get() && b.is_active() != open {
                WidgetExt::activate_action(&w.win, "win.find", None).ok();
            }
        });
        w.start_tint_poll();
        let style = adw::StyleManager::default();
        let set_hc = {
            let win = w.win.downgrade();
            move |style: &adw::StyleManager| {
                if let Some(win) = win.upgrade() {
                    if style.is_high_contrast() {
                        win.add_css_class("high-contrast");
                    } else {
                        win.remove_css_class("high-contrast");
                    }
                }
            }
        };
        set_hc(&style);
        style.connect_high_contrast_notify(set_hc);
        w
    }

    fn app(&self) -> Rc<App> {
        self.app.upgrade().expect("app alive")
    }

    pub fn current_tab(&self) -> Option<Rc<Tab>> {
        let page = self.tab_view.selected_page()?;
        self.tab_for_child(&page.child())
    }

    fn tab_for_child(&self, child: &gtk::Widget) -> Option<Rc<Tab>> {
        self.tabs
            .borrow()
            .iter()
            .find(|t| t.root.upcast_ref::<gtk::Widget>() == child)
            .cloned()
    }

    fn current_cwd(&self) -> Option<PathBuf> {
        self.current_tab().and_then(|t| t.cwd())
    }

    // --- tabs -----------------------------------------------------------

    /// Creates a tab running `launch` and selects it.
    pub fn new_tab(&self, launch: Launch) -> adw::TabPage {
        let app = self.app();
        let term = vte4::Terminal::builder()
            .hexpand(true)
            .vexpand(true)
            .allow_hyperlink(true)
            .scroll_on_keystroke(true)
            .scroll_on_output(false)
            .enable_sixel(true)
            .pointer_autohide(true)
            .build();
        app.apply_to(&term);
        if let Ok(re) = vte4::Regex::for_match(search::URL_PATTERN, search::BASE_FLAGS) {
            let tag = term.match_add_regex(&re, 0);
            term.match_set_cursor_name(tag, "pointer");
        }
        // Show where an OSC 8 hyperlink really points, as GNOME Terminal
        // does: its text can be anything.
        term.connect_hyperlink_hover_uri_notify(|t| {
            let tip = t.hyperlink_hover_uri().map(|u| links::display_form(&u));
            t.set_tooltip_text(tip.as_deref());
        });

        let scrolled = gtk::ScrolledWindow::builder()
            .child(&term)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        // Find bar.
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(tr("Find"))
            .hexpand(true)
            .build()
            .labelled(tr("Find"));
        let prev = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text(tr("Previous Match (older output)"))
            .build()
            .labelled(tr("Previous Match (older output)"));
        let next = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text(tr("Next Match (newer output)"))
            .build()
            .labelled(tr("Next Match (newer output)"));
        // The glyphs are only the visible face; the names are spoken.
        let match_case = gtk::ToggleButton::builder()
            .label(tr("Aa"))
            .tooltip_text(tr("Match Case"))
            .build()
            .labelled(tr("Match Case"));
        let use_regex = gtk::ToggleButton::builder()
            .label(".*")
            .tooltip_text(tr("Regular Expression"))
            .build()
            .labelled(tr("Regular Expression"));
        let nav = gtk::Box::builder().css_classes(["linked"]).build();
        nav.append(&prev);
        nav.append(&next);
        let bar_box = gtk::Box::builder().spacing(6).build();
        bar_box.append(&search_entry);
        bar_box.append(&nav);
        bar_box.append(&match_case);
        bar_box.append(&use_regex);
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .child(&bar_box)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(&clamp)
            .show_close_button(true)
            .build();
        search_bar.connect_entry(&search_entry);

        let banner = adw::Banner::builder().button_label(tr("_Close")).build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&banner);
        root.append(&search_bar);
        root.append(&scrolled);

        let tab = Rc::new(Tab {
            root,
            term,
            search_bar,
            search_entry,
            match_case,
            use_regex,
            banner,
            child_pid: Cell::new(None),
            is_command: !launch.command.is_empty(),
            link: RefCell::default(),
            link_is_hyperlink: Cell::new(false),
            window: RefCell::new(self.this.clone()),
        });
        self.tabs.borrow_mut().push(tab.clone());
        self.setup_tab(&tab, &prev, &next);

        let page = self.tab_view.append(&tab.root);
        page.set_title(tr("Terminal"));
        self.tab_view.set_selected_page(&page);
        self.spawn(&tab, launch);
        tab.term.grab_focus();
        page
    }

    fn spawn(&self, tab: &Rc<Tab>, launch: Launch) {
        let argv: Vec<String> = if launch.command.is_empty() {
            let setting = self
                .app
                .upgrade()
                .map(|a| a.prefs.get("shell", Vec::<String>::new()));
            process::shell_argv(setting.unwrap_or_default())
        } else {
            launch.command
        };
        let cwd = launch
            .cwd
            .filter(|p| p.is_dir())
            .unwrap_or_else(glib::home_dir)
            .to_string_lossy()
            .into_owned();
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let env = ["COLORTERM=truecolor", "TERM_PROGRAM=helix-terminal"];
        let tk = Rc::downgrade(tab);
        let what = argv[0].clone();
        tab.term.spawn_async(
            vte4::PtyFlags::DEFAULT,
            Some(&cwd),
            &argv_refs,
            &env,
            glib::SpawnFlags::SEARCH_PATH,
            || {},
            -1,
            None::<&gio::Cancellable>,
            move |res| {
                let Some(tab) = tk.upgrade() else { return };
                match res {
                    Ok(pid) => {
                        tab.child_pid.set(Some(pid.0));
                        if let Some(w) = tab.window() {
                            w.update_titles();
                        }
                    }
                    Err(e) => {
                        let msg = format!(
                            "\x1b[1;31m{}\x1b[0m\r\n",
                            trf!(
                                "Failed to start “{what}”: {error}",
                                what = what,
                                error = e.message()
                            )
                        );
                        tab.term.feed(msg.as_bytes());
                        tab.show_banner(
                            &trf!("Could not start “{what}”", what = what),
                            Urgency::Assertive,
                        );
                    }
                }
            },
        );
    }

    fn setup_tab(&self, tab: &Rc<Tab>, prev: &gtk::Button, next: &gtk::Button) {
        let term = &tab.term;

        let tk = Rc::downgrade(tab);
        term.connect_child_exited(move |_, status| {
            let Some(tab) = tk.upgrade() else { return };
            let Some(w) = tab.window() else { return };
            tab.child_pid.set(None);
            if tab.is_command {
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    status
                };
                tab.show_banner(
                    &trf!("Process exited with status {code}", code = code),
                    Urgency::Polite,
                );
            } else {
                let page = w.tab_view.page(&tab.root);
                w.tab_view.close_page(&page);
            }
        });

        let tk = Rc::downgrade(tab);
        tab.banner.connect_button_clicked(move |_| {
            if let Some((t, w)) = tk.upgrade().and_then(|t| Some((t.clone(), t.window()?))) {
                w.tab_view.close_page(&w.tab_view.page(&t.root));
            }
        });

        let wk = Rc::downgrade(tab);
        term.connect_window_title_changed(move |_| {
            if let Some(w) = wk.upgrade().and_then(|t| t.window()) {
                w.update_titles();
            }
        });
        let wk = Rc::downgrade(tab);
        term.connect_current_directory_uri_changed(move |_| {
            if let Some(w) = wk.upgrade().and_then(|t| t.window()) {
                w.update_titles();
            }
        });

        // Bell.
        let tk = Rc::downgrade(tab);
        term.connect_bell(move |term| {
            let Some(tab) = tk.upgrade() else { return };
            let Some(w) = tab.window() else { return };
            if w.app().visual_bell() {
                term.add_css_class("bell-flash");
                let t = term.clone();
                glib::timeout_add_local_once(Duration::from_millis(150), move || {
                    t.remove_css_class("bell-flash");
                });
            }
            let page = w.tab_view.page(&tab.root);
            if !page.is_selected() {
                page.set_needs_attention(true);
            }
        });

        // Zoom requests from the terminal (Ctrl+scroll).
        let wk = Rc::downgrade(tab);
        term.connect_increase_font_size(move |_| {
            if let Some(w) = wk.upgrade().and_then(|t| t.window()) {
                let app = w.app();
                app.set_font_scale(search::zoom_in(app.font_scale()));
            }
        });
        let wk = Rc::downgrade(tab);
        term.connect_decrease_font_size(move |_| {
            if let Some(w) = wk.upgrade().and_then(|t| t.window()) {
                let app = w.app();
                app.set_font_scale(search::zoom_out(app.font_scale()));
            }
        });
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let wk = Rc::downgrade(tab);
        scroll.connect_scroll(move |c, _, dy| {
            if !c
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
                || dy == 0.0
            {
                return glib::Propagation::Proceed;
            }
            if let Some(w) = wk.upgrade().and_then(|t| t.window()) {
                let app = w.app();
                let s = if dy < 0.0 {
                    search::zoom_in(app.font_scale())
                } else {
                    search::zoom_out(app.font_scale())
                };
                app.set_font_scale(s);
            }
            glib::Propagation::Stop
        });
        term.add_controller(scroll);

        // Links: remember what is under the pointer; Ctrl+click opens it.
        let click = gtk::GestureClick::builder().button(0).build();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tk = Rc::downgrade(tab);
        click.connect_pressed(move |g, _, x, y| {
            let Some(tab) = tk.upgrade() else { return };
            let Some(w) = tab.window() else { return };
            let hyperlink = tab.term.check_hyperlink_at(x, y).map(|s| s.to_string());
            let is_hyperlink = hyperlink.is_some();
            let link = hyperlink.or_else(|| {
                tab.term
                    .check_match_at(x, y)
                    .0
                    .map(|s| search::normalize_url(&s))
            });
            *tab.link.borrow_mut() = link.clone();
            tab.link_is_hyperlink.set(is_hyperlink);
            w.update_link_actions();
            let ctrl = g
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            if g.current_button() == 1 && ctrl {
                if let Some(uri) = link {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    w.open_uri(&uri, is_hyperlink);
                }
            }
        });
        term.add_controller(click);

        // The link offered by the context menu always describes what the
        // menu was opened on: the pointer target for the secondary
        // button (above), otherwise the selection when it is a URL. A
        // selection change replaces the remembered link, and so does
        // opening the menu from the keyboard (Menu / Shift+F10).
        let tk = Rc::downgrade(tab);
        term.connect_selection_changed(move |_| {
            if let Some(tab) = tk.upgrade() {
                tab.link_from_selection();
            }
        });
        let menu_keys = gtk::EventControllerKey::new();
        menu_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tk = Rc::downgrade(tab);
        menu_keys.connect_key_pressed(move |_, key, _, state| {
            let shift_f10 = key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK);
            if key == gdk::Key::Menu || shift_f10 {
                if let Some(tab) = tk.upgrade() {
                    tab.link_from_selection();
                }
            }
            glib::Propagation::Proceed
        });
        term.add_controller(menu_keys);

        let ctx = gio::Menu::new();
        let a = gio::Menu::new();
        a.append(Some(tr("_Open Link")), Some("win.open-link"));
        a.append(Some(tr("Copy _Link")), Some("win.copy-link"));
        ctx.append_section(None, &a);
        let b = gio::Menu::new();
        b.append(Some(tr("_Copy")), Some("win.copy"));
        b.append(Some(tr("_Paste")), Some("win.paste"));
        b.append(Some(tr("Select _All")), Some("win.select-all"));
        ctx.append_section(None, &b);
        let c = gio::Menu::new();
        c.append(Some(tr("_Find…")), Some("win.find"));
        c.append(Some(tr("New _Tab")), Some("win.new-tab"));
        ctx.append_section(None, &c);
        term.set_context_menu_model(Some(&ctx));

        // Drag and drop of files and text.
        let drop = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
        drop.set_types(&[gdk::FileList::static_type(), glib::Type::STRING]);
        let tk = Rc::downgrade(tab);
        drop.connect_drop(move |_, value, _, _| {
            let Some(tab) = tk.upgrade() else {
                return false;
            };
            if let Ok(list) = value.get::<gdk::FileList>() {
                let paths: Vec<String> = list
                    .files()
                    .iter()
                    .map(|f| match f.path() {
                        Some(p) => p.to_string_lossy().into_owned(),
                        None => f.uri().to_string(),
                    })
                    .collect();
                tab.term
                    .paste_text(&paths::dropped_paths_text(paths.iter().map(String::as_str)));
                tab.term.grab_focus();
                return true;
            }
            if let Ok(text) = value.get::<String>() {
                tab.term.paste_text(&text);
                tab.term.grab_focus();
                return true;
            }
            false
        });
        term.add_controller(drop);

        // Find bar.
        let tk = Rc::downgrade(tab);
        tab.search_entry.connect_search_changed(move |_| {
            if let Some(t) = tk.upgrade() {
                if t.update_search() {
                    t.find(true);
                }
            }
        });
        for toggle in [&tab.match_case, &tab.use_regex] {
            let tk = Rc::downgrade(tab);
            toggle.connect_toggled(move |_| {
                if let Some(t) = tk.upgrade() {
                    if t.update_search() {
                        t.set_search_error(None);
                    }
                }
            });
        }
        let tk = Rc::downgrade(tab);
        tab.search_entry.connect_activate(move |_| {
            if let Some(t) = tk.upgrade() {
                t.find(true);
            }
        });
        let tk = Rc::downgrade(tab);
        tab.search_entry.connect_next_match(move |_| {
            if let Some(t) = tk.upgrade() {
                t.find(true);
            }
        });
        let tk = Rc::downgrade(tab);
        tab.search_entry.connect_previous_match(move |_| {
            if let Some(t) = tk.upgrade() {
                t.find(false);
            }
        });
        let keys = gtk::EventControllerKey::new();
        let tk = Rc::downgrade(tab);
        keys.connect_key_pressed(move |_, key, _, state| {
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            if shift
                && matches!(
                    key,
                    gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
                )
            {
                if let Some(t) = tk.upgrade() {
                    t.find(false);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        tab.search_entry.add_controller(keys);
        let tk = Rc::downgrade(tab);
        tab.search_entry.connect_stop_search(move |_| {
            if let Some(t) = tk.upgrade() {
                t.search_bar.set_search_mode(false);
            }
        });
        let tk = Rc::downgrade(tab);
        tab.search_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if let Some(t) = tk.upgrade() {
                    if let Some(w) = t.window() {
                        w.sync_find_button();
                    }
                    if !bar.is_search_mode() {
                        t.term.search_set_regex(None, 0);
                        t.term.unselect_all();
                        t.term.grab_focus();
                    }
                }
            });
        let tk = Rc::downgrade(tab);
        prev.connect_clicked(move |_| {
            if let Some(t) = tk.upgrade() {
                t.find(true);
            }
        });
        let tk = Rc::downgrade(tab);
        next.connect_clicked(move |_| {
            if let Some(t) = tk.upgrade() {
                t.find(false);
            }
        });
    }

    /// Makes the Find toggle show whether the current tab's find bar is
    /// open.
    fn sync_find_button(&self) {
        let open = self
            .current_tab()
            .is_some_and(|t| t.search_bar.is_search_mode());
        if self.find_button.is_active() != open {
            self.syncing_find.set(true);
            self.find_button.set_active(open);
            self.syncing_find.set(false);
        }
    }

    /// Opens a link. URLs matched in the text are shown as they are and
    /// were already limited to known schemes; an OSC 8 hyperlink's target
    /// is not on screen, so anything but a web or mail link is confirmed
    /// first, showing the target.
    fn open_uri(&self, uri: &str, is_hyperlink: bool) {
        if is_hyperlink {
            links::launch(&self.win, uri);
        } else {
            links::launch_now(&self.win, uri);
        }
    }

    fn update_link_actions(&self) {
        let has = self
            .current_tab()
            .map(|t| t.link.borrow().is_some())
            .unwrap_or(false);
        for name in ["open-link", "copy-link"] {
            if let Some(a) = self
                .win
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                a.set_enabled(has);
            }
        }
    }

    pub fn update_titles(&self) {
        let tabs = self.tabs.borrow().clone();
        for t in &tabs {
            let page = self.tab_view.page(&t.root);
            let title = t.title();
            page.set_title(&title);
            let tip = t
                .cwd()
                .map(|p| paths::display_path(&p, &glib::home_dir()))
                .unwrap_or_default();
            page.set_tooltip(&glib::markup_escape_text(&tip));
        }
        let title = self
            .current_tab()
            .map(|t| t.title())
            .unwrap_or_else(|| tr("Terminal").into());
        self.win.set_title(Some(&title));
    }

    // --- tint -----------------------------------------------------------

    fn start_tint_poll(&self) {
        let wk = self.this.clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(w) = wk.upgrade() else {
                return glib::ControlFlow::Break;
            };
            w.update_tint();
            glib::ControlFlow::Continue
        });
    }

    fn update_tint(&self) {
        let (tint, pgrp) = match self.current_tab() {
            Some(t) if t.child_pid.get().is_some() => {
                let pgrp = t.foreground();
                (process::tint_for_pgrp(pgrp), pgrp)
            }
            _ => (Tint::None, None),
        };
        let status = match tint {
            Tint::Root => tr("Running as root").to_string(),
            Tint::Remote => pgrp
                .and_then(|pg| process::remote_host(&process::command_line(pg)))
                .map_or_else(
                    || tr("Connected to a remote host").to_string(),
                    |host| trf!("Connected to {host}", host = host),
                ),
            Tint::None => String::new(),
        };
        if tint == self.tint.get() && *self.tint_status.borrow() == status {
            return;
        }
        let was = self.tint.replace(tint);
        self.tint_status.replace(status.clone());
        self.title.set_subtitle(&status);
        match tint {
            Tint::Root => self
                .tint_icon
                .set_icon_name(Some("dialog-warning-symbolic")),
            Tint::Remote => self
                .tint_icon
                .set_icon_name(Some("network-server-symbolic")),
            Tint::None => {}
        }
        self.tint_icon.set_visible(tint != Tint::None);
        let spoken = match (tint, was) {
            (Tint::None, Tint::Root) => tr("No longer running as root"),
            (Tint::None, Tint::Remote) => tr("Remote session ended"),
            (Tint::None, Tint::None) => "",
            _ => &status,
        };
        a11y::announce(&self.title, spoken, Urgency::Polite);
        self.win.remove_css_class("root-tint");
        self.win.remove_css_class("remote-tint");
        match tint {
            Tint::Root => self.win.add_css_class("root-tint"),
            Tint::Remote => self.win.add_css_class("remote-tint"),
            Tint::None => {}
        }
    }

    // --- closing --------------------------------------------------------

    fn connect_signals(&self) {
        let wk = self.this.clone();
        self.tab_view.connect_close_page(move |tv, page| {
            let Some(w) = wk.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let Some(tab) = w.tab_for_child(&page.child()) else {
                tv.close_page_finish(page, true);
                return glib::Propagation::Stop;
            };
            if w.closing.get() || !tab.is_busy() {
                w.finish_close(page, &tab);
                return glib::Propagation::Stop;
            }
            let page = page.clone();
            glib::spawn_future_local(async move {
                let names = vec![tab.busy_name()];
                if confirm_close(&w.win, &names, false).await {
                    w.finish_close(&page, &tab);
                } else {
                    w.tab_view.close_page_finish(&page, false);
                }
            });
            glib::Propagation::Stop
        });

        let wk = self.this.clone();
        self.tab_view.connect_page_detached(move |tv, _, _| {
            let Some(w) = wk.upgrade() else { return };
            if tv.n_pages() == 0 && !w.closing.get() {
                // Deferred so a tab dragged to another window is adopted first.
                glib::idle_add_local_once(move || {
                    if w.tab_view.n_pages() == 0 && !w.closing.get() {
                        w.closing.set(true);
                        w.win.close();
                    }
                });
            }
        });
        let wk = self.this.clone();
        self.tab_view.connect_page_attached(move |_, page, _| {
            // A tab dragged in from another window.
            let Some(w) = wk.upgrade() else { return };
            if w.tab_for_child(&page.child()).is_some() {
                return;
            }
            let app = w.app();
            let windows = app.windows.borrow().clone();
            for other in windows.iter().filter(|o| !Rc::ptr_eq(o, &w)) {
                let found = other.tab_for_child(&page.child());
                if let Some(t) = found {
                    other.tabs.borrow_mut().retain(|x| !Rc::ptr_eq(x, &t));
                    *t.window.borrow_mut() = Rc::downgrade(&w);
                    w.tabs.borrow_mut().push(t);
                    break;
                }
            }
            w.update_titles();
        });
        let wk = self.this.clone();
        self.tab_view.connect_create_window(move |_| {
            let w = wk.upgrade()?;
            let nw = w.app().new_window();
            nw.win.present();
            Some(nw.tab_view.clone())
        });
        let wk = self.this.clone();
        self.tab_view.connect_selected_page_notify(move |tv| {
            let Some(w) = wk.upgrade() else { return };
            if let Some(p) = tv.selected_page() {
                p.set_needs_attention(false);
            }
            w.update_titles();
            w.update_tint();
            w.update_link_actions();
            w.sync_find_button();
            if let Some(t) = w.current_tab() {
                glib::idle_add_local_once(move || {
                    if !t.search_bar.is_search_mode() {
                        t.term.grab_focus();
                    }
                });
            }
        });
        let wk = self.this.clone();
        self.overview.connect_create_tab(move |_| {
            let w = wk.upgrade().expect("window alive");
            let cwd = w.current_cwd();
            w.new_tab(Launch {
                cwd,
                command: Vec::new(),
            })
        });

        let wk = self.this.clone();
        self.win.connect_close_request(move |_| {
            let Some(w) = wk.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if w.closing.get() {
                w.app().remove_window(&w);
                return glib::Propagation::Proceed;
            }
            let names: Vec<String> = w
                .tabs
                .borrow()
                .iter()
                .filter(|t| t.is_busy())
                .map(|t| t.busy_name())
                .collect();
            if names.is_empty() {
                w.closing.set(true);
                w.app().remove_window(&w);
                return glib::Propagation::Proceed;
            }
            glib::spawn_future_local(async move {
                if confirm_close(&w.win, &names, true).await {
                    w.closing.set(true);
                    w.win.close();
                }
            });
            glib::Propagation::Stop
        });
    }

    fn finish_close(&self, page: &adw::TabPage, tab: &Rc<Tab>) {
        self.tabs.borrow_mut().retain(|t| !Rc::ptr_eq(t, tab));
        self.tab_view.close_page_finish(page, true);
    }

    // --- actions --------------------------------------------------------

    fn install_actions(&self) {
        let add = |name: &str, f: WindowAction| {
            let a = gio::SimpleAction::new(name, None);
            let wk = self.this.clone();
            a.connect_activate(move |_, _| {
                if let Some(w) = wk.upgrade() {
                    f(&w);
                }
            });
            self.win.add_action(&a);
        };
        add(
            "new-tab",
            Box::new(|w| {
                let cwd = w.current_cwd();
                w.new_tab(Launch {
                    cwd,
                    command: Vec::new(),
                });
            }),
        );
        add(
            "new-window",
            Box::new(|w| {
                let cwd = w.current_cwd();
                w.app().open(
                    false,
                    Launch {
                        cwd,
                        command: Vec::new(),
                    },
                );
            }),
        );
        add(
            "close-tab",
            Box::new(|w| {
                if let Some(p) = w.tab_view.selected_page() {
                    w.tab_view.close_page(&p);
                }
            }),
        );
        add(
            "copy",
            Box::new(|w| {
                if let Some(t) = w.current_tab() {
                    t.term.copy_clipboard_format(vte4::Format::Text);
                }
            }),
        );
        add(
            "paste",
            Box::new(|w| {
                if let Some(t) = w.current_tab() {
                    t.term.paste_clipboard();
                }
            }),
        );
        add(
            "select-all",
            Box::new(|w| {
                if let Some(t) = w.current_tab() {
                    t.term.select_all();
                }
            }),
        );
        add(
            "find",
            Box::new(|w| {
                if let Some(t) = w.current_tab() {
                    let on = !t.search_bar.is_search_mode();
                    t.search_bar.set_search_mode(on);
                    if on {
                        if let Some(sel) = t.term.text_selected(vte4::Format::Text) {
                            let sel = sel.trim();
                            if !sel.is_empty() && !sel.contains('\n') {
                                t.search_entry.set_text(sel);
                            }
                        }
                        t.search_entry.grab_focus();
                    }
                }
            }),
        );
        add(
            "find-next",
            Box::new(|w| {
                if let Some(t) = w.current_tab() {
                    if !t.search_bar.is_search_mode() {
                        t.search_bar.set_search_mode(true);
                        t.search_entry.grab_focus();
                    }
                    t.find(true);
                }
            }),
        );
        add(
            "find-previous",
            Box::new(|w| {
                if let Some(t) = w.current_tab() {
                    if !t.search_bar.is_search_mode() {
                        t.search_bar.set_search_mode(true);
                        t.search_entry.grab_focus();
                    }
                    t.find(false);
                }
            }),
        );
        add(
            "zoom-in",
            Box::new(|w| {
                let app = w.app();
                app.set_font_scale(search::zoom_in(app.font_scale()));
            }),
        );
        add(
            "zoom-out",
            Box::new(|w| {
                let app = w.app();
                app.set_font_scale(search::zoom_out(app.font_scale()));
            }),
        );
        add("zoom-reset", Box::new(|w| w.app().set_font_scale(1.0)));
        add(
            "open-link",
            Box::new(|w| {
                if let Some(t) = w.current_tab() {
                    let uri = t.link.borrow().clone();
                    if let Some(uri) = uri {
                        w.open_uri(&uri, t.link_is_hyperlink.get());
                    }
                }
            }),
        );
        add(
            "copy-link",
            Box::new(|w| {
                if let Some(uri) = w.current_tab().and_then(|t| t.link.borrow().clone()) {
                    w.win.clipboard().set_text(&uri);
                }
            }),
        );
        add(
            "tab-overview",
            Box::new(|w| w.overview.set_open(!w.overview.is_open())),
        );
        add(
            "fullscreen",
            Box::new(|w| {
                if w.win.is_fullscreen() {
                    w.win.unfullscreen();
                } else {
                    w.win.fullscreen();
                }
            }),
        );
        add(
            "show-shortcuts",
            Box::new(|w| crate::shortcuts::show(&w.win)),
        );
        self.update_link_actions();
    }
}

/// Asks whether to close despite running programs. Returns true to close.
async fn confirm_close(win: &adw::ApplicationWindow, names: &[String], window: bool) -> bool {
    let heading = if window {
        tr("Close Window?")
    } else {
        tr("Close Tab?")
    };
    let body = if names.len() == 1 {
        trf!("“{}” is still running. Closing will stop it.", names[0])
    } else {
        let list: Vec<String> = names.iter().map(|n| format!("• {n}")).collect();
        trf!(
            "Some programs are still running:\n{list}\nClosing will stop them.",
            list = list.join("\n")
        )
    };
    let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
    dialog.add_responses(&[("cancel", tr("_Cancel")), ("close", tr("C_lose"))]);
    dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.choose_future(win).await == "close"
}

#[cfg(test)]
mod tests {
    use super::zoom_percent;

    #[test]
    fn zoom_level_text() {
        assert_eq!(zoom_percent(1.0), "100%");
        assert_eq!(zoom_percent(crate::search::zoom_in(1.0)), "110%");
        assert_eq!(zoom_percent(crate::search::MIN_SCALE), "50%");
    }
}
