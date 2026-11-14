//! GTK user interface: window, month and week views, sidebar.

use adw::prelude::*;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, Timelike, Utc, Weekday};
use gtk::{gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use helix_eds::ical::{self, Zones};
use helix_eds::{Source, SourceKind};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::backend::{self, Backend, Cmd, Msg};
use crate::colors;
use crate::dates;
use crate::editor::{self, Target};
use crate::keynav;
use crate::layout;
use crate::model::{Item, Store};

pub const APP_ID: &str = "org.helix.Calendar";
const SCHEMA: &str = "org.gnome.calendar";
const IFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const DESKTOP_CAL_SCHEMA: &str = "org.gnome.desktop.calendar";
const DEFAULTS_SCHEMA: &str = "org.gnome.Evolution.DefaultSources";
const GUTTER: i32 = 56;
const HOUR_PX: f64 = 48.0;
const MONTH_CHIPS: usize = 3;

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub date: Option<NaiveDate>,
    pub uuid: Option<String>,
    pub files: Vec<std::path::PathBuf>,
}

pub fn run(opts: Options) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(&app, "Calendar", "View and edit your events");
    app.connect_startup(|_| load_css());
    let ui: Rc<RefCell<Option<Rc<Ui>>>> = Rc::default();
    let opts = RefCell::new(Some(opts));
    app.connect_activate(move |app| {
        let existing = ui.borrow().clone();
        match existing {
            Some(u) => u.window.present(),
            None => {
                let u = Ui::build(app, opts.borrow_mut().take().unwrap_or_default());
                u.window.present();
                *ui.borrow_mut() = Some(u);
            }
        }
    });
    app.run_with_args::<&str>(&[])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".month-grid { background: @view_bg_color; }
         .weekday-header { padding: 6px; font-weight: bold; font-size: smaller; }
         .week-number { font-size: smaller; padding: 4px; opacity: 0.55; }
         .day-cell { border-top: 1px solid alpha(currentColor, 0.12); border-left: 1px solid alpha(currentColor, 0.12); padding: 2px; }
         .day-cell.other-month { background: alpha(currentColor, 0.035); }
         .day-cell.other-month .day-number { opacity: 0.5; }
         .day-cell.selected { background: alpha(@accent_bg_color, 0.12); }
         .day-number { padding: 2px 6px; margin: 2px; border-radius: 999px; font-feature-settings: \"tnum\"; }
         .day-cell.today .day-number { background: @accent_bg_color; color: @accent_fg_color; font-weight: bold; }
         .event-chip { padding: 0 6px; min-height: 20px; margin: 1px 2px; border-radius: 5px; font-size: smaller; }
         .more-chip { padding: 0 6px; min-height: 18px; margin: 1px 2px; font-size: smaller; }
         .week-event { padding: 2px 6px; border-radius: 6px; font-size: smaller; }
         .event-chip:focus-visible, .week-event:focus-visible { outline: 2px solid @view_fg_color; outline-offset: 1px; }
         .week-day-header { padding: 6px 0; }
         .day-cell:focus-visible, .week-day-cell:focus-visible { outline: 2px solid alpha(@accent_color, 0.7); outline-offset: -2px; }
         .week-day-header.today { color: @accent_color; font-weight: bold; }
         .hour-label { font-size: smaller; opacity: 0.6; font-feature-settings: \"tnum\"; }
         .cal-dot { min-width: 12px; min-height: 12px; border-radius: 999px; }
         .agenda-time { font-feature-settings: \"tnum\"; font-size: smaller; opacity: 0.7; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewKind {
    Week,
    Month,
}

#[derive(Default)]
pub struct State {
    pub sources: Vec<Source>,
    pub writable: HashMap<String, bool>,
    pub store: Store,
    gens: HashMap<String, u64>,
    loaded: HashSet<String>,
    /// Requested range (UTC) of the live views.
    range: Option<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)>,
    pending_delete: HashSet<(String, String)>,
    search_tag: u64,
    search_results: Option<Vec<(String, ical::Event)>>,
    startup: Option<Options>,
    unavailable: bool,
}

struct WeekLayout {
    days: Vec<NaiveDate>,
    /// (button, day index, start minute, end minute, column, columns)
    events: Vec<(gtk::Button, usize, i64, i64, usize, usize)>,
    labels: Vec<(gtk::Label, i32)>,
}

pub struct Ui {
    pub window: adw::ApplicationWindow,
    prefs: Prefs,
    iface: Prefs,
    desk_cal: Prefs,
    defaults: Prefs,
    pub backend: Backend,
    pub zones: RefCell<Zones>,
    pub state: RefCell<State>,
    first_weekday: Weekday,
    date: Cell<NaiveDate>,
    selected: Cell<NaiveDate>,
    /// Week grid slot cursor (minutes after midnight on `selected`).
    week_cursor: Cell<i64>,
    view: Cell<ViewKind>,
    zoom: Cell<f64>,
    render_pending: Cell<bool>,
    color_css: gtk::CssProvider,
    cal_classes: RefCell<HashMap<String, String>>,

    toasts: adw::ToastOverlay,
    split: adw::OverlaySplitView,
    title: gtk::Label,
    week_toggle: gtk::ToggleButton,
    month_toggle: gtk::ToggleButton,
    view_stack: gtk::Stack,
    main_stack: gtk::Stack,
    month_box: gtk::Box,
    week_header: gtk::Box,
    week_scroll: gtk::ScrolledWindow,
    week_area: gtk::DrawingArea,
    week_fixed: gtk::Fixed,
    week_layout: RefCell<WeekLayout>,
    week_scrolled_once: Cell<bool>,
    picker: gtk::Calendar,
    cal_list: gtk::ListBox,
    agenda_title: gtk::Label,
    agenda: gtk::ListBox,
    agenda_items: RefCell<Vec<Item>>,
    search_bar: gtk::SearchBar,
    search: gtk::SearchEntry,
    unavailable_page: adw::StatusPage,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

impl Ui {
    fn build(app: &adw::Application, opts: Options) -> Rc<Ui> {
        let prefs = Prefs::new(SCHEMA);
        let iface = Prefs::new(IFACE_SCHEMA);
        let desk_cal = Prefs::new(DESKTOP_CAL_SCHEMA);
        let defaults = Prefs::new(DEFAULTS_SCHEMA);
        let (w, h) = prefs.get("window-size", (768i32, 600i32));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Calendar")
            .default_width(w.max(360))
            .default_height(h.max(400))
            .width_request(360)
            .height_request(400)
            .build();
        if prefs.get("window-maximized", true) {
            window.maximize();
        }

        // Header.
        let header = adw::HeaderBar::new();
        let new_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New Event")
            .build()
            .labelled("New Event");
        new_button.set_action_name(Some("win.new-event"));
        let sidebar_button = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Toggle Sidebar")
            .build()
            .labelled("Toggle Sidebar");
        let today_button = gtk::Button::with_label("Today");
        today_button.set_action_name(Some("win.today"));
        let prev = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Previous")
            .build()
            .labelled("Previous");
        prev.set_action_name(Some("win.previous"));
        let next = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text("Next")
            .build()
            .labelled("Next");
        next.set_action_name(Some("win.next"));
        let nav = gtk::Box::builder().css_classes(["linked"]).build();
        nav.append(&prev);
        nav.append(&next);
        header.pack_start(&sidebar_button);
        header.pack_start(&new_button);
        header.pack_start(&today_button);
        header.pack_start(&nav);
        let title = gtk::Label::builder()
            .css_classes(["title"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        header.set_title_widget(Some(&title));
        let week_toggle = gtk::ToggleButton::with_label("Week");
        let month_toggle = gtk::ToggleButton::with_label("Month");
        month_toggle.set_group(Some(&week_toggle));
        let switcher = gtk::Box::builder().css_classes(["linked"]).build();
        switcher.append(&week_toggle);
        switcher.append(&month_toggle);
        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some("_Manage Calendars…"), Some("win.manage"));
        s1.append(Some("_Import…"), Some("win.import"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
        s2.append(Some("_About Calendar"), Some("app.about"));
        menu.append_section(None, &s2);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Main Menu")
            .build()
            .labelled("Main Menu");
        menu_button.set_primary(true);
        let search_button = gtk::ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search")
            .build()
            .labelled("Search");
        header.pack_end(&menu_button);
        header.pack_end(&search_button);
        header.pack_end(&switcher);

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search events")
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(
                &adw::Clamp::builder()
                    .maximum_size(400)
                    .child(&search)
                    .build(),
            )
            .build();
        search_bar.connect_entry(&search);
        search_button
            .bind_property("active", &search_bar, "search-mode-enabled")
            .bidirectional()
            .build();

        // Sidebar.
        let picker = gtk::Calendar::builder()
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .build();
        picker.set_show_week_numbers(false);
        let cal_list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        // Activating a row (click on the name, Enter or Space) toggles its
        // calendar, like its check box.
        cal_list.connect_row_activated(|_, row| {
            let check = row
                .child()
                .and_then(|c| c.first_child())
                .and_downcast::<gtk::CheckButton>();
            if let Some(check) = check.filter(|c| c.is_sensitive()) {
                check.set_active(!check.is_active());
            }
        });
        let agenda_title = label("", &["heading"]);
        agenda_title.set_xalign(0.0);
        agenda_title.set_margin_start(12);
        agenda_title.set_margin_top(12);
        agenda_title.set_wrap(true);
        let agenda = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        let side_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        side_box.append(&picker);
        let cal_header = label("Calendars", &["heading"]);
        cal_header.set_xalign(0.0);
        cal_header.set_margin_start(12);
        cal_header.set_margin_top(6);
        side_box.append(&cal_header);
        side_box.append(&cal_list);
        side_box.append(&agenda_title);
        side_box.append(&agenda);
        let side_scroll = gtk::ScrolledWindow::builder()
            .child(&side_box)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        // Month view.
        let month_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        month_box.add_css_class("month-grid");

        // Week view.
        let week_header = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let week_area = gtk::DrawingArea::builder().focusable(true).build();
        let week_fixed = gtk::Fixed::new();
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&week_area));
        overlay.add_overlay(&week_fixed);
        let week_scroll = gtk::ScrolledWindow::builder()
            .child(&overlay)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let week_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        week_box.add_css_class("month-grid");
        week_box.append(&week_header);
        week_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        week_box.append(&week_scroll);

        let view_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        view_stack.add_named(&month_box, Some("month"));
        view_stack.add_named(&week_box, Some("week"));

        let split = adw::OverlaySplitView::builder()
            .sidebar(&side_scroll)
            .content(&view_stack)
            .build();
        split.set_min_sidebar_width(250.0);
        split.set_max_sidebar_width(300.0);
        split
            .bind_property("show-sidebar", &sidebar_button, "active")
            .bidirectional()
            .sync_create()
            .build();

        let unavailable_page = adw::StatusPage::builder()
            .icon_name("dialog-warning-symbolic")
            .title("Calendar Service Unavailable")
            .build();
        let retry = gtk::Button::builder()
            .label("Retry")
            .css_classes(["pill"])
            .halign(gtk::Align::Center)
            .build();
        unavailable_page.set_child(Some(&retry));

        let main_stack = gtk::Stack::new();
        main_stack.add_named(&split, Some("main"));
        main_stack.add_named(&unavailable_page, Some("unavailable"));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&search_bar);
        toolbar.set_content(Some(&main_stack));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));
        window.set_content(Some(&toasts));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 800sp").unwrap());
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let color_css = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &color_css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }

        let (tx, rx) = async_channel::unbounded();
        let backend = backend::start(tx);
        let start = opts.date.unwrap_or_else(today);
        let view = if prefs.get_enum_nick("active-view", "month") == "week" {
            ViewKind::Week
        } else {
            ViewKind::Month
        };
        let zoom = prefs.get("week-view-zoom-level", 1.0f64).clamp(0.5, 3.0);

        let ui = Rc::new(Ui {
            window,
            prefs,
            iface,
            desk_cal,
            defaults,
            backend,
            zones: RefCell::new(Zones::system()),
            state: RefCell::new(State {
                startup: Some(opts.clone()),
                ..Default::default()
            }),
            first_weekday: dates::first_weekday(),
            date: Cell::new(start),
            selected: Cell::new(start),
            week_cursor: Cell::new(9 * 60),
            view: Cell::new(view),
            zoom: Cell::new(zoom),
            render_pending: Cell::new(false),
            color_css,
            cal_classes: RefCell::default(),
            toasts,
            split,
            title,
            week_toggle,
            month_toggle,
            view_stack,
            main_stack,
            month_box,
            week_header,
            week_scroll,
            week_area,
            week_fixed,
            week_layout: RefCell::new(WeekLayout {
                days: vec![],
                events: vec![],
                labels: vec![],
            }),
            week_scrolled_once: Cell::new(false),
            picker,
            cal_list,
            agenda_title,
            agenda,
            agenda_items: RefCell::default(),
            search_bar,
            search,
            unavailable_page,
        });
        match view {
            ViewKind::Week => ui.week_toggle.set_active(true),
            ViewKind::Month => ui.month_toggle.set_active(true),
        }
        ui.connect(retry);
        ui.install_actions();
        let weak = Rc::downgrade(&ui);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                match weak.upgrade() {
                    Some(ui) => ui.handle(msg),
                    None => break,
                }
            }
        });
        // Redraw the "now" line and today marker every minute.
        let weak = Rc::downgrade(&ui);
        glib::timeout_add_seconds_local(60, move || match weak.upgrade() {
            Some(ui) => {
                ui.week_area.queue_draw();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        ui.navigate_to(start);
        for f in &opts.files {
            ui.import_file(f.clone());
        }
        ui
    }

    fn connect(self: &Rc<Self>, retry: gtk::Button) {
        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.backend.send(Cmd::Resync);
            }
        });
        let weak = Rc::downgrade(self);
        self.week_toggle.connect_toggled(move |b| {
            if let (Some(ui), true) = (weak.upgrade(), b.is_active()) {
                ui.set_view(ViewKind::Week);
            }
        });
        let weak = Rc::downgrade(self);
        self.month_toggle.connect_toggled(move |b| {
            if let (Some(ui), true) = (weak.upgrade(), b.is_active()) {
                ui.set_view(ViewKind::Month);
            }
        });
        let weak = Rc::downgrade(self);
        self.picker.connect_day_selected(move |c| {
            let Some(ui) = weak.upgrade() else { return };
            let g = c.date();
            if let Some(d) =
                NaiveDate::from_ymd_opt(g.year(), g.month() as u32, g.day_of_month() as u32)
            {
                if d != ui.selected.get() {
                    ui.navigate_to(d);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.agenda.connect_row_activated(move |_, row| {
            let Some(ui) = weak.upgrade() else { return };
            let name = row.widget_name().to_string();
            ui.activate_agenda_row(&name);
        });
        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |e| {
            let Some(ui) = weak.upgrade() else { return };
            let q = e.text().to_string();
            if q.trim().is_empty() {
                ui.state.borrow_mut().search_results = None;
                ui.render_agenda();
                return;
            }
            let tag = {
                let mut st = ui.state.borrow_mut();
                st.search_tag += 1;
                st.search_tag
            };
            ui.backend.send(Cmd::Search { query: q, tag });
        });
        let weak = Rc::downgrade(self);
        self.search_bar
            .connect_search_mode_enabled_notify(move |b| {
                if let (Some(ui), false) = (weak.upgrade(), b.is_search_mode()) {
                    ui.search.set_text("");
                }
            });

        // Week grid drawing and input.
        let weak = Rc::downgrade(self);
        self.week_area.set_draw_func(move |area, cr, w, h| {
            if let Some(ui) = weak.upgrade() {
                ui.draw_week(area, cr, w, h);
            }
        });
        let weak = Rc::downgrade(self);
        self.week_area.connect_resize(move |_, w, _| {
            if let Some(ui) = weak.upgrade() {
                ui.place_week_children(w);
            }
        });
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |g, n, x, y| {
            let Some(ui) = weak.upgrade() else { return };
            let w = g.widget().map(|w| w.width()).unwrap_or(0);
            let Some((day, minutes)) = ui.week_hit(w, x, y) else {
                return;
            };
            ui.selected.set(day);
            // The keyboard slot cursor follows the pointer.
            ui.week_cursor.set(minutes / keynav::SLOT * keynav::SLOT);
            ui.week_area.grab_focus();
            ui.update_week_cursor_label();
            ui.week_area.queue_draw();
            ui.render_agenda();
            if n == 2 {
                let start = day.and_time(NaiveTime::MIN) + Duration::minutes(minutes / 30 * 30);
                editor::open(
                    &ui,
                    Target::New {
                        start,
                        end: start + Duration::hours(1),
                        all_day: false,
                    },
                );
            }
        });
        self.week_fixed.parent().unwrap().add_controller(click);

        // Keyboard slot cursor on the week grid.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !mods.is_empty() {
                return glib::Propagation::Proceed;
            }
            ui.week_grid_key(key)
        });
        self.week_area.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_enter(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.update_week_cursor_label();
                ui.scroll_to_week_cursor();
                ui.week_area.queue_draw();
            }
        });
        let weak = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.week_area.queue_draw();
            }
        });
        self.week_area.add_controller(focus);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |c, _, dy| {
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if c.current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                ui.set_zoom(ui.zoom.get() * if dy < 0.0 { 1.1 } else { 1.0 / 1.1 });
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.week_scroll.add_controller(scroll);

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |w| {
            if let Some(ui) = weak.upgrade() {
                ui.flush_deletes();
                if !w.is_maximized() {
                    let (width, height) = w.default_size();
                    ui.prefs.set("window-size", (width, height));
                }
                ui.prefs.set("window-maximized", w.is_maximized());
            }
            glib::Propagation::Proceed
        });
    }

    fn install_actions(self: &Rc<Self>) {
        let w = &self.window;
        let app = w.application().expect("app");
        type F = Box<dyn Fn(&Rc<Ui>)>;
        let add = |name: &str, accels: &[&str], f: F| {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(ui) = weak.upgrade() {
                    f(&ui);
                }
            });
            w.add_action(&a);
            if !accels.is_empty() {
                app.set_accels_for_action(&format!("win.{name}"), accels);
            }
        };
        add(
            "new-event",
            &["<Control>n"],
            Box::new(|ui| ui.new_event_on(ui.selected.get(), false)),
        );
        add(
            "today",
            &["<Control>t"],
            Box::new(|ui| ui.navigate_to(today())),
        );
        add("previous", &["<Alt>Left"], Box::new(|ui| ui.step(-1)));
        add("next", &["<Alt>Right"], Box::new(|ui| ui.step(1)));
        add(
            "week-view",
            &["<Control>1"],
            Box::new(|ui| ui.week_toggle.set_active(true)),
        );
        add(
            "month-view",
            &["<Control>2"],
            Box::new(|ui| ui.month_toggle.set_active(true)),
        );
        add(
            "search",
            &["<Control>f"],
            Box::new(|ui| {
                ui.search_bar.set_search_mode(true);
                ui.search.grab_focus();
            }),
        );
        add(
            "sidebar",
            &["F9"],
            Box::new(|ui| ui.split.set_show_sidebar(!ui.split.shows_sidebar())),
        );
        add(
            "zoom-in",
            &["<Control>plus", "<Control>equal"],
            Box::new(|ui| ui.set_zoom(ui.zoom.get() * 1.25)),
        );
        add(
            "zoom-out",
            &["<Control>minus"],
            Box::new(|ui| ui.set_zoom(ui.zoom.get() / 1.25)),
        );
        add(
            "zoom-reset",
            &["<Control>0"],
            Box::new(|ui| ui.set_zoom(1.0)),
        );
        add("manage", &[], Box::new(crate::manage::open));
        add("import", &["<Control>i"], Box::new(|ui| ui.choose_import()));
        add(
            "shortcuts",
            &["<Control>question"],
            Box::new(|ui| show_shortcuts(&ui.window)),
        );
    }

    // ----- helpers used by the editor and dialogs -----

    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    pub fn twelve(&self) -> bool {
        self.iface.get_enum_nick("clock-format", "24h") == "12h"
    }

    pub fn zone_name(&self) -> String {
        self.zones.borrow().local_name().to_string()
    }

    pub fn sources(&self) -> Vec<Source> {
        self.state.borrow().sources.clone()
    }

    /// Calendars as (uid, "Name", writable), sorted by name.
    pub fn calendar_choices(&self) -> Vec<(String, String, bool)> {
        let st = self.state.borrow();
        let mut v: Vec<(String, String, bool)> = st
            .sources
            .iter()
            .filter(|s| s.is(SourceKind::Calendar) && s.enabled())
            .map(|s| {
                (
                    s.uid.clone(),
                    s.display_name(),
                    st.writable.get(&s.uid).copied().unwrap_or(false),
                )
            })
            .collect();
        v.sort_by_key(|(_, n, _)| n.to_lowercase());
        v
    }

    pub fn default_calendar(&self) -> String {
        let configured = self
            .defaults
            .get("default-calendar", "system-calendar".to_string());
        let st = self.state.borrow();
        if st.writable.get(&configured).copied().unwrap_or(false) {
            return configured;
        }
        if st.writable.get("system-calendar").copied().unwrap_or(false) {
            return "system-calendar".into();
        }
        let mut w: Vec<&String> = st
            .writable
            .iter()
            .filter(|(_, w)| **w)
            .map(|(k, _)| k)
            .collect();
        w.sort();
        w.first().map(|s| s.to_string()).unwrap_or(configured)
    }

    pub fn remember_calendar(&self, uid: &str) {
        if self.defaults.get("default-calendar", String::new()) != uid {
            self.defaults.set("default-calendar", uid.to_string());
        }
    }

    pub fn delete_with_undo(self: &Rc<Self>, cal: &str, uid: &str, summary: &str) {
        let key = (cal.to_string(), uid.to_string());
        self.state.borrow_mut().pending_delete.insert(key.clone());
        self.schedule_render();
        let title = if summary.trim().is_empty() {
            "Event deleted".to_string()
        } else {
            format!("“{}” deleted", summary.trim())
        };
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(&title).as_str())
            .button_label("Undo")
            .timeout(5)
            .build();
        let undone = Rc::new(Cell::new(false));
        let (weak, u, k) = (Rc::downgrade(self), undone.clone(), key.clone());
        toast.connect_button_clicked(move |_| {
            u.set(true);
            if let Some(ui) = weak.upgrade() {
                ui.state.borrow_mut().pending_delete.remove(&k);
                ui.schedule_render();
            }
        });
        let weak = Rc::downgrade(self);
        toast.connect_dismissed(move |_| {
            if !undone.get() {
                if let Some(ui) = weak.upgrade() {
                    ui.commit_delete(&key);
                }
            }
        });
        self.toasts.add_toast(toast);
    }

    fn commit_delete(&self, key: &(String, String)) {
        if self.state.borrow_mut().pending_delete.remove(key) {
            self.backend.send(Cmd::Remove {
                cal: key.0.clone(),
                ids: vec![(key.1.clone(), String::new())],
                mod_type: helix_eds::ModType::All,
            });
        }
    }

    fn flush_deletes(&self) {
        let keys: Vec<(String, String)> =
            self.state.borrow().pending_delete.iter().cloned().collect();
        for k in &keys {
            self.commit_delete(k);
        }
        if !keys.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }

    fn cal_class(&self, uid: &str) -> String {
        self.cal_classes
            .borrow()
            .get(uid)
            .cloned()
            .unwrap_or_else(|| "cal-default".into())
    }

    fn update_color_css(&self) {
        let mut css = colors::class_css("cal-default", colors::DEFAULT);
        let mut map = HashMap::new();
        for (i, s) in self
            .state
            .borrow()
            .sources
            .iter()
            .filter(|s| s.is(SourceKind::Calendar))
            .enumerate()
        {
            let class = format!("cal-{i}");
            let bg = colors::normalize(s.color());
            css.push_str(&colors::class_css(&class, &bg));
            map.insert(s.uid.clone(), class);
        }
        self.color_css.load_from_string(&css);
        *self.cal_classes.borrow_mut() = map;
    }

    // ----- backend messages -----

    fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Unavailable(e) => {
                self.unavailable_page.set_description(Some(&format!(
                    "Could not reach Evolution Data Server.\n{}",
                    glib::markup_escape_text(&e)
                )));
                self.main_stack.set_visible_child_name("unavailable");
                self.state.borrow_mut().unavailable = true;
            }
            Msg::Sources(sources) => {
                self.main_stack.set_visible_child_name("main");
                {
                    let mut st = self.state.borrow_mut();
                    st.unavailable = false;
                    st.sources = sources;
                }
                self.update_color_css();
                self.render_calendar_list();
                self.schedule_render();
            }
            Msg::Opened { uid, writable } => {
                self.state.borrow_mut().writable.insert(uid, writable);
            }
            Msg::Closed(uid) => {
                let mut st = self.state.borrow_mut();
                st.writable.remove(&uid);
                st.store.clear(&uid);
                st.gens.remove(&uid);
                st.loaded.remove(&uid);
                drop(st);
                self.schedule_render();
            }
            Msg::Reset { cal, gen } => {
                let mut st = self.state.borrow_mut();
                st.gens.insert(cal.clone(), gen);
                st.store.clear(&cal);
                drop(st);
                self.schedule_render();
            }
            Msg::Upsert { cal, gen, events } => {
                if self.state.borrow().gens.get(&cal) == Some(&gen) {
                    self.state.borrow_mut().store.upsert(&cal, events);
                    self.schedule_render();
                }
            }
            Msg::Removed { cal, gen, ids } => {
                if self.state.borrow().gens.get(&cal) == Some(&gen) {
                    self.state.borrow_mut().store.remove(&cal, &ids);
                    self.schedule_render();
                }
            }
            Msg::Loaded { cal, gen } => {
                if self.state.borrow().gens.get(&cal) == Some(&gen) {
                    let n = self
                        .state
                        .borrow()
                        .store
                        .calendars
                        .get(&cal)
                        .map_or(0, |m| m.len());
                    glib::g_debug!(
                        "helix-calendar",
                        "calendar {cal} loaded with {n} components"
                    );
                    self.state.borrow_mut().loaded.insert(cal);
                    self.apply_startup();
                }
            }
            Msg::TimezoneNeeded(tzid) => self.backend.send(Cmd::ResolveTimezone(tzid)),
            Msg::TimezoneData(text) => {
                self.zones.borrow_mut().add_vtimezones(&ical::parse(&text));
                self.schedule_render();
            }
            Msg::SearchResults { tag, results } => {
                if self.state.borrow().search_tag == tag && !self.search.text().trim().is_empty() {
                    self.state.borrow_mut().search_results = Some(results);
                    self.render_agenda();
                }
            }
            Msg::Found(found) => match found.map(|b| *b) {
                Some((cal, e)) => {
                    let zones = self.zones.borrow();
                    let (s, en) = (e.start.local(&zones), e.end.local(&zones));
                    drop(zones);
                    self.navigate_to(s.date());
                    let rid = if e.is_recurring() {
                        e.start.value_string()
                    } else {
                        String::new()
                    };
                    editor::open(
                        self,
                        Target::Existing {
                            cal,
                            component: e.key(),
                            rid,
                            start: s,
                            end: en,
                            all_day: e.all_day(),
                        },
                    );
                }
                None => self.toast("Event not found"),
            },
            Msg::Imported(n) => self.toast(&format!(
                "Imported {n} event{}",
                if n == 1 { "" } else { "s" }
            )),
            Msg::Error(e) => self.toast(&e),
        }
    }

    fn apply_startup(self: &Rc<Self>) {
        let ready = {
            let st = self.state.borrow();
            !st.writable.is_empty() && st.writable.keys().all(|c| st.loaded.contains(c))
        };
        if !ready {
            return;
        }
        let Some(opts) = self.state.borrow_mut().startup.take() else {
            return;
        };
        if let Some(uid) = opts.uuid {
            self.backend.send(Cmd::Find(uid));
        }
    }

    // ----- navigation -----

    fn set_view(self: &Rc<Self>, v: ViewKind) {
        self.view.set(v);
        self.prefs.set_enum_nick(
            "active-view",
            if v == ViewKind::Week { "week" } else { "month" },
        );
        self.view_stack
            .set_visible_child_name(if v == ViewKind::Week { "week" } else { "month" });
        self.update_range();
        self.render();
    }

    fn step(self: &Rc<Self>, dir: i32) {
        let d = match self.view.get() {
            ViewKind::Month => dates::add_months(self.date.get(), dir),
            ViewKind::Week => self.date.get() + Duration::weeks(dir as i64),
        };
        self.navigate_to(d);
    }

    fn navigate_to(self: &Rc<Self>, d: NaiveDate) {
        self.date.set(d);
        self.selected.set(d);
        if let Ok(g) =
            glib::DateTime::from_local(d.year(), d.month() as i32, d.day() as i32, 0, 0, 0.0)
        {
            self.picker.select_day(&g);
        }
        self.update_range();
        self.render();
    }

    /// Asks the backend for views over the shown month and week with margin.
    fn update_range(&self) {
        let d = self.date.get();
        let grid = dates::month_grid(d.year(), d.month(), self.first_weekday);
        let ws = dates::week_start(d, self.first_weekday);
        let from = grid[0].min(ws) - Duration::days(7);
        let to = (*grid.last().unwrap()).max(ws + Duration::days(6)) + Duration::days(8);
        let local = self.zones.borrow().local;
        let f = ical::to_utc(local, from.and_time(NaiveTime::MIN));
        let t = ical::to_utc(local, to.and_time(NaiveTime::MIN));
        let changed = self
            .state
            .borrow()
            .range
            .is_none_or(|(a, b)| a > f || b < t);
        if changed {
            // Request a little more than needed so small moves need no new views.
            let f = f - Duration::days(14);
            let t = t + Duration::days(14);
            self.state.borrow_mut().range = Some((f, t));
            self.backend.send(Cmd::SetRange(f, t));
        }
    }

    fn set_zoom(self: &Rc<Self>, z: f64) {
        let z = z.clamp(0.5, 3.0);
        let adj = self.week_scroll.vadjustment();
        let ratio = z / self.zoom.get();
        self.zoom.set(z);
        self.prefs.set("week-view-zoom-level", z);
        if self.view.get() == ViewKind::Week {
            self.render_week();
            adj.set_value(adj.value() * ratio);
        }
    }

    // ----- rendering -----

    fn schedule_render(self: &Rc<Self>) {
        if self.render_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
            if let Some(ui) = weak.upgrade() {
                ui.render_pending.set(false);
                ui.render();
            }
        });
    }

    fn items(&self, from: NaiveDate, to: NaiveDate) -> Vec<Item> {
        let st = self.state.borrow();
        let zones = self.zones.borrow();
        let mut items = st.store.items(
            from.and_time(NaiveTime::MIN),
            to.and_time(NaiveTime::MIN),
            &zones,
            |_| true,
        );
        items.retain(|i| {
            !st.pending_delete
                .contains(&(i.calendar.clone(), i.uid.clone()))
        });
        items
    }

    fn render(self: &Rc<Self>) {
        let d = self.date.get();
        match self.view.get() {
            ViewKind::Month => {
                self.title.set_label(&dates::month_title(d));
                self.render_month();
            }
            ViewKind::Week => {
                let ws = dates::week_start(d, self.first_weekday);
                self.title
                    .set_label(&dates::range_title(ws, ws + Duration::days(6)));
                self.render_week();
            }
        }
        self.render_agenda();
    }

    fn chip(self: &Rc<Self>, item: &Item, show_time: bool) -> gtk::Button {
        let twelve = self.twelve();
        let text = if show_time && !item.spans_days() {
            format!(
                "{} {}",
                dates::format_time(item.start.time(), twelve),
                item.title()
            )
        } else {
            item.title().to_string()
        };
        let l = gtk::Label::builder()
            .label(&text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let b = gtk::Button::builder()
            .child(&l)
            .css_classes(["event-chip", &self.cal_class(&item.calendar)])
            .build();
        b.set_tooltip_text(Some(&self.describe(item)));
        helix_app_common::set_accessible_label(&b, &self.item_label(item));
        let weak = Rc::downgrade(self);
        let it = item.clone();
        b.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.open_item(&it);
            }
        });
        b
    }

    /// The name of calendar `uid` (empty if unknown).
    fn cal_name(&self, uid: &str) -> String {
        self.sources()
            .into_iter()
            .find(|s| s.uid == uid)
            .map(|s| s.display_name())
            .unwrap_or_default()
    }

    /// "Standup, Fri 13 Nov, 09:00 – 09:15, Work" for screen readers.
    fn item_label(&self, item: &Item) -> String {
        let when = self.describe_when(item);
        let cal = self.cal_name(&item.calendar);
        let mut s = format!("{}, {when}", item.title());
        if !cal.is_empty() {
            s.push_str(&format!(", {cal}"));
        }
        s
    }

    /// Tooltip: title, time, location and calendar, one per line.
    fn describe(&self, item: &Item) -> String {
        let mut s = format!("{}\n{}", item.title(), self.describe_when(item));
        if !item.location.is_empty() {
            s.push('\n');
            s.push_str(&item.location);
        }
        let cal = self.cal_name(&item.calendar);
        if !cal.is_empty() {
            s.push('\n');
            s.push_str(&format!("Calendar: {cal}"));
        }
        s
    }

    /// When an item happens, always with its date.
    fn describe_when(&self, item: &Item) -> String {
        let twelve = self.twelve();
        if item.all_day {
            let last = (item.end - Duration::days(1)).date().max(item.start.date());
            if last == item.start.date() {
                format!("{}, all day", dates::short_date(item.start.date()))
            } else {
                format!(
                    "{} – {}",
                    dates::short_date(item.start.date()),
                    dates::short_date(last)
                )
            }
        } else if item.start.date() == item.end.date() || item.end == item.start {
            format!(
                "{}, {} – {}",
                dates::short_date(item.start.date()),
                dates::format_time(item.start.time(), twelve),
                dates::format_time(item.end.time(), twelve)
            )
        } else {
            format!(
                "{} {} – {} {}",
                dates::short_date(item.start.date()),
                dates::format_time(item.start.time(), twelve),
                dates::short_date(item.end.date()),
                dates::format_time(item.end.time(), twelve)
            )
        }
    }

    fn open_item(self: &Rc<Self>, item: &Item) {
        editor::open(
            self,
            Target::Existing {
                cal: item.calendar.clone(),
                component: item.component.clone(),
                rid: item.rid.clone(),
                start: item.start,
                end: item.end,
                all_day: item.all_day,
            },
        );
    }

    pub fn new_event_on(self: &Rc<Self>, day: NaiveDate, all_day: bool) {
        if self.state.borrow().unavailable {
            return;
        }
        if all_day {
            let s = day.and_time(NaiveTime::MIN);
            editor::open(
                self,
                Target::New {
                    start: s,
                    end: s + Duration::days(1),
                    all_day: true,
                },
            );
        } else {
            let (s, e) = editor::default_slot(day, Local::now().naive_local());
            editor::open(
                self,
                Target::New {
                    start: s,
                    end: e,
                    all_day: false,
                },
            );
        }
    }

    /// Rebuilds the month grid, keeping focus on the same day cell.
    fn render_month(self: &Rc<Self>) {
        let month = self.month_box.clone();
        helix_app_common::focus::rebuild_keeping_focus(&month, None, || self.render_month_grid());
    }

    /// Moves the selection (and keyboard focus) to `day` from the month or
    /// week grid.
    fn move_to_day(self: &Rc<Self>, day: NaiveDate, focus_week_header: bool) {
        self.navigate_to(day);
        let key = day.to_string();
        if focus_week_header {
            helix_app_common::focus::focus_key(&self.week_header, &key);
        } else {
            helix_app_common::focus::focus_key(&self.month_box, &key);
        }
    }

    /// Arrow / Home / End / Page keys and Enter on a day cell (month view)
    /// or a day header (week view).
    fn day_key(
        self: &Rc<Self>,
        day: NaiveDate,
        key: gtk::gdk::Key,
        week: bool,
    ) -> glib::Propagation {
        use gtk::gdk::Key;
        if matches!(key, Key::Return | Key::KP_Enter | Key::space) {
            if self.selected.get() != day {
                self.move_to_day(day, week);
            }
            return glib::Propagation::Stop;
        }
        let Some(nav) = keynav::NavKey::from_gdk(key) else {
            return glib::Propagation::Proceed;
        };
        let target = if week {
            match nav {
                keynav::NavKey::Up | keynav::NavKey::Down => return glib::Propagation::Proceed,
                _ => keynav::week_target(day, 0, nav).0,
            }
        } else {
            keynav::month_target(day, nav, self.first_weekday)
        };
        self.move_to_day(target, week);
        glib::Propagation::Stop
    }

    fn render_month_grid(self: &Rc<Self>) {
        while let Some(c) = self.month_box.first_child() {
            self.month_box.remove(&c);
        }
        let d = self.date.get();
        let days = dates::month_grid(d.year(), d.month(), self.first_weekday);
        let weeks = days.len() / 7;
        let items = self.items(days[0], *days.last().unwrap() + Duration::days(1));
        let show_weeks = self.desk_cal.get("show-weekdate", false);
        let grid = gtk::Grid::builder()
            .column_homogeneous(true)
            .row_homogeneous(false)
            .vexpand(true)
            .hexpand(true)
            .accessible_role(gtk::AccessibleRole::Grid)
            .build();
        helix_app_common::set_accessible_label(&grid, &dates::month_title(d));
        grid.update_relation(&[
            gtk::accessible::Relation::RowCount(weeks as i32),
            gtk::accessible::Relation::ColCount(7),
        ]);
        let col0 = if show_weeks { 1 } else { 0 };
        for (i, day) in days.iter().take(7).enumerate() {
            let l = label(
                dates::weekday_short(day.weekday()),
                &["weekday-header", "dim-label"],
            );
            grid.attach(&l, i as i32 + col0, 0, 1, 1);
        }
        let today = today();
        let sel = self.selected.get();
        for w in 0..weeks {
            if show_weeks {
                let l = label(&dates::iso_week(days[w * 7]).to_string(), &["week-number"]);
                l.set_valign(gtk::Align::Start);
                grid.attach(&l, 0, w as i32 + 1, 1, 1);
            }
            for i in 0..7 {
                let day = days[w * 7 + i];
                let cell = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .focusable(true)
                    .accessible_role(gtk::AccessibleRole::GridCell)
                    .build();
                cell.add_css_class("day-cell");
                helix_app_common::focus::set_key(&cell, &day.to_string());
                cell.update_relation(&[
                    gtk::accessible::Relation::RowIndex(w as i32 + 1),
                    gtk::accessible::Relation::ColIndex(i as i32 + 1),
                ]);
                cell.update_state(&[gtk::accessible::State::Selected(Some(day == sel))]);
                cell.set_vexpand(true);
                cell.set_hexpand(true);
                cell.set_overflow(gtk::Overflow::Hidden);
                if day.month() != d.month() {
                    cell.add_css_class("other-month");
                }
                if day == today {
                    cell.add_css_class("today");
                }
                if day == sel {
                    cell.add_css_class("selected");
                }
                let num = label(&day.day().to_string(), &["day-number"]);
                num.set_halign(gtk::Align::End);
                cell.append(&num);
                let on_day: Vec<&Item> = items.iter().filter(|it| it.on_day(day)).collect();
                let titles: Vec<&str> = on_day.iter().map(|it| it.title()).collect();
                helix_app_common::set_accessible_label(
                    &cell,
                    &keynav::day_label(day, today, day == sel, &titles),
                );
                let keys = gtk::EventControllerKey::new();
                let weak = Rc::downgrade(self);
                keys.connect_key_pressed(move |_, key, _, mods| {
                    let Some(ui) = weak.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    if !mods.is_empty() {
                        return glib::Propagation::Proceed;
                    }
                    ui.day_key(day, key, false)
                });
                cell.add_controller(keys);
                let limit = if on_day.len() > MONTH_CHIPS + 1 {
                    MONTH_CHIPS
                } else {
                    on_day.len()
                };
                for it in &on_day[..limit] {
                    cell.append(&self.chip(it, true));
                }
                if on_day.len() > limit {
                    let more = gtk::MenuButton::builder()
                        .label(format!("+{} more", on_day.len() - limit))
                        .css_classes(["flat", "more-chip"])
                        .halign(gtk::Align::Start)
                        .build();
                    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    let head = label(&dates::long_date(day), &["heading"]);
                    head.set_margin_bottom(6);
                    list.append(&head);
                    for it in &on_day {
                        list.append(&self.chip(it, true));
                    }
                    list.set_width_request(220);
                    more.set_popover(Some(&gtk::Popover::builder().child(&list).build()));
                    cell.append(&more);
                }
                let click = gtk::GestureClick::new();
                let weak = Rc::downgrade(self);
                click.connect_released(move |_, n, _, _| {
                    let Some(ui) = weak.upgrade() else { return };
                    if n >= 2 {
                        ui.new_event_on(day, true);
                    } else if ui.selected.get() != day {
                        ui.selected.set(day);
                        if day.month() != ui.date.get().month() {
                            ui.navigate_to(day);
                        } else {
                            ui.render_month();
                            ui.render_agenda();
                        }
                    }
                    if n == 1 {
                        // Keyboard navigation continues from the clicked day.
                        helix_app_common::focus::focus_key(&ui.month_box, &day.to_string());
                    }
                });
                cell.add_controller(click);
                grid.attach(&cell, i as i32 + col0, w as i32 + 1, 1, 1);
            }
        }
        self.month_box.append(&grid);
    }

    fn hour_height(&self) -> f64 {
        HOUR_PX * self.zoom.get()
    }

    /// Rebuilds the week view, keeping focus on the same day header.
    fn render_week(self: &Rc<Self>) {
        let header = self.week_header.clone();
        helix_app_common::focus::rebuild_keeping_focus(&header, None, || self.render_week_view());
        self.update_week_cursor_label();
    }

    fn render_week_view(self: &Rc<Self>) {
        let ws = dates::week_start(self.date.get(), self.first_weekday);
        let days: Vec<NaiveDate> = (0..7).map(|i| ws + Duration::days(i)).collect();
        let items = self.items(ws, ws + Duration::days(7));
        let today = today();
        let twelve = self.twelve();

        // Header with day names and the all-day strip.
        while let Some(c) = self.week_header.first_child() {
            self.week_header.remove(&c);
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let gutter = gtk::Box::new(gtk::Orientation::Vertical, 0);
        gutter.set_width_request(GUTTER);
        if self.desk_cal.get("show-weekdate", false) {
            gutter.append(&label(
                &format!("W{}", dates::iso_week(ws)),
                &["week-number"],
            ));
        }
        row.append(&gutter);
        let grid = gtk::Grid::builder()
            .column_homogeneous(true)
            .hexpand(true)
            .accessible_role(gtk::AccessibleRole::Grid)
            .build();
        helix_app_common::set_accessible_label(&grid, &self.title.label());
        let sel = self.selected.get();
        for (i, day) in days.iter().enumerate() {
            let l = label(
                &format!("{} {}", dates::weekday_short(day.weekday()), day.day()),
                &["week-day-header"],
            );
            if *day == today {
                l.add_css_class("today");
            }
            // A focusable cell around the header: keyboard day navigation
            // and the day's accessible description.
            let cell = gtk::Box::builder()
                .focusable(true)
                .accessible_role(gtk::AccessibleRole::GridCell)
                .css_classes(["week-day-cell"])
                .build();
            l.set_hexpand(true);
            cell.append(&l);
            helix_app_common::focus::set_key(&cell, &day.to_string());
            let titles: Vec<&str> = items
                .iter()
                .filter(|it| it.on_day(*day))
                .map(|it| it.title())
                .collect();
            helix_app_common::set_accessible_label(
                &cell,
                &keynav::day_label(*day, today, *day == sel, &titles),
            );
            cell.update_state(&[gtk::accessible::State::Selected(Some(*day == sel))]);
            cell.update_relation(&[gtk::accessible::Relation::ColIndex(i as i32 + 1)]);
            let keys = gtk::EventControllerKey::new();
            let weak = Rc::downgrade(self);
            let kd = *day;
            keys.connect_key_pressed(move |_, key, _, mods| {
                let Some(ui) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if !mods.is_empty() {
                    return glib::Propagation::Proceed;
                }
                ui.day_key(kd, key, true)
            });
            cell.add_controller(keys);
            let click = gtk::GestureClick::new();
            let weak = Rc::downgrade(self);
            let d = *day;
            click.connect_released(move |_, n, _, _| {
                if let Some(ui) = weak.upgrade() {
                    ui.selected.set(d);
                    ui.render_agenda();
                    if n >= 2 {
                        ui.new_event_on(d, true);
                    }
                }
            });
            l.add_controller(click);
            grid.attach(&cell, i as i32, 0, 1, 1);
        }
        // All-day / multi-day items packed into rows.
        let mut rows_end: Vec<usize> = Vec::new();
        for it in items.iter().filter(|i| i.spans_days()) {
            let first = (it.start.date() - ws).num_days().max(0) as usize;
            let last_day = if it.end > it.start {
                (it.end - Duration::seconds(1)).date()
            } else {
                it.start.date()
            };
            let last = ((last_day - ws).num_days().min(6)).max(first as i64) as usize;
            let r = match rows_end.iter().position(|&e| e < first) {
                Some(r) => {
                    rows_end[r] = last;
                    r
                }
                None => {
                    rows_end.push(last);
                    rows_end.len() - 1
                }
            };
            grid.attach(
                &self.chip(it, false),
                first as i32,
                r as i32 + 1,
                (last - first + 1) as i32,
                1,
            );
        }
        row.append(&grid);
        self.week_header.append(&row);

        // Timed events.
        while let Some(c) = self.week_fixed.first_child() {
            self.week_fixed.remove(&c);
        }
        let hh = self.hour_height();
        let total = (hh * 24.0).round() as i32;
        self.week_area.set_content_height(total);
        self.week_area.set_height_request(total);
        let mut labels = Vec::new();
        for h in 1..24 {
            let l = label(
                &dates::format_time(NaiveTime::from_hms_opt(h, 0, 0).unwrap(), twelve),
                &["hour-label"],
            );
            self.week_fixed.put(&l, 4.0, h as f64 * hh - 8.0);
            labels.push((l, h as i32));
        }
        let mut events = Vec::new();
        for (di, day) in days.iter().enumerate() {
            let ds = day.and_time(NaiveTime::MIN);
            let de = ds + Duration::days(1);
            let todays: Vec<&Item> = items
                .iter()
                .filter(|i| !i.spans_days() && i.on_day(*day))
                .collect();
            let spans: Vec<(i64, i64)> = todays
                .iter()
                .map(|i| {
                    let s = (i.start.max(ds) - ds).num_minutes();
                    let e = (i.end.min(de) - ds).num_minutes();
                    (s, e.max(s))
                })
                .collect();
            let cols = layout::columns(&spans);
            for ((it, (s, e)), (c, n)) in todays.iter().zip(spans).zip(cols) {
                let b = gtk::Button::builder()
                    .css_classes(["week-event", &self.cal_class(&it.calendar)])
                    .build();
                helix_app_common::set_accessible_label(&b, &self.item_label(it));
                let text = format!(
                    "{}\n{}",
                    it.title(),
                    dates::format_time(it.start.time(), twelve)
                );
                let l = gtk::Label::builder()
                    .label(&text)
                    .xalign(0.0)
                    .yalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .valign(gtk::Align::Start)
                    .build();
                b.set_child(Some(&l));
                b.set_tooltip_text(Some(&self.describe(it)));
                let weak = Rc::downgrade(self);
                let item = (*it).clone();
                b.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        ui.open_item(&item);
                    }
                });
                self.week_fixed.put(&b, 0.0, 0.0);
                events.push((b, di, s, e, c, n));
            }
        }
        *self.week_layout.borrow_mut() = WeekLayout {
            days,
            events,
            labels,
        };
        let width = self.week_area.width();
        if width > 0 {
            self.place_week_children(width);
        }
        self.week_area.queue_draw();
        if !self.week_scrolled_once.replace(true) {
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(ui) = weak.upgrade() {
                    let now = Local::now().time();
                    let target = (now.hour() as f64 - 1.0).max(7.0f64.min(now.hour() as f64))
                        * ui.hour_height();
                    ui.week_scroll.vadjustment().set_value(target);
                }
            });
        }
    }

    fn place_week_children(&self, width: i32) {
        let hh = self.hour_height();
        let col_w = ((width - GUTTER) as f64 / 7.0).max(10.0);
        let lay = self.week_layout.borrow();
        for (b, di, s, e, c, n) in &lay.events {
            let sub = col_w / *n as f64;
            let x = GUTTER as f64 + *di as f64 * col_w + *c as f64 * sub + 1.0;
            let y = *s as f64 / 60.0 * hh + 1.0;
            let h = ((*e - *s) as f64 / 60.0 * hh - 2.0).max(18.0);
            b.set_size_request((sub - 3.0).max(8.0) as i32, h as i32);
            self.week_fixed.move_(b, x, y);
        }
        for (l, h) in &lay.labels {
            self.week_fixed
                .move_(l, 4.0, (*h as f64 * hh - 8.0).max(0.0));
        }
    }

    fn week_hit(&self, width: i32, x: f64, y: f64) -> Option<(NaiveDate, i64)> {
        if x < GUTTER as f64 {
            return None;
        }
        let col_w = (width - GUTTER) as f64 / 7.0;
        let di = ((x - GUTTER as f64) / col_w).floor() as usize;
        let day = *self.week_layout.borrow().days.get(di)?;
        let minutes = (y / self.hour_height() * 60.0).clamp(0.0, 24.0 * 60.0 - 30.0) as i64;
        Some((day, minutes))
    }

    /// Arrow / Home / End / Page keys move the week grid's slot cursor;
    /// Enter creates a one-hour event at it.
    fn week_grid_key(self: &Rc<Self>, key: gtk::gdk::Key) -> glib::Propagation {
        use gtk::gdk::Key;
        let day = self.selected.get();
        let minutes = self.week_cursor.get();
        if matches!(key, Key::Return | Key::KP_Enter | Key::space) {
            if !self.state.borrow().unavailable {
                let start = day.and_time(NaiveTime::MIN) + Duration::minutes(minutes);
                editor::open(
                    self,
                    Target::New {
                        start,
                        end: start + Duration::hours(1),
                        all_day: false,
                    },
                );
            }
            return glib::Propagation::Stop;
        }
        let Some(nav) = keynav::NavKey::from_gdk(key) else {
            return glib::Propagation::Proceed;
        };
        let (d, m) = keynav::week_target(day, minutes, nav);
        self.week_cursor.set(m);
        if d != day {
            self.navigate_to(d);
        }
        self.update_week_cursor_label();
        self.scroll_to_week_cursor();
        self.week_area.queue_draw();
        glib::Propagation::Stop
    }

    /// The grid's accessible name describes the slot under the cursor;
    /// moving the cursor also announces it (a name change alone is not
    /// spoken).
    fn update_week_cursor_label(&self) {
        let day = self.selected.get();
        let m = self.week_cursor.get();
        let start = day.and_time(NaiveTime::MIN) + Duration::minutes(m);
        let end = start + Duration::minutes(keynav::SLOT);
        let titles: Vec<String> = self
            .items(day, day + Duration::days(1))
            .into_iter()
            .filter(|i| !i.spans_days() && i.start < end && i.end > start)
            .map(|i| i.title().to_string())
            .collect();
        let mut text = format!(
            "{}, {}",
            dates::long_date(day),
            dates::format_time(start.time(), self.twelve())
        );
        if !titles.is_empty() {
            text.push_str(&format!(", {}", titles.join(", ")));
        }
        helix_app_common::set_accessible_label(&self.week_area, &text);
        if self.week_area.has_focus() {
            helix_app_common::announce_change(
                &self.week_area,
                &text,
                helix_app_common::Urgency::Polite,
            );
        }
    }

    fn scroll_to_week_cursor(&self) {
        let adj = self.week_scroll.vadjustment();
        let hh = self.hour_height();
        let y = self.week_cursor.get() as f64 / 60.0 * hh;
        if y < adj.value() {
            adj.set_value(y);
        } else if y + hh / 2.0 > adj.value() + adj.page_size() {
            adj.set_value(y + hh / 2.0 - adj.page_size());
        }
    }

    fn draw_week(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: i32, _h: i32) {
        let fg = area.color();
        let hh = self.hour_height();
        let (r, g, b) = (fg.red() as f64, fg.green() as f64, fg.blue() as f64);
        cr.set_line_width(1.0);
        for h in 0..24 {
            let y = (h as f64 * hh).floor() + 0.5;
            cr.set_source_rgba(r, g, b, 0.12);
            cr.move_to(GUTTER as f64, y);
            cr.line_to(w as f64, y);
            let _ = cr.stroke();
            let y2 = (h as f64 * hh + hh / 2.0).floor() + 0.5;
            cr.set_source_rgba(r, g, b, 0.05);
            cr.move_to(GUTTER as f64, y2);
            cr.line_to(w as f64, y2);
            let _ = cr.stroke();
        }
        let col_w = (w - GUTTER) as f64 / 7.0;
        cr.set_source_rgba(r, g, b, 0.12);
        for i in 0..7 {
            let x = (GUTTER as f64 + i as f64 * col_w).floor() + 0.5;
            cr.move_to(x, 0.0);
            cr.line_to(x, hh * 24.0);
            let _ = cr.stroke();
        }
        let lay = self.week_layout.borrow();
        let now = Local::now().naive_local();
        let sel = self.selected.get();
        if let Some(si) = lay.days.iter().position(|d| *d == sel) {
            cr.set_source_rgba(r, g, b, 0.04);
            cr.rectangle(GUTTER as f64 + si as f64 * col_w, 0.0, col_w, hh * 24.0);
            let _ = cr.fill();
        }
        if area.has_focus() {
            if let Some(si) = lay.days.iter().position(|d| *d == sel) {
                let accent = area.color();
                let y = self.week_cursor.get() as f64 / 60.0 * hh;
                cr.set_source_rgba(
                    accent.red() as f64,
                    accent.green() as f64,
                    accent.blue() as f64,
                    0.7,
                );
                cr.set_line_width(2.0);
                cr.rectangle(
                    GUTTER as f64 + si as f64 * col_w + 2.0,
                    y + 1.0,
                    col_w - 4.0,
                    hh / 2.0 - 2.0,
                );
                let _ = cr.stroke();
            }
        }
        if let Some(di) = lay.days.iter().position(|d| *d == now.date()) {
            let y = (now.time().num_seconds_from_midnight() as f64 / 3600.0) * hh;
            let x0 = GUTTER as f64 + di as f64 * col_w;
            cr.set_source_rgb(0.88, 0.11, 0.14);
            cr.set_line_width(2.0);
            cr.move_to(x0, y);
            cr.line_to(x0 + col_w, y);
            let _ = cr.stroke();
            cr.arc(x0, y, 4.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }

    // ----- sidebar -----

    /// Rebuilds the calendar list, keeping focus on the toggled check box.
    fn render_calendar_list(self: &Rc<Self>) {
        let list = self.cal_list.clone();
        helix_app_common::focus::rebuild_keeping_focus(&list, None, || {
            self.render_calendar_list_rows()
        });
    }

    fn render_calendar_list_rows(self: &Rc<Self>) {
        while let Some(c) = self.cal_list.first_child() {
            self.cal_list.remove(&c);
        }
        let sources = self.sources();
        let mut cals: Vec<&Source> = sources
            .iter()
            .filter(|s| s.is(SourceKind::Calendar) && s.enabled())
            .collect();
        cals.sort_by_key(|s| {
            (
                helix_eds::source::account_label(s, &sources).to_lowercase(),
                s.display_name().to_lowercase(),
            )
        });
        for s in cals {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_margin_start(4);
            let check = gtk::CheckButton::builder()
                .active(s.selected())
                .sensitive(s.writable)
                .build();
            let dot = gtk::Box::builder()
                .css_classes(["cal-dot", &format!("{}-dot", self.cal_class(&s.uid))])
                .valign(gtk::Align::Center)
                .build();
            let name = gtk::Label::builder()
                .label(s.display_name())
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            name.set_tooltip_text(Some(&helix_eds::source::account_label(s, &sources)));
            helix_app_common::label_widget(&check, &name);
            helix_app_common::focus::set_key(&row, &s.uid);
            helix_app_common::focus::set_part(&check, "check");
            row.append(&check);
            row.append(&dot);
            row.append(&name);
            let weak = Rc::downgrade(self);
            let src = s.clone();
            check.connect_toggled(move |c| {
                if let Some(ui) = weak.upgrade() {
                    let mut s = src.clone();
                    s.set_selected(c.is_active());
                    ui.backend.send(Cmd::WriteSource(Box::new(s)));
                }
            });
            self.cal_list.append(&row);
        }
    }

    /// Rebuilds the agenda, keeping focus on the same event (or a
    /// neighbour) and the scroll position.
    fn render_agenda(self: &Rc<Self>) {
        let list = self.agenda.clone();
        helix_app_common::focus::rebuild_keeping_focus(&list, None, || self.render_agenda_rows());
    }

    fn render_agenda_rows(self: &Rc<Self>) {
        while let Some(c) = self.agenda.first_child() {
            self.agenda.remove(&c);
        }
        let twelve = self.twelve();
        let results = self.state.borrow().search_results.clone();
        if let Some(results) = results {
            self.agenda_title
                .set_label(&format!("{} Search Results", results.len()));
            let zones = self.zones.borrow();
            for (i, (cal, e)) in results.iter().enumerate() {
                let s = e.start.local(&zones);
                let when = if e.all_day() {
                    dates::short_date(s.date())
                } else {
                    format!(
                        "{} {}",
                        dates::short_date(s.date()),
                        dates::format_time(s.time(), twelve)
                    )
                };
                let title = if e.summary.trim().is_empty() {
                    "(No title)"
                } else {
                    e.summary.as_str()
                };
                let row = self.agenda_row(cal, title, &when);
                helix_app_common::focus::set_key(&row, &format!("{cal}|{title}|{when}"));
                row.set_widget_name(&format!("search:{i}"));
                self.agenda.append(&row);
            }
            return;
        }
        let day = self.selected.get();
        self.agenda_title.set_label(&dates::long_date(day));
        let items: Vec<Item> = self
            .items(day, day + Duration::days(1))
            .into_iter()
            .filter(|i| i.on_day(day))
            .collect();
        if items.is_empty() {
            let l = label("No events", &["dim-label"]);
            l.set_margin_top(6);
            l.set_margin_bottom(6);
            let row = gtk::ListBoxRow::builder()
                .child(&l)
                .activatable(false)
                .selectable(false)
                .build();
            self.agenda.append(&row);
        }
        for (i, it) in items.iter().enumerate() {
            let when = if it.spans_days() {
                "All day".to_string()
            } else {
                format!(
                    "{} – {}",
                    dates::format_time(it.start.time(), twelve),
                    dates::format_time(it.end.time(), twelve)
                )
            };
            let row = self.agenda_row(&it.calendar, it.title(), &when);
            helix_app_common::focus::set_key(
                &row,
                &format!("{}|{}|{when}", it.calendar, it.title()),
            );
            row.set_widget_name(&format!("item:{i}"));
            self.agenda.append(&row);
        }
        *self.agenda_items.borrow_mut() = items;
    }

    /// An agenda row: colour dot, title, and "time · calendar" (the
    /// calendar is named in text, not only by its colour).
    fn agenda_row(&self, cal: &str, title: &str, when: &str) -> gtk::ListBoxRow {
        let cal_name = self.cal_name(cal);
        let when = if cal_name.is_empty() {
            when.to_string()
        } else {
            format!("{when} · {cal_name}")
        };
        let when = when.as_str();
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        b.set_margin_top(4);
        b.set_margin_bottom(4);
        let dot = gtk::Box::builder()
            .css_classes(["cal-dot", &format!("{}-dot", self.cal_class(cal))])
            .valign(gtk::Align::Center)
            .build();
        let v = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let t = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        v.append(&t);
        v.append(
            &gtk::Label::builder()
                .label(when)
                .xalign(0.0)
                .css_classes(["agenda-time"])
                .build(),
        );
        b.append(&dot);
        b.append(&v);
        gtk::ListBoxRow::builder().child(&b).build()
    }

    fn activate_agenda_row(self: &Rc<Self>, name: &str) {
        if let Some(i) = name
            .strip_prefix("item:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let item = self.agenda_items.borrow().get(i).cloned();
            if let Some(item) = item {
                self.open_item(&item);
            }
        } else if let Some(i) = name
            .strip_prefix("search:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let found = self
                .state
                .borrow()
                .search_results
                .as_ref()
                .and_then(|r| r.get(i).cloned());
            if let Some((cal, e)) = found {
                self.handle(Msg::Found(Some(Box::new((cal, e)))));
            }
        }
    }

    // ----- import -----

    fn choose_import(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Calendar files"));
        filter.add_mime_type("text/calendar");
        filter.add_suffix("ics");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Import Events")
            .filters(&filters)
            .build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if let (Some(ui), Ok(file)) = (weak.upgrade(), res) {
                if let Some(p) = file.path() {
                    ui.import_file(p);
                }
            }
        });
    }

    fn import_file(self: &Rc<Self>, path: std::path::PathBuf) {
        let text = match std::fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                self.toast(&format!("Could not read {}: {e}", path.display()));
                return;
            }
        };
        let comps = ical::parse(&text);
        let mut events = Vec::new();
        let mut tzs = Vec::new();
        for c in &comps {
            let mut ev = Vec::new();
            c.find_all("VEVENT", &mut ev);
            events.extend(ev.into_iter().map(|e| e.to_ics()));
            let mut tz = Vec::new();
            c.find_all("VTIMEZONE", &mut tz);
            tzs.extend(tz.into_iter().map(|t| t.to_ics()));
        }
        if events.is_empty() {
            self.toast("No events found in the file");
            return;
        }
        let ui = self.clone();
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        // Wait until calendars are known before asking where to import.
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let cals: Vec<(String, String, bool)> =
                ui.calendar_choices().into_iter().filter(|c| c.2).collect();
            if cals.is_empty() {
                return if ui.state.borrow().unavailable {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                };
            }
            let dialog = adw::AlertDialog::new(
                Some("Import Events"),
                Some(&format!(
                    "Add {} event{} from “{file}” to:",
                    events.len(),
                    if events.len() == 1 { "" } else { "s" }
                )),
            );
            let names: Vec<&str> = cals.iter().map(|(_, n, _)| n.as_str()).collect();
            let dd = gtk::DropDown::from_strings(&names);
            helix_app_common::set_accessible_label(&dd, "Import into Calendar");
            let def = ui.default_calendar();
            dd.set_selected(cals.iter().position(|(u, _, _)| *u == def).unwrap_or(0) as u32);
            dialog.set_extra_child(Some(&dd));
            dialog.add_response("cancel", "_Cancel");
            dialog.add_response("import", "_Import");
            dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("import"));
            dialog.set_close_response("cancel");
            let (u2, ev, tz) = (ui.clone(), events.clone(), tzs.clone());
            dialog.connect_response(Some("import"), move |_, _| {
                if let Some((cal, _, _)) = cals.get(dd.selected() as usize) {
                    u2.backend.send(Cmd::Import {
                        cal: cal.clone(),
                        events: ev.clone(),
                        timezones: tz.clone(),
                    });
                }
            });
            dialog.present(Some(&ui.window));
            glib::ControlFlow::Break
        });
    }
}

fn show_shortcuts(win: &adw::ApplicationWindow) {
    let rows = [
        ("New event on the selected day", "Ctrl+N"),
        (
            "Move between days (month view, week day headers)",
            "Arrows, Home, End",
        ),
        (
            "Previous / next month (month view), week (week view)",
            "Page Up / Page Down",
        ),
        ("Select the focused day", "Enter"),
        (
            "Move the week grid's time cursor",
            "Arrows, Home, End, Page Up / Page Down",
        ),
        ("New event at the week grid's time cursor", "Enter"),
        ("Go to today", "Ctrl+T"),
        ("Previous", "Alt+Left"),
        ("Next", "Alt+Right"),
        ("Week view", "Ctrl+1"),
        ("Month view", "Ctrl+2"),
        ("Search", "Ctrl+F"),
        ("Toggle sidebar", "F9"),
        ("Zoom week grid", "Ctrl+Plus / Ctrl+Minus / Ctrl+0"),
        ("Import", "Ctrl+I"),
        ("Keyboard shortcuts", "Ctrl+?"),
        ("Quit", "Ctrl+Q"),
    ];
    let group = adw::PreferencesGroup::new();
    for (title, keys) in rows {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(
            &gtk::Label::builder()
                .label(keys)
                .css_classes(["dim-label"])
                .build(),
        );
        group.add(&row);
    }
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let dialog = adw::PreferencesDialog::builder()
        .title("Keyboard Shortcuts")
        .build();
    dialog.add(&page);
    dialog.present(Some(win));
}
