//! GTK user interface.

use adw::prelude::*;
use chrono::{DateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use crate::alarm::{self, Alarm, AlarmEntry, Event, RingState, Weekdays};
use crate::background;
use crate::clock::{Clock, SystemClock};
use crate::gwdb;
use crate::location;
use crate::sound::{self, Sound};
use crate::stopwatch::{self, Stopwatch};
use crate::store::{self, WorldClock};
use crate::sun::{self, SunTimes};
use crate::timer::{self, Timer};
use crate::tz::{self, Location};

pub const APP_ID: &str = "org.helix.Clocks";
const SCHEMA: &str = "org.gnome.clocks";
const WINDOW_SCHEMA: &str = "org.gnome.clocks.state.window";
const IFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const PAGES: [(&str, &str, &str); 4] = [
    ("world", "World", "globe-symbolic"),
    ("alarm", "Alarms", "alarm-symbolic"),
    ("stopwatch", "Stopwatch", "stopwatch-symbolic"),
    ("timer", "Timer", "timer-symbolic"),
];

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(
        &app,
        "Clocks",
        "World clocks, alarms, a stopwatch and timers",
    );
    app.add_main_option(
        "hidden",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Run in the background to ring alarms, without opening a window",
        None,
    );
    let hidden = Rc::new(Cell::new(false));
    {
        let hidden = hidden.clone();
        app.connect_handle_local_options(move |app, opts| {
            if opts.contains("hidden") {
                if app.register(None::<&gio::Cancellable>).is_err() {
                    return 1;
                }
                if app.is_remote() {
                    // Already running: nothing to do (and no window to show).
                    return 0;
                }
                hidden.set(true);
            }
            -1
        });
    }
    app.connect_startup(|_| load_css());
    let ui: Rc<RefCell<Option<Rc<Ui>>>> = Rc::default();
    app.connect_activate(move |app| {
        let existing = ui.borrow().clone();
        match existing {
            Some(u) => u.window.present(),
            None => {
                let u = Ui::build(app);
                if hidden.replace(false) {
                    // Started at login: stay hidden while alarms need us,
                    // otherwise there is nothing to do.
                    if !u.has_active_alarms() {
                        u.window.set_hide_on_close(false);
                        u.window.destroy();
                        return;
                    }
                } else {
                    u.window.present();
                }
                *ui.borrow_mut() = Some(u);
            }
        }
    });
    app.run()
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".clock-time { font-feature-settings: \"tnum\"; }
         .world-time { font-size: 18pt; font-weight: 300; }
         .details-time { font-size: 48pt; font-weight: 200; }
         .alarm-time { font-size: 22pt; font-weight: 300; }
         .stopwatch-time { font-size: 54pt; font-weight: 200; }
         .timer-time { font-size: 30pt; font-weight: 300; }
         .day-toggle { min-width: 44px; }
         .alarm-inactive .alarm-time { opacity: 0.55; }",
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

fn beep() {
    if let Some(d) = gtk::gdk::Display::default() {
        d.beep();
    }
}

struct WorldRow {
    /// The accessible description last set on the row.
    description: RefCell<String>,
    time: gtk::Label,
    icon: gtk::Image,
    row: adw::ActionRow,
    zone: Tz,
    coords: Option<(f64, f64)>,
    country: Option<String>,
}

/// A world clock row's accessible description: the time, day or night
/// (shown only as an icon) and the subtitle — "10:30 PM, Night, Japan •
/// Tomorrow • +8 hours".
fn world_row_description(time: &str, day: bool, subtitle: &str) -> String {
    let when = if day { "Day" } else { "Night" };
    if subtitle.is_empty() {
        format!("{time}, {when}")
    } else {
        format!("{time}, {when}, {subtitle}")
    }
}

struct Details {
    clock: WorldClock,
    time: gtk::Label,
    date: gtk::Label,
    rows: Vec<(adw::ActionRow, &'static str)>,
}

struct TimerRow {
    /// Tells the timer apart for assistive technology ("Tea",
    /// "5 minutes timer").
    label: String,
    time: gtk::Label,
    progress: gtk::ProgressBar,
    start: gtk::Button,
    reset: gtk::Button,
}

/// The widgets of one alarm row, updated in place.
struct AlarmRow {
    row: gtk::ListBoxRow,
    time: gtk::Label,
    sub: gtk::Label,
    switch: gtk::Switch,
}

struct Ui {
    app: adw::Application,
    window: adw::ApplicationWindow,
    clock: SystemClock,
    prefs: Prefs,
    win_prefs: Prefs,
    db: Vec<Location>,
    local: Tz,
    twelve: Cell<bool>,
    last_second: Cell<i64>,

    stack: adw::ViewStack,
    add_button: gtk::Button,
    back_button: gtk::Button,

    // World
    world: RefCell<Vec<WorldClock>>,
    world_nav: adw::NavigationView,
    world_stack: gtk::Stack,
    world_list: gtk::ListBox,
    world_rows: RefCell<Vec<WorldRow>>,
    details: RefCell<Option<Details>>,

    // Alarms
    alarms: RefCell<Vec<AlarmEntry>>,
    alarm_stack: gtk::Stack,
    alarm_list: gtk::ListBox,
    /// The alarm rows, in list order, by alarm id.
    alarm_rows: RefCell<Vec<(String, AlarmRow)>>,
    alarm_note: gtk::Label,
    ringing: RefCell<HashMap<String, adw::AlertDialog>>,
    id_salt: Cell<u64>,
    player: RefCell<sound::Player>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// The `autostart` value last sent to the Background portal.
    autostart_requested: Cell<Option<bool>>,
    /// GWeather location database (station codes), if installed.
    gw: Option<Rc<gwdb::Db>>,
    /// The geolocated current place, once known.
    here: RefCell<Option<WorldClock>>,

    // Stopwatch
    stopwatch: RefCell<Stopwatch>,
    sw_time: gtk::Label,
    sw_split: gtk::Label,
    sw_primary: gtk::Button,
    sw_secondary: gtk::Button,
    sw_laps: gtk::ListBox,
    sw_laps_box: gtk::Widget,

    // Timers
    timers: RefCell<Vec<Timer>>,
    timer_stack: gtk::Stack,
    timer_list: gtk::ListBox,
    timer_rows: RefCell<Vec<TimerRow>>,
    timer_cancel: gtk::Button,
}

impl Ui {
    fn build(app: &adw::Application) -> Rc<Ui> {
        let prefs = Prefs::new(SCHEMA);
        let win_prefs = Prefs::new(WINDOW_SCHEMA);
        let iface = Prefs::new(IFACE_SCHEMA);
        let clock = SystemClock::default();
        let db = tz::load_locations();
        let local = tz::local_zone()
            .or_else(|| {
                glib::TimeZone::local()
                    .identifier()
                    .as_str()
                    .parse::<Tz>()
                    .ok()
            })
            .unwrap_or(chrono_tz::UTC);
        let now = clock.now();

        let gw = gwdb::Db::load().map(Rc::new);
        let world = prefs
            .get_variant("world-clocks")
            .map(|v| store::world_clocks_from_variant(&v, &db, gw.as_deref()))
            .unwrap_or_default();
        let mut alarms: Vec<AlarmEntry> = prefs
            .get_variant("alarms")
            .map(|v| store::alarms_from_variant(&v))
            .unwrap_or_default()
            .into_iter()
            .map(|a| AlarmEntry::new(a, now, &local))
            .collect();
        alarms.sort_by_key(|e| (e.alarm.hour, e.alarm.minute));
        let timers = prefs
            .get_variant("timers")
            .map(|v| store::timers_from_variant(&v))
            .unwrap_or_default();

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Clocks")
            .width_request(360)
            .height_request(294)
            .build();
        let (w, h) = win_prefs.get::<(i32, i32)>("size", (870, 690));
        window.set_default_size(w.max(360), h.max(294));
        if win_prefs.get("maximized", false) {
            window.maximize();
        }

        // Header
        let stack = adw::ViewStack::new();
        let header = adw::HeaderBar::new();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        header.set_title_widget(Some(&switcher));
        let add_button = gtk::Button::from_icon_name("list-add-symbolic");
        header.pack_start(&add_button);
        let back_button = gtk::Button::from_icon_name("go-previous-symbolic");
        back_button.set_tooltip_text(Some("Back"));
        helix_app_common::set_accessible_label(&back_button, "Back");
        back_button.set_visible(false);
        header.pack_start(&back_button);
        let menu = gio::Menu::new();
        menu.append(Some("_Keyboard Shortcuts"), Some("win.show-help-overlay"));
        menu.append(Some("_About Clocks"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build()
            .labelled("Main Menu");
        header.pack_end(&menu_button);

        // World page
        let world_list = boxed_list();
        let world_stack = gtk::Stack::new();
        let world_empty = adw::StatusPage::builder()
            .icon_name("globe-symbolic")
            .title("Add World Clocks")
            .description("Use the + button to add world clocks")
            .vexpand(true)
            .build();
        let empty_add = gtk::Button::builder()
            .label("_Add Location")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .build();
        empty_add.add_css_class("pill");
        empty_add.add_css_class("suggested-action");
        world_empty.set_child(Some(&empty_add));
        world_stack.add_named(&world_empty, Some("empty"));
        world_stack.add_named(&scrolled_clamped(&world_list), Some("list"));
        let world_nav = adw::NavigationView::new();
        world_nav.add(&adw::NavigationPage::new(&world_stack, "World"));

        // Alarms page
        let alarm_list = boxed_list();
        let alarm_stack = gtk::Stack::new();
        let alarm_empty = adw::StatusPage::builder()
            .icon_name("alarm-symbolic")
            .title("No Alarms")
            .description("Use the + button to add alarms")
            .vexpand(true)
            .build();
        let alarm_add = gtk::Button::builder()
            .label("_Add Alarm")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .build();
        alarm_add.add_css_class("pill");
        alarm_add.add_css_class("suggested-action");
        alarm_empty.set_child(Some(&alarm_add));
        alarm_stack.add_named(&alarm_empty, Some("empty"));
        let alarm_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        alarm_box.append(&alarm_list);
        let note = label(
            "Alarms only ring while Clocks is running.",
            &["dim-label", "caption"],
        );
        note.set_visible(false);
        alarm_box.append(&note);
        alarm_stack.add_named(&scrolled_clamped(&alarm_box), Some("list"));

        // Stopwatch page
        let sw_time = label("00:00.0", &["stopwatch-time", "clock-time"]);
        let sw_split = label("", &["dim-label", "clock-time", "title-4"]);
        let sw_primary = gtk::Button::with_label("Start");
        let sw_secondary = gtk::Button::with_label("Lap");
        for b in [&sw_primary, &sw_secondary] {
            b.add_css_class("pill");
            b.set_width_request(120);
        }
        sw_primary.add_css_class("suggested-action");
        sw_secondary.set_sensitive(false);
        let sw_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        sw_buttons.set_halign(gtk::Align::Center);
        sw_buttons.append(&sw_primary);
        sw_buttons.append(&sw_secondary);
        let sw_laps = boxed_list();
        let sw_laps_scroll = scrolled_clamped(&sw_laps);
        sw_laps_scroll.set_visible(false);
        let sw_page = gtk::Box::new(gtk::Orientation::Vertical, 24);
        sw_page.set_margin_top(36);
        sw_page.append(&sw_time);
        sw_page.append(&sw_split);
        sw_page.append(&sw_buttons);
        sw_page.append(&sw_laps_scroll);

        // Timer page
        let timer_list = boxed_list();
        let timer_stack = gtk::Stack::new();
        let timer_cancel = gtk::Button::builder()
            .label("_Cancel")
            .use_underline(true)
            .halign(gtk::Align::Center)
            .build();
        timer_cancel.add_css_class("pill");

        stack.add_titled_with_icon(&world_nav, Some(PAGES[0].0), PAGES[0].1, PAGES[0].2);
        stack.add_titled_with_icon(&alarm_stack, Some(PAGES[1].0), PAGES[1].1, PAGES[1].2);
        stack.add_titled_with_icon(&sw_page, Some(PAGES[2].0), PAGES[2].1, PAGES[2].2);
        stack.add_titled_with_icon(&timer_stack, Some(PAGES[3].0), PAGES[3].1, PAGES[3].2);

        let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&stack));
        toolbar.add_bottom_bar(&switcher_bar);
        window.set_content(Some(&toolbar));
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 550sp").unwrap());
        bp.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
        bp.add_setter(
            &header,
            "title-widget",
            Some(&None::<gtk::Widget>.to_value()),
        );
        window.add_breakpoint(bp);

        let twelve = iface.get_enum_nick("clock-format", "24h") == "12h";
        let ui = Rc::new(Ui {
            app: app.clone(),
            window: window.clone(),
            clock,
            prefs,
            win_prefs,
            db,
            local,
            twelve: Cell::new(twelve),
            last_second: Cell::new(i64::MIN),
            stack: stack.clone(),
            add_button: add_button.clone(),
            back_button: back_button.clone(),
            world: RefCell::new(world),
            world_nav: world_nav.clone(),
            world_stack,
            world_list,
            world_rows: RefCell::default(),
            details: RefCell::default(),
            alarms: RefCell::new(alarms),
            alarm_stack,
            alarm_list,
            alarm_rows: RefCell::default(),
            alarm_note: note,
            ringing: RefCell::default(),
            id_salt: Cell::new(0),
            player: RefCell::default(),
            hold: RefCell::default(),
            autostart_requested: Cell::new(None),
            gw,
            here: RefCell::default(),
            stopwatch: RefCell::default(),
            sw_time,
            sw_split,
            sw_primary: sw_primary.clone(),
            sw_secondary: sw_secondary.clone(),
            sw_laps,
            sw_laps_box: sw_laps_scroll.upcast(),
            timers: RefCell::new(timers),
            timer_stack,
            timer_list,
            timer_rows: RefCell::default(),
            timer_cancel: timer_cancel.clone(),
        });

        ui.build_timer_pages();

        // Wiring
        let page = ui.win_prefs.get_enum_nick("panel-id", "world");
        if PAGES.iter().any(|p| p.0 == page) {
            stack.set_visible_child_name(&page);
        }
        let weak = Rc::downgrade(&ui);
        stack.connect_visible_child_name_notify(with_ui(&weak, |ui| ui.update_header()));
        world_nav.connect_visible_page_notify(with_ui(&weak, |ui| {
            if ui.world_nav.navigation_stack().n_items() <= 1 {
                ui.details.replace(None);
            }
            ui.update_header()
        }));
        back_button.connect_clicked(with_ui(&weak, |ui| {
            ui.world_nav.pop();
        }));
        add_button.connect_clicked(with_ui(&weak, |ui| ui.new_item()));
        empty_add.connect_clicked(with_ui(&weak, |ui| ui.show_add_location()));
        alarm_add.connect_clicked(with_ui(&weak, |ui| ui.show_alarm_dialog(None)));
        sw_primary.connect_clicked(with_ui(&weak, |ui| ui.stopwatch_primary()));
        sw_secondary.connect_clicked(with_ui(&weak, |ui| ui.stopwatch_secondary()));
        timer_cancel.connect_clicked(with_ui(&weak, |ui| {
            ui.timer_stack.set_visible_child_name("list")
        }));

        if let Some(s) = iface.gsettings() {
            let weak = weak.clone();
            s.connect_changed(Some("clock-format"), move |s, _| {
                if let Some(ui) = weak.upgrade() {
                    ui.twelve.set(s.string("clock-format") == "12h");
                    ui.rebuild_world();
                    ui.rebuild_alarms();
                }
            });
            // Keep the settings object alive with the window.
            unsafe { window.set_data("iface-settings", s.clone()) };
        }
        {
            let weak = weak.clone();
            window.connect_close_request(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.save_window_state();
                }
                glib::Propagation::Proceed
            });
        }

        ui.install_actions();
        ui.rebuild_world();
        ui.rebuild_alarms();
        ui.rebuild_timers();
        ui.update_stopwatch();
        ui.update_header();
        ui.update_keep_alive();
        ui.locate();

        let weak2 = weak.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || match weak2.upgrade() {
            Some(ui) => {
                ui.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        ui
    }

    fn page(&self) -> String {
        self.stack
            .visible_child_name()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    fn update_header(&self) {
        let page = self.page();
        let in_details = page == "world" && self.world_nav.navigation_stack().n_items() > 1;
        self.back_button.set_visible(in_details);
        self.add_button
            .set_visible(!in_details && page != "stopwatch");
        let add_label = match page.as_str() {
            "world" => "Add Location",
            "alarm" => "Add Alarm",
            _ => "Add Timer",
        };
        self.add_button.set_tooltip_text(Some(add_label));
        helix_app_common::set_accessible_label(&self.add_button, add_label);
    }

    fn new_item(self: &Rc<Self>) {
        match self.page().as_str() {
            "world" => self.show_add_location(),
            "alarm" => self.show_alarm_dialog(None),
            "timer" => {
                self.timer_cancel
                    .set_visible(!self.timers.borrow().is_empty());
                self.timer_stack.set_visible_child_name("setup");
            }
            _ => {}
        }
    }

    fn save_window_state(&self) {
        let maximized = self.window.is_maximized();
        self.win_prefs.set("maximized", maximized);
        if !maximized {
            let (w, h) = self.window.default_size();
            self.win_prefs.set("size", (w, h));
        }
        self.win_prefs.set_enum_nick("panel-id", &self.page());
    }

    fn install_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let page = gio::SimpleAction::new("page", Some(glib::VariantTy::STRING));
        page.connect_activate(move |_, p| {
            if let (Some(ui), Some(name)) = (weak.upgrade(), p.and_then(|p| p.str())) {
                ui.stack.set_visible_child_name(name);
            }
        });
        self.window.add_action(&page);
        for (i, (name, _, _)) in PAGES.iter().enumerate() {
            self.app
                .set_accels_for_action(&format!("win.page::{name}"), &[&format!("<Alt>{}", i + 1)]);
        }
        let weak = Rc::downgrade(self);
        let new = gio::SimpleAction::new("new", None);
        new.connect_activate(move |_, _| {
            if let Some(ui) = weak.upgrade() {
                ui.new_item();
            }
        });
        self.window.add_action(&new);
        self.app.set_accels_for_action("win.new", &["<Control>n"]);

        for (name, snooze) in [("alarm-snooze", true), ("alarm-stop", false)] {
            let weak = Rc::downgrade(self);
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            a.connect_activate(move |_, p| {
                if let (Some(ui), Some(id)) = (weak.upgrade(), p.and_then(|p| p.str())) {
                    ui.respond_alarm(id, snooze);
                }
            });
            self.app.add_action(&a);
        }

        // Stopwatch keys, active only on the stopwatch page.
        let ctl = gtk::ShortcutController::new();
        for (trigger, which) in [("space", 0), ("l", 1), ("Delete", 2), ("BackSpace", 2)] {
            let weak = Rc::downgrade(self);
            let action = gtk::CallbackAction::new(move |_, _| {
                let Some(ui) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if ui.page() != "stopwatch" {
                    return glib::Propagation::Proceed;
                }
                match which {
                    0 => ui.stopwatch_primary(),
                    1 => {
                        if ui.stopwatch.borrow().is_running() {
                            ui.stopwatch_secondary()
                        }
                    }
                    _ => {
                        let paused =
                            matches!(ui.stopwatch.borrow().state, stopwatch::State::Paused { .. });
                        if paused {
                            ui.stopwatch_secondary()
                        }
                    }
                }
                glib::Propagation::Stop
            });
            if let Some(t) = gtk::ShortcutTrigger::parse_string(trigger) {
                ctl.add_shortcut(gtk::Shortcut::new(Some(t), Some(action)));
            }
        }
        self.window.add_controller(ctl);

        let builder = gtk::Builder::from_string(SHORTCUTS_UI);
        if let Some(overlay) = builder.object::<gtk::ShortcutsWindow>("help_overlay") {
            self.window.set_help_overlay(Some(&overlay));
        }
        self.app
            .set_accels_for_action("win.show-help-overlay", &["<Control>question"]);
    }

    fn has_active_alarms(&self) -> bool {
        self.alarms.borrow().iter().any(|e| e.alarm.active)
    }

    /// Keep the process alive (application hold; the window hides on close)
    /// while an alarm is enabled or a timer runs, so they can still fire,
    /// and ask the Background portal for permission and login autostart
    /// while alarms are enabled.
    fn update_keep_alive(&self) {
        let alarms = self.has_active_alarms();
        let keep = alarms || self.timers.borrow().iter().any(Timer::is_running);
        self.window.set_hide_on_close(keep);
        {
            let mut hold = self.hold.borrow_mut();
            if keep && hold.is_none() {
                *hold = Some(self.app.hold());
            } else if !keep {
                hold.take();
            }
        }
        let prev = self.autostart_requested.get();
        // Only withdraw autostart after having asked for it in this session.
        if prev != Some(alarms) && (alarms || prev.is_some()) {
            self.autostart_requested.set(Some(alarms));
            let note = self.alarm_note.downgrade();
            glib::spawn_future_local(async move {
                let r = gio::spawn_blocking(move || background::request(APP_ID, alarms)).await;
                let granted = match r {
                    Ok(Ok(g)) => g.background,
                    Ok(Err(e)) => {
                        eprintln!("helix-clocks: background portal: {e}");
                        false
                    }
                    Err(_) => false,
                };
                if let Some(note) = note.upgrade() {
                    note.set_visible(alarms && !granted);
                }
            });
        }
    }

    /// Looks up the current place (Location portal / GeoClue) once, if the
    /// `geolocation` setting allows it.
    fn locate(self: &Rc<Self>) {
        if !self.prefs.get("geolocation", true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let r = gio::spawn_blocking(|| location::locate(APP_ID, Duration::from_secs(30))).await;
            let Some(ui) = weak.upgrade() else { return };
            match r {
                Ok(Ok(fix)) => {
                    let here = ui.place_at((fix.latitude, fix.longitude));
                    ui.here.replace(Some(here));
                    ui.rebuild_world();
                }
                Ok(Err(e)) => eprintln!("helix-clocks: location unavailable: {e}"),
                Err(_) => {}
            }
        });
    }

    /// Names a position: the nearest GWeather city (within 100 km) with its
    /// zone, else the nearest `zone1970.tab` city's zone.
    fn place_at(&self, at: (f64, f64)) -> WorldClock {
        let tz_city = tz::nearest(&self.db, at);
        let city = self.gw.as_deref().and_then(|g| g.nearest_city(at, 100.0));
        let zone = city
            .and_then(|c| c.tz.as_deref())
            .and_then(tz::zone_by_id)
            .or(tz_city.map(|l| l.zone))
            .unwrap_or(self.local);
        let name = city
            .map(|c| c.name.clone())
            .unwrap_or_else(|| tz::city_from_zone(zone.name()));
        WorldClock {
            name,
            zone,
            coords: Some(at),
            country: city.and_then(|c| c.country.clone()),
            station: None,
        }
    }

    fn tick(self: &Rc<Self>) {
        let mono = self.clock.mono();
        if self.stopwatch.borrow().is_running() {
            self.update_stopwatch();
        }
        self.tick_timers(mono);
        let now = self.clock.now();
        if now.timestamp() != self.last_second.get() {
            self.last_second.set(now.timestamp());
            self.update_world(now);
            self.tick_alarms(now);
        }
    }

    // ---------------------------------------------------------------- World

    /// Rebuilds the list, keeping keyboard focus on the same row (or a
    /// neighbour after a removal) and the scroll position.
    fn rebuild_world(self: &Rc<Self>) {
        let list = self.world_list.clone();
        let fallback = self.add_button.clone().upcast::<gtk::Widget>();
        helix_app_common::focus::rebuild_keeping_focus(&list, Some(&fallback), || {
            self.rebuild_world_rows()
        });
    }

    fn rebuild_world_rows(self: &Rc<Self>) {
        while let Some(c) = self.world_list.first_child() {
            self.world_list.remove(&c);
        }
        let mut rows = Vec::new();
        let mut entries: Vec<(WorldClock, bool)> = Vec::new();
        if self.prefs.get("geolocation", true) {
            let located = self.here.borrow().clone();
            let here = self.db.iter().find(|l| l.zone == self.local);
            let mut wc = located
                .or_else(|| here.map(WorldClock::from_location))
                .unwrap_or(WorldClock {
                    name: tz::city_from_zone(self.local.name()),
                    zone: self.local,
                    coords: None,
                    country: None,
                    station: None,
                });
            wc.country = Some("Current Location".into());
            entries.push((wc, false));
        }
        entries.extend(self.world.borrow().iter().cloned().map(|c| (c, true)));
        for (i, (wc, removable)) in entries.into_iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&wc.name))
                .activatable(true)
                .build();
            helix_app_common::focus::set_key(
                &row,
                &format!("{}|{}|{removable}", wc.zone.name(), wc.name),
            );
            // Day or night is also in the row's description.
            let icon = gtk::Image::builder()
                .pixel_size(24)
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build();
            let time = label("", &["world-time", "clock-time"]);
            row.add_prefix(&icon);
            row.add_suffix(&time);
            if removable {
                let remove = gtk::Button::from_icon_name("user-trash-symbolic");
                remove.set_valign(gtk::Align::Center);
                remove.add_css_class("flat");
                remove.set_tooltip_text(Some("Remove"));
                helix_app_common::set_accessible_label(&remove, &format!("Remove {}", wc.name));
                helix_app_common::focus::set_part(&remove, "remove");
                let weak = Rc::downgrade(self);
                let idx = i - (self.prefs.get("geolocation", true) as usize);
                remove.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        if idx < ui.world.borrow().len() {
                            ui.world.borrow_mut().remove(idx);
                        }
                        ui.save_world();
                        ui.rebuild_world();
                    }
                });
                row.add_suffix(&remove);
            }
            let weak = Rc::downgrade(self);
            let clock = wc.clone();
            row.connect_activated(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.show_details(clock.clone());
                }
            });
            self.world_list.append(&row);
            rows.push(WorldRow {
                description: RefCell::default(),
                time,
                icon,
                row,
                zone: wc.zone,
                coords: wc.coords,
                country: wc.country,
            });
        }
        let empty = rows.is_empty();
        self.world_rows.replace(rows);
        self.world_stack
            .set_visible_child_name(if empty { "empty" } else { "list" });
        self.update_world(self.clock.now());
    }

    fn save_world(&self) {
        self.prefs.set_variant(
            "world-clocks",
            store::world_clocks_to_variant(&self.world.borrow(), self.gw.as_deref()),
        );
    }

    fn is_day(zone: Tz, coords: Option<(f64, f64)>, now: DateTime<Utc>) -> bool {
        let lt = now.with_timezone(&zone);
        match coords {
            Some((la, lo)) => sun::is_day(now, lt.date_naive(), la, lo),
            None => sun::is_day_by_hour(chrono::Timelike::hour(&lt)),
        }
    }

    fn update_world(&self, now: DateTime<Utc>) {
        let twelve = self.twelve.get();
        let local_off = tz::utc_offset(self.local, now);
        for r in self.world_rows.borrow().iter() {
            let lt = now.with_timezone(&r.zone);
            r.time.set_text(&tz::format_time(lt.time(), twelve));
            let day = Self::is_day(r.zone, r.coords, now);
            r.icon.set_icon_name(Some(if day {
                "weather-clear-symbolic"
            } else {
                "weather-clear-night-symbolic"
            }));
            r.icon
                .set_tooltip_text(Some(if day { "Day" } else { "Night" }));
            let rel = tz::relative_day_at(r.zone, self.local, now);
            let diff = tz::describe_offset(tz::utc_offset(r.zone, now) - local_off);
            let mut parts: Vec<String> = Vec::new();
            if let Some(c) = &r.country {
                parts.push(c.clone());
            }
            if rel != tz::RelativeDay::Today {
                parts.push(rel.label().into());
            }
            parts.push(diff);
            let subtitle = parts.join(" • ");
            r.row.set_subtitle(&glib::markup_escape_text(&subtitle));
            // The time, day or night and the subtitle, as text.
            let description = world_row_description(&r.time.text(), day, &subtitle);
            if r.description.replace(description.clone()) != description {
                r.row
                    .update_property(&[gtk::accessible::Property::Description(&description)]);
            }
        }
        if let Some(d) = self.details.borrow().as_ref() {
            self.update_details(d, now);
        }
    }

    fn show_details(self: &Rc<Self>, wc: WorldClock) {
        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page_box.set_margin_top(24);
        page_box.set_margin_bottom(24);
        page_box.set_margin_start(12);
        page_box.set_margin_end(12);
        let title = label(&wc.name, &["title-1"]);
        let time = label("", &["details-time", "clock-time"]);
        let date = label("", &["title-4", "dim-label"]);
        page_box.append(&title);
        if let Some(c) = &wc.country {
            page_box.append(&label(c, &["dim-label"]));
        }
        page_box.append(&time);
        page_box.append(&date);
        let group = adw::PreferencesGroup::new();
        group.set_margin_top(18);
        let mut rows = Vec::new();
        for (key, title) in [
            ("zone", "Time Zone"),
            ("offset", "UTC Offset"),
            ("diff", "Difference"),
            ("rise", "Sunrise"),
            ("set", "Sunset"),
            ("length", "Day Length"),
        ] {
            if wc.coords.is_none() && matches!(key, "rise" | "set" | "length") {
                continue;
            }
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle_selectable(true)
                .build();
            row.add_css_class("property");
            group.add(&row);
            rows.push((row, key));
        }
        page_box.append(&group);
        let page = adw::NavigationPage::new(&scrolled_clamped(&page_box), &wc.name);
        let d = Details {
            clock: wc,
            time,
            date,
            rows,
        };
        self.update_details(&d, self.clock.now());
        self.details.replace(Some(d));
        self.world_nav.push(&page);
        self.update_header();
    }

    fn update_details(&self, d: &Details, now: DateTime<Utc>) {
        let zone = d.clock.zone;
        let lt = now.with_timezone(&zone);
        let twelve = self.twelve.get();
        d.time.set_text(&tz::format_time_secs(lt.time(), twelve));
        d.date.set_text(&lt.format("%A, %-d %B %Y").to_string());
        let off = tz::utc_offset(zone, now);
        let sun = d
            .clock
            .coords
            .map(|(la, lo)| sun::sun_times(lt.date_naive(), la, lo));
        let fmt = |t: DateTime<Utc>| tz::format_time(t.with_timezone(&zone).time(), twelve);
        for (row, key) in &d.rows {
            let text = match *key {
                "zone" => format!("{} ({})", zone.name(), tz::abbreviation(zone, now)),
                "offset" => tz::format_utc_offset(off),
                "diff" => tz::describe_offset(off - tz::utc_offset(self.local, now)),
                "rise" | "set" | "length" => match sun {
                    Some(SunTimes::Normal { rise, set }) => match *key {
                        "rise" => fmt(rise),
                        "set" => fmt(set),
                        _ => {
                            let m = (set - rise).num_minutes();
                            format!("{} h {} min", m / 60, m % 60)
                        }
                    },
                    Some(SunTimes::PolarDay) => "Polar day".into(),
                    Some(SunTimes::PolarNight) => "Polar night".into(),
                    None => String::new(),
                },
                _ => String::new(),
            };
            row.set_subtitle(&glib::markup_escape_text(&text));
        }
    }

    fn show_add_location(self: &Rc<Self>) {
        let dialog = adw::Dialog::builder()
            .title("Add a New World Clock")
            .content_width(420)
            .content_height(520)
            .build();
        let header = adw::HeaderBar::new();
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search for a city or time zone")
            .hexpand(true)
            .build();
        let search_bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        search_bar.set_margin_start(12);
        search_bar.set_margin_end(12);
        search_bar.set_margin_bottom(6);
        search_bar.append(&search);
        let list = boxed_list();
        let results = gtk::Stack::new();
        let hint = adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title("Search for a City")
            .build();
        hint.add_css_class("compact");
        let none = adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title("No Results")
            .build();
        none.add_css_class("compact");
        results.add_named(&hint, Some("hint"));
        results.add_named(&none, Some("none"));
        results.add_named(&scrolled_clamped(&list), Some("list"));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&search_bar);
        toolbar.set_content(Some(&results));
        dialog.set_child(Some(&toolbar));

        let shown: Rc<RefCell<Vec<usize>>> = Rc::default();
        {
            let weak = Rc::downgrade(self);
            let list = list.clone();
            let shown = shown.clone();
            search.connect_search_changed(move |e| {
                let Some(ui) = weak.upgrade() else { return };
                while let Some(c) = list.first_child() {
                    list.remove(&c);
                }
                let text = e.text();
                let hits: Vec<usize> = tz::search(&ui.db, &text).into_iter().take(100).collect();
                results.set_visible_child_name(if text.trim().is_empty() {
                    "hint"
                } else if hits.is_empty() {
                    "none"
                } else {
                    "list"
                });
                let now = ui.clock.now();
                let local_off = tz::utc_offset(ui.local, now);
                for &i in &hits {
                    let l = &ui.db[i];
                    let added = ui
                        .world
                        .borrow()
                        .iter()
                        .any(|w| w.zone == l.zone && w.name == l.city);
                    let row = adw::ActionRow::builder()
                        .title(glib::markup_escape_text(&l.city))
                        .subtitle(glib::markup_escape_text(&format!(
                            "{} • {} • {}",
                            l.subtitle(),
                            l.zone_id(),
                            tz::describe_offset(tz::utc_offset(l.zone, now) - local_off)
                        )))
                        .activatable(!added)
                        .build();
                    row.add_suffix(&label(
                        &tz::format_time(now.with_timezone(&l.zone).time(), ui.twelve.get()),
                        &["clock-time"],
                    ));
                    if added {
                        let check = gtk::Image::builder()
                            .icon_name("object-select-symbolic")
                            .accessible_role(gtk::AccessibleRole::Presentation)
                            .build();
                        check.set_tooltip_text(Some("Already added"));
                        row.add_suffix(&check);
                        let subtitle = row.subtitle().unwrap_or_default();
                        row.update_property(&[gtk::accessible::Property::Description(&format!(
                            "{subtitle} • Already added"
                        ))]);
                    }
                    list.append(&row);
                }
                shown.replace(hits);
            });
        }
        {
            let weak = Rc::downgrade(self);
            let dialog = dialog.clone();
            list.connect_row_activated(move |_, row| {
                let Some(ui) = weak.upgrade() else { return };
                let Some(&i) = shown.borrow().get(row.index() as usize) else {
                    return;
                };
                let wc = WorldClock::from_location(&ui.db[i]);
                if !ui.world.borrow().contains(&wc) {
                    ui.world.borrow_mut().push(wc);
                    ui.save_world();
                    ui.rebuild_world();
                }
                dialog.close();
            });
        }
        {
            let list = list.clone();
            search.connect_activate(move |_| {
                if let Some(row) = list.row_at_index(0) {
                    row.emit_activate();
                }
            });
        }
        dialog.present(Some(&self.window));
        search.grab_focus();
    }

    // --------------------------------------------------------------- Alarms

    fn save_alarms(&self) {
        let alarms: Vec<Alarm> = self
            .alarms
            .borrow()
            .iter()
            .map(|e| e.alarm.clone())
            .collect();
        self.prefs
            .set_variant("alarms", store::alarms_to_variant(&alarms));
        self.update_keep_alive();
    }

    /// Shows the alarms. While the same alarms are listed in the same
    /// order (toggling, ringing, snoozing, stopping, editing a name) the
    /// rows are updated in place, so focus stays where it is, also on a
    /// dialog's return. Otherwise the list is rebuilt, keeping keyboard
    /// focus on the same row (or a neighbour after a removal) and the
    /// scroll position.
    fn rebuild_alarms(self: &Rc<Self>) {
        let same = {
            let alarms = self.alarms.borrow();
            let rows = self.alarm_rows.borrow();
            rows.len() == alarms.len()
                && rows
                    .iter()
                    .zip(alarms.iter())
                    .all(|((id, _), e)| *id == e.alarm.id)
        };
        if same && !self.alarm_rows.borrow().is_empty() {
            let twelve = self.twelve.get();
            let alarms = self.alarms.borrow();
            for ((_, row), e) in self.alarm_rows.borrow().iter().zip(alarms.iter()) {
                self.fill_alarm_row(row, e, twelve);
            }
            return;
        }
        let list = self.alarm_list.clone();
        let fallback = self.add_button.clone().upcast::<gtk::Widget>();
        helix_app_common::focus::rebuild_keeping_focus(&list, Some(&fallback), || {
            self.rebuild_alarms_rows()
        });
    }

    /// Shows `e` in `row`: time, name, repeat summary and state, switch.
    fn fill_alarm_row(&self, row: &AlarmRow, e: &AlarmEntry, twelve: bool) {
        let a = &e.alarm;
        let t = NaiveTime::from_hms_opt(a.hour, a.minute, 0).unwrap_or_default();
        row.time.set_label(&tz::format_time(t, twelve));
        let mut parts = vec![a.name.clone()];
        let summary = a.days.summary();
        if !summary.is_empty() {
            parts.push(summary);
        }
        match e.state {
            RingState::Snoozed { until } => parts.push(format!(
                "Snoozed until {}",
                tz::format_time(until.with_timezone(&self.local).time(), twelve)
            )),
            RingState::Ringing { .. } => parts.push("Ringing".into()),
            RingState::Idle => {}
        }
        row.sub.set_label(&parts.join(" • "));
        // The switch's handler ignores a state the alarm already has.
        if row.switch.is_active() != a.active {
            row.switch.set_active(a.active);
        }
        helix_app_common::set_accessible_label(&row.switch, &format!("Enable alarm {}", a.name));
        if a.active {
            row.row.remove_css_class("alarm-inactive");
        } else {
            row.row.add_css_class("alarm-inactive");
        }
    }

    /// After `dialog` closes, focuses the alarm row `key` (it may have
    /// been rebuilt while the dialog was open), or the Add button.
    fn focus_alarm_after(self: &Rc<Self>, dialog: &impl IsA<adw::Dialog>, key: Option<String>) {
        let weak = Rc::downgrade(self);
        dialog.as_ref().connect_closed(move |_| {
            let weak = weak.clone();
            let key = key.clone();
            glib::idle_add_local_once(move || {
                let Some(ui) = weak.upgrade() else { return };
                let done = key
                    .as_deref()
                    .is_some_and(|k| helix_app_common::focus::focus_key(&ui.alarm_list, k));
                if !done {
                    ui.add_button.grab_focus();
                }
            });
        });
    }

    fn rebuild_alarms_rows(self: &Rc<Self>) {
        while let Some(c) = self.alarm_list.first_child() {
            self.alarm_list.remove(&c);
        }
        self.alarm_rows.borrow_mut().clear();
        let twelve = self.twelve.get();
        let entries = self.alarms.borrow().clone();
        self.alarm_stack
            .set_visible_child_name(if entries.is_empty() { "empty" } else { "list" });
        for e in entries {
            let a = &e.alarm;
            let row = gtk::ListBoxRow::new();
            row.set_activatable(true);
            helix_app_common::focus::set_key(&row, &a.id);
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            hb.set_margin_top(12);
            hb.set_margin_bottom(12);
            hb.set_margin_start(12);
            hb.set_margin_end(12);
            let vb = gtk::Box::new(gtk::Orientation::Vertical, 2);
            vb.set_hexpand(true);
            let time = label("", &["alarm-time", "clock-time"]);
            time.set_xalign(0.0);
            vb.append(&time);
            let sub = label("", &["dim-label"]);
            sub.set_xalign(0.0);
            sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
            vb.append(&sub);
            hb.append(&vb);
            let switch = gtk::Switch::builder()
                .active(a.active)
                .valign(gtk::Align::Center)
                .build();
            helix_app_common::focus::set_part(&switch, "switch");
            hb.append(&switch);
            row.set_child(Some(&hb));
            let widgets = AlarmRow {
                row: row.clone(),
                time,
                sub,
                switch: switch.clone(),
            };
            self.fill_alarm_row(&widgets, &e, twelve);
            self.alarm_rows.borrow_mut().push((a.id.clone(), widgets));
            let id = a.id.clone();
            let weak = Rc::downgrade(self);
            switch.connect_active_notify(move |s| {
                let Some(ui) = weak.upgrade() else { return };
                let now = ui.clock.now();
                let active = s.is_active();
                let changed = {
                    let mut alarms = ui.alarms.borrow_mut();
                    match alarms.iter_mut().find(|e| e.alarm.id == id) {
                        Some(e) if e.alarm.active != active => {
                            e.set_active(active, now, &ui.local);
                            true
                        }
                        _ => false,
                    }
                };
                if changed {
                    ui.dismiss_ringing(&id);
                    ui.save_alarms();
                    let weak = Rc::downgrade(&ui);
                    glib::idle_add_local_once(move || {
                        if let Some(ui) = weak.upgrade() {
                            ui.rebuild_alarms();
                        }
                    });
                }
            });
            let weak = Rc::downgrade(self);
            let id = a.id.clone();
            row.connect_activate(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                // The row may have been updated in place since it was made.
                let alarm = ui
                    .alarms
                    .borrow()
                    .iter()
                    .find(|e| e.alarm.id == id)
                    .map(|e| e.alarm.clone());
                if let Some(alarm) = alarm {
                    ui.show_alarm_dialog(Some(alarm));
                }
            });
            self.alarm_list.append(&row);
        }
    }

    fn show_alarm_dialog(self: &Rc<Self>, existing: Option<Alarm>) {
        let editing = existing.is_some();
        let a = existing.unwrap_or_else(|| {
            let salt = self.id_salt.get() + 1;
            self.id_salt.set(salt);
            Alarm {
                id: alarm::new_id(self.clock.now(), salt),
                ..Alarm::default()
            }
        });
        let twelve = self.twelve.get();
        let dialog = adw::Dialog::builder()
            .title(if editing { "Edit Alarm" } else { "New Alarm" })
            .content_width(420)
            .build();
        let header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .show_start_title_buttons(false)
            .build();
        let cancel = gtk::Button::with_mnemonic("_Cancel");
        let save = gtk::Button::with_mnemonic(if editing { "_Done" } else { "_Add" });
        save.add_css_class("suggested-action");
        header.pack_start(&cancel);
        header.pack_end(&save);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(12);
        content.set_margin_bottom(24);
        content.set_margin_start(12);
        content.set_margin_end(12);

        // Time
        let time_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        time_box.set_halign(gtk::Align::Center);
        let (hmin, hmax, hval) = if twelve {
            let h12 = match a.hour % 12 {
                0 => 12,
                h => h,
            };
            (1.0, 12.0, h12 as f64)
        } else {
            (0.0, 23.0, a.hour as f64)
        };
        let hour = time_spin(hmin, hmax, hval);
        helix_app_common::set_accessible_label(&hour, "Hour");
        let minute = time_spin(0.0, 59.0, a.minute as f64);
        helix_app_common::set_accessible_label(&minute, "Minute");
        time_box.append(&hour);
        time_box.append(&label(":", &["alarm-time"]));
        time_box.append(&minute);
        let ampm = gtk::DropDown::from_strings(&["AM", "PM"]);
        helix_app_common::set_accessible_label(&ampm, "AM or PM");
        ampm.set_valign(gtk::Align::Center);
        ampm.set_selected(if a.hour >= 12 { 1 } else { 0 });
        ampm.set_visible(twelve);
        time_box.append(&ampm);
        content.append(&time_box);

        // Days
        let days_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        days_box.set_halign(gtk::Align::Center);
        let day_buttons: Vec<(chrono::Weekday, gtk::ToggleButton)> = Weekdays::ALL
            .iter()
            .map(|d| {
                let b = gtk::ToggleButton::with_label(alarm::short_name(*d));
                b.add_css_class("day-toggle");
                b.add_css_class("circular");
                b.set_active(a.days.contains(*d));
                days_box.append(&b);
                (*d, b)
            })
            .collect();
        let repeat_label = label("Repeat", &["heading"]);
        repeat_label.set_xalign(0.0);
        content.append(&repeat_label);
        content.append(&days_box);

        let group = adw::PreferencesGroup::new();
        let name = adw::EntryRow::builder().title("Name").text(&a.name).build();
        group.add(&name);
        let labels: Vec<String> = alarm::DURATION_CHOICES
            .iter()
            .map(|m| format!("{m} minute{}", if *m == 1 { "" } else { "s" }))
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let pick = |v: u32| {
            alarm::DURATION_CHOICES
                .iter()
                .position(|m| *m == v)
                .unwrap_or(3) as u32
        };
        let ring = adw::ComboRow::builder()
            .title("Ring Duration")
            .model(&gtk::StringList::new(&label_refs))
            .selected(pick(a.ring_minutes))
            .build();
        let snooze = adw::ComboRow::builder()
            .title("Snooze Duration")
            .model(&gtk::StringList::new(&label_refs))
            .selected(pick(a.snooze_minutes))
            .build();
        group.add(&ring);
        group.add(&snooze);
        content.append(&group);

        if editing {
            let delete = gtk::Button::with_mnemonic("_Delete Alarm");
            delete.add_css_class("destructive-action");
            delete.add_css_class("pill");
            delete.set_halign(gtk::Align::Center);
            let weak = Rc::downgrade(self);
            let id = a.id.clone();
            let d = dialog.clone();
            delete.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    let old: Vec<String> = ui
                        .alarms
                        .borrow()
                        .iter()
                        .map(|e| e.alarm.id.clone())
                        .collect();
                    ui.dismiss_ringing(&id);
                    ui.alarms.borrow_mut().retain(|e| e.alarm.id != id);
                    let new: Vec<String> = ui
                        .alarms
                        .borrow()
                        .iter()
                        .map(|e| e.alarm.id.clone())
                        .collect();
                    ui.save_alarms();
                    ui.rebuild_alarms();
                    // Back in the list: the alarm that took its place.
                    let next = helix_app_common::focus::pick_focus(&old, &id, &new)
                        .map(|i| new[i].clone());
                    ui.focus_alarm_after(&d, next);
                }
                d.close();
            });
            content.append(&delete);
        }

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        dialog.set_child(Some(&toolbar));
        dialog.set_default_widget(Some(&save));

        {
            let d = dialog.clone();
            cancel.connect_clicked(move |_| {
                d.close();
            });
        }
        let weak = Rc::downgrade(self);
        let d = dialog.clone();
        save.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            let mut h = hour.value() as u32;
            if twelve {
                h %= 12;
                if ampm.selected() == 1 {
                    h += 12;
                }
            }
            let mut days = Weekdays::default();
            for (wd, b) in &day_buttons {
                days.set(*wd, b.is_active());
            }
            let text = name.text().trim().to_string();
            let edited = Alarm {
                id: a.id.clone(),
                name: if text.is_empty() {
                    "Alarm".into()
                } else {
                    text
                },
                active: true,
                hour: h,
                minute: minute.value() as u32,
                days,
                ring_minutes: alarm::DURATION_CHOICES
                    [ring.selected() as usize % alarm::DURATION_CHOICES.len()],
                snooze_minutes: alarm::DURATION_CHOICES
                    [snooze.selected() as usize % alarm::DURATION_CHOICES.len()],
            };
            ui.dismiss_ringing(&edited.id);
            let edited_id = edited.id.clone();
            let entry = AlarmEntry::new(edited, ui.clock.now(), &ui.local);
            {
                let mut alarms = ui.alarms.borrow_mut();
                alarms.retain(|e| e.alarm.id != entry.alarm.id);
                alarms.push(entry);
                alarms.sort_by_key(|e| (e.alarm.hour, e.alarm.minute));
            }
            ui.save_alarms();
            ui.rebuild_alarms();
            // A new time can reorder (rebuild) the list: return to the
            // alarm's row. A new alarm leaves focus on the Add button.
            if editing {
                ui.focus_alarm_after(&d, Some(edited_id));
            }
            d.close();
        });
        dialog.present(Some(&self.window));
    }

    fn tick_alarms(self: &Rc<Self>, now: DateTime<Utc>) {
        let mut events = Vec::new();
        let mut any_ringing = false;
        for e in self.alarms.borrow_mut().iter_mut() {
            if let Some(ev) = e.tick(now, &self.local) {
                events.push((e.alarm.clone(), ev));
            }
            any_ringing |= e.is_ringing();
        }
        {
            let mut player = self.player.borrow_mut();
            if any_ringing && player.failed() && now.timestamp() % 2 == 0 {
                beep();
            } else if !any_ringing && player.playing() == Some(Sound::Alarm) {
                player.stop();
            }
        }
        if events.is_empty() {
            return;
        }
        for (a, ev) in events {
            match ev {
                Event::Ring => self.show_ringing(&a),
                Event::Timeout => self.dismiss_ringing(&a.id),
                Event::Missed => {
                    let n = gio::Notification::new("Missed Alarm");
                    let t = NaiveTime::from_hms_opt(a.hour, a.minute, 0).unwrap_or_default();
                    n.set_body(Some(&format!(
                        "{} ({})",
                        a.name,
                        tz::format_time(t, self.twelve.get())
                    )));
                    self.app
                        .send_notification(Some(&format!("alarm-missed-{}", a.id)), &n);
                }
            }
        }
        self.save_alarms();
        self.rebuild_alarms();
    }

    fn show_ringing(self: &Rc<Self>, a: &Alarm) {
        let t = tz::format_time(
            NaiveTime::from_hms_opt(a.hour, a.minute, 0).unwrap_or_default(),
            self.twelve.get(),
        );
        let n = gio::Notification::new(&a.name);
        n.set_body(Some(&t));
        n.set_priority(gio::NotificationPriority::Urgent);
        n.set_icon(&gio::ThemedIcon::new("alarm-symbolic"));
        n.add_button_with_target_value("Snooze", "app.alarm-snooze", Some(&a.id.to_variant()));
        n.add_button_with_target_value("Stop", "app.alarm-stop", Some(&a.id.to_variant()));
        n.set_default_action("app.activate");
        self.app
            .send_notification(Some(&format!("alarm-{}", a.id)), &n);
        if self.player.borrow().playing() != Some(Sound::Alarm)
            && !self.player.borrow_mut().play(Sound::Alarm)
        {
            beep();
        }

        if self.ringing.borrow().contains_key(&a.id) {
            return;
        }
        self.window.present();
        let dialog = adw::AlertDialog::new(Some(&a.name), Some(&t));
        dialog.add_responses(&[("snooze", "_Snooze"), ("stop", "_Stop")]);
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("stop"));
        dialog.set_close_response("stop");
        let weak = Rc::downgrade(self);
        let id = a.id.clone();
        dialog.connect_response(None, move |_, resp| {
            if let Some(ui) = weak.upgrade() {
                if ui.ringing.borrow().contains_key(&id) {
                    ui.respond_alarm(&id, resp == "snooze");
                }
            }
        });
        self.ringing
            .borrow_mut()
            .insert(a.id.clone(), dialog.clone());
        dialog.present(Some(&self.window));
    }

    /// Closes the ringing dialog and notification for `id`, if any.
    fn dismiss_ringing(&self, id: &str) {
        let dialog = self.ringing.borrow_mut().remove(id);
        if let Some(d) = dialog {
            d.force_close();
        }
        self.app.withdraw_notification(&format!("alarm-{id}"));
        if self.ringing.borrow().is_empty() && self.player.borrow().playing() == Some(Sound::Alarm)
        {
            self.player.borrow_mut().stop();
        }
    }

    fn respond_alarm(self: &Rc<Self>, id: &str, snooze: bool) {
        self.dismiss_ringing(id);
        let now = self.clock.now();
        {
            let mut alarms = self.alarms.borrow_mut();
            let Some(e) = alarms.iter_mut().find(|e| e.alarm.id == id) else {
                return;
            };
            if !matches!(
                e.state,
                RingState::Ringing { .. } | RingState::Snoozed { .. }
            ) {
                return;
            }
            if snooze {
                e.snooze(now);
            } else {
                e.stop(now, &self.local);
            }
        }
        self.save_alarms();
        self.rebuild_alarms();
    }

    // ------------------------------------------------------------ Stopwatch

    fn stopwatch_primary(&self) {
        self.stopwatch.borrow_mut().toggle(self.clock.mono());
        self.update_stopwatch();
    }

    fn stopwatch_secondary(&self) {
        let mono = self.clock.mono();
        let running = self.stopwatch.borrow().is_running();
        if running {
            let lap = self.stopwatch.borrow_mut().lap(mono);
            if let Some(lap) = lap {
                self.sw_laps.prepend(&lap_row(&lap));
            }
        } else {
            self.stopwatch.borrow_mut().reset();
            while let Some(c) = self.sw_laps.first_child() {
                self.sw_laps.remove(&c);
            }
        }
        self.update_stopwatch();
    }

    fn update_stopwatch(&self) {
        let sw = self.stopwatch.borrow();
        let mono = self.clock.mono();
        self.sw_time
            .set_text(&stopwatch::format_elapsed(sw.elapsed(mono), false));
        let laps = sw.laps().len();
        self.sw_split.set_visible(laps > 0);
        if laps > 0 {
            self.sw_split.set_text(&format!(
                "Lap {}  {}",
                laps + 1,
                stopwatch::format_elapsed(sw.current_split(mono), false)
            ));
        }
        let (primary, secondary, sec_sensitive) = match sw.state {
            stopwatch::State::Reset => ("Start", "Lap", false),
            stopwatch::State::Running { .. } => ("Pause", "Lap", true),
            stopwatch::State::Paused { .. } => ("Resume", "Reset", true),
        };
        if self.sw_primary.label().as_deref() != Some(primary) {
            self.sw_primary.set_label(primary);
            if sw.is_running() {
                self.sw_primary.remove_css_class("suggested-action");
            } else {
                self.sw_primary.add_css_class("suggested-action");
            }
        }
        self.sw_secondary.set_label(secondary);
        self.sw_secondary.set_sensitive(sec_sensitive);
        self.sw_laps_box
            .set_visible(self.sw_laps.first_child().is_some());
    }

    // ---------------------------------------------------------------- Timer

    fn build_timer_pages(self: &Rc<Self>) {
        let setup = gtk::Box::new(gtk::Orientation::Vertical, 18);
        setup.set_margin_top(24);
        setup.set_margin_bottom(24);
        setup.set_margin_start(12);
        setup.set_margin_end(12);
        let presets = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(4)
            .min_children_per_line(2)
            .homogeneous(true)
            .column_spacing(12)
            .row_spacing(12)
            .build();
        for secs in timer::PRESETS {
            let b = gtk::Button::with_label(&timer::describe_duration(secs));
            b.add_css_class("pill");
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.add_timer(Timer::new("", Duration::from_secs(secs)));
                }
            });
            presets.insert(&b, -1);
        }
        setup.append(&label("Presets", &["heading"]));
        setup.append(&presets);

        setup.append(&label("Custom", &["heading"]));
        let spins = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        spins.set_halign(gtk::Align::Center);
        let h = time_spin(0.0, 99.0, 0.0);
        let m = time_spin(0.0, 59.0, 5.0);
        let s = time_spin(0.0, 59.0, 0.0);
        for (i, (spin, name)) in [(&h, "Hours"), (&m, "Minutes"), (&s, "Seconds")]
            .into_iter()
            .enumerate()
        {
            helix_app_common::set_accessible_label(spin, name);
            if i > 0 {
                spins.append(&label(":", &["timer-time"]));
            }
            spins.append(spin);
        }
        setup.append(&spins);
        let name_group = adw::PreferencesGroup::new();
        let name = adw::EntryRow::builder().title("Name (optional)").build();
        name_group.add(&name);
        setup.append(&name_group);
        let start = gtk::Button::with_mnemonic("_Start");
        start.add_css_class("pill");
        start.add_css_class("suggested-action");
        start.set_halign(gtk::Align::Center);
        let weak = Rc::downgrade(self);
        let name2 = name.clone();
        start.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            let secs = h.value() as u64 * 3600 + m.value() as u64 * 60 + s.value() as u64;
            if secs > 0 {
                ui.add_timer(Timer::new(name2.text().trim(), Duration::from_secs(secs)));
                name2.set_text("");
            }
        });
        setup.append(&start);
        setup.append(&self.timer_cancel);
        self.timer_stack
            .add_named(&scrolled_clamped(&setup), Some("setup"));
        self.timer_stack
            .add_named(&scrolled_clamped(&self.timer_list), Some("list"));
    }

    fn add_timer(self: &Rc<Self>, mut t: Timer) {
        t.start(self.clock.mono());
        self.timers.borrow_mut().push(t);
        self.save_timers();
        self.rebuild_timers();
    }

    fn save_timers(&self) {
        self.prefs
            .set_variant("timers", store::timers_to_variant(&self.timers.borrow()));
        self.update_keep_alive();
    }

    /// Rebuilds the list, keeping keyboard focus on the same row (or a
    /// neighbour after a removal) and the scroll position.
    fn rebuild_timers(self: &Rc<Self>) {
        let list = self.timer_list.clone();
        let fallback = self.add_button.clone().upcast::<gtk::Widget>();
        helix_app_common::focus::rebuild_keeping_focus(&list, Some(&fallback), || {
            self.rebuild_timers_rows()
        });
    }

    fn rebuild_timers_rows(self: &Rc<Self>) {
        while let Some(c) = self.timer_list.first_child() {
            self.timer_list.remove(&c);
        }
        let mut rows = Vec::new();
        let count = self.timers.borrow().len();
        let labels = {
            let timers = self.timers.borrow();
            let named: Vec<(&str, u64)> = timers
                .iter()
                .map(|t| (t.name.as_str(), t.duration.as_secs()))
                .collect();
            timer::timer_labels(&named)
        };
        for (i, timer_label) in labels.into_iter().enumerate() {
            let name = self.timers.borrow()[i].name.clone();
            let row = gtk::ListBoxRow::new();
            row.set_activatable(false);
            // Timers have no id; after a deletion the same index is the
            // next timer, which is where focus should go.
            helix_app_common::focus::set_key(&row, &i.to_string());
            let vb = gtk::Box::new(gtk::Orientation::Vertical, 6);
            vb.set_margin_top(12);
            vb.set_margin_bottom(12);
            vb.set_margin_start(12);
            vb.set_margin_end(12);
            if !name.is_empty() {
                let l = label(&name, &["heading"]);
                l.set_xalign(0.0);
                vb.append(&l);
            }
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let time = label("", &["timer-time", "clock-time"]);
            time.set_hexpand(true);
            time.set_xalign(0.0);
            hb.append(&time);
            let start = gtk::Button::new();
            start.add_css_class("circular");
            start.set_valign(gtk::Align::Center);
            let reset = gtk::Button::from_icon_name("view-refresh-symbolic");
            reset.set_tooltip_text(Some("Reset"));
            helix_app_common::set_accessible_label(&reset, &format!("Reset {timer_label}"));
            reset.add_css_class("circular");
            reset.set_valign(gtk::Align::Center);
            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.set_tooltip_text(Some("Delete"));
            helix_app_common::set_accessible_label(&delete, &format!("Delete {timer_label}"));
            delete.add_css_class("circular");
            delete.add_css_class("flat");
            delete.set_valign(gtk::Align::Center);
            helix_app_common::focus::set_part(&start, "start");
            helix_app_common::focus::set_part(&reset, "reset");
            helix_app_common::focus::set_part(&delete, "delete");
            hb.append(&start);
            hb.append(&reset);
            hb.append(&delete);
            vb.append(&hb);
            let progress = gtk::ProgressBar::new();
            helix_app_common::set_accessible_label(&progress, &timer_label);
            vb.append(&progress);
            row.set_child(Some(&vb));
            row.update_property(&[gtk::accessible::Property::Label(&timer_label)]);
            self.timer_list.append(&row);

            let weak = Rc::downgrade(self);
            start.connect_clicked(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                let mono = ui.clock.mono();
                if let Some(t) = ui.timers.borrow_mut().get_mut(i) {
                    if t.is_running() {
                        t.pause(mono)
                    } else {
                        t.start(mono)
                    }
                }
                ui.update_keep_alive();
                ui.update_timers(mono);
            });
            let weak = Rc::downgrade(self);
            reset.connect_clicked(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                if let Some(t) = ui.timers.borrow_mut().get_mut(i) {
                    t.reset();
                }
                ui.update_keep_alive();
                ui.update_timers(ui.clock.mono());
            });
            let weak = Rc::downgrade(self);
            delete.connect_clicked(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                if i < ui.timers.borrow().len() {
                    ui.timers.borrow_mut().remove(i);
                }
                ui.save_timers();
                let weak = Rc::downgrade(&ui);
                glib::idle_add_local_once(move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.rebuild_timers();
                    }
                });
            });
            rows.push(TimerRow {
                label: timer_label,
                time,
                progress,
                start,
                reset,
            });
        }
        self.timer_rows.replace(rows);
        self.timer_cancel.set_visible(count > 0);
        self.timer_stack
            .set_visible_child_name(if count == 0 { "setup" } else { "list" });
        self.update_timers(self.clock.mono());
    }

    fn update_timers(&self, mono: Duration) {
        let timers = self.timers.borrow();
        for (t, r) in timers.iter().zip(self.timer_rows.borrow().iter()) {
            r.time.set_text(&timer::format_countdown(t.remaining(mono)));
            r.progress.set_fraction(t.progress(mono));
            r.progress.set_visible(t.state != timer::State::Stopped);
            let (icon, tip) = match t.state {
                timer::State::Running { .. } => ("media-playback-pause-symbolic", "Pause"),
                timer::State::Paused { .. } => ("media-playback-start-symbolic", "Resume"),
                timer::State::Stopped => ("media-playback-start-symbolic", "Start"),
            };
            r.start.set_icon_name(icon);
            r.start.set_tooltip_text(Some(tip));
            helix_app_common::set_accessible_label(&r.start, &format!("{tip} {}", r.label));
            if t.is_running() {
                r.start.remove_css_class("suggested-action");
            } else {
                r.start.add_css_class("suggested-action");
            }
            r.reset.set_sensitive(t.state != timer::State::Stopped);
        }
    }

    fn tick_timers(&self, mono: Duration) {
        let mut finished = Vec::new();
        let any_active = {
            let mut timers = self.timers.borrow_mut();
            for t in timers.iter_mut() {
                if t.tick(mono) {
                    finished.push((t.name.clone(), t.duration));
                }
            }
            timers.iter().any(|t| t.is_running())
        };
        if any_active || !finished.is_empty() {
            self.update_timers(mono);
        }
        for (name, dur) in finished {
            let n = gio::Notification::new("Time is up!");
            let body = if name.is_empty() {
                format!("{} timer", timer::describe_duration(dur.as_secs()))
            } else {
                name
            };
            helix_app_common::announce(
                &self.window,
                &format!("Time is up: {body}"),
                helix_app_common::Urgency::Assertive,
            );
            n.set_body(Some(&body));
            n.set_priority(gio::NotificationPriority::High);
            n.set_icon(&gio::ThemedIcon::new("alarm-symbolic"));
            n.set_default_action("app.activate");
            self.app.send_notification(Some("timer"), &n);
            let alarm_ringing = self.player.borrow().playing() == Some(Sound::Alarm);
            if !alarm_ringing && !self.player.borrow_mut().play(Sound::Timer) {
                beep();
                for delay in [400, 800] {
                    glib::timeout_add_local_once(Duration::from_millis(delay), beep);
                }
            }
            self.update_keep_alive();
        }
    }
}

fn with_ui<W>(weak: &Weak<Ui>, f: impl Fn(&Rc<Ui>) + 'static) -> impl Fn(&W) + 'static {
    let weak = weak.clone();
    move |_| {
        if let Some(ui) = weak.upgrade() {
            f(&ui)
        }
    }
}

fn boxed_list() -> gtk::ListBox {
    let l = gtk::ListBox::new();
    l.set_selection_mode(gtk::SelectionMode::None);
    l.add_css_class("boxed-list");
    l.set_valign(gtk::Align::Start);
    l
}

fn scrolled_clamped(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder().maximum_size(600).child(child).build();
    clamp.set_margin_top(12);
    clamp.set_margin_bottom(12);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build()
}

fn time_spin(min: f64, max: f64, value: f64) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, 1.0);
    s.set_value(value);
    s.set_wrap(true);
    s.set_numeric(true);
    s.set_orientation(gtk::Orientation::Vertical);
    s.add_css_class("alarm-time");
    s.add_css_class("clock-time");
    s.connect_output(|s| {
        s.set_text(&format!("{:02}", s.value() as i64));
        glib::Propagation::Stop
    });
    s
}

fn lap_row(lap: &stopwatch::Lap) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(format!("Lap {}", lap.number))
        .build();
    if let Some(d) = lap.delta_ms {
        row.set_subtitle(&stopwatch::format_delta(d));
    }
    let split = label(&stopwatch::format_elapsed(lap.split, true), &["clock-time"]);
    let total = label(
        &stopwatch::format_elapsed(lap.total, true),
        &["clock-time", "dim-label"],
    );
    split.set_width_chars(10);
    total.set_width_chars(10);
    row.add_suffix(&split);
    row.add_suffix(&total);
    row
}

const SHORTCUTS_UI: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <object class="GtkShortcutsWindow" id="help_overlay">
    <property name="modal">True</property>
    <child>
      <object class="GtkShortcutsSection">
        <property name="section-name">shortcuts</property>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">General</property>
            <child><object class="GtkShortcutsShortcut"><property name="title">World</property><property name="accelerator">&lt;Alt&gt;1</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Alarms</property><property name="accelerator">&lt;Alt&gt;2</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Stopwatch</property><property name="accelerator">&lt;Alt&gt;3</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Timer</property><property name="accelerator">&lt;Alt&gt;4</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">New location, alarm or timer</property><property name="accelerator">&lt;Control&gt;n</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Keyboard shortcuts</property><property name="accelerator">&lt;Control&gt;question</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Close window</property><property name="accelerator">&lt;Control&gt;w</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Quit</property><property name="accelerator">&lt;Control&gt;q</property></object></child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">Stopwatch</property>
            <child><object class="GtkShortcutsShortcut"><property name="title">Start / pause</property><property name="accelerator">space</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Lap</property><property name="accelerator">l</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Reset</property><property name="accelerator">Delete</property></object></child>
          </object>
        </child>
      </object>
    </child>
  </object>
</interface>
"#;
#[cfg(test)]
mod tests {
    #[test]
    fn world_rows_say_day_or_night() {
        assert_eq!(
            super::world_row_description("10:30 PM", false, "Japan • +8 hours"),
            "10:30 PM, Night, Japan • +8 hours"
        );
        assert_eq!(
            super::world_row_description("09:00", true, ""),
            "09:00, Day"
        );
    }
}
