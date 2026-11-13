//! GTK user interface.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use crate::api::{self, Forecast};
use crate::autoloc;
use crate::gwdb;
use crate::import;
use crate::graph::{self, Rgba};
use crate::store::{self, CachedForecast, Locations, Place};
use crate::timefmt::{self, DateTime};
use crate::units::{self, Pressure, Speed, Temperature};
use crate::wmo;

pub const APP_ID: &str = "org.helix.Weather";
const SCHEMA: &str = "org.gnome.Weather";
const GWEATHER_SCHEMA: &str = "org.gnome.GWeather4";
const APP_DIR: &str = "helix-weather";
const HOURS: usize = 48;
const COL_W: i32 = 64;
const GRAPH_H: i32 = 110;
const AUTO_REFRESH_SECS: i64 = 30 * 60;

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The viewer's (not the location's) current UTC offset in seconds.
fn viewer_offset() -> i64 {
    glib::DateTime::now_local().map(|d| d.utc_offset().as_seconds()).unwrap_or(0)
}

fn locations_path() -> PathBuf {
    helix_app_common::config_dir(APP_DIR).join("locations.json")
}

fn cache_dir() -> PathBuf {
    let dir = glib::user_cache_dir().join(APP_DIR);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

struct Units {
    temperature: Temperature,
    speed: Speed,
    pressure: Pressure,
}

impl Units {
    fn load(prefs: &Prefs) -> Units {
        let imperial = units::locale_is_imperial(&units::measurement_locale());
        Units {
            temperature: Temperature::from_nick(&prefs.get_enum_nick("temperature-unit", "default"), imperial),
            speed: Speed::from_nick(&prefs.get_enum_nick("speed-unit", "default"), imperial),
            pressure: Pressure::from_nick(&prefs.get_enum_nick("pressure-unit", "default"), imperial),
        }
    }
}

struct Inner {
    window: adw::ApplicationWindow,
    prefs: Prefs,
    gweather: Prefs,
    use_12h: bool,
    units: RefCell<Units>,
    locations: RefCell<Locations>,
    forecast: RefCell<Option<(Forecast, i64)>>,
    offline: Cell<bool>,
    fetching: Cell<bool>,
    generation: Cell<u64>,
    search_generation: Cell<u64>,
    search_results: RefCell<Vec<Place>>,
    locating: Cell<bool>,

    title: adw::WindowTitle,
    auto_row: adw::SwitchRow,
    places_button: gtk::MenuButton,
    search_entry: gtk::SearchEntry,
    results_list: gtk::ListBox,
    results_scroller: gtk::ScrolledWindow,
    search_status: gtk::Label,
    saved_list: gtk::ListBox,
    saved_header: gtk::Label,
    refresh_button: gtk::Button,
    header_spinner: gtk::Spinner,
    banner: adw::Banner,
    stack: gtk::Stack,
    error_page: adw::StatusPage,
    content: gtk::Box,
    updated_label: gtk::Label,
    unit_action: gio::SimpleAction,
}

#[derive(Clone)]
struct Ui(Rc<Inner>);

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(
        &app,
        "Weather",
        "Show weather conditions and forecast\n\nWeather data by Open-Meteo.com (CC BY 4.0, non-commercial use). \
         Place names may come from © OpenStreetMap contributors (Nominatim).",
    );
    app.connect_startup(|_| load_css());
    app.connect_activate(|app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        let ui = build(app);
        // Signal handlers only hold weak references; keep the state alive
        // for as long as the window exists.
        let holder = RefCell::new(Some(ui.clone()));
        ui.0.window.connect_destroy(move |_| {
            holder.borrow_mut().take();
        });
        ui.0.window.present();
        ui.select_current();
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".weather-temp { font-size: 48pt; font-weight: 300; }
         .weather-card { padding: 12px; }
         .weather-card-value { font-size: 14pt; font-weight: bold; }
         .weather-day { padding: 8px 12px; }
         .weather-hour { font-size: 10pt; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn build(app: &adw::Application) -> Ui {
    let prefs = Prefs::new(SCHEMA);
    let gweather = Prefs::new(GWEATHER_SCHEMA);
    let use_12h = Prefs::new("org.gnome.desktop.interface").get_enum_nick("clock-format", "24h") == "12h";

    let w = prefs.get::<i32>("window-width", 0);
    let h = prefs.get::<i32>("window-height", 0);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Weather")
        .default_width(if w > 200 { w } else { 900 })
        .default_height(if h > 200 { h } else { 700 })
        .build();
    if prefs.get("window-maximized", false) {
        window.maximize();
    }

    // ---- header ----
    let title = adw::WindowTitle::new("Weather", "");
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));

    let search_entry = gtk::SearchEntry::builder().placeholder_text("Search for a city").search_delay(300).build();
    let search_status = gtk::Label::builder().css_classes(["dim-label"]).wrap(true).visible(false).margin_top(6).build();
    let results_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    let results_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(280)
        .child(&results_list)
        .visible(false)
        .build();
    let saved_header = gtk::Label::builder().label("Saved Locations").xalign(0.0).css_classes(["heading"]).margin_top(6).build();
    let saved_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    let saved_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(280)
        .child(&saved_list)
        .build();
    let pop_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).width_request(340).build();
    let auto_row = adw::SwitchRow::builder().title("Automatic Location").subtitle("Show the weather where you are").build();
    let auto_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).margin_bottom(6).build();
    auto_list.append(&auto_row);
    pop_box.append(&auto_list);
    pop_box.append(&search_entry);
    pop_box.append(&search_status);
    pop_box.append(&results_scroller);
    pop_box.append(&saved_header);
    pop_box.append(&saved_scroller);
    let popover = gtk::Popover::builder().child(&pop_box).build();
    let places_button = gtk::MenuButton::builder()
        .icon_name("find-location-symbolic")
        .tooltip_text("Places (Ctrl+L)")
        .popover(&popover)
        .build();
    header.pack_start(&places_button);

    let menu = gio::Menu::new();
    let units_section = gio::Menu::new();
    units_section.append(Some("_Celsius"), Some("app.temperature-unit::centigrade"));
    units_section.append(Some("_Fahrenheit"), Some("app.temperature-unit::fahrenheit"));
    menu.append_section(Some("Temperature Unit"), &units_section);
    let misc = gio::Menu::new();
    misc.append(Some("_Refresh"), Some("app.refresh"));
    misc.append(Some("_About Weather"), Some("app.about"));
    misc.append(Some("_Quit"), Some("app.quit"));
    menu.append_section(None, &misc);
    let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).tooltip_text("Main Menu").build();
    menu_button.set_primary(true);
    header.pack_end(&menu_button);
    let refresh_button = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Refresh (F5)").action_name("app.refresh").build();
    header.pack_end(&refresh_button);
    let header_spinner = gtk::Spinner::builder().visible(false).build();
    header.pack_end(&header_spinner);

    // ---- pages ----
    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).vexpand(true).build();

    let empty_button = gtk::Button::builder().label("Search Locations").css_classes(["pill", "suggested-action"]).halign(gtk::Align::Center).build();
    let here_button = gtk::Button::builder().label("Use Current Location").css_classes(["pill"]).halign(gtk::Align::Center).build();
    let empty_buttons = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).halign(gtk::Align::Center).build();
    empty_buttons.append(&empty_button);
    empty_buttons.append(&here_button);
    let empty_page = adw::StatusPage::builder()
        .icon_name("weather-few-clouds-symbolic")
        .title("Add a Location")
        .description("Search for a city to see its weather.")
        .child(&empty_buttons)
        .build();
    stack.add_named(&empty_page, Some("empty"));

    let loading = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
    stack.add_named(&loading, Some("loading"));

    let retry = gtk::Button::builder().label("Retry").css_classes(["pill", "suggested-action"]).halign(gtk::Align::Center).action_name("app.refresh").build();
    let error_page = adw::StatusPage::builder().icon_name("network-offline-symbolic").title("Could Not Load the Forecast").child(&retry).build();
    stack.add_named(&error_page, Some("error"));

    let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(24).build();
    let updated_label = gtk::Label::builder().css_classes(["dim-label", "caption"]).halign(gtk::Align::Center).build();
    let page_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .build();
    page_box.append(&content);
    page_box.append(&updated_label);
    let credit = gtk::Label::builder()
        .label("Weather data by <a href=\"https://open-meteo.com/\">Open-Meteo.com</a>")
        .use_markup(true)
        .css_classes(["dim-label", "caption"])
        .halign(gtk::Align::Center)
        .build();
    page_box.append(&credit);
    let clamp = adw::Clamp::builder().maximum_size(960).child(&page_box).build();
    let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build();
    stack.add_named(&scroller, Some("weather"));

    let banner = adw::Banner::builder().button_label("Retry").build();
    banner.set_action_name(Some("app.refresh"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&banner);
    body.append(&stack);
    toolbar.set_content(Some(&body));
    window.set_content(Some(&toolbar));

    let units_now = Units::load(&gweather);
    let unit_action = gio::SimpleAction::new_stateful(
        "temperature-unit",
        Some(glib::VariantTy::STRING),
        &units_now.temperature.nick().to_variant(),
    );

    let locations = load_locations(&prefs);
    auto_row.set_active(locations.automatic_location);

    let ui = Ui(Rc::new(Inner {
        window,
        prefs,
        gweather,
        use_12h,
        units: RefCell::new(units_now),
        locations: RefCell::new(locations),
        forecast: RefCell::new(None),
        offline: Cell::new(false),
        fetching: Cell::new(false),
        generation: Cell::new(0),
        search_generation: Cell::new(0),
        search_results: RefCell::new(Vec::new()),
        locating: Cell::new(false),
        title,
        auto_row: auto_row.clone(),
        places_button,
        search_entry,
        results_list,
        results_scroller,
        search_status,
        saved_list,
        saved_header,
        refresh_button,
        header_spinner,
        banner,
        stack,
        error_page,
        content,
        updated_label,
        unit_action,
    }));

    ui.connect(app, &empty_button, &popover);
    {
        let auto_row = auto_row.clone();
        here_button.connect_clicked(move |_| auto_row.set_active(true));
    }
    let weak = ui.weak();
    auto_row.connect_active_notify(move |row| {
        if let Some(ui) = Ui::upgrade(&weak) {
            ui.set_automatic(row.is_active());
        }
    });
    ui.rebuild_saved();
    if ui.0.locations.borrow().automatic_location {
        ui.locate();
    }
    ui
}

/// Whether the installed `org.gnome.Weather` schema has `key`.
fn schema_has(prefs: &Prefs, key: &str) -> bool {
    prefs.gsettings().and_then(|g| g.settings_schema()).is_some_and(|s| s.has_key(key))
}

/// Loads the saved locations. On first run (no file yet) GNOME Weather's
/// locations are imported, best effort and read-only.
fn load_locations(prefs: &Prefs) -> Locations {
    let path = locations_path();
    let mut locations = Locations::load(&path);
    if !path.exists() {
        let gw = gwdb::Db::load();
        let imported = import::import(gw.as_ref());
        if !imported.is_empty() {
            eprintln!("helix-weather: imported {} location(s) from GNOME Weather", imported.len());
            locations.locations = imported;
            locations.selected = 0;
            if let Err(e) = locations.save(&path) {
                eprintln!("helix-weather: could not save locations: {e}");
            }
        }
    }
    // GNOME's setting wins when its schema is installed.
    if schema_has(prefs, "automatic-location") {
        locations.automatic_location = prefs.get("automatic-location", false);
    }
    if !locations.automatic_location {
        locations.set_automatic(None);
    }
    locations
}

impl Ui {
    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn upgrade(w: &Weak<Inner>) -> Option<Ui> {
        w.upgrade().map(Ui)
    }

    fn connect(&self, app: &adw::Application, empty_button: &gtk::Button, popover: &gtk::Popover) {
        let s = &self.0;

        // Actions.
        let weak = self.weak();
        let refresh = gio::SimpleAction::new("refresh", None);
        refresh.connect_activate(move |_, _| {
            if let Some(ui) = Ui::upgrade(&weak) {
                ui.refresh();
            }
        });
        app.add_action(&refresh);
        app.set_accels_for_action("app.refresh", &["F5", "<Control>r"]);

        let weak = self.weak();
        let search = gio::SimpleAction::new("search", None);
        search.connect_activate(move |_, _| {
            if let Some(ui) = Ui::upgrade(&weak) {
                ui.0.places_button.popup();
            }
        });
        app.add_action(&search);
        app.set_accels_for_action("app.search", &["<Control>l", "<Control>f"]);

        let weak = self.weak();
        s.unit_action.connect_activate(move |action, param| {
            let Some(nick) = param.and_then(|p| p.get::<String>()) else { return };
            action.set_state(&nick.to_variant());
            if let Some(ui) = Ui::upgrade(&weak) {
                let t = Temperature::from_nick(&nick, false);
                ui.0.gweather.set_enum_nick("temperature-unit", t.nick());
                ui.0.units.borrow_mut().temperature = t;
                ui.render();
            }
        });
        app.add_action(&s.unit_action);

        // Follow external changes to the GWeather unit keys.
        if let Some(settings) = s.gweather.gsettings() {
            let weak = self.weak();
            settings.connect_changed(None, move |_, key| {
                if !matches!(key, "temperature-unit" | "speed-unit" | "pressure-unit") {
                    return;
                }
                if let Some(ui) = Ui::upgrade(&weak) {
                    let units = Units::load(&ui.0.gweather);
                    ui.0.unit_action.set_state(&units.temperature.nick().to_variant());
                    *ui.0.units.borrow_mut() = units;
                    ui.render();
                }
            });
        }

        empty_button.set_action_name(Some("app.search"));

        // Popover: focus the search entry when shown.
        let weak = self.weak();
        popover.connect_show(move |_| {
            if let Some(ui) = Ui::upgrade(&weak) {
                ui.0.search_entry.grab_focus();
            }
        });

        let weak = self.weak();
        s.search_entry.connect_search_changed(move |entry| {
            if let Some(ui) = Ui::upgrade(&weak) {
                ui.search(entry.text().trim());
            }
        });
        let weak = self.weak();
        s.search_entry.connect_activate(move |_| {
            if let Some(ui) = Ui::upgrade(&weak) {
                let first = ui.0.search_results.borrow().first().cloned();
                if let Some(p) = first {
                    ui.add_place(p);
                }
            }
        });
        let weak = self.weak();
        s.search_entry.connect_stop_search(move |_| {
            if let Some(ui) = Ui::upgrade(&weak) {
                ui.0.places_button.popdown();
            }
        });

        let weak = self.weak();
        s.results_list.connect_row_activated(move |_, row| {
            if let Some(ui) = Ui::upgrade(&weak) {
                let place = ui.0.search_results.borrow().get(row.index() as usize).cloned();
                if let Some(p) = place {
                    ui.add_place(p);
                }
            }
        });

        let weak = self.weak();
        s.saved_list.connect_row_activated(move |_, row| {
            if let Some(ui) = Ui::upgrade(&weak) {
                ui.0.locations.borrow_mut().selected = row.index() as usize;
                ui.save_locations();
                ui.0.places_button.popdown();
                ui.rebuild_saved();
                ui.select_current();
            }
        });

        // Persist window state.
        let weak = self.weak();
        s.window.connect_close_request(move |win| {
            if let Some(ui) = Ui::upgrade(&weak) {
                let maximized = win.is_maximized();
                ui.0.prefs.set("window-maximized", maximized);
                if !maximized {
                    let (w, h) = win.default_size();
                    ui.0.prefs.set("window-width", w);
                    ui.0.prefs.set("window-height", h);
                }
            }
            glib::Propagation::Proceed
        });

        // Minute tick: update relative times and auto-refresh.
        let weak = self.weak();
        glib::timeout_add_seconds_local(60, move || {
            let Some(ui) = Ui::upgrade(&weak) else { return glib::ControlFlow::Break };
            ui.update_status_texts();
            let stale = ui.0.forecast.borrow().as_ref().is_none_or(|(_, fetched)| now_unix() - fetched >= AUTO_REFRESH_SECS);
            if stale && !ui.0.fetching.get() && ui.0.locations.borrow().selected_place().is_some() {
                if ui.0.locations.borrow().automatic_location {
                    // The user may have moved; this also refreshes if the
                    // place changed.
                    ui.locate();
                }
                ui.refresh();
            }
            glib::ControlFlow::Continue
        });
    }

    /// Turns automatic location on (and looks it up) or off.
    fn set_automatic(&self, on: bool) {
        let s = &self.0;
        if s.auto_row.is_active() != on {
            // Re-enters through the notify handler.
            s.auto_row.set_active(on);
            return;
        }
        if s.locations.borrow().automatic_location == on {
            return;
        }
        s.locations.borrow_mut().automatic_location = on;
        if schema_has(&s.prefs, "automatic-location") {
            s.prefs.set("automatic-location", on);
        }
        if on {
            self.save_locations();
            self.locate();
        } else {
            let before = s.locations.borrow().selected_place().cloned();
            s.locations.borrow_mut().set_automatic(None);
            self.save_locations();
            self.rebuild_saved();
            if before.as_ref() != s.locations.borrow().selected_place() {
                self.select_current();
            }
        }
    }

    /// Looks up the current place (portal / GeoClue + reverse geocoding).
    fn locate(&self) {
        let s = &self.0;
        if s.locating.replace(true) {
            return;
        }
        if s.locations.borrow().selected_place().is_none() {
            s.stack.set_visible_child_name("loading");
        }
        self.set_fetching(true);
        let lang = api::ui_language(&messages_locale());
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || autoloc::current_place(APP_ID, &lang)).await;
            let Some(ui) = Ui::upgrade(&weak) else { return };
            ui.0.locating.set(false);
            ui.set_fetching(false);
            if !ui.0.locations.borrow().automatic_location {
                ui.select_current();
                return;
            }
            match res.unwrap_or_else(|_| Err("Location lookup failed".into())) {
                Ok(place) => {
                    let before = ui.0.locations.borrow().selected_place().cloned();
                    ui.0.locations.borrow_mut().set_automatic(Some(place));
                    ui.save_locations();
                    ui.rebuild_saved();
                    let after = ui.0.locations.borrow().selected_place().cloned();
                    if before != after {
                        ui.select_current();
                    }
                }
                Err(e) => {
                    eprintln!("helix-weather: current location unavailable: {e}");
                    if ui.0.locations.borrow().selected_place().is_none() {
                        ui.0.error_page.set_title("Could Not Find Your Location");
                        ui.0.error_page.set_description(Some(&glib::markup_escape_text(&e)));
                        ui.0.stack.set_visible_child_name("error");
                    }
                }
            }
        });
    }

    fn save_locations(&self) {
        if let Err(e) = self.0.locations.borrow().save(&locations_path()) {
            eprintln!("helix-weather: could not save locations: {e}");
        }
    }

    fn add_place(&self, place: Place) {
        self.0.locations.borrow_mut().add(place);
        self.save_locations();
        self.0.search_entry.set_text("");
        self.0.places_button.popdown();
        self.rebuild_saved();
        self.select_current();
    }

    fn remove_place(&self, index: usize) {
        if self.0.locations.borrow().locations.get(index).is_some_and(|p| p.automatic) {
            self.set_automatic(false);
            return;
        }
        let (was_selected, removed) = {
            let mut locs = self.0.locations.borrow_mut();
            let was = locs.selected == index;
            let removed = locs.locations.get(index).cloned();
            locs.remove(index);
            (was, removed)
        };
        if let Some(p) = removed {
            store::remove_cache(&cache_dir(), &p);
        }
        self.save_locations();
        self.rebuild_saved();
        if was_selected {
            self.select_current();
        }
    }

    fn rebuild_saved(&self) {
        let s = &self.0;
        while let Some(child) = s.saved_list.first_child() {
            s.saved_list.remove(&child);
        }
        let locs = s.locations.borrow();
        s.saved_header.set_visible(!locs.locations.is_empty());
        s.saved_list.set_visible(!locs.locations.is_empty());
        for (i, p) in locs.locations.iter().enumerate() {
            let subtitle = match (p.automatic, p.subtitle()) {
                (true, sub) if sub.is_empty() => "Current location".to_string(),
                (true, sub) => format!("Current location • {sub}"),
                (false, sub) => sub,
            };
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(&p.name)).subtitle(glib::markup_escape_text(&subtitle)).activatable(true).build();
            if p.automatic {
                row.add_suffix(&gtk::Image::from_icon_name("find-location-symbolic"));
            }
            if i == locs.selected {
                row.add_prefix(&gtk::Image::from_icon_name("object-select-symbolic"));
            } else {
                row.add_prefix(&gtk::Image::builder().icon_name("object-select-symbolic").opacity(0.0).build());
            }
            let remove = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Remove").valign(gtk::Align::Center).css_classes(["flat"]).build();
            let weak = self.weak();
            remove.connect_clicked(move |_| {
                if let Some(ui) = Ui::upgrade(&weak) {
                    ui.remove_place(i);
                }
            });
            row.add_suffix(&remove);
            s.saved_list.append(&row);
        }
    }

    fn search(&self, query: &str) {
        let s = &self.0;
        let gen = s.search_generation.get() + 1;
        s.search_generation.set(gen);
        if query.chars().count() < 2 {
            s.search_results.borrow_mut().clear();
            self.show_results(None);
            return;
        }
        s.search_status.set_label("Searching…");
        s.search_status.set_visible(true);
        let query = query.to_string();
        let lang = api::ui_language(&messages_locale());
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || api::search(&query, &lang)).await;
            let Some(ui) = Ui::upgrade(&weak) else { return };
            if ui.0.search_generation.get() != gen {
                return;
            }
            match res {
                Ok(Ok(places)) => {
                    *ui.0.search_results.borrow_mut() = places;
                    ui.show_results(None);
                }
                Ok(Err(e)) => {
                    ui.0.search_results.borrow_mut().clear();
                    ui.show_results(Some(&e));
                }
                Err(_) => {
                    ui.0.search_results.borrow_mut().clear();
                    ui.show_results(Some("Search failed"));
                }
            }
        });
    }

    fn show_results(&self, error: Option<&str>) {
        let s = &self.0;
        while let Some(child) = s.results_list.first_child() {
            s.results_list.remove(&child);
        }
        let results = s.search_results.borrow();
        for p in results.iter() {
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(&p.name)).subtitle(glib::markup_escape_text(&p.subtitle())).activatable(true).build();
            row.add_suffix(&gtk::Image::from_icon_name("list-add-symbolic"));
            s.results_list.append(&row);
        }
        s.results_scroller.set_visible(!results.is_empty());
        let query_len = s.search_entry.text().trim().chars().count();
        match error {
            Some(e) => {
                s.search_status.set_label(e);
                s.search_status.set_visible(true);
            }
            None if results.is_empty() && query_len >= 2 => {
                s.search_status.set_label("No locations found");
                s.search_status.set_visible(true);
            }
            None => s.search_status.set_visible(false),
        }
    }

    /// Shows the selected location (cached data first) and refreshes it.
    fn select_current(&self) {
        let s = &self.0;
        s.generation.set(s.generation.get() + 1);
        s.offline.set(false);
        s.banner.set_revealed(false);
        let place = s.locations.borrow().selected_place().cloned();
        let Some(place) = place else {
            *s.forecast.borrow_mut() = None;
            s.title.set_title("Weather");
            s.title.set_subtitle("");
            s.stack.set_visible_child_name("empty");
            return;
        };
        s.title.set_title(&place.name);
        s.title.set_subtitle(&place.subtitle());
        let cached = store::load_cache(&cache_dir(), &place).and_then(|c| api::parse_forecast(&c.body).ok().map(|f| (f, c.fetched)));
        let have_cache = cached.is_some();
        *s.forecast.borrow_mut() = cached;
        if have_cache {
            self.render();
        } else {
            s.stack.set_visible_child_name("loading");
        }
        self.refresh();
    }

    fn set_fetching(&self, on: bool) {
        let s = &self.0;
        s.fetching.set(on);
        s.header_spinner.set_visible(on);
        s.header_spinner.set_spinning(on);
        s.refresh_button.set_sensitive(!on);
    }

    fn refresh(&self) {
        let s = &self.0;
        let place = s.locations.borrow().selected_place().cloned();
        let Some(place) = place else {
            if s.locations.borrow().automatic_location {
                self.locate();
            }
            return;
        };
        let gen = s.generation.get();
        if s.forecast.borrow().is_none() {
            s.stack.set_visible_child_name("loading");
        }
        self.set_fetching(true);
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let fetch_place = place.clone();
            let res = gio::spawn_blocking(move || api::fetch_forecast(&fetch_place)).await;
            let Some(ui) = Ui::upgrade(&weak) else { return };
            if ui.0.generation.get() != gen {
                // The selection changed meanwhile; that change started its own fetch.
                return;
            }
            ui.set_fetching(false);
            let res = res.unwrap_or_else(|_| Err("The forecast request failed unexpectedly".into()));
            match res.and_then(|body| api::parse_forecast(&body).map(|f| (f, body))) {
                Ok((forecast, body)) => {
                    let fetched = now_unix();
                    if let Err(e) = store::save_cache(&cache_dir(), &place, &CachedForecast { fetched, body }) {
                        eprintln!("helix-weather: could not write cache: {e}");
                    }
                    *ui.0.forecast.borrow_mut() = Some((forecast, fetched));
                    ui.0.offline.set(false);
                    ui.render();
                }
                Err(e) => {
                    eprintln!("helix-weather: forecast update failed: {e}");
                    if ui.0.forecast.borrow().is_some() {
                        ui.0.offline.set(true);
                        ui.update_status_texts();
                    } else {
                        ui.0.error_page.set_title("Could Not Load the Forecast");
                        ui.0.error_page.set_description(Some(&glib::markup_escape_text(&e)));
                        ui.0.stack.set_visible_child_name("error");
                    }
                }
            }
        });
    }

    fn update_status_texts(&self) {
        let s = &self.0;
        let forecast = s.forecast.borrow();
        let Some((_, fetched)) = forecast.as_ref() else {
            s.banner.set_revealed(false);
            return;
        };
        let local = DateTime::from_unix(*fetched, viewer_offset());
        let text = timefmt::updated_text(*fetched, now_unix(), &local, s.use_12h);
        s.updated_label.set_label(&text);
        if s.offline.get() {
            s.banner.set_title(&format!("Offline — showing the last forecast ({})", text.to_lowercase()));
            s.banner.set_revealed(true);
        } else {
            s.banner.set_revealed(false);
        }
    }

    fn render(&self) {
        let s = &self.0;
        let forecast = s.forecast.borrow();
        let Some((f, _)) = forecast.as_ref() else { return };
        while let Some(child) = s.content.first_child() {
            s.content.remove(&child);
        }
        let units = s.units.borrow();
        let fg = fg_color(&s.window);
        let local_now = DateTime::from_unix(now_unix(), f.utc_offset);

        s.content.append(&current_section(f, &local_now, &units, s.use_12h));

        let hours = f.hours_from(&local_now, HOURS);
        if !hours.is_empty() {
            s.content.append(&heading("Hourly Forecast"));
            s.content.append(&hourly_section(hours, &units, s.use_12h, fg));
        }

        let days = f.days_from(&local_now);
        if !days.is_empty() {
            s.content.append(&heading("Daily Forecast"));
            s.content.append(&daily_section(days, &local_now, &units, fg));
        }
        drop(units);
        drop(forecast);
        self.update_status_texts();
        s.stack.set_visible_child_name("weather");
    }
}

fn messages_locale() -> String {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    String::new()
}

fn fg_color(widget: &impl IsA<gtk::Widget>) -> Rgba {
    let c = widget.as_ref().color();
    Rgba { r: c.red() as f64, g: c.green() as f64, b: c.blue() as f64, a: c.alpha() as f64 }
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["title-4"]).build()
}

fn detail_card(caption: &str, value: &str, icon: &str) -> gtk::Box {
    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).css_classes(["card", "weather-card"]).build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(&gtk::Image::from_icon_name(icon));
    top.append(&gtk::Label::builder().label(caption).css_classes(["dim-label", "caption-heading"]).xalign(0.0).build());
    b.append(&top);
    b.append(&gtk::Label::builder().label(value).css_classes(["weather-card-value"]).xalign(0.0).build());
    b
}

fn current_section(f: &Forecast, local_now: &DateTime, units: &Units, use_12h: bool) -> gtk::Box {
    let c = &f.current;
    let section = gtk::Box::new(gtk::Orientation::Vertical, 18);

    let top = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(24).halign(gtk::Align::Center).build();
    let icon = gtk::Image::builder().icon_name(wmo::icon_name(c.code, c.is_day)).pixel_size(96).build();
    top.append(&icon);
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
    text.append(&gtk::Label::builder().label(units.temperature.format(c.temperature)).css_classes(["weather-temp"]).xalign(0.0).build());
    text.append(&gtk::Label::builder().label(wmo::description(c.code)).css_classes(["title-3"]).xalign(0.0).build());
    if let Some(a) = c.apparent {
        text.append(&gtk::Label::builder().label(format!("Feels like {}", units.temperature.format_short(a))).css_classes(["dim-label"]).xalign(0.0).build());
    }
    top.append(&text);
    section.append(&top);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(5)
        .column_spacing(12)
        .row_spacing(12)
        .build();
    let wind = match (c.wind_speed, c.wind_direction) {
        (Some(s), Some(d)) => format!("{} {}", units.speed.format(s), units::compass(d)),
        (Some(s), None) => units.speed.format(s),
        _ => "—".into(),
    };
    flow.append(&detail_card("Wind", &wind, "weather-windy-symbolic"));
    let humidity = c.humidity.map(|h| format!("{}%", h.round() as i64)).unwrap_or_else(|| "—".into());
    flow.append(&detail_card("Humidity", &humidity, "weather-showers-scattered-symbolic"));
    let pressure = c.pressure.map(|p| units.pressure.format(p)).unwrap_or_else(|| "—".into());
    flow.append(&detail_card("Pressure", &pressure, "speedometer-symbolic"));
    let today = f.today(local_now).or_else(|| f.today(&c.time));
    let fmt = |t: Option<DateTime>| t.map(|t| timefmt::format_time(&t, use_12h)).unwrap_or_else(|| "—".into());
    flow.append(&detail_card("Sunrise", &fmt(today.and_then(|d| d.sunrise)), "daytime-sunrise-symbolic"));
    flow.append(&detail_card("Sunset", &fmt(today.and_then(|d| d.sunset)), "daytime-sunset-symbolic"));
    for child in std::iter::successors(flow.first_child(), |c| c.next_sibling()) {
        child.set_focusable(false);
    }
    section.append(&flow);
    section
}

fn hourly_section(hours: &[api::Hour], units: &Units, use_12h: bool, fg: Rgba) -> gtk::Widget {
    let n = hours.len() as i32;
    let strip = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).margin_top(12).margin_bottom(12).build();

    let top = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).homogeneous(true).build();
    let bottom = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).homogeneous(true).build();
    for (i, h) in hours.iter().enumerate() {
        let col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).width_request(COL_W).build();
        let label = if i == 0 { "Now".to_string() } else { timefmt::format_hour(&h.time, use_12h) };
        col.append(&gtk::Label::builder().label(label).css_classes(["weather-hour", "dim-label"]).build());
        let icon = gtk::Image::builder().icon_name(wmo::icon_name(h.code, h.is_day)).pixel_size(24).tooltip_text(wmo::description(h.code)).build();
        col.append(&icon);
        top.append(&col);

        let precip = h.precipitation.filter(|p| *p >= 10.0).map(|p| format!("{}%", p.round() as i64)).unwrap_or_default();
        bottom.append(&gtk::Label::builder().label(precip).css_classes(["weather-hour", "accent"]).width_request(COL_W).build());
    }

    let values: Vec<f64> = hours.iter().map(|h| units.temperature.convert(h.temperature)).collect();
    let labels: Vec<String> = hours.iter().map(|h| units.temperature.format_short(h.temperature)).collect();
    let area = gtk::DrawingArea::builder().content_width(COL_W * n).content_height(GRAPH_H).build();
    area.set_draw_func(move |_, cr, _, h| {
        graph::draw_temperature(cr, &values, &labels, COL_W as f64, h as f64, fg);
    });

    strip.append(&top);
    strip.append(&area);
    strip.append(&bottom);

    let scroller = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&strip)
        .build();
    let frame = gtk::Box::builder().css_classes(["card"]).build();
    scroller.set_hexpand(true);
    frame.append(&scroller);
    frame.upcast()
}

fn daily_section(days: &[api::Day], local_now: &DateTime, units: &Units, fg: Rgba) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    let lows: Vec<f64> = days.iter().map(|d| d.min).collect();
    let highs: Vec<f64> = days.iter().map(|d| d.max).collect();
    let (min, _) = graph::min_max(&lows);
    let (_, max) = graph::min_max(&highs);
    for d in days {
        let row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).css_classes(["weather-day"]).build();
        let names = gtk::Box::builder().orientation(gtk::Orientation::Vertical).width_request(110).valign(gtk::Align::Center).build();
        names.append(&gtk::Label::builder().label(timefmt::day_label(&d.date, local_now)).xalign(0.0).css_classes(["heading"]).build());
        names.append(&gtk::Label::builder().label(format!("{} {}", d.date.day, d.date.month_short())).xalign(0.0).css_classes(["dim-label", "caption"]).build());
        row.append(&names);
        row.append(&gtk::Image::builder().icon_name(wmo::icon_name(d.code, true)).pixel_size(24).build());
        row.append(&gtk::Label::builder().label(wmo::description(d.code)).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
        let precip = d.precipitation.filter(|p| *p >= 10.0).map(|p| format!("{}%", p.round() as i64)).unwrap_or_default();
        row.append(&gtk::Label::builder().label(precip).width_chars(4).xalign(1.0).css_classes(["accent", "caption"]).build());
        row.append(&gtk::Label::builder().label(units.temperature.format_short(d.min)).width_chars(4).xalign(1.0).css_classes(["dim-label"]).build());
        let (lo, hi) = (d.min, d.max);
        let bar = gtk::DrawingArea::builder().content_width(110).content_height(20).valign(gtk::Align::Center).build();
        bar.set_draw_func(move |_, cr, w, h| {
            graph::draw_range_bar(cr, (lo, hi), (min, max), w as f64, h as f64, fg);
        });
        row.append(&bar);
        row.append(&gtk::Label::builder().label(units.temperature.format_short(d.max)).width_chars(4).xalign(0.0).css_classes(["heading"]).build());
        let lrow = gtk::ListBoxRow::builder().child(&row).activatable(false).build();
        list.append(&lrow);
    }
    list
}
