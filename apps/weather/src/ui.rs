//! GTK user interface.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use crate::api::{self, Forecast};
use crate::autoloc;
use crate::graph;
use crate::gwdb;
use crate::import;
use crate::store::{self, CachedForecast, Locations, Place};
use crate::timefmt::{self, DateTime};
use crate::units::{self, Pressure, Speed, Temperature};
use crate::wmo;
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;
use helix_app_common::trnf;

pub const APP_ID: &str = "org.helix.Weather";
const SCHEMA: &str = "org.gnome.Weather";
const GWEATHER_SCHEMA: &str = "org.gnome.GWeather4";
const APP_DIR: &str = "helix-weather";
const HOURS: usize = 48;
const COL_W: i32 = 64;
const GRAPH_H: i32 = 110;
const AUTO_REFRESH_SECS: i64 = 30 * 60;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The viewer's (not the location's) current UTC offset in seconds.
fn viewer_offset() -> i64 {
    glib::DateTime::now_local()
        .map(|d| d.utc_offset().as_seconds())
        .unwrap_or(0)
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
            temperature: Temperature::from_nick(
                &prefs.get_enum_nick("temperature-unit", "default"),
                imperial,
            ),
            speed: Speed::from_nick(&prefs.get_enum_nick("speed-unit", "default"), imperial),
            pressure: Pressure::from_nick(
                &prefs.get_enum_nick("pressure-unit", "default"),
                imperial,
            ),
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
    /// The window is narrower than 450sp: compact current conditions and
    /// daily rows.
    narrow: Cell<bool>,
    /// The running refresh was asked for by the user (menu, button, F5),
    /// so its end is announced; automatic refreshes stay quiet.
    manual_refresh: Cell<bool>,

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
    error_search: gtk::Button,
    /// The technical error text, under "Details" on the error page.
    error_details: gtk::Label,
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
        tr("Weather"),
        tr("Show weather conditions and forecast\n\nWeather data by Open-Meteo.com (CC BY 4.0, non-commercial use). \
         Place names may come from © OpenStreetMap contributors (Nominatim)."),
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
        // The current temperature is display text (em, so it follows the
        // text size); everything else uses Adwaita's typography classes.
        ".weather-temp { font-size: 4.4em; font-weight: 300; }
         .weather-temp.narrow { font-size: 2.9em; }
         .weather-card { padding: 12px; }
         .weather-day { padding: 8px 12px; }
         .weather-hours > flowboxchild { padding: 0; margin: 0; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build(app: &adw::Application) -> Ui {
    let prefs = Prefs::new(SCHEMA);
    let gweather = Prefs::new(GWEATHER_SCHEMA);
    let use_12h =
        Prefs::new("org.gnome.desktop.interface").get_enum_nick("clock-format", "24h") == "12h";

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(tr("Weather"))
        .default_width(900)
        .default_height(700)
        .width_request(360)
        .height_request(294)
        .build();
    {
        use helix_app_common::window_state::{persist, SizeKeys, Store};
        persist(
            &window,
            Store::settings(
                &prefs,
                SizeKeys::Split("window-width", "window-height"),
                "window-maximized",
                "helix-weather",
            ),
        );
    }

    // ---- header ----
    let title = adw::WindowTitle::new(tr("Weather"), "");
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search for a city"))
        .search_delay(300)
        .build();
    let search_status = gtk::Label::builder()
        .css_classes(["dim-label"])
        .wrap(true)
        .visible(false)
        .margin_top(6)
        .build();
    let results_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let results_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(280)
        .child(&results_list)
        .visible(false)
        .build();
    let saved_header = gtk::Label::builder()
        .label(tr("Saved Locations"))
        .xalign(0.0)
        .css_classes(["heading"])
        .margin_top(6)
        .build();
    let saved_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let saved_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(280)
        .child(&saved_list)
        .build();
    let pop_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .width_request(300)
        .build();
    let auto_row = adw::SwitchRow::builder()
        .title(tr("Automatic Location"))
        .subtitle(tr("Show the weather where you are"))
        .build();
    let auto_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .margin_bottom(6)
        .build();
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
        .popover(&popover)
        .build()
        .with_shortcut(tr("Places"), "<Control>l");
    header.pack_start(&places_button);

    let menu = gio::Menu::new();
    let units_section = gio::Menu::new();
    units_section.append(
        Some(tr("_Celsius")),
        Some("app.temperature-unit::centigrade"),
    );
    units_section.append(
        Some(tr("_Fahrenheit")),
        Some("app.temperature-unit::fahrenheit"),
    );
    menu.append_section(Some(tr("Temperature Unit")), &units_section);
    let misc = gio::Menu::new();
    misc.append(Some(tr("_Refresh")), Some("app.refresh"));
    menu.append_section(None, &misc);
    // HIG primary menu: no Quit (Ctrl+Q still works).
    let help = gio::Menu::new();
    help.append(
        Some(helix_app_common::i18n::tr(
            helix_app_common::shortcuts::MENU_LABEL,
        )),
        Some("win.show-help-overlay"),
    );
    help.append(Some(tr("_About Weather")), Some("app.about"));
    menu.append_section(None, &help);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text(tr("Main Menu"))
        .build()
        .labelled(tr("Main Menu"));
    menu_button.set_primary(true);
    header.pack_end(&menu_button);
    let refresh_button = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .action_name("app.refresh")
        .build()
        .with_shortcut(tr("Refresh"), "F5");
    header.pack_end(&refresh_button);
    let header_spinner = gtk::Spinner::builder()
        .visible(false)
        .build()
        .labelled(tr("Updating Forecast"));
    header.pack_end(&header_spinner);

    // ---- pages ----
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vexpand(true)
        .build();

    let empty_button = gtk::Button::builder()
        .label(tr("Search Locations"))
        .css_classes(["pill", "suggested-action"])
        .halign(gtk::Align::Center)
        .build();
    let here_button = gtk::Button::builder()
        .label(tr("Use Current Location"))
        .css_classes(["pill"])
        .halign(gtk::Align::Center)
        .build();
    let empty_buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    empty_buttons.append(&empty_button);
    empty_buttons.append(&here_button);
    let empty_page = adw::StatusPage::builder()
        .icon_name("weather-few-clouds-symbolic")
        .title(tr("Add a Location"))
        .description(tr("Search for a city to see its weather."))
        .child(&empty_buttons)
        .build();
    stack.add_named(&empty_page, Some("empty"));

    let loading = gtk::Spinner::builder()
        .spinning(true)
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build()
        .labelled(tr("Loading Forecast"));
    stack.add_named(&loading, Some("loading"));

    let retry = gtk::Button::builder()
        .label(tr("Retry"))
        .css_classes(["pill", "suggested-action"])
        .halign(gtk::Align::Center)
        .action_name("app.refresh")
        .build();
    // When the current location cannot be found: pick a place by hand.
    let error_search = gtk::Button::builder()
        .label(tr("Search Locations"))
        .css_classes(["pill"])
        .halign(gtk::Align::Center)
        .action_name("app.search")
        .visible(false)
        .build();
    let error_buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    error_buttons.append(&retry);
    error_buttons.append(&error_search);
    // The page says what went wrong in plain words; the technical text is
    // there on demand.
    let error_details = gtk::Label::builder()
        .wrap(true)
        .selectable(true)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    let details = gtk::Expander::builder()
        .label(tr("Details"))
        .child(&error_details)
        .margin_top(12)
        .build();
    error_buttons.append(&details);
    let error_page = adw::StatusPage::builder()
        .icon_name("network-offline-symbolic")
        .title(tr("Could Not Load the Forecast"))
        .child(&error_buttons)
        .build();
    stack.add_named(&error_page, Some("error"));

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .build();
    let updated_label = gtk::Label::builder()
        .css_classes(["dim-label", "caption"])
        .halign(gtk::Align::Center)
        .build();
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
        // Translators: {provider} is a link to the weather service.
        .label(trf!(
            "Weather data by {provider}",
            provider = "<a href=\"https://open-meteo.com/\">Open-Meteo.com</a>"
        ))
        .use_markup(true)
        .css_classes(["dim-label", "caption"])
        .halign(gtk::Align::Center)
        .build();
    page_box.append(&credit);
    let clamp = adw::Clamp::builder()
        .maximum_size(960)
        .child(&page_box)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    stack.add_named(&scroller, Some("weather"));

    let banner = adw::Banner::builder().button_label(tr("Retry")).build();
    banner.set_action_name(Some("app.refresh"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&banner);
    body.append(&stack);
    toolbar.set_content(Some(&body));
    window.set_content(Some(&toolbar));
    window.set_help_overlay(Some(&helix_app_common::shortcuts::window(SHORTCUT_GROUPS)));

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
        narrow: Cell::new(false),
        manual_refresh: Cell::new(false),
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
        error_search,
        error_details,
        content,
        updated_label,
        unit_action,
    }));

    ui.connect(app, &empty_button, &popover);
    // Narrow windows (phones, tiled halves): the page is rebuilt with a
    // stacked current-conditions block and daily rows without the
    // description and range bar.
    let narrow = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 450sp").expect("valid condition"),
    );
    for on in [true, false] {
        let weak = ui.weak();
        let handler = move |_: &adw::Breakpoint| {
            if let Some(ui) = Ui::upgrade(&weak) {
                if ui.0.narrow.replace(on) != on {
                    ui.render();
                }
            }
        };
        if on {
            narrow.connect_apply(handler);
        } else {
            narrow.connect_unapply(handler);
        }
    }
    ui.0.window.add_breakpoint(narrow);
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
    prefs
        .gsettings()
        .and_then(|g| g.settings_schema())
        .is_some_and(|s| s.has_key(key))
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
            eprintln!(
                "helix-weather: imported {} location(s) from GNOME Weather",
                imported.len()
            );
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
                ui.0.manual_refresh.set(true);
                ui.refresh();
            }
        });
        app.add_action(&refresh);

        let weak = self.weak();
        let search = gio::SimpleAction::new("search", None);
        search.connect_activate(move |_, _| {
            if let Some(ui) = Ui::upgrade(&weak) {
                ui.0.places_button.popup();
            }
        });
        app.add_action(&search);
        helix_app_common::shortcuts::set_accels(app, ACCELS);

        let weak = self.weak();
        s.unit_action.connect_activate(move |action, param| {
            let Some(nick) = param.and_then(|p| p.get::<String>()) else {
                return;
            };
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
                    ui.0.unit_action
                        .set_state(&units.temperature.nick().to_variant());
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
                let place =
                    ui.0.search_results
                        .borrow()
                        .get(row.index() as usize)
                        .cloned();
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

        // Minute tick: update relative times and auto-refresh.
        let weak = self.weak();
        glib::timeout_add_seconds_local(60, move || {
            let Some(ui) = Ui::upgrade(&weak) else {
                return glib::ControlFlow::Break;
            };
            ui.update_status_texts();
            let stale =
                ui.0.forecast
                    .borrow()
                    .as_ref()
                    .is_none_or(|(_, fetched)| now_unix() - fetched >= AUTO_REFRESH_SECS);
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
            match res.unwrap_or_else(|_| Err(tr("Location lookup failed").into())) {
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
                        ui.0.error_page
                            .set_title(tr("Could Not Find Your Location"));
                        ui.0.error_page
                            .set_icon_name(Some("find-location-symbolic"));
                        ui.0.error_search.set_visible(true);
                        ui.show_error(
                            tr("Turn on location services, or search for a place instead."),
                            &e,
                        );
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
        if self
            .0
            .locations
            .borrow()
            .locations
            .get(index)
            .is_some_and(|p| p.automatic)
        {
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
                (true, sub) if sub.is_empty() => tr("Current location").to_string(),
                (true, sub) => trf!("Current location • {sub}", sub = sub),
                (false, sub) => sub,
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&p.name))
                .subtitle(glib::markup_escape_text(&subtitle))
                .activatable(true)
                .build();
            // The subtitle says "Current location" and the row's selected
            // state which place is shown; the icons only show it.
            if p.automatic {
                row.add_suffix(&decorative_icon("find-location-symbolic"));
            }
            let selected = i == locs.selected;
            let check = decorative_icon("object-select-symbolic");
            if !selected {
                check.set_opacity(0.0);
            }
            row.add_prefix(&check);
            row.update_state(&[gtk::accessible::State::Selected(Some(selected))]);
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Remove"))
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build()
                .labelled(&remove_place_name(&p.name));
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
        s.search_status.set_label(tr("Searching…"));
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
                    eprintln!("helix-weather: search failed: {e}");
                    ui.0.search_results.borrow_mut().clear();
                    ui.show_results(Some(friendly_error(&e)));
                }
                Err(_) => {
                    ui.0.search_results.borrow_mut().clear();
                    ui.show_results(Some(tr("Search failed")));
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
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&p.name))
                .subtitle(glib::markup_escape_text(&p.subtitle()))
                .activatable(true)
                .build();
            row.add_suffix(&gtk::Image::from_icon_name("list-add-symbolic"));
            s.results_list.append(&row);
        }
        s.results_scroller.set_visible(!results.is_empty());
        let query_len = s.search_entry.text().trim().chars().count();
        let status = match error {
            Some(e) => {
                s.search_status.set_label(e);
                s.search_status.set_visible(true);
                e.to_string()
            }
            None if results.is_empty() && query_len >= 2 => {
                s.search_status.set_label(tr("No locations found"));
                s.search_status.set_visible(true);
                tr("No locations found").to_string()
            }
            None => {
                s.search_status.set_visible(false);
                match results.len() {
                    0 => String::new(),
                    n => trnf!("{n} location found", "{n} locations found", n),
                }
            }
        };
        // Spoken once the results stop changing while typing.
        if query_len >= 2 {
            helix_app_common::announce_settled(&s.search_entry, &status);
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
            s.title.set_title(tr("Weather"));
            s.title.set_subtitle("");
            s.stack.set_visible_child_name("empty");
            return;
        };
        s.title.set_title(&place.name);
        s.title.set_subtitle(&place.subtitle());
        let cached = store::load_cache(&cache_dir(), &place)
            .and_then(|c| api::parse_forecast(&c.body).ok().map(|f| (f, c.fetched)));
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
            let manual = ui.0.manual_refresh.replace(false);
            let res =
                res.unwrap_or_else(|_| Err(tr("The forecast request failed unexpectedly").into()));
            match res.and_then(|body| api::parse_forecast(&body).map(|f| (f, body))) {
                Ok((forecast, body)) => {
                    let fetched = now_unix();
                    if let Err(e) =
                        store::save_cache(&cache_dir(), &place, &CachedForecast { fetched, body })
                    {
                        eprintln!("helix-weather: could not write cache: {e}");
                    }
                    *ui.0.forecast.borrow_mut() = Some((forecast, fetched));
                    let was_offline = ui.0.offline.replace(false);
                    ui.render();
                    if was_offline {
                        helix_app_common::announce(
                            &ui.0.updated_label,
                            tr("Back online, forecast updated"),
                            helix_app_common::Urgency::Polite,
                        );
                    } else if manual {
                        helix_app_common::announce(
                            &ui.0.updated_label,
                            tr("Forecast updated"),
                            helix_app_common::Urgency::Polite,
                        );
                    }
                }
                Err(e) => {
                    eprintln!("helix-weather: forecast update failed: {e}");
                    if ui.0.forecast.borrow().is_some() {
                        ui.0.offline.set(true);
                        ui.update_status_texts();
                        if manual {
                            helix_app_common::announce(
                                &ui.0.banner,
                                &trf!("Could not update the forecast. {}", friendly_error(&e)),
                                helix_app_common::Urgency::Polite,
                            );
                        }
                    } else {
                        ui.0.error_page.set_title(tr("Could Not Load the Forecast"));
                        ui.0.error_page
                            .set_icon_name(Some("network-offline-symbolic"));
                        ui.0.error_search.set_visible(false);
                        ui.show_error(friendly_error(&e), &e);
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
            s.banner.set_title(&trf!(
                "Offline — showing the last forecast ({})",
                text.to_lowercase()
            ));
            s.banner.set_revealed(true);
            helix_app_common::announce_change(
                &s.banner,
                tr("Offline, showing the last forecast"),
                helix_app_common::Urgency::Polite,
            );
        } else {
            helix_app_common::announce_change(&s.banner, "", helix_app_common::Urgency::Polite);
            s.banner.set_revealed(false);
        }
    }

    /// Shows the error page with `friendly` text (and `raw` under
    /// Details), and announces it.
    fn show_error(&self, friendly: &str, raw: &str) {
        let s = &self.0;
        s.error_page
            .set_description(Some(&glib::markup_escape_text(friendly)));
        s.error_details.set_label(raw);
        s.stack.set_visible_child_name("error");
        helix_app_common::announce(
            &s.error_page,
            &format!("{}. {friendly}", s.error_page.title()),
            helix_app_common::Urgency::Assertive,
        );
    }

    fn render(&self) {
        let s = &self.0;
        let forecast = s.forecast.borrow();
        let Some((f, _)) = forecast.as_ref() else {
            return;
        };
        // Refreshes (also the automatic ones) rebuild the page; keep the
        // focused hour or day, the scroll position and the hourly strip's
        // position.
        let content = s.content.clone();
        let hourly = hourly_adjustment(content.upcast_ref()).map(|a| a.value());
        helix_app_common::focus::rebuild_keeping_focus(&content, None, || self.render_content(f));
        if let (Some(value), Some(adj)) = (hourly, hourly_adjustment(content.upcast_ref())) {
            helix_app_common::focus::restore_scroll(&adj, value);
        }
        drop(forecast);
        self.update_status_texts();
        s.stack.set_visible_child_name("weather");
    }

    fn render_content(&self, f: &Forecast) {
        let s = &self.0;
        while let Some(child) = s.content.first_child() {
            s.content.remove(&child);
        }
        let units = s.units.borrow();
        let local_now = DateTime::from_unix(now_unix(), f.utc_offset);

        s.content.append(&current_section(
            f,
            &local_now,
            &units,
            s.use_12h,
            s.narrow.get(),
        ));

        let hours = f.hours_from(&local_now, HOURS);
        if !hours.is_empty() {
            s.content.append(&heading(tr("Hourly Forecast")));
            s.content.append(&hourly_section(hours, &units, s.use_12h));
        }

        let days = f.days_from(&local_now);
        if !days.is_empty() {
            s.content.append(&heading(tr("Daily Forecast")));
            s.content
                .append(&daily_section(days, &local_now, &units, s.narrow.get()));
        }
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

/// The colours to draw `area` in, read now from the style: it carries the
/// `accent` class, so its own colour is the accent colour, and text uses
/// its parent's (normal) colour.
fn graph_colors(area: &gtk::DrawingArea) -> graph::Colors {
    let accent = area.color();
    graph::Colors {
        fg: area.parent().map_or(accent, |p| p.color()).into(),
        accent: accent.into(),
    }
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["title-4"])
        .build()
}

fn detail_card(caption: &str, value: &str, icon: &str) -> gtk::Box {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["card", "weather-card"])
        .build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(&gtk::Image::from_icon_name(icon));
    top.append(
        &gtk::Label::builder()
            .label(caption)
            .css_classes(["dim-label", "caption-heading"])
            .xalign(0.0)
            .build(),
    );
    b.append(&top);
    b.append(
        &gtk::Label::builder()
            .label(value)
            .css_classes(["weather-card-value", "title-3"])
            .xalign(0.0)
            .build(),
    );
    b
}

fn current_section(
    f: &Forecast,
    local_now: &DateTime,
    units: &Units,
    use_12h: bool,
    narrow: bool,
) -> gtk::Box {
    let c = &f.current;
    let section = gtk::Box::new(gtk::Orientation::Vertical, 18);

    // Narrow: the icon above a smaller, centred temperature.
    let top = gtk::Box::builder()
        .orientation(if narrow {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        })
        .spacing(if narrow { 6 } else { 24 })
        .halign(gtk::Align::Center)
        .build();
    let icon = gtk::Image::builder()
        .icon_name(wmo::icon_name(c.code, c.is_day))
        .pixel_size(if narrow { 64 } else { 96 })
        .build();
    top.append(&icon);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .build();
    let xalign = if narrow { 0.5 } else { 0.0 };
    let temp = gtk::Label::builder()
        .label(units.temperature.format(c.temperature))
        .css_classes(["weather-temp"])
        .xalign(xalign)
        .build();
    if narrow {
        temp.add_css_class("narrow");
    }
    text.append(&temp);
    text.append(
        &gtk::Label::builder()
            .label(wmo::description(c.code))
            .css_classes(["title-3"])
            .xalign(xalign)
            .wrap(true)
            .justify(if narrow {
                gtk::Justification::Center
            } else {
                helix_app_common::i18n::justify_start()
            })
            .build(),
    );
    if let Some(a) = c.apparent {
        text.append(
            &gtk::Label::builder()
                .label(trf!("Feels like {}", units.temperature.format_short(a)))
                .css_classes(["dim-label"])
                .xalign(xalign)
                .build(),
        );
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
    flow.append(&detail_card(tr("Wind"), &wind, "weather-windy-symbolic"));
    let humidity = c
        .humidity
        .map(|h| format!("{}%", h.round() as i64))
        .unwrap_or_else(|| "—".into());
    flow.append(&detail_card(
        tr("Humidity"),
        &humidity,
        "weather-showers-scattered-symbolic",
    ));
    let pressure = c
        .pressure
        .map(|p| units.pressure.format(p))
        .unwrap_or_else(|| "—".into());
    flow.append(&detail_card(
        tr("Pressure"),
        &pressure,
        "speedometer-symbolic",
    ));
    let today = f.today(local_now).or_else(|| f.today(&c.time));
    let fmt = |t: Option<DateTime>| {
        t.map(|t| timefmt::format_time(&t, use_12h))
            .unwrap_or_else(|| "—".into())
    };
    flow.append(&detail_card(
        tr("Sunrise"),
        &fmt(today.and_then(|d| d.sunrise)),
        "daytime-sunrise-symbolic",
    ));
    flow.append(&detail_card(
        tr("Sunset"),
        &fmt(today.and_then(|d| d.sunset)),
        "daytime-sunset-symbolic",
    ));
    for child in std::iter::successors(flow.first_child(), |c| c.next_sibling()) {
        child.set_focusable(false);
    }
    // Nothing in the cards takes focus. Without this the flow box claims
    // Tab (it "moves" to a cell that cannot take focus) and keyboard focus
    // never reaches the forecast below.
    flow.set_can_focus(false);
    section.append(&flow);
    section
}

fn hourly_section(hours: &[api::Hour], units: &Units, use_12h: bool) -> gtk::Widget {
    let n = hours.len() as i32;
    let strip = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(12)
        .margin_bottom(12)
        .build();

    // One focusable item per hour, in a single-line list: Tab enters it,
    // the arrow keys move between hours (scrolling the strip), and each
    // item is named with everything its column shows. The graph and the
    // precipitation figures below repeat the items' data for the eye.
    let top = gtk::FlowBox::builder()
        .accessible_role(gtk::AccessibleRole::List)
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(hours.len().max(1) as u32)
        .max_children_per_line(hours.len().max(1) as u32)
        .column_spacing(0)
        .css_classes(["weather-hours"])
        .build();
    helix_app_common::set_accessible_label(&top, tr("Hourly Forecast"));
    let bottom = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    for (i, h) in hours.iter().enumerate() {
        let col = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .width_request(COL_W)
            .build();
        let label = if i == 0 {
            tr("Now").to_string()
        } else {
            timefmt::format_hour(&h.time, use_12h)
        };
        col.append(
            &gtk::Label::builder()
                .label(&label)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
        let icon = gtk::Image::builder()
            .icon_name(wmo::icon_name(h.code, h.is_day))
            .pixel_size(24)
            .tooltip_text(wmo::description(h.code))
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        col.append(&icon);
        let item = gtk::FlowBoxChild::builder()
            .accessible_role(gtk::AccessibleRole::ListItem)
            .child(&col)
            .build();
        let name = hour_description(
            &label,
            &units.temperature.format_short(h.temperature),
            wmo::description(h.code),
            h.precipitation,
        );
        item.update_property(&[gtk::accessible::Property::Label(&name)]);
        // Keeps focus on the same hour when a refresh rebuilds the page.
        helix_app_common::focus::set_key(&item, &format!("hour {:?}", h.time));
        top.insert(&item, -1);

        let precip = h
            .precipitation
            .filter(|p| *p >= 10.0)
            .map(|p| format!("{}%", p.round() as i64))
            .unwrap_or_default();
        bottom.append(
            &gtk::Label::builder()
                .label(precip)
                .css_classes(["caption", "accent"])
                .width_request(COL_W)
                .build(),
        );
    }

    let values: Vec<f64> = hours
        .iter()
        .map(|h| units.temperature.convert(h.temperature))
        .collect();
    let labels: Vec<String> = hours
        .iter()
        .map(|h| units.temperature.format_short(h.temperature))
        .collect();
    // The hour items carry the temperatures as text.
    let area = gtk::DrawingArea::builder()
        .content_width(COL_W * n)
        .content_height(GRAPH_H)
        .css_classes(["accent"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    area.set_draw_func(move |a, cr, _, h| {
        graph::draw_temperature(
            cr,
            &a.pango_context(),
            &values,
            &labels,
            COL_W as f64,
            h as f64,
            graph_colors(a),
        );
    });
    helix_app_common::style::redraw_on_style_change(&area);

    strip.append(&top);
    strip.append(&area);
    strip.append(&bottom);

    let scroller = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&strip)
        .build();
    // Moving between hours with the keyboard scrolls the strip.
    top.set_hadjustment(&scroller.hadjustment());
    // A flow box moves Tab between its cells; the strip is one stop, so Tab
    // leaves it (the arrow keys, Home and End move between hours).
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let top = top.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            let back = match key {
                gtk::gdk::Key::Tab | gtk::gdk::Key::KP_Tab => false,
                gtk::gdk::Key::ISO_Left_Tab => true,
                _ => return glib::Propagation::Proceed,
            };
            let Some(top) = top.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let Some(root) = top.root() else {
                return glib::Propagation::Proceed;
            };
            let dir = if back {
                gtk::DirectionType::TabBackward
            } else {
                gtk::DirectionType::TabForward
            };
            // With the strip unfocusable for a moment, the window's focus
            // chain skips it and goes to the widget after (or before) it.
            top.set_can_focus(false);
            let moved = root.child_focus(dir);
            top.set_can_focus(true);
            if moved {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    top.add_controller(keys);
    scroller.set_widget_name(HOURLY_SCROLLER);
    let frame = gtk::Box::builder().css_classes(["card"]).build();
    scroller.set_hexpand(true);
    frame.append(&scroller);
    frame.upcast()
}

/// The horizontal adjustment of the hourly strip under `root`, if shown.
fn hourly_adjustment(root: &gtk::Widget) -> Option<gtk::Adjustment> {
    fn find(w: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
        if w.widget_name() == HOURLY_SCROLLER {
            return w.downcast_ref::<gtk::ScrolledWindow>().cloned();
        }
        std::iter::successors(w.first_child(), |c| c.next_sibling()).find_map(|c| find(&c))
    }
    find(root).map(|s| s.hadjustment())
}

/// Plain words for a forecast, search or location error; the technical
/// text stays available under "Details" and in the log.
fn friendly_error(raw: &str) -> &'static str {
    // The errors come translated from `api` and `autoloc`; compare with the
    // translated messages up to where their detail goes.
    const MARK: char = '\u{1}';
    let starts = |message: String| {
        let head = message.split(MARK).next().unwrap_or_default();
        !head.is_empty() && raw.starts_with(head)
    };
    if starts(trf!(
        "Could not connect to the weather service: {t}",
        t = MARK
    )) || starts(trf!("Could not read response: {e}", e = MARK))
    {
        tr("Could not reach the weather service. Check your internet connection and try again.")
    } else if starts(trf!(
        "The weather service returned HTTP {code}",
        code = format!("5{MARK}")
    )) {
        tr("The weather service is having problems. Try again later.")
    } else if starts(trf!("Unexpected forecast data: {e}", e = MARK))
        || starts(trf!("Unexpected search data: {e}", e = MARK))
        || starts(trf!("Unexpected reverse geocoding data: {e}", e = MARK))
    {
        tr("The weather service sent data that could not be read. Try again later.")
    } else {
        tr("The weather service could not answer this request. Try again later.")
    }
}

/// The widget name of the hourly strip's scrolled window, so a refresh can
/// keep its horizontal position.
const HOURLY_SCROLLER: &str = "hourly-scroller";

/// What an hour of the hourly forecast says to assistive technology:
/// "15:00, 12°, Partly cloudy, 40% chance of precipitation".
fn hour_description(
    time: &str,
    temperature: &str,
    sky: &str,
    precipitation: Option<f64>,
) -> String {
    // Translators: an hour of the forecast for screen readers, "15:00, 12°,
    // Partly cloudy".
    let mut text = trf!(
        "{time}, {temperature}, {sky}",
        time = time,
        temperature = temperature,
        sky = sky
    );
    if let Some(p) = precipitation.filter(|p| *p >= 10.0) {
        text.push_str(&trf!(", {}% chance of precipitation", p.round() as i64));
    }
    text
}

/// What a day row says to assistive technology: "Tomorrow, Slight rain,
/// 58% chance of precipitation, low 1°, high 8°".
fn day_description(
    day: &str,
    sky: &str,
    precipitation: Option<f64>,
    low: &str,
    high: &str,
) -> String {
    // Translators: a day of the forecast for screen readers, "Tomorrow,
    // Slight rain".
    let mut text = trf!("{day}, {sky}", day = day, sky = sky);
    if let Some(p) = precipitation.filter(|p| *p >= 10.0) {
        text.push_str(&trf!(", {}% chance of precipitation", p.round() as i64));
    }
    text.push_str(&trf!(", low {low}, high {high}", low = low, high = high));
    text
}

fn daily_section(
    days: &[api::Day],
    local_now: &DateTime,
    units: &Units,
    narrow: bool,
) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let lows: Vec<f64> = days.iter().map(|d| d.min).collect();
    let highs: Vec<f64> = days.iter().map(|d| d.max).collect();
    let (min, _) = graph::min_max(&lows);
    let (_, max) = graph::min_max(&highs);
    for d in days {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .css_classes(["weather-day"])
            .build();
        // Narrow: the names take the description's place and the range bar
        // is left out, so a row needs about 290 px.
        let names = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(if narrow { 80 } else { 110 })
            .hexpand(narrow)
            .valign(gtk::Align::Center)
            .build();
        names.append(
            &gtk::Label::builder()
                .label(timefmt::day_label(&d.date, local_now))
                .xalign(0.0)
                .css_classes(["heading"])
                .build(),
        );
        names.append(
            &gtk::Label::builder()
                .label(format!("{} {}", d.date.day, d.date.month_short()))
                .xalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
        row.append(&names);
        row.append(
            &gtk::Image::builder()
                .icon_name(wmo::icon_name(d.code, true))
                .pixel_size(24)
                .build(),
        );
        if !narrow {
            row.append(
                &gtk::Label::builder()
                    .label(wmo::description(d.code))
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
        }
        let precip = d
            .precipitation
            .filter(|p| *p >= 10.0)
            .map(|p| format!("{}%", p.round() as i64))
            .unwrap_or_default();
        row.append(
            &gtk::Label::builder()
                .label(precip)
                .width_chars(4)
                .xalign(1.0)
                .css_classes(["accent", "caption"])
                .build(),
        );
        row.append(
            &gtk::Label::builder()
                .label(units.temperature.format_short(d.min))
                .width_chars(4)
                .xalign(1.0)
                .css_classes(["dim-label"])
                .build(),
        );
        let (lo, hi) = (d.min, d.max);
        let bar = gtk::DrawingArea::builder()
            .content_width(110)
            .content_height(20)
            .valign(gtk::Align::Center)
            .css_classes(["accent"])
            .build();
        bar.set_draw_func(move |a, cr, w, h| {
            graph::draw_range_bar(
                cr,
                (lo, hi),
                (min, max),
                w as f64,
                h as f64,
                graph_colors(a),
            );
        });
        helix_app_common::style::redraw_on_style_change(&bar);
        if !narrow {
            row.append(&bar);
        }
        row.append(
            &gtk::Label::builder()
                .label(units.temperature.format_short(d.max))
                .width_chars(4)
                .xalign(0.0)
                .css_classes(["heading"])
                .build(),
        );
        let lrow = gtk::ListBoxRow::builder()
            .child(&row)
            .activatable(false)
            .build();
        // Everything the row shows, also what narrow windows leave out.
        lrow.update_property(&[gtk::accessible::Property::Label(&day_description(
            &timefmt::day_label(&d.date, local_now),
            wmo::description(d.code),
            d.precipitation,
            &units.temperature.format_short(d.min),
            &units.temperature.format_short(d.max),
        ))]);
        helix_app_common::focus::set_key(&lrow, &format!("{:?}", d.date));
        list.append(&lrow);
    }
    list
}
/// An icon that only repeats what text or state already says.
fn decorative_icon(name: &str) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(name)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build()
}
/// Accessible name of a saved place's Remove button: "Remove Berlin".
fn remove_place_name(place: &str) -> String {
    trf!("Remove {place}", place = place)
}

/// Accelerators registered on the application.
const ACCELS: &[(&str, &[&str])] = &[
    ("app.refresh", &["F5", "<Control>r"]),
    ("app.search", &["<Control>l", "<Control>f"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

const SHORTCUT_GROUPS: &[helix_app_common::shortcuts::Group] = {
    use helix_app_common::shortcuts::Item::Keys;
    &[
        (
            n_("General"),
            &[
                Keys(n_("Search for a Place"), "<Control>l <Control>f"),
                Keys(n_("Refresh"), "F5 <Control>r"),
                Keys(n_("Keyboard Shortcuts"), "<Control>question"),
                Keys(n_("Close Window"), "<Control>w"),
                Keys(n_("Quit"), "<Control>q"),
            ],
        ),
        (
            n_("Hourly Forecast"),
            &[
                Keys(n_("Previous or Next Hour"), "Left Right"),
                Keys(n_("First or Last Hour"), "Home End"),
            ],
        ),
    ]
};

#[cfg(test)]
mod name_tests {
    #[test]
    fn hours_are_described_in_words() {
        assert_eq!(
            super::hour_description("15:00", "12°", "Partly cloudy", Some(40.0)),
            "15:00, 12°, Partly cloudy, 40% chance of precipitation"
        );
        assert_eq!(
            super::hour_description("Now", "3°", "Clear sky", Some(5.0)),
            "Now, 3°, Clear sky"
        );
    }

    #[test]
    fn days_are_described_in_words() {
        assert_eq!(
            super::day_description("Tomorrow", "Slight rain", Some(58.0), "1°", "8°"),
            "Tomorrow, Slight rain, 58% chance of precipitation, low 1°, high 8°"
        );
    }

    #[test]
    fn errors_are_explained_in_plain_words() {
        use super::friendly_error;
        assert!(
            friendly_error("Could not connect to the weather service: dns failed")
                .contains("internet connection")
        );
        assert!(friendly_error("The weather service returned HTTP 503").contains("problems"));
        assert!(
            friendly_error("Unexpected forecast data: missing field").contains("could not be read")
        );
        assert!(!friendly_error("Latitude must be in range").contains("Latitude"));
    }

    #[test]
    fn remove_names_its_place() {
        assert_eq!(super::remove_place_name("Berlin"), "Remove Berlin");
    }
}

#[cfg(test)]
mod shortcut_tests {
    #[test]
    fn every_accelerator_is_listed() {
        helix_app_common::shortcuts::assert_all_listed(super::SHORTCUT_GROUPS, super::ACCELS);
    }
}
