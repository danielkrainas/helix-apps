//! Window, map, search, place details, favorites and directions.

use crate::favorites::Favorites;
use crate::geo::{self, LatLon, LaunchTarget, Units};
use crate::nominatim::Place;
use crate::osrm::{Mode, Route};
use crate::service::{self, Endpoint, Endpoints, Job, Reply, Service};
use crate::settings::{self, gnome, Services, SystemConfig, UserSettings};
use crate::tiles::{self, TileSource};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use shumate::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

const APP_ID: &str = "org.helix.Maps";
const APP_NAME: &str = "Maps";
const COMMENTS: &str = "Find places around the world";
const SCHEMA: &str = "org.gnome.Maps";
const DEFAULT_MODE: &str = "pedestrian";

pub fn run(target: Option<LaunchTarget>) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(&app, APP_NAME, COMMENTS);
    app.connect_startup(|_| load_css());
    let target = RefCell::new(target);
    app.connect_activate(move |app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        let ui = Ui::build(app);
        let holder = RefCell::new(Some(ui.clone()));
        ui.0.window.connect_destroy(move |_| {
            holder.borrow_mut().take();
        });
        if let Some(t) = target.borrow_mut().take() {
            ui.launch(t);
        }
        ui.0.window.present();
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".maps-location-dot { min-width: 16px; min-height: 16px; border-radius: 50%;
             background: @accent_bg_color; border: 3px solid white;
             box-shadow: 0 0 0 6px alpha(@accent_bg_color, 0.25); }
         .maps-start-dot { min-width: 12px; min-height: 12px; border-radius: 50%;
             background: @success_color; border: 3px solid white; }
         .maps-pin { color: @destructive_color; -gtk-icon-shadow: 0 1px 2px alpha(black, 0.5); }
         .maps-sidebar { padding: 12px; }
         .maps-coords { font-family: monospace; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

struct PlacePage {
    title: gtk::Label,
    category: gtk::Label,
    address: gtk::Label,
    coords: gtk::Label,
    dms: gtk::Label,
    extras: gtk::ListBox,
    favorite: gtk::Button,
    spinner: gtk::Spinner,
}

struct RoutePage {
    from: gtk::Entry,
    to: gtk::Entry,
    modes: Vec<(Mode, gtk::ToggleButton)>,
    find: gtk::Button,
    spinner: gtk::Spinner,
    note: gtk::Label,
    summary: gtk::Label,
    steps: gtk::ListBox,
}

pub struct Inner {
    window: adw::ApplicationWindow,
    /// `org.gnome.Maps` keys (in memory when the schema is not installed).
    prefs: Prefs,
    /// Helix-only settings and the user's endpoint overrides.
    user: RefCell<UserSettings>,
    settings_path: PathBuf,
    /// System-wide endpoint configuration (`$XDG_CONFIG_DIRS`).
    system: SystemConfig,
    /// Effective endpoints: defaults < system < user.
    services: RefCell<Services>,
    favorites: RefCell<Favorites>,
    simple_map: shumate::SimpleMap,
    map: shumate::Map,
    viewport: shumate::Viewport,
    pin_layer: shumate::MarkerLayer,
    location_layer: shumate::MarkerLayer,
    route_layer: shumate::PathLayer,
    route_markers: shumate::MarkerLayer,
    toasts: adw::ToastOverlay,
    split: adw::OverlaySplitView,
    stack: gtk::Stack,
    search: gtk::SearchEntry,
    results_popover: gtk::Popover,
    results_list: gtk::ListBox,
    results: RefCell<Vec<Place>>,
    favorites_list: gtk::ListBox,
    place: RefCell<Option<Place>>,
    place_page: PlacePage,
    route_page: RoutePage,
    from_place: RefCell<Option<Place>>,
    to_place: RefCell<Option<Place>>,
    route: RefCell<Option<Route>>,
    setting_entry: Cell<bool>,
    service: Service,
    next_id: Cell<u64>,
    search_id: Cell<u64>,
    reverse_id: Cell<u64>,
    route_id: Cell<u64>,
    locate_button: gtk::Button,
    locating: Cell<bool>,
    context_point: Cell<Option<LatLon>>,
    context_menu: gtk::PopoverMenu,
    save_pending: Cell<bool>,
}

#[derive(Clone)]
pub struct Ui(Rc<Inner>);

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .build()
        .labelled(helix_app_common::a11y::name_from_tooltip(tooltip))
}

/// An icon-only button that acts as a toggle: exposed as a toggle button
/// with a fixed name, its pressed state set by [`set_pressed`].
fn icon_toggle(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .accessible_role(gtk::AccessibleRole::ToggleButton)
        .build()
        .labelled(helix_app_common::a11y::name_from_tooltip(tooltip));
    set_pressed(&b, false);
    b
}

fn set_pressed(button: &gtk::Button, pressed: bool) {
    button.update_state(&[gtk::accessible::State::Pressed(if pressed {
        gtk::AccessibleTristate::True
    } else {
        gtk::AccessibleTristate::False
    })]);
}

/// Accessible name of a place detail's link button, saying what it opens:
/// "Open Website", "Open Wikipedia Article", "Call +49 30 1234".
fn link_name(label: &str, value: &str) -> String {
    match label {
        "Website" => "Open Website".into(),
        "Wikipedia" => "Open Wikipedia Article".into(),
        "Phone" => format!("Call {}", value.split(';').next().unwrap_or(value).trim()),
        other => format!("Open {other}"),
    }
}

fn dim_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build()
}

/// Wikipedia tag (`lang:Title`) to URL.
fn wikipedia_url(tag: &str) -> Option<String> {
    let (lang, title) = tag.split_once(':')?;
    if lang.is_empty()
        || title.is_empty()
        || !lang.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
    {
        return None;
    }
    Some(format!(
        "https://{lang}.wikipedia.org/wiki/{}",
        geo::percent_encode(&title.replace(' ', "_"))
    ))
}

fn link_for(label: &str, value: &str) -> Option<String> {
    match label {
        "Website" if settings::is_http_url(value) => Some(value.to_string()),
        "Website" => Some(format!("https://{value}")),
        "Wikipedia" => wikipedia_url(value),
        "Phone" => Some(format!(
            "tel:{}",
            value.split(';').next().unwrap_or(value).replace(' ', "")
        )),
        _ => None,
    }
}

impl Ui {
    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    fn up(w: &Weak<Inner>) -> Option<Ui> {
        w.upgrade().map(Ui)
    }

    fn build(app: &adw::Application) -> Ui {
        let settings_path = helix_app_common::config_dir("helix-maps").join("settings.json");
        let user = UserSettings::load(&settings_path);
        let system = SystemConfig::from_env();
        let services = user.services(&system);
        let prefs = Prefs::new(SCHEMA);
        let favorites =
            Favorites::load(&helix_app_common::data_dir("helix-maps").join("favorites.json"));

        let (width, height) = gnome::window_size(&prefs.get::<Vec<i32>>("window-size", Vec::new()));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(width)
            .default_height(height)
            .width_request(360)
            .height_request(300)
            .build();
        if prefs.get("window-maximized", true) {
            window.maximize();
        }

        // --- Map -----------------------------------------------------------
        let simple_map = shumate::SimpleMap::builder()
            .vexpand(true)
            .hexpand(true)
            .build();
        // libshumate's zoom buttons only carry tooltips.
        helix_app_common::a11y::name_buttons_from_tooltips(&simple_map);
        let map = simple_map.map().expect("simple map has a map");
        let viewport = simple_map.viewport().expect("simple map has a viewport");
        let route_layer = shumate::PathLayer::builder()
            .viewport(&viewport)
            .stroke_width(6.0)
            .stroke_color(&gdk::RGBA::new(0.21, 0.52, 0.89, 0.9))
            .outline_width(1.5)
            .outline_color(&gdk::RGBA::new(1.0, 1.0, 1.0, 0.9))
            .build();
        let route_markers = shumate::MarkerLayer::new(&viewport);
        let location_layer = shumate::MarkerLayer::new(&viewport);
        let pin_layer = shumate::MarkerLayer::new(&viewport);
        simple_map.add_overlay_layer(&route_layer);
        simple_map.add_overlay_layer(&route_markers);
        simple_map.add_overlay_layer(&location_layer);
        simple_map.add_overlay_layer(&pin_layer);

        let map_overlay = gtk::Overlay::new();
        map_overlay.set_child(Some(&simple_map));

        // Right-click / long-press context menu.
        let menu = gio::Menu::new();
        menu.append(Some("What's Here?"), Some("win.whats-here"));
        menu.append(Some("Directions From Here"), Some("win.route-from-here"));
        menu.append(Some("Directions To Here"), Some("win.route-to-here"));
        menu.append(Some("Copy Location"), Some("win.copy-here"));
        let context_menu = gtk::PopoverMenu::from_model(Some(&menu));
        context_menu.set_parent(&map_overlay);
        context_menu.set_has_arrow(false);
        context_menu.set_halign(gtk::Align::Start);

        // --- Header ----------------------------------------------------------
        let header = adw::HeaderBar::new();
        let locate_button = icon_button("find-location-symbolic", "Current Location (Ctrl+L)");
        locate_button.set_action_name(Some("win.locate"));
        header.pack_start(&locate_button);

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search for places or coordinates")
            .hexpand(true)
            .build()
            .labelled("Search for Places");
        search.set_key_capture_widget(None::<&gtk::Widget>);
        let clamp = adw::Clamp::builder()
            .maximum_size(480)
            .child(&search)
            .build();
        header.set_title_widget(Some(&clamp));

        let results_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        results_list.add_css_class("navigation-sidebar");
        let results_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .min_content_width(360)
            .child(&results_list)
            .build();
        let results_popover = gtk::Popover::builder()
            .child(&results_scroll)
            .position(gtk::PositionType::Bottom)
            .has_arrow(false)
            .autohide(true)
            .build();
        results_popover.set_parent(&search);

        let menu_model = gio::Menu::new();
        menu_model.append(Some("_Preferences"), Some("win.preferences"));
        menu_model.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
        menu_model.append(Some("_About Maps"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu_model)
            .primary(true)
            .tooltip_text("Main Menu")
            .build()
            .labelled("Main Menu");
        header.pack_end(&menu_button);

        let favorites_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        favorites_list.add_css_class("navigation-sidebar");
        let fav_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .min_content_width(280)
            .child(&favorites_list)
            .build();
        let fav_popover = gtk::Popover::builder().child(&fav_scroll).build();
        let fav_button = gtk::MenuButton::builder()
            .icon_name("starred-symbolic")
            .popover(&fav_popover)
            .tooltip_text("Favorites")
            .build()
            .labelled("Favorites");
        header.pack_end(&fav_button);
        let directions_button = icon_toggle("go-jump-symbolic", "Directions (Ctrl+D)");
        directions_button.set_action_name(Some("win.directions"));
        header.pack_end(&directions_button);

        // --- Sidebar ---------------------------------------------------------
        let (place_widget, place_page) = build_place_page();
        let (route_widget, route_page) = build_route_page(Mode::from_nick(
            &prefs.get_enum_nick("transportation-type", DEFAULT_MODE),
        ));
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&place_widget, Some("place"));
        stack.add_named(&route_widget, Some("route"));

        let split = adw::OverlaySplitView::builder()
            .sidebar(&stack)
            .content(&map_overlay)
            .show_sidebar(false)
            .min_sidebar_width(300.0)
            .max_sidebar_width(380.0)
            .build();
        // Directions is pressed while the sidebar shows the route page.
        {
            let sync = {
                let (b, split, stack) = (
                    directions_button.clone(),
                    split.downgrade(),
                    stack.downgrade(),
                );
                move || {
                    if let (Some(split), Some(stack)) = (split.upgrade(), stack.upgrade()) {
                        set_pressed(
                            &b,
                            split.shows_sidebar()
                                && stack.visible_child_name().as_deref() == Some("route"),
                        );
                    }
                }
            };
            let s = sync.clone();
            split.connect_show_sidebar_notify(move |_| s());
            stack.connect_visible_child_name_notify(move |_| sync());
        }

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 640sp").unwrap());
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let (reply_tx, reply_rx) = async_channel::unbounded::<Reply>();
        let service = Service::start(reply_tx);

        let ui = Ui(Rc::new(Inner {
            window,
            prefs,
            user: RefCell::new(user),
            settings_path,
            system,
            services: RefCell::new(services),
            favorites: RefCell::new(favorites),
            simple_map,
            map,
            viewport,
            pin_layer,
            location_layer,
            route_layer,
            route_markers,
            toasts,
            split,
            stack,
            search,
            results_popover,
            results_list,
            results: RefCell::default(),
            favorites_list,
            place: RefCell::default(),
            place_page,
            route_page,
            from_place: RefCell::default(),
            to_place: RefCell::default(),
            route: RefCell::default(),
            setting_entry: Cell::new(false),
            service,
            next_id: Cell::new(1),
            search_id: Cell::new(0),
            reverse_id: Cell::new(0),
            route_id: Cell::new(0),
            locate_button,
            locating: Cell::new(false),
            context_point: Cell::new(None),
            context_menu,
            save_pending: Cell::new(false),
        }));

        ui.apply_tile_source();
        {
            let p = &ui.0.prefs;
            let at = gnome::location(&p.get::<Vec<f64>>("last-viewed-location", vec![0.0, 0.0]));
            ui.0.viewport.set_location(at.lat, at.lon);
            let max_zoom = ui.0.services.borrow().tile_max_zoom;
            ui.0.viewport
                .set_zoom_level(gnome::zoom(p.get("zoom-level", 0), max_zoom));
            ui.0.viewport
                .set_rotation(gnome::rotation(p.get("rotation", 0.0)));
            ui.apply_show_scale();
        }
        ui.update_route_note();
        ui.refresh_favorites();
        ui.connect_signals(&map_overlay);
        ui.install_actions();

        let w = ui.weak();
        glib::spawn_future_local(async move {
            while let Ok(reply) = reply_rx.recv().await {
                let Some(ui) = Ui::up(&w) else { break };
                ui.handle_reply(reply);
            }
        });
        ui
    }

    fn launch(&self, target: LaunchTarget) {
        match target {
            LaunchTarget::Point { at, zoom, label } => {
                let name = label.unwrap_or_else(|| "Dropped Pin".to_string());
                self.select_place(Place::at(at, &name), false);
                self.0.viewport.set_location(at.lat, at.lon);
                self.0.viewport.set_zoom_level(zoom.unwrap_or(15.0));
            }
            LaunchTarget::Search(q) => {
                self.0.search.set_text(&q);
                self.run_search();
            }
        }
    }

    // --- Settings ------------------------------------------------------------

    fn save_settings(&self) {
        if let Err(e) = self.0.user.borrow().save(&self.0.settings_path) {
            eprintln!("helix-maps: could not save settings: {e}");
        }
    }

    fn apply_show_scale(&self) {
        if let Some(scale) = self.0.simple_map.scale() {
            scale.set_visible(self.0.prefs.get("show-scale", true));
        }
    }

    fn max_search_results(&self) -> u32 {
        gnome::max_results(self.0.prefs.get("max-search-results", 20))
    }

    fn endpoints(&self) -> Endpoints {
        let s = self.0.services.borrow();
        Endpoints {
            geocoder: s.geocoder_url.clone(),
            router: s.router_url.clone(),
            language: service::accept_language(
                &glib::language_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
            ),
        }
    }

    fn units(&self) -> Units {
        Units::from_name(&self.0.user.borrow().units)
    }

    fn apply_tile_source(&self) {
        let s = self.0.services.borrow();
        let source = TileSource::new(&s.tile_url_template);
        let renderer = shumate::RasterRenderer::new_full(
            &format!("helix-{:016x}", settings::fnv1a(&s.tile_url_template)),
            "Map",
            &s.tile_attribution,
            &s.tile_attribution_url,
            0,
            s.tile_max_zoom,
            256,
            shumate::MapProjection::Mercator,
            &source,
        );
        self.0.simple_map.set_map_source(Some(&renderer));
        self.0.viewport.set_min_zoom_level(2);
        self.0.viewport.set_max_zoom_level(s.tile_max_zoom);
    }

    fn toast(&self, msg: &str) {
        self.0.toasts.add_toast(adw::Toast::new(msg));
    }

    fn next_id(&self) -> u64 {
        let id = self.0.next_id.get();
        self.0.next_id.set(id + 1);
        id
    }

    // --- Signals ---------------------------------------------------------------

    fn connect_signals(&self, map_overlay: &gtk::Overlay) {
        let i = &self.0;

        let w = self.weak();
        i.search.connect_activate(move |_| {
            if let Some(ui) = Ui::up(&w) {
                ui.run_search();
            }
        });
        let w = self.weak();
        i.search.connect_stop_search(move |e| {
            e.set_text("");
            if let Some(ui) = Ui::up(&w) {
                ui.0.results_popover.popdown();
            }
        });
        let w = self.weak();
        i.results_list.connect_row_activated(move |_, row| {
            let Some(ui) = Ui::up(&w) else { return };
            let place = ui.0.results.borrow().get(row.index() as usize).cloned();
            if let Some(p) = place {
                ui.0.results_popover.popdown();
                ui.select_place(p, true);
            }
        });
        // Down arrow moves from the entry into the result list.
        let key = gtk::EventControllerKey::new();
        let w = self.weak();
        key.connect_key_pressed(move |_, keyval, _, _| {
            let Some(ui) = Ui::up(&w) else {
                return glib::Propagation::Proceed;
            };
            if keyval == gdk::Key::Down && ui.0.results_popover.is_visible() {
                if let Some(row) = ui.0.results_list.row_at_index(0) {
                    row.grab_focus();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        i.search.add_controller(key);

        let w = self.weak();
        i.favorites_list.connect_row_activated(move |list, row| {
            let Some(ui) = Ui::up(&w) else { return };
            let place =
                ui.0.favorites
                    .borrow()
                    .places
                    .get(row.index() as usize)
                    .cloned();
            if let Some(p) = place {
                if let Some(pop) = list.ancestor(gtk::Popover::static_type()) {
                    pop.downcast::<gtk::Popover>().unwrap().popdown();
                }
                ui.select_place(p, true);
            }
        });

        // Context menu on right click and long press.
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let w = self.weak();
        click.connect_pressed(move |g, _, x, y| {
            if let Some(ui) = Ui::up(&w) {
                g.set_state(gtk::EventSequenceState::Claimed);
                ui.show_context_menu(x, y);
            }
        });
        map_overlay.add_controller(click);
        let long = gtk::GestureLongPress::builder().touch_only(true).build();
        let w = self.weak();
        long.connect_pressed(move |_, x, y| {
            if let Some(ui) = Ui::up(&w) {
                ui.show_context_menu(x, y);
            }
        });
        map_overlay.add_controller(long);

        // Place page buttons.
        let w = self.weak();
        i.place_page.favorite.connect_clicked(move |_| {
            if let Some(ui) = Ui::up(&w) {
                ui.toggle_favorite();
            }
        });

        // Directions page.
        for (entry, is_from) in [(&i.route_page.from, true), (&i.route_page.to, false)] {
            let w = self.weak();
            entry.connect_changed(move |_| {
                let Some(ui) = Ui::up(&w) else { return };
                if !ui.0.setting_entry.get() {
                    let slot = if is_from {
                        &ui.0.from_place
                    } else {
                        &ui.0.to_place
                    };
                    slot.borrow_mut().take();
                }
            });
            let w = self.weak();
            entry.connect_activate(move |_| {
                if let Some(ui) = Ui::up(&w) {
                    ui.find_route();
                }
            });
        }
        let w = self.weak();
        i.route_page.find.connect_clicked(move |_| {
            if let Some(ui) = Ui::up(&w) {
                ui.find_route();
            }
        });
        for (mode, button) in &i.route_page.modes {
            let w = self.weak();
            let mode = *mode;
            button.connect_toggled(move |b| {
                let Some(ui) = Ui::up(&w) else { return };
                if b.is_active() {
                    // `transit` is kept until the user picks another mode.
                    if Mode::from_nick(
                        &ui.0
                            .prefs
                            .get_enum_nick("transportation-type", DEFAULT_MODE),
                    ) != mode
                    {
                        ui.0.prefs.set_enum_nick("transportation-type", mode.nick());
                    }
                    ui.update_route_note();
                    if ui.0.route.borrow().is_some() {
                        ui.find_route();
                    }
                }
            });
        }
        let w = self.weak();
        i.route_page.steps.connect_row_activated(move |_, row| {
            let Some(ui) = Ui::up(&w) else { return };
            let step =
                ui.0.route
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.steps.get(row.index() as usize).cloned());
            if let Some(s) = step {
                ui.0.map.go_to_full(s.location.lat, s.location.lon, 17.0);
            }
        });

        let w = self.weak();
        i.window.connect_close_request(move |win| {
            if let Some(ui) = Ui::up(&w) {
                let (width, height) = win.default_size();
                let maximized = win.is_maximized();
                ui.0.prefs.set("window-maximized", maximized);
                if !maximized && width > 0 && height > 0 {
                    ui.0.prefs.set("window-size", vec![width, height]);
                }
                ui.save_view_state();
            }
            glib::Propagation::Proceed
        });

        // Persist the viewed area a couple of seconds after it stops moving,
        // so it survives the process being killed.
        for prop in ["latitude", "longitude", "zoom-level", "rotation"] {
            let w = self.weak();
            i.viewport.connect_notify_local(Some(prop), move |_, _| {
                let Some(ui) = Ui::up(&w) else { return };
                if ui.0.save_pending.replace(true) {
                    return;
                }
                let w = ui.weak();
                glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                    if let Some(ui) = Ui::up(&w) {
                        ui.0.save_pending.set(false);
                        ui.save_view_state();
                    }
                });
            });
        }
    }

    fn save_view_state(&self) {
        let v = &self.0.viewport;
        let p = &self.0.prefs;
        let at = gnome::location(&[v.latitude(), v.longitude()]);
        p.set("last-viewed-location", vec![at.lat, at.lon]);
        // The GNOME key is an integer zoom level.
        p.set("zoom-level", v.zoom_level().round() as i32);
        p.set("rotation", gnome::rotation(v.rotation()));
    }

    fn install_actions(&self) {
        let win = &self.0.window;
        let add = |name: &str, f: Box<dyn Fn(&Ui)>| {
            let action = gio::SimpleAction::new(name, None);
            let w = self.weak();
            action.connect_activate(move |_, _| {
                if let Some(ui) = Ui::up(&w) {
                    f(&ui);
                }
            });
            win.add_action(&action);
        };
        add(
            "search",
            Box::new(|ui| {
                ui.0.search.grab_focus();
            }),
        );
        add("locate", Box::new(|ui| ui.locate()));
        add("directions", Box::new(|ui| ui.toggle_directions()));
        add("zoom-in", Box::new(|ui| ui.0.map.zoom_in()));
        add("zoom-out", Box::new(|ui| ui.0.map.zoom_out()));
        add(
            "close-sidebar",
            Box::new(|ui| ui.0.split.set_show_sidebar(false)),
        );
        add("preferences", Box::new(|ui| ui.show_preferences()));
        add("shortcuts", Box::new(|ui| ui.show_shortcuts()));
        add(
            "whats-here",
            Box::new(|ui| {
                if let Some(p) = ui.0.context_point.get() {
                    ui.whats_here(p);
                }
            }),
        );
        add(
            "route-from-here",
            Box::new(|ui| {
                if let Some(p) = ui.0.context_point.get() {
                    ui.set_endpoint(true, Place::at(p, &geo::format_coordinates(p)));
                }
            }),
        );
        add(
            "route-to-here",
            Box::new(|ui| {
                if let Some(p) = ui.0.context_point.get() {
                    ui.set_endpoint(false, Place::at(p, &geo::format_coordinates(p)));
                }
            }),
        );
        add(
            "copy-here",
            Box::new(|ui| {
                if let Some(p) = ui.0.context_point.get() {
                    ui.copy(&geo::format_coordinates(p), "Location copied");
                }
            }),
        );
        add(
            "place-directions",
            Box::new(|ui| {
                if let Some(p) = ui.0.place.borrow().clone() {
                    ui.set_endpoint(false, p);
                }
            }),
        );
        add(
            "place-copy",
            Box::new(|ui| {
                if let Some(p) = ui.0.place.borrow().as_ref() {
                    ui.copy(&geo::format_coordinates(p.location()), "Location copied");
                }
            }),
        );
        add(
            "place-share",
            Box::new(|ui| {
                if let Some(p) = ui.0.place.borrow().as_ref() {
                    ui.copy(&geo::geo_uri(p.location()), "Location link copied");
                }
            }),
        );
        add(
            "place-open",
            Box::new(|ui| {
                if let Some(p) = ui.0.place.borrow().as_ref() {
                    ui.open_uri(&p.osm_url());
                }
            }),
        );
        add(
            "place-close",
            Box::new(|ui| ui.0.split.set_show_sidebar(false)),
        );
        add("route-swap", Box::new(|ui| ui.swap_endpoints()));
        add(
            "route-my-location",
            Box::new(|ui| {
                let loc = ui.0.location_layer.markers().first().map(|m| LatLon {
                    lat: m.latitude(),
                    lon: m.longitude(),
                });
                match loc {
                    Some(p) => ui.set_endpoint(true, Place::at(p, "My Location")),
                    None => ui.locate_then_route(),
                }
            }),
        );

        let app = win.application().expect("window has an application");
        app.set_accels_for_action("win.search", &["<Control>f"]);
        app.set_accels_for_action("win.locate", &["<Control>l"]);
        app.set_accels_for_action("win.directions", &["<Control>d"]);
        app.set_accels_for_action(
            "win.zoom-in",
            &["<Control>equal", "<Control>plus", "<Control>KP_Add"],
        );
        app.set_accels_for_action("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]);
        app.set_accels_for_action("win.preferences", &["<Control>comma"]);
        app.set_accels_for_action("win.shortcuts", &["<Control>question"]);

        let esc = gtk::ShortcutController::new();
        esc.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::NamedAction::new("win.close-sidebar")),
        ));
        win.add_controller(esc);
    }

    // --- Map helpers -------------------------------------------------------------

    fn visible_bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let map = &self.0.map;
        let (w, h) = (map.width() as f64, map.height() as f64);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let (n, west) = self.0.viewport.widget_coords_to_location(map, 0.0, 0.0);
        let (s, east) = self.0.viewport.widget_coords_to_location(map, w, h);
        Some((west, s, east, n))
    }

    fn fly_to_bounds(&self, south: f64, north: f64, west: f64, east: f64) {
        let map = &self.0.map;
        let (w, h) = (map.width().max(400) as f64, map.height().max(300) as f64);
        // Leave a margin around the box.
        let z = geo::zoom_for_bounds(south, north, west, east, w * 0.8, h * 0.8)
            .min(self.0.services.borrow().tile_max_zoom as f64);
        map.go_to_full((south + north) / 2.0, (west + east) / 2.0, z);
    }

    fn show_context_menu(&self, x: f64, y: f64) {
        let overlay = self.0.context_menu.parent().expect("menu parented");
        let pt = overlay
            .compute_point(&self.0.map, &gtk::graphene::Point::new(x as f32, y as f32))
            .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
        let (lat, lon) =
            self.0
                .viewport
                .widget_coords_to_location(&self.0.map, pt.x() as f64, pt.y() as f64);
        let Some(p) = LatLon::new(lat, lon.clamp(-180.0, 180.0)) else {
            return;
        };
        self.0.context_point.set(Some(p));
        self.0
            .context_menu
            .set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.0.context_menu.popup();
    }

    fn copy(&self, text: &str, toast: &str) {
        self.0.window.clipboard().set_text(text);
        self.toast(toast);
    }

    fn open_uri(&self, uri: &str) {
        let w = self.weak();
        gtk::UriLauncher::new(uri).launch(
            Some(&self.0.window),
            None::<&gio::Cancellable>,
            move |r| {
                if let (Err(e), Some(ui)) = (r, Ui::up(&w)) {
                    ui.toast(&format!("Could not open link: {e}"));
                }
            },
        );
    }

    fn set_pin(&self, p: LatLon) {
        self.0.pin_layer.remove_all();
        let image = gtk::Image::builder()
            .icon_name("mark-location-symbolic")
            .pixel_size(36)
            .margin_bottom(36)
            .css_classes(["maps-pin"])
            .build();
        let marker = shumate::Marker::builder()
            .child(&image)
            .latitude(p.lat)
            .longitude(p.lon)
            .build();
        self.0.pin_layer.add_marker(&marker);
    }

    // --- Search ----------------------------------------------------------------

    fn run_search(&self) {
        let text = self.0.search.text().trim().to_string();
        if text.is_empty() {
            return;
        }
        // Coordinates and geo: URIs need no network round trip.
        let direct = geo::parse_coordinates(&text)
            .map(|p| (p, None))
            .or_else(|| match geo::parse_launch_arg(&text) {
                Some(LaunchTarget::Point { at, label, .. }) => Some((at, label)),
                _ => None,
            });
        if let Some((p, label)) = direct {
            let name = label.unwrap_or_else(|| geo::format_coordinates(p));
            self.select_place(Place::at(p, &name), false);
            self.0.map.go_to_full(p.lat, p.lon, 15.0);
            return;
        }
        let query = match geo::parse_launch_arg(&text) {
            Some(LaunchTarget::Search(q)) => q,
            _ => text,
        };
        let id = self.next_id();
        self.0.search_id.set(id);
        self.show_results_message("Searching…");
        let limit = self.max_search_results();
        self.0.service.submit(
            Job::Search {
                id,
                query,
                limit,
                viewbox: self.visible_bounds(),
            },
            self.endpoints(),
        );
    }

    fn show_results_message(&self, msg: &str) {
        self.0.results.borrow_mut().clear();
        self.0.results_list.remove_all();
        let label = gtk::Label::builder()
            .label(msg)
            .margin_top(12)
            .margin_bottom(12)
            .css_classes(["dim-label"])
            .build();
        let row = gtk::ListBoxRow::builder()
            .child(&label)
            .activatable(false)
            .selectable(false)
            .build();
        self.0.results_list.append(&row);
        self.0.results_popover.popup();
    }

    fn show_results(&self, places: Vec<Place>) {
        if places.is_empty() {
            self.show_results_message("No results found");
            helix_app_common::announce(
                &self.0.window,
                "No results found",
                helix_app_common::Urgency::Polite,
            );
            return;
        }
        let found = match places.len() {
            1 => "1 place found".to_string(),
            n => format!("{n} places found"),
        };
        helix_app_common::announce(&self.0.window, &found, helix_app_common::Urgency::Polite);
        self.0.results_list.remove_all();
        for p in &places {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&p.name).as_str())
                .subtitle(glib::markup_escape_text(&p.short_address()).as_str())
                .activatable(true)
                .build();
            if !p.category.is_empty() {
                row.add_suffix(
                    &gtk::Label::builder()
                        .label(&p.category)
                        .css_classes(["dim-label", "caption"])
                        .build(),
                );
            }
            self.0.results_list.append(&row);
        }
        *self.0.results.borrow_mut() = places;
        self.0.results_popover.popup();
    }

    // --- Place details -------------------------------------------------------------

    fn select_place(&self, place: Place, fly: bool) {
        let p = place.location();
        self.set_pin(p);
        if fly {
            match place.bbox {
                Some([s, n, w, e]) if (n - s).abs() > 1e-4 || (e - w).abs() > 1e-4 => {
                    self.fly_to_bounds(s, n, w, e)
                }
                _ => {
                    let z = self.0.viewport.zoom_level().max(16.0);
                    self.0.map.go_to_full(p.lat, p.lon, z);
                }
            }
        }
        *self.0.place.borrow_mut() = Some(place);
        self.update_place_page(false);
        self.0.stack.set_visible_child_name("place");
        self.0.split.set_show_sidebar(true);
    }

    fn update_place_page(&self, loading: bool) {
        let page = &self.0.place_page;
        let place = self.0.place.borrow();
        let Some(place) = place.as_ref() else { return };
        page.spinner.set_visible(loading);
        page.spinner.set_spinning(loading);
        page.title.set_label(if place.name.is_empty() {
            "Unnamed Place"
        } else {
            &place.name
        });
        page.category.set_label(&place.category);
        page.category.set_visible(!place.category.is_empty());
        page.address.set_label(&place.address);
        page.address
            .set_visible(!place.address.is_empty() && place.address != place.name);
        page.coords
            .set_label(&geo::format_coordinates(place.location()));
        page.dms.set_label(&geo::format_dms(place.location()));
        page.extras.remove_all();
        for (label, value) in &place.extras {
            let row = adw::ActionRow::builder()
                .title(label.as_str())
                .subtitle(glib::markup_escape_text(value).as_str())
                .subtitle_selectable(true)
                .build();
            row.add_css_class("property");
            if let Some(url) = link_for(label, value) {
                let b = icon_button("web-browser-symbolic", &link_name(label, value));
                b.set_valign(gtk::Align::Center);
                b.add_css_class("flat");
                let w = self.weak();
                b.connect_clicked(move |_| {
                    if let Some(ui) = Ui::up(&w) {
                        ui.open_uri(&url);
                    }
                });
                row.add_suffix(&b);
            }
            page.extras.append(&row);
        }
        page.extras.set_visible(!place.extras.is_empty());
        let fav = self.0.favorites.borrow().contains(place);
        page.favorite.set_icon_name(if fav {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        });
        page.favorite.set_tooltip_text(Some(if fav {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        }));
        // The name stays "Favorite"; the pressed state says whether it is.
        set_pressed(&page.favorite, fav);
    }

    fn whats_here(&self, p: LatLon) {
        self.select_place(Place::at(p, "Dropped Pin"), false);
        self.update_place_page(true);
        let id = self.next_id();
        self.0.reverse_id.set(id);
        let zoom = self.0.viewport.zoom_level().round().clamp(3.0, 18.0) as u32;
        self.0
            .service
            .submit(Job::Reverse { id, at: p, zoom }, self.endpoints());
    }

    fn toggle_favorite(&self) {
        let place = self.0.place.borrow().clone();
        let Some(place) = place else { return };
        let now = self.0.favorites.borrow_mut().toggle(&place);
        self.toast(if now {
            "Added to favorites"
        } else {
            "Removed from favorites"
        });
        self.update_place_page(false);
        self.refresh_favorites();
    }

    fn refresh_favorites(&self) {
        let list = &self.0.favorites_list;
        list.remove_all();
        let favs = self.0.favorites.borrow();
        if favs.places.is_empty() {
            let label = gtk::Label::builder()
                .label("No favorites yet.\nStar a place to add it here.")
                .justify(gtk::Justification::Center)
                .margin_top(12)
                .margin_bottom(12)
                .css_classes(["dim-label"])
                .build();
            list.append(
                &gtk::ListBoxRow::builder()
                    .child(&label)
                    .activatable(false)
                    .selectable(false)
                    .build(),
            );
            return;
        }
        for p in &favs.places {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&p.name).as_str())
                .subtitle(glib::markup_escape_text(&p.short_address()).as_str())
                .activatable(true)
                .build();
            list.append(&row);
        }
    }

    // --- Current location ------------------------------------------------------------

    fn locate_inner(&self, then_route: bool) {
        if self.0.locating.replace(true) {
            return;
        }
        self.0.locate_button.set_sensitive(false);
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(crate::location::locate());
        });
        let w = self.weak();
        glib::spawn_future_local(async move {
            let result = rx
                .recv()
                .await
                .unwrap_or_else(|_| Err("Location unavailable".into()));
            let Some(ui) = Ui::up(&w) else { return };
            ui.0.locating.set(false);
            ui.0.locate_button.set_sensitive(true);
            match result {
                Ok(fix) => {
                    let Some(p) = LatLon::new(fix.lat, fix.lon) else {
                        ui.toast("Location unavailable");
                        return;
                    };
                    ui.0.location_layer.remove_all();
                    let dot = gtk::Box::builder()
                        .css_classes(["maps-location-dot"])
                        .build();
                    if fix.accuracy.is_finite() {
                        dot.set_tooltip_text(Some(&format!(
                            "Your location (accuracy {})",
                            geo::format_distance(fix.accuracy, ui.units())
                        )));
                    }
                    let marker = shumate::Marker::builder()
                        .child(&dot)
                        .latitude(p.lat)
                        .longitude(p.lon)
                        .build();
                    ui.0.location_layer.add_marker(&marker);
                    if then_route {
                        ui.set_endpoint(true, Place::at(p, "My Location"));
                    } else {
                        ui.0.map
                            .go_to_full(p.lat, p.lon, geo::zoom_for_accuracy(fix.accuracy));
                    }
                }
                Err(e) => ui.toast(&e),
            }
        });
    }

    fn locate(&self) {
        self.locate_inner(false);
    }

    fn locate_then_route(&self) {
        self.locate_inner(true);
    }

    // --- Directions --------------------------------------------------------------

    fn toggle_directions(&self) {
        let i = &self.0;
        if i.split.shows_sidebar() && i.stack.visible_child_name().as_deref() == Some("route") {
            i.split.set_show_sidebar(false);
        } else {
            i.stack.set_visible_child_name("route");
            i.split.set_show_sidebar(true);
            i.route_page.from.grab_focus();
        }
    }

    fn set_entry(&self, entry: &gtk::Entry, text: &str) {
        self.0.setting_entry.set(true);
        entry.set_text(text);
        self.0.setting_entry.set(false);
    }

    fn set_endpoint(&self, from: bool, place: Place) {
        let page = &self.0.route_page;
        let entry = if from { &page.from } else { &page.to };
        self.set_entry(entry, &place.name);
        *(if from {
            &self.0.from_place
        } else {
            &self.0.to_place
        })
        .borrow_mut() = Some(place);
        self.0.stack.set_visible_child_name("route");
        self.0.split.set_show_sidebar(true);
        let other_empty = if from {
            page.to.text().is_empty()
        } else {
            page.from.text().is_empty()
        };
        if other_empty {
            (if from { &page.to } else { &page.from }).grab_focus();
        } else {
            self.find_route();
        }
    }

    fn swap_endpoints(&self) {
        let page = &self.0.route_page;
        let (a, b) = (page.from.text().to_string(), page.to.text().to_string());
        self.set_entry(&page.from, &b);
        self.set_entry(&page.to, &a);
        self.0.from_place.swap(&self.0.to_place);
        if !a.is_empty() && !b.is_empty() {
            self.find_route();
        }
    }

    fn current_mode(&self) -> Mode {
        Mode::from_nick(
            &self
                .0
                .prefs
                .get_enum_nick("transportation-type", DEFAULT_MODE),
        )
    }

    fn update_route_note(&self) {
        let demo = self.0.services.borrow().uses_demo_router();
        let note = &self.0.route_page.note;
        note.set_visible(demo && self.current_mode() != Mode::Car);
        note.set_label("The public demo routing server may only provide car routes. Configure a routing server in Preferences for other modes.");
    }

    fn find_route(&self) {
        let page = &self.0.route_page;
        let endpoint = |entry: &gtk::Entry, slot: &RefCell<Option<Place>>| -> Option<Endpoint> {
            if let Some(p) = slot.borrow().clone() {
                return Some(Endpoint::Resolved(Box::new(p)));
            }
            let t = entry.text().trim().to_string();
            (!t.is_empty()).then_some(Endpoint::Text(t))
        };
        let (Some(from), Some(to)) = (
            endpoint(&page.from, &self.0.from_place),
            endpoint(&page.to, &self.0.to_place),
        ) else {
            self.toast("Enter a start and a destination");
            return;
        };
        let id = self.next_id();
        self.0.route_id.set(id);
        page.spinner.set_visible(true);
        page.spinner.set_spinning(true);
        page.find.set_sensitive(false);
        self.0.service.submit(
            Job::Route {
                id,
                mode: self.current_mode(),
                from,
                to,
            },
            self.endpoints(),
        );
    }

    fn clear_route(&self) {
        self.0.route_layer.remove_all();
        self.0.route_markers.remove_all();
        self.0.route_page.steps.remove_all();
        self.0.route_page.summary.set_visible(false);
        self.0.route.borrow_mut().take();
    }

    fn show_route(&self, route: Route, from: Place, to: Place) {
        self.clear_route();
        let page = &self.0.route_page;
        // Fill resolved endpoints back into the form.
        if self.0.from_place.borrow().is_none() {
            *self.0.from_place.borrow_mut() = Some(from.clone());
        }
        if self.0.to_place.borrow().is_none() {
            *self.0.to_place.borrow_mut() = Some(to.clone());
        }
        for p in &route.path {
            self.0
                .route_layer
                .add_node(&shumate::Coordinate::new_full(p.lat, p.lon));
        }
        let start = gtk::Box::builder().css_classes(["maps-start-dot"]).build();
        self.0.route_markers.add_marker(
            &shumate::Marker::builder()
                .child(&start)
                .latitude(from.lat)
                .longitude(from.lon)
                .build(),
        );
        let pin = gtk::Image::builder()
            .icon_name("mark-location-symbolic")
            .pixel_size(32)
            .margin_bottom(32)
            .css_classes(["maps-pin"])
            .build();
        self.0.route_markers.add_marker(
            &shumate::Marker::builder()
                .child(&pin)
                .latitude(to.lat)
                .longitude(to.lon)
                .build(),
        );
        let units = self.units();
        page.summary.set_label(&format!(
            "{} · {}",
            geo::format_distance(route.distance, units),
            geo::format_duration(route.duration)
        ));
        page.summary.set_visible(true);
        helix_app_common::announce(
            &page.summary,
            &format!("Route found: {}", page.summary.label()),
            helix_app_common::Urgency::Polite,
        );
        for s in &route.steps {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&s.instruction).as_str())
                .activatable(true)
                .build();
            if s.distance > 0.0 {
                row.set_subtitle(&geo::format_distance(s.distance, units));
            }
            page.steps.append(&row);
        }
        if let Some((s, n, w, e)) = route.bounds() {
            self.fly_to_bounds(s, n, w, e);
        }
        *self.0.route.borrow_mut() = Some(route);
    }

    // --- Replies ---------------------------------------------------------------

    fn handle_reply(&self, reply: Reply) {
        match reply {
            Reply::Search { id, result } => {
                if id != self.0.search_id.get() {
                    return;
                }
                match result {
                    Ok(places) => self.show_results(places),
                    Err(e) => {
                        self.show_results_message(&e);
                        helix_app_common::announce(
                            &self.0.window,
                            &format!("Search failed: {e}"),
                            helix_app_common::Urgency::Assertive,
                        );
                    }
                }
            }
            Reply::Reverse { id, at, result } => {
                if id != self.0.reverse_id.get() {
                    return;
                }
                let current = self.0.place.borrow().as_ref().map(|p| p.location());
                if current != Some(at) {
                    return;
                }
                match result {
                    Ok(Some(mut place)) => {
                        // Keep the pin where the user pointed.
                        place.lat = at.lat;
                        place.lon = at.lon;
                        place.bbox = None;
                        *self.0.place.borrow_mut() = Some(place);
                        self.update_place_page(false);
                    }
                    Ok(None) => self.update_place_page(false),
                    Err(e) => {
                        self.update_place_page(false);
                        self.toast(&e);
                    }
                }
            }
            Reply::Route { id, result } => {
                if id != self.0.route_id.get() {
                    return;
                }
                let page = &self.0.route_page;
                page.spinner.set_visible(false);
                page.spinner.set_spinning(false);
                page.find.set_sensitive(true);
                match result {
                    Ok(r) => {
                        let (route, from, to) = *r;
                        self.show_route(route, from, to)
                    }
                    Err(e) => {
                        self.clear_route();
                        self.toast(&e);
                    }
                }
            }
        }
    }

    // --- Dialogs -----------------------------------------------------------------

    fn show_shortcuts(&self) {
        let rows = [
            ("Search", "Ctrl+F"),
            ("Go to current location", "Ctrl+L"),
            ("Directions", "Ctrl+D"),
            ("Zoom in", "Ctrl+="),
            ("Zoom out", "Ctrl+−"),
            ("Close sidebar", "Escape"),
            ("Preferences", "Ctrl+,"),
            ("Keyboard shortcuts", "Ctrl+?"),
            ("Close window", "Ctrl+W"),
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
        dialog.present(Some(&self.0.window));
    }

    fn show_preferences(&self) {
        let s = self.0.services.borrow().clone();
        let max_results = self.max_search_results();
        let imperial = self.0.user.borrow().units == "imperial";
        let dialog = adw::PreferencesDialog::builder()
            .title("Preferences")
            .build();
        let page = adw::PreferencesPage::new();

        let tiles = adw::PreferencesGroup::builder()
            .title("Map Tiles")
            .description(
                "The default OpenStreetMap tile server is run by volunteers and has a usage policy. \
                 Production use needs a commercial or self-hosted provider.",
            )
            .build();
        let tile_url = adw::EntryRow::builder()
            .title("Tile URL ({z}, {x}, {y}, optional {s})")
            .show_apply_button(true)
            .build();
        tile_url.set_text(&s.tile_url_template);
        let attribution = adw::EntryRow::builder()
            .title("Attribution")
            .show_apply_button(true)
            .build();
        attribution.set_text(&s.tile_attribution);
        let attribution_url = adw::EntryRow::builder()
            .title("Attribution link")
            .show_apply_button(true)
            .build();
        attribution_url.set_text(&s.tile_attribution_url);
        let max_zoom = adw::SpinRow::with_range(2.0, 22.0, 1.0);
        max_zoom.set_title("Maximum zoom level");
        max_zoom.set_value(s.tile_max_zoom as f64);
        tiles.add(&tile_url);
        tiles.add(&attribution);
        tiles.add(&attribution_url);
        tiles.add(&max_zoom);

        let services = adw::PreferencesGroup::builder()
            .title("Services")
            .description(
                "Search uses a Nominatim-compatible server, directions an OSRM-compatible server. \
                 The public defaults allow at most one request per second.",
            )
            .build();
        let geocoder = adw::EntryRow::builder()
            .title("Search server")
            .show_apply_button(true)
            .build();
        geocoder.set_text(&s.geocoder_url);
        let router = adw::EntryRow::builder()
            .title("Routing server")
            .show_apply_button(true)
            .build();
        router.set_text(&s.router_url);
        let results = adw::SpinRow::with_range(1.0, 50.0, 1.0);
        results.set_title("Maximum search results");
        results.set_value(max_results as f64);
        services.add(&geocoder);
        services.add(&router);
        services.add(&results);

        let general = adw::PreferencesGroup::builder().title("General").build();
        let units = adw::ComboRow::builder()
            .title("Units")
            .model(&gtk::StringList::new(&["Metric", "Imperial"]))
            .selected(if imperial { 1 } else { 0 })
            .build();
        general.add(&units);
        let show_scale = adw::SwitchRow::builder()
            .title("Show scale")
            .active(self.0.prefs.get("show-scale", true))
            .build();
        general.add(&show_scale);
        let clear = gtk::Button::builder()
            .label("Clear")
            .valign(gtk::Align::Center)
            .build();
        let clear_row = adw::ActionRow::builder()
            .title("Tile cache")
            .subtitle("Downloaded map tiles are kept for 7 days")
            .build();
        clear_row.add_suffix(&clear);
        general.add(&clear_row);
        let reset = gtk::Button::builder()
            .label("Reset")
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        let files = self.0.system.files();
        let reset_row = adw::ActionRow::builder()
            .title("Reset service settings to defaults")
            .subtitle(if files.is_empty() {
                "Built-in defaults; administrators can set them in /etc/xdg/helix-maps/endpoints.toml".to_string()
            } else {
                format!(
                    "System defaults from {}",
                    files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
                )
            })
            .build();
        reset_row.add_suffix(&reset);
        general.add(&reset_row);

        page.add(&tiles);
        page.add(&services);
        page.add(&general);
        dialog.add(&page);

        // Applies an endpoint edit to the effective configuration, stores
        // the difference to the system configuration as the user's
        // overrides, saves and refreshes dependents.
        let apply = {
            let w = self.weak();
            let dialog = dialog.clone();
            move |f: &dyn Fn(&mut Services) -> Result<bool, &'static str>| {
                let Some(ui) = Ui::up(&w) else { return };
                let mut wanted = ui.0.services.borrow().clone();
                match f(&mut wanted) {
                    Ok(tiles_changed) => {
                        let inherited = ui.0.system.services();
                        let effective = {
                            let mut user = ui.0.user.borrow_mut();
                            user.endpoints.set_from(&wanted, &inherited);
                            user.services(&ui.0.system)
                        };
                        *ui.0.services.borrow_mut() = effective;
                        ui.save_settings();
                        if tiles_changed {
                            ui.apply_tile_source();
                        }
                        ui.update_route_note();
                    }
                    Err(msg) => dialog.add_toast(adw::Toast::new(msg)),
                }
            }
        };
        let apply = Rc::new(apply);

        let a = apply.clone();
        tile_url.connect_apply(move |r| {
            let t = r.text().trim().to_string();
            a(&move |s: &mut Services| {
                settings::validate_tile_template(&t)?;
                s.tile_url_template = t.clone();
                Ok(true)
            });
        });
        let a = apply.clone();
        attribution.connect_apply(move |r| {
            let t = r.text().trim().to_string();
            a(&move |s: &mut Services| {
                s.tile_attribution = t.clone();
                Ok(true)
            });
        });
        let a = apply.clone();
        attribution_url.connect_apply(move |r| {
            let t = r.text().trim().to_string();
            a(&move |s: &mut Services| {
                s.tile_attribution_url = t.clone();
                Ok(true)
            });
        });
        let a = apply.clone();
        max_zoom.connect_value_notify(move |r| {
            let v = r.value() as u32;
            a(&move |s: &mut Services| {
                s.tile_max_zoom = v;
                Ok(true)
            });
        });
        let a = apply.clone();
        geocoder.connect_apply(move |r| {
            let t = r.text().trim().to_string();
            a(&move |s: &mut Services| {
                s.geocoder_url = settings::normalize_base_url(&t)
                    .ok_or("The search server must be an http(s) URL")?;
                Ok(false)
            });
        });
        let a = apply.clone();
        router.connect_apply(move |r| {
            let t = r.text().trim().to_string();
            a(&move |s: &mut Services| {
                s.router_url = settings::normalize_base_url(&t)
                    .ok_or("The routing server must be an http(s) URL")?;
                Ok(false)
            });
        });
        let w = self.weak();
        results.connect_value_notify(move |r| {
            if let Some(ui) = Ui::up(&w) {
                ui.0.prefs.set("max-search-results", r.value() as i32);
            }
        });
        let w = self.weak();
        units.connect_selected_notify(move |r| {
            let Some(ui) = Ui::up(&w) else { return };
            let u = if r.selected() == 1 {
                Units::Imperial
            } else {
                Units::Metric
            };
            ui.0.user.borrow_mut().units = u.name().to_string();
            ui.save_settings();
        });
        let w = self.weak();
        show_scale.connect_active_notify(move |r| {
            if let Some(ui) = Ui::up(&w) {
                ui.0.prefs.set("show-scale", r.is_active());
                ui.apply_show_scale();
            }
        });
        let d = dialog.clone();
        clear.connect_clicked(move |b| {
            b.set_sensitive(false);
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(tiles::clear_cache());
            });
            let d = d.clone();
            let b = b.clone();
            glib::spawn_future_local(async move {
                let msg = match rx.recv().await {
                    Ok(Ok(())) => "Tile cache cleared".to_string(),
                    Ok(Err(e)) => format!("Could not clear the cache: {e}"),
                    Err(_) => "Could not clear the cache".to_string(),
                };
                b.set_sensitive(true);
                d.add_toast(adw::Toast::new(&msg));
            });
        });
        // Reset drops the user's endpoint overrides, so the system-wide
        // configuration (or the built-in defaults) applies again.
        let a = apply.clone();
        let system = self.0.system.services();
        reset.connect_clicked(move |_| {
            let d = system.clone();
            tile_url.set_text(&d.tile_url_template);
            attribution.set_text(&d.tile_attribution);
            attribution_url.set_text(&d.tile_attribution_url);
            geocoder.set_text(&d.geocoder_url);
            router.set_text(&d.router_url);
            max_zoom.set_value(d.tile_max_zoom as f64);
            a(&move |s: &mut Services| {
                *s = d.clone();
                Ok(true)
            });
        });
        dialog.present(Some(&self.0.window));
    }
}

fn build_place_page() -> (gtk::Widget, PlacePage) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .css_classes(["maps-sidebar"])
        .build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .selectable(true)
        .css_classes(["title-2"])
        .build();
    let spinner = gtk::Spinner::builder().visible(false).build();
    let close = icon_button("window-close-symbolic", "Close");
    close.add_css_class("flat");
    close.set_valign(gtk::Align::Start);
    close.set_action_name(Some("win.place-close"));
    top.append(&title);
    top.append(&spinner);
    top.append(&close);
    content.append(&top);

    let category = dim_label("");
    let address = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    content.append(&category);
    content.append(&address);

    let buttons = gtk::Box::builder()
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    let directions = gtk::Button::builder()
        .label("Directions")
        .action_name("win.place-directions")
        .css_classes(["suggested-action", "pill"])
        .build();
    buttons.append(&directions);
    let favorite = icon_toggle("non-starred-symbolic", "Add to Favorites");
    helix_app_common::set_accessible_label(&favorite, "Favorite");
    favorite.add_css_class("circular");
    buttons.append(&favorite);
    for (icon, tip, action) in [
        ("edit-copy-symbolic", "Copy Location", "win.place-copy"),
        ("send-to-symbolic", "Share Location Link", "win.place-share"),
        (
            "web-browser-symbolic",
            "Open in OpenStreetMap",
            "win.place-open",
        ),
    ] {
        let b = icon_button(icon, tip);
        b.add_css_class("circular");
        b.set_action_name(Some(action));
        buttons.append(&b);
    }
    content.append(&buttons);

    let coords = gtk::Label::builder()
        .xalign(0.0)
        .selectable(true)
        .css_classes(["maps-coords"])
        .build();
    let dms = dim_label("");
    content.append(&coords);
    content.append(&dms);

    let extras = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .margin_top(6)
        .build();
    content.append(&extras);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    (
        scroll.upcast(),
        PlacePage {
            title,
            category,
            address,
            coords,
            dms,
            extras,
            favorite,
            spinner,
        },
    )
}

fn build_route_page(current: Mode) -> (gtk::Widget, RoutePage) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .css_classes(["maps-sidebar"])
        .build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top.append(
        &gtk::Label::builder()
            .label("Directions")
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["title-2"])
            .build(),
    );
    let close = icon_button("window-close-symbolic", "Close");
    close.add_css_class("flat");
    close.set_action_name(Some("win.place-close"));
    top.append(&close);
    content.append(&top);

    let mode_box = gtk::Box::builder()
        .css_classes(["linked"])
        .homogeneous(true)
        .build();
    let mut modes = Vec::new();
    let mut group: Option<gtk::ToggleButton> = None;
    for m in Mode::ALL {
        let b = gtk::ToggleButton::builder()
            .label(m.label())
            .active(m == current)
            .build();
        if let Some(g) = &group {
            b.set_group(Some(g));
        } else {
            group = Some(b.clone());
        }
        mode_box.append(&b);
        modes.push((m, b));
    }
    content.append(&mode_box);

    let entries = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(6)
        .build();
    let from = gtk::Entry::builder()
        .placeholder_text("From")
        .hexpand(true)
        .build()
        .labelled("From");
    let to = gtk::Entry::builder()
        .placeholder_text("To")
        .hexpand(true)
        .build()
        .labelled("To");
    let my_loc = icon_button("find-location-symbolic", "Start at My Location");
    my_loc.set_action_name(Some("win.route-my-location"));
    let swap = icon_button(
        "object-flip-vertical-symbolic",
        "Swap Start and Destination",
    );
    swap.set_action_name(Some("win.route-swap"));
    entries.attach(&from, 0, 0, 1, 1);
    entries.attach(&my_loc, 1, 0, 1, 1);
    entries.attach(&to, 0, 1, 1, 1);
    entries.attach(&swap, 1, 1, 1, 1);
    content.append(&entries);

    let action_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let find = gtk::Button::builder()
        .label("Find Route")
        .css_classes(["suggested-action"])
        .hexpand(true)
        .build();
    let spinner = gtk::Spinner::builder().visible(false).build();
    action_box.append(&find);
    action_box.append(&spinner);
    content.append(&action_box);

    let note = dim_label("");
    note.add_css_class("caption");
    content.append(&note);

    let summary = gtk::Label::builder()
        .xalign(0.0)
        .visible(false)
        .css_classes(["title-4"])
        .build();
    content.append(&summary);
    let steps = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    content.append(&steps);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    (
        scroll.upcast(),
        RoutePage {
            from,
            to,
            modes,
            find,
            spinner,
            note,
            summary,
            steps,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_buttons_say_what_they_open() {
        assert_eq!(link_name("Website", "example.org"), "Open Website");
        assert_eq!(
            link_name("Wikipedia", "de:Brandenburger Tor"),
            "Open Wikipedia Article"
        );
        assert_eq!(
            link_name("Phone", "+49 30 1234;+49 30 5678"),
            "Call +49 30 1234"
        );
    }

    #[test]
    fn links() {
        assert_eq!(
            wikipedia_url("de:Brandenburger Tor").unwrap(),
            "https://de.wikipedia.org/wiki/Brandenburger_Tor"
        );
        assert_eq!(wikipedia_url("nolang"), None);
        assert_eq!(
            link_for("Website", "example.org").unwrap(),
            "https://example.org"
        );
        assert_eq!(link_for("Website", "http://x.org").unwrap(), "http://x.org");
        assert_eq!(
            link_for("Phone", "+49 30 1234;+49 30 5678").unwrap(),
            "tel:+49301234"
        );
        assert_eq!(link_for("Cuisine", "pizza"), None);
    }
}
