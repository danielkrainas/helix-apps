//! GTK user interface: window, views, player controller.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use crate::index::{self, ScanReport};
use crate::model::{self, Library, Track};
use crate::mpris::{self, Command, Mpris};
use crate::player::{self, Event, Player};
use crate::playlist::{self, Playlist, Stats};
use crate::queue::{Queue, Repeat, Rng};
use crate::tracker;
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;
use helix_app_common::trnf;

pub const APP_ID: &str = "org.helixdesktop.Music";
pub const APP_NAME: &str = n_("Music");
pub const DATA: &str = "helix-music";
const SCHEMA: &str = "org.gnome.Music";
const MAX_MONITORS: usize = 2000;

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(
        &app,
        tr(APP_NAME),
        tr("Play and organize your music collection"),
    );
    app.connect_startup(|app| {
        load_css();
        for (action, accels) in ACCELS {
            app.set_accels_for_action(action, accels);
        }
    });
    app.connect_activate(|app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        let ui = App::new(app);
        // Callbacks only hold weak references; the window owns the app state
        // until it is destroyed.
        let keep = RefCell::new(Some(ui.clone()));
        ui.window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });
        ui.window.present();
        ui.start_scan();
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

pub(crate) const ACCELS: &[(&str, &[&str])] = &[
    ("win.play-pause", &["<Control>space"]),
    ("win.previous", &["<Control>b"]),
    ("win.next", &["<Control>n"]),
    ("win.repeat", &["<Control>r"]),
    ("win.shuffle", &["<Control>s"]),
    ("win.search", &["<Control>f"]),
    ("win.view::albums", &["<Alt>1"]),
    ("win.view::artists", &["<Alt>2"]),
    ("win.view::songs", &["<Alt>3"]),
    ("win.view::playlists", &["<Alt>4"]),
    ("win.rescan", &["F5"]),
    ("win.preferences", &["<Control>comma"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

fn load_css() {
    helix_app_common::style::add_css(
        ".hm-cover { border-radius: 8px; }
         .hm-tile { padding: 6px; }
         .hm-playerbar { padding: 6px 12px; border-top: 1px solid alpha(currentColor, 0.15); }
         .hm-num { min-width: 2.5em; }
         .hm-disc { margin-top: 12px; }",
        ".hm-playerbar { border-top-color: alpha(currentColor, 0.6); }",
    );
}

pub fn toast(text: &str) -> adw::Toast {
    let t = adw::Toast::new(text);
    t.set_use_markup(false);
    t
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Helix-only persistent state (things GNOME keeps elsewhere or not at all).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppState {
    pub folders: Vec<PathBuf>,
    pub volume: f64,
    pub repeat_with_shuffle: String,
    /// `track` or `album`: which gain applies while GNOME's boolean
    /// `replaygain` key is on.
    pub replaygain_mode: String,
    /// Track-change notifications while the window is not focused.
    pub notifications: bool,
    /// GNOME Music's favourites and playlists were imported (done once).
    pub gnome_music_imported: bool,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            folders: Vec::new(),
            volume: 1.0,
            repeat_with_shuffle: "all".into(),
            replaygain_mode: "album".into(),
            notifications: true,
            gnome_music_imported: false,
        }
    }
}

const NOTIFICATION_ID: &str = "now-playing";

/// "Artist — Album" for notifications.
pub fn notification_body(t: &Track) -> String {
    format!("{} — {}", t.artist_name(), t.album_name())
}

pub fn music_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Music)
        .unwrap_or_else(|| glib::home_dir().join("Music"))
}

/// A short name for a folder in messages: "~/Music" for folders in the
/// home folder, the full path otherwise.
pub fn friendly_folder_name(dir: &std::path::Path, home: &std::path::Path) -> String {
    match dir.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => dir.display().to_string(),
    }
}

/// Registered track row widgets, updated when the playing track or a
/// favourite changes.
pub struct RowHandle {
    pub path: PathBuf,
    pub indicator: glib::WeakRef<gtk::Stack>,
    pub heart: glib::WeakRef<gtk::ToggleButton>,
}

pub struct PlayerBar {
    pub revealer: gtk::Revealer,
    /// The bar's layout: one row when wide, two when narrow.
    grid: gtk::Grid,
    info_box: gtk::Box,
    seek_box: gtk::Box,
    side_box: gtk::Box,
    pub cover: gtk::Image,
    pub title: gtk::Label,
    pub artist: gtk::Label,
    pub play: gtk::Button,
    pub prev: gtk::Button,
    pub next: gtk::Button,
    pub elapsed: gtk::Label,
    pub total: gtk::Label,
    pub scale: gtk::Scale,
    pub heart: gtk::ToggleButton,
    pub shuffle: gtk::ToggleButton,
    pub repeat: gtk::Button,
    pub volume: gtk::Scale,
}

pub struct App {
    pub app: adw::Application,
    pub window: adw::ApplicationWindow,
    /// Applied below [`NARROW_SP`]; views add their own setters.
    pub narrow: adw::Breakpoint,
    /// Album and playlist headers, laid out again when `narrow` changes.
    pub headers: RefCell<Vec<glib::WeakRef<gtk::Box>>>,
    pub prefs: Prefs,
    pub toasts: adw::ToastOverlay,
    pub nav: adw::NavigationView,
    pub stack: adw::ViewStack,
    pub main_stack: gtk::Stack,
    pub empty_page: adw::StatusPage,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub search_box: gtk::Box,
    pub albums_model: gio::ListStore,
    pub songs_model: gio::ListStore,
    pub songs_sorted: gtk::SortListModel,
    pub artists_list: gtk::ListBox,
    pub playlists_list: gtk::ListBox,
    pub bar: PlayerBar,

    pub lib: RefCell<Rc<Library>>,
    pub stats: RefCell<Stats>,
    pub playlists: RefCell<Vec<Playlist>>,
    pub queue: RefCell<Queue<PathBuf>>,
    pub player: Option<Player>,
    pub mpris: Option<Mpris>,
    pub state: RefCell<AppState>,
    pub textures: RefCell<HashMap<String, Option<gdk::Texture>>>,
    pub rows: RefCell<Vec<RowHandle>>,
    pub queue_view: RefCell<Option<glib::WeakRef<gtk::ListBox>>>,
    pub playlist_view: RefCell<Option<(glib::WeakRef<gtk::Box>, crate::pages::PlRef)>>,

    pub playing: Cell<bool>,
    pub scanning: Cell<bool>,
    pub scan_again: Cell<bool>,
    pub scanned_once: Cell<bool>,
    pub monitors: RefCell<Vec<gio::FileMonitor>>,
    pub rescan_timer: RefCell<Option<glib::SourceId>>,
    pub inhibit_cookie: Cell<u32>,
    /// Listening to Tracker's GraphUpdated signal.
    pub tracker_watching: Cell<bool>,
    /// Whether the last scan used the Tracker index.
    pub using_tracker: Cell<bool>,
    pub played_ms: Cell<u64>,
    pub counted: Cell<bool>,
    pub last_tick: Cell<Option<Instant>>,
    pub seeking: Cell<bool>,
    pub suppress_ui: Cell<bool>,
}

impl App {
    pub fn data_dir() -> PathBuf {
        helix_app_common::data_dir(DATA)
    }

    pub fn covers_dir() -> PathBuf {
        glib::user_cache_dir().join(DATA).join("covers")
    }

    fn playlists_dir() -> PathBuf {
        Self::data_dir().join("playlists")
    }

    fn state_file() -> PathBuf {
        helix_app_common::config_dir(DATA).join("state.json")
    }

    fn new(application: &adw::Application) -> Rc<App> {
        let prefs = Prefs::new(SCHEMA);
        let state: AppState = std::fs::read(Self::state_file())
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title(tr(APP_NAME))
            .build();
        window.set_default_size(1000, 700);
        window.set_width_request(360);
        window.set_height_request(294);
        {
            use helix_app_common::window_state::{persist, SizeKeys, Store};
            persist(
                &window,
                Store::settings(
                    &prefs,
                    SizeKeys::Pair("window-size"),
                    "window-maximized",
                    "helix-music",
                ),
            );
        }

        // Root page: header with view switcher, search bar, view stack.
        let stack = adw::ViewStack::new();
        let header = adw::HeaderBar::new();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        header.set_title_widget(Some(&switcher));
        let search_btn = gtk::ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text(tr("Search"))
            .build()
            .labelled(tr("Search"));
        header.pack_start(&search_btn);
        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some(tr("_Rescan Library")), Some("win.rescan"));
        s1.append(Some(tr("_Import Playlist…")), Some("win.import-playlist"));
        s1.append(
            Some(tr("Import Playlists from _Search Index")),
            Some("win.import-index-playlists"),
        );
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append(Some(tr("_Preferences")), Some("win.preferences"));
        s2.append(
            Some(helix_app_common::i18n::tr(
                helix_app_common::shortcuts::MENU_LABEL,
            )),
            Some("win.show-help-overlay"),
        );
        s2.append(Some(tr("_About Music")), Some("app.about"));
        menu.append_section(None, &s2);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text(tr("Main Menu"))
            .build()
            .labelled(tr("Main Menu"));
        header.pack_end(&menu_btn);

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search songs, albums and artists"))
            .hexpand(true)
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(500)
            .child(&search_entry)
            .build();
        let search_bar = gtk::SearchBar::builder().child(&clamp).build();
        search_bar.connect_entry(&search_entry);
        search_btn
            .bind_property("active", &search_bar, "search-mode-enabled")
            .bidirectional()
            .build();

        let albums_model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let songs_model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let songs_sorted = gtk::SortListModel::new(Some(songs_model.clone()), None::<gtk::Sorter>);
        let artists_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let playlists_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();

        let empty_page = adw::StatusPage::builder()
            .icon_name("folder-music-symbolic")
            .title(tr("No Music Found"))
            .build();
        let scanning_page = adw::StatusPage::builder()
            .title(tr("Scanning Library…"))
            .build();
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .build();
        scanning_page.set_child(Some(&spinner));
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let main_stack = gtk::Stack::new();
        main_stack.add_named(&stack, Some("library"));
        main_stack.add_named(&empty_page, Some("empty"));
        main_stack.add_named(&scanning_page, Some("scanning"));
        main_stack.add_named(&search_box_scroller(&search_box), Some("search"));
        main_stack.set_visible_child_name("scanning");

        let root_tv = adw::ToolbarView::new();
        root_tv.add_top_bar(&header);
        root_tv.add_top_bar(&search_bar);
        root_tv.set_content(Some(&main_stack));
        // Narrow windows: the views move to a bar at the bottom.
        let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
        root_tv.add_bottom_bar(&switcher_bar);
        let root_page = adw::NavigationPage::builder()
            .child(&root_tv)
            .title(tr(APP_NAME))
            .tag("root")
            .build();
        let nav = adw::NavigationView::new();
        nav.add(&root_page);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&nav));
        toasts.set_vexpand(true);

        let bar = build_player_bar();
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&toasts);
        outer.append(&bar.revealer);
        window.set_content(Some(&outer));

        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            NARROW_SP,
            adw::LengthUnit::Sp,
        ));
        let narrow_title = adw::WindowTitle::new(tr(APP_NAME), "");
        narrow.add_setter(&header, "title-widget", Some(&narrow_title.to_value()));
        narrow.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
        bar.add_narrow_setters(&narrow);
        window.add_breakpoint(narrow.clone());

        let (cmd_tx, cmd_rx) = async_channel::unbounded::<Command>();
        let mpris = (std::env::var_os("HELIX_MUSIC_NO_MPRIS").is_none())
            .then(|| Mpris::start(cmd_tx.clone()));
        let sink = std::env::var(player::SINK_ENV).ok();

        let mut queue = Queue::new(Rng::from_time());
        let repeat_nick = prefs.get_enum_nick("repeat", "none");
        if repeat_nick == "shuffle" {
            queue.set_shuffle(true);
            queue.repeat = Repeat::from_nick(&state.repeat_with_shuffle).unwrap_or(Repeat::All);
        } else {
            queue.repeat = Repeat::from_nick(&repeat_nick).unwrap_or_default();
        }
        let stats = Stats::load(&Self::data_dir().join("stats.json"));
        let tracks = index::load(&Self::data_dir().join("library.json"));
        let playlists = playlist::load_all(&Self::playlists_dir());

        let app = Rc::new_cyclic(|weak: &Weak<App>| {
            let player = match Player::new(sink.as_deref()) {
                Ok(mut p) => {
                    p.set_replaygain(player::ReplayGain::from_settings(
                        prefs.get("replaygain", false),
                        &state.replaygain_mode,
                    ));
                    let w = weak.clone();
                    p.attach(move |ev| {
                        if let Some(app) = w.upgrade() {
                            app.on_player_event(ev);
                        }
                    });
                    Some(p)
                }
                Err(e) => {
                    eprintln!("helix-music: playback unavailable: {e}");
                    None
                }
            };
            App {
                app: application.clone(),
                window,
                narrow,
                headers: RefCell::default(),
                prefs,
                toasts,
                nav,
                stack,
                main_stack,
                empty_page,
                search_bar,
                search_entry,
                search_box,
                albums_model,
                songs_model,
                songs_sorted,
                artists_list,
                playlists_list,
                bar,
                lib: RefCell::new(Rc::new(Library::new(tracks))),
                stats: RefCell::new(stats),
                playlists: RefCell::new(playlists),
                queue: RefCell::new(queue),
                player,
                mpris,
                state: RefCell::new(state),
                textures: RefCell::default(),
                rows: RefCell::default(),
                queue_view: RefCell::default(),
                playlist_view: RefCell::default(),
                playing: Cell::new(false),
                scanning: Cell::new(false),
                scan_again: Cell::new(false),
                scanned_once: Cell::new(false),
                monitors: RefCell::default(),
                rescan_timer: RefCell::default(),
                inhibit_cookie: Cell::new(0),
                tracker_watching: Cell::new(false),
                using_tracker: Cell::new(false),
                played_ms: Cell::new(0),
                counted: Cell::new(false),
                last_tick: Cell::new(None),
                seeking: Cell::new(false),
                suppress_ui: Cell::new(false),
            }
        });
        if app.player.is_none() {
            app.toasts.add_toast(toast(tr(
                "Playback is unavailable (GStreamer could not be initialised)",
            )));
        }

        for applied in [true, false] {
            let w = Rc::downgrade(&app);
            let relayout = move |_: &adw::Breakpoint| {
                if let Some(app) = w.upgrade() {
                    app.layout_headers(applied);
                }
            };
            if applied {
                app.narrow.connect_apply(relayout);
            } else {
                app.narrow.connect_unapply(relayout);
            }
        }
        app.build_views();
        app.connect_player_bar();
        app.install_actions();
        app.install_keys();
        app.connect_search();
        app.connect_close();
        app.connect_notifications();
        let volume = app.state.borrow().volume;
        app.bar.volume.set_value(volume);
        app.apply_volume(volume);
        app.update_mode_buttons();
        app.refresh_library_views();
        app.update_now_playing();

        let w = Rc::downgrade(&app);
        glib::spawn_future_local(async move {
            while let Ok(cmd) = cmd_rx.recv().await {
                let Some(app) = w.upgrade() else { break };
                app.on_mpris(cmd);
            }
        });
        let w = Rc::downgrade(&app);
        glib::timeout_add_local(Duration::from_millis(250), move || match w.upgrade() {
            Some(app) => {
                app.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        app
    }

    pub fn lib(&self) -> Rc<Library> {
        self.lib.borrow().clone()
    }

    pub fn save_state(&self) {
        if let Ok(data) = serde_json::to_vec_pretty(&*self.state.borrow()) {
            let _ = playlist::write_atomic(&Self::state_file(), &data);
        }
    }

    pub fn save_stats(&self) {
        if let Err(e) = self
            .stats
            .borrow()
            .save(&Self::data_dir().join("stats.json"))
        {
            eprintln!("helix-music: cannot save statistics: {e}");
        }
    }

    pub fn library_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![music_dir()];
        roots.extend(self.state.borrow().folders.iter().cloned());
        roots
    }

    /// Cover texture for a cover key (cached).
    pub fn texture(&self, key: Option<&str>) -> Option<gdk::Texture> {
        let key = key?;
        if let Some(t) = self.textures.borrow().get(key) {
            return t.clone();
        }
        let tex = gdk::Texture::from_filename(index::cover_path(&Self::covers_dir(), key)).ok();
        self.textures
            .borrow_mut()
            .insert(key.to_string(), tex.clone());
        tex
    }

    pub fn set_cover(&self, image: &gtk::Image, key: Option<&str>) {
        match self.texture(key) {
            Some(t) => image.set_paintable(Some(&t)),
            None => image.set_icon_name(Some("audio-x-generic-symbolic")),
        }
    }

    // ----- scanning -------------------------------------------------------

    pub fn start_scan(self: &Rc<Self>) {
        if self.scanning.replace(true) {
            self.scan_again.set(true);
            return;
        }
        let roots = self.library_roots();
        let previous = self.lib().tracks.clone();
        let watching = self.tracker_watching.get();
        type ScanResult = (Vec<Track>, ScanReport, Option<tracker::Endpoint>);
        let (tx, rx) = async_channel::bounded::<ScanResult>(1);
        std::thread::Builder::new()
            .name("music-scan".into())
            .spawn(move || {
                let covers = App::covers_dir();
                // LocalSearch/Tracker when it runs (read-only queries); the
                // built-in scanner covers everything else.
                let ep = tracker::Endpoint::connect();
                let indexed = ep
                    .as_ref()
                    .and_then(|ep| match tracker::library(ep, &roots) {
                        Ok(l) => Some(l),
                        Err(e) => {
                            eprintln!(
                                "helix-music: Tracker query failed, scanning folders instead: {e}"
                            );
                            None
                        }
                    });
                let used = indexed.is_some();
                let (tracks, report) =
                    index::scan_hybrid(&roots, indexed, previous, Some(&covers), now_secs());
                if report.changed() || !App::data_dir().join("library.json").exists() {
                    if let Err(e) = index::save(&App::data_dir().join("library.json"), &tracks) {
                        eprintln!("helix-music: cannot save index: {e}");
                    }
                }
                let ep = ep.filter(|_| used && !watching);
                let _ = tx.send_blocking((tracks, report, ep));
            })
            .expect("spawn scanner");
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok((tracks, report, ep)) = rx.recv().await else {
                return;
            };
            let Some(app) = w.upgrade() else { return };
            app.scanning.set(false);
            app.using_tracker
                .set(report.from_tracker > 0 || ep.is_some());
            if let Some(ep) = ep {
                app.watch_tracker(ep);
            }
            let first = !app.scanned_once.replace(true);
            if report.changed() && !first {
                // A rescan found changes while the user may be listening
                // or browsing; say so without moving focus.
                let text = trnf!(
                    "Library updated: {n} song",
                    "Library updated: {n} songs",
                    tracks.len()
                );
                helix_app_common::announce(
                    &app.main_stack,
                    &text,
                    helix_app_common::Urgency::Polite,
                );
            }
            if report.changed() || first {
                if report.changed() {
                    *app.lib.borrow_mut() = Rc::new(Library::new(tracks));
                    app.textures.borrow_mut().clear();
                }
                app.refresh_library_views();
            }
            app.watch_dirs(&report.dirs);
            if first {
                app.import_gnome_music_once();
            }
            if app.scan_again.replace(false) {
                app.start_scan();
            }
        });
    }

    fn watch_dirs(self: &Rc<Self>, dirs: &[PathBuf]) {
        let mut monitors = Vec::new();
        for d in dirs.iter().take(MAX_MONITORS) {
            let Ok(m) = gio::File::for_path(d)
                .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            else {
                continue;
            };
            let w = Rc::downgrade(self);
            m.connect_changed(move |_, file, _, ev| {
                use gio::FileMonitorEvent as E;
                if !matches!(
                    ev,
                    E::ChangesDoneHint
                        | E::Deleted
                        | E::Created
                        | E::MovedIn
                        | E::MovedOut
                        | E::Renamed
                ) {
                    return;
                }
                let p = file.path().unwrap_or_default();
                if ev == E::Created && !p.is_dir() {
                    return; // wait for ChangesDoneHint
                }
                if let Some(app) = w.upgrade() {
                    app.schedule_rescan();
                }
            });
            monitors.push(m);
        }
        *self.monitors.borrow_mut() = monitors;
    }

    /// Rescans (debounced) when the Tracker index changes.
    fn watch_tracker(self: &Rc<Self>, ep: tracker::Endpoint) {
        if self.tracker_watching.replace(true) {
            return;
        }
        let (tx, rx) = async_channel::unbounded::<()>();
        tracker::watch(ep, move || {
            let _ = tx.try_send(());
        });
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while rx.recv().await.is_ok() {
                let Some(app) = w.upgrade() else { break };
                app.schedule_rescan();
            }
        });
    }

    fn schedule_rescan(self: &Rc<Self>) {
        if let Some(id) = self.rescan_timer.borrow_mut().take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_secs(2), move || {
            if let Some(app) = w.upgrade() {
                app.rescan_timer.borrow_mut().take();
                app.start_scan();
            }
        });
        *self.rescan_timer.borrow_mut() = Some(id);
    }

    // ----- playback -------------------------------------------------------

    pub fn current_track(&self) -> Option<Track> {
        let q = self.queue.borrow();
        let path = q.current()?;
        self.lib().track_by_path(path).cloned()
    }

    /// Replaces the queue with `tracks` (library indices) and plays `start`.
    pub fn play_tracks(self: &Rc<Self>, tracks: &[usize], start: usize, shuffle: Option<bool>) {
        if tracks.is_empty() {
            return;
        }
        let lib = self.lib();
        let paths: Vec<PathBuf> = tracks.iter().map(|&i| lib.tracks[i].path.clone()).collect();
        {
            let mut q = self.queue.borrow_mut();
            if let Some(s) = shuffle {
                q.set_shuffle(s);
            }
            let start = if shuffle == Some(true) {
                Rng::from_time().below(paths.len())
            } else {
                start
            };
            q.set(paths, start);
        }
        if shuffle.is_some() {
            self.update_mode_buttons();
            self.save_mode();
        }
        self.start_current(true);
    }

    /// Loads the queue's current track.
    pub fn start_current(self: &Rc<Self>, play: bool) {
        let Some(path) = self.queue.borrow().current().cloned() else {
            self.stop();
            return;
        };
        self.played_ms.set(0);
        self.counted.set(false);
        if let Some(p) = &self.player {
            p.load(&model::path_to_uri(&path), play);
        }
        self.update_next_uri();
        self.update_now_playing();
        if play {
            self.notify_track();
        }
    }

    /// Shows a "now playing" notification when the window is not focused.
    pub fn notify_track(&self) {
        if !self.state.borrow().notifications || self.window.is_active() {
            return;
        }
        let Some(t) = self.current_track() else {
            return;
        };
        let n = gio::Notification::new(&t.title);
        n.set_body(Some(&notification_body(&t)));
        match &t.cover {
            Some(k) => n.set_icon(&gio::FileIcon::new(&gio::File::for_path(
                index::cover_path(&Self::covers_dir(), k),
            ))),
            None => n.set_icon(&gio::ThemedIcon::new("audio-x-generic")),
        }
        n.set_priority(gio::NotificationPriority::Low);
        n.add_button(tr("Next"), "app.next-track");
        self.app.send_notification(Some(NOTIFICATION_ID), &n);
    }

    pub fn update_next_uri(&self) {
        let next = self
            .queue
            .borrow_mut()
            .peek_next(true)
            .map(|p| model::path_to_uri(p));
        if let Some(p) = &self.player {
            p.set_next(next);
        }
    }

    pub fn play_pause(self: &Rc<Self>) {
        let Some(p) = &self.player else { return };
        if self.queue.borrow().is_empty() {
            // Nothing queued: play the whole library.
            let all = self.lib().all_tracks_by_album();
            self.play_tracks(&all, 0, None);
            return;
        }
        match p.state() {
            player::State::Playing => p.pause(),
            player::State::Paused => p.play(),
            player::State::Stopped => self.start_current(true),
        }
    }

    pub fn play(self: &Rc<Self>) {
        if self
            .player
            .as_ref()
            .is_some_and(|p| p.state() != player::State::Playing)
        {
            self.play_pause();
        }
    }

    pub fn pause(&self) {
        if let Some(p) = &self.player {
            if p.state() == player::State::Playing {
                p.pause();
            }
        }
    }

    pub fn stop(&self) {
        if let Some(p) = &self.player {
            p.stop();
        }
        self.set_playing(false);
        self.update_position(Some(0));
    }

    pub fn next(self: &Rc<Self>) {
        let moved = self.queue.borrow_mut().advance(false).is_some();
        if moved {
            let play = self.playing.get();
            self.start_current(
                play || self
                    .player
                    .as_ref()
                    .is_some_and(|p| p.state() == player::State::Stopped),
            );
        }
    }

    pub fn previous(self: &Rc<Self>) {
        let pos = self
            .player
            .as_ref()
            .and_then(|p| p.position_ms())
            .unwrap_or(0);
        if pos > 3000 {
            self.seek_ms(0);
            return;
        }
        let moved = self.queue.borrow_mut().previous().is_some();
        if moved {
            self.start_current(true);
        }
    }

    pub fn seek_ms(&self, ms: u64) {
        if let Some(p) = &self.player {
            p.seek(ms);
        }
        if let Some(m) = &self.mpris {
            m.seeked(ms as i64 * 1000);
        }
        self.update_position(Some(ms));
    }

    pub fn apply_volume(&self, level: f64) {
        if let Some(p) = &self.player {
            p.set_volume(level);
        }
        if let Some(m) = &self.mpris {
            m.update(|s| s.volume = level);
        }
    }

    fn on_player_event(self: &Rc<Self>, ev: Event) {
        match ev {
            Event::StreamStart { gapless: true } => {
                // playbin moved on to the URI handed over in about-to-finish.
                self.queue.borrow_mut().advance(true);
                self.played_ms.set(0);
                self.counted.set(false);
                self.update_next_uri();
                self.update_now_playing();
                self.notify_track();
            }
            Event::StreamStart { gapless: false } | Event::DurationChanged => {
                self.update_position(None)
            }
            Event::Eos => {
                // End of queue (nothing was queued gaplessly).
                if let Some(p) = &self.player {
                    p.stop();
                }
                self.set_playing(false);
                self.update_position(Some(0));
            }
            Event::Error(e) => {
                let name = self.current_track().map(|t| t.title).unwrap_or_default();
                // The toast is spoken by libadwaita; say it urgently as well,
                // since playback stopped or skipped without any action.
                let text = trf!("Cannot play “{name}”", name = name);
                helix_app_common::announce(
                    &self.bar.title,
                    &text,
                    helix_app_common::Urgency::Assertive,
                );
                self.toasts.add_toast(toast(&text));
                eprintln!("helix-music: playback error: {e}");
                let has_next = self.queue.borrow().has_next();
                let moved = has_next && self.queue.borrow_mut().advance(false).is_some();
                if moved {
                    self.start_current(true);
                } else {
                    self.stop();
                }
            }
            Event::State(s) => self.set_playing(s == player::State::Playing),
        }
    }

    fn set_playing(&self, playing: bool) {
        let changed = self.playing.replace(playing) != playing;
        self.bar.play.set_icon_name(if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        self.bar
            .play
            .set_tooltip_text(Some(if playing { tr("Pause") } else { tr("Play") }));
        helix_app_common::set_accessible_label(
            &self.bar.play,
            &self.bar.play.tooltip_text().unwrap_or_default(),
        );
        if !playing {
            self.last_tick.set(None);
        }
        self.announce_track();
        if changed {
            self.update_inhibit();
        }
        let status = if playing {
            "Playing"
        } else if self
            .player
            .as_ref()
            .is_some_and(|p| p.state() == player::State::Paused)
        {
            "Paused"
        } else {
            "Stopped"
        };
        if let Some(m) = &self.mpris {
            m.update(|s| s.status = status);
        }
    }

    pub fn update_inhibit(&self) {
        let want = self.playing.get() && self.prefs.get("inhibit-suspend", false);
        let cookie = self.inhibit_cookie.get();
        if want && cookie == 0 {
            let c = self.app.inhibit(
                Some(&self.window),
                gtk::ApplicationInhibitFlags::SUSPEND,
                Some(tr("Playing music")),
            );
            self.inhibit_cookie.set(c);
        } else if !want && cookie != 0 {
            self.app.uninhibit(cookie);
            self.inhibit_cookie.set(0);
        }
    }

    /// Periodic position update and play counting.
    fn tick(self: &Rc<Self>) {
        if !self.playing.get() {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_tick.replace(Some(now)) {
            let add = now.duration_since(last).as_millis().min(1000) as u64;
            self.played_ms.set(self.played_ms.get() + add);
        }
        if !self.counted.get() {
            if let Some(t) = self.current_track() {
                let dur = self
                    .player
                    .as_ref()
                    .and_then(|p| p.duration_ms())
                    .unwrap_or(t.duration_ms);
                if playlist::counts_as_play(self.played_ms.get(), dur) {
                    self.counted.set(true);
                    self.stats.borrow_mut().record_play(&t.path, now_secs());
                    self.save_stats();
                }
            }
        }
        self.update_position(None);
    }

    pub fn update_position(&self, pos: Option<u64>) {
        if self.seeking.get() {
            return;
        }
        let p = self.player.as_ref();
        let pos = pos.or_else(|| p.and_then(|p| p.position_ms())).unwrap_or(0);
        let dur = p
            .and_then(|p| p.duration_ms())
            .or_else(|| self.current_track().map(|t| t.duration_ms))
            .unwrap_or(0);
        self.suppress_ui.set(true);
        self.bar.scale.set_range(0.0, (dur.max(1)) as f64);
        self.bar.scale.set_value(pos.min(dur) as f64);
        self.suppress_ui.set(false);
        self.bar.elapsed.set_label(&model::format_duration(pos));
        self.bar.total.set_label(&model::format_duration(dur));
        self.bar
            .scale
            .update_property(
                &[gtk::accessible::Property::ValueText(&model::position_text(
                    pos, dur,
                ))],
            );
        if let Some(m) = &self.mpris {
            m.update(|s| s.position_us = pos as i64 * 1000);
        }
    }

    /// Tells screen readers about a new track once it plays (track changes
    /// happen without focus moving).
    fn announce_track(&self) {
        if !self.playing.get() {
            return;
        }
        if let Some(t) = self.current_track() {
            let artist = t.artist_name();
            let text = if artist.is_empty() {
                trf!("Now playing: {}", t.title)
            } else {
                trf!(
                    "Now playing: {title} by {artist}",
                    title = t.title,
                    artist = artist
                )
            };
            helix_app_common::announce_change(
                &self.bar.title,
                &text,
                helix_app_common::Urgency::Polite,
            );
        }
    }

    pub fn update_now_playing(self: &Rc<Self>) {
        let track = self.current_track();
        let has = !self.queue.borrow().is_empty();
        self.bar.revealer.set_reveal_child(has);
        match &track {
            Some(t) => {
                self.bar.title.set_label(&t.title);
                self.bar.artist.set_label(t.artist_name());
                self.set_cover(&self.bar.cover, t.cover.as_deref());
                self.bar.heart.set_sensitive(true);
                let fav = self.stats.borrow().is_favorite(&t.path);
                sync_heart(&self.bar.heart, fav);
                self.window
                    .set_title(Some(&format!("{} — {}", t.title, t.artist_name())));
                self.announce_track();
            }
            None => {
                self.bar.title.set_label("");
                self.bar.artist.set_label("");
                self.bar
                    .cover
                    .set_icon_name(Some("audio-x-generic-symbolic"));
                self.bar.heart.set_sensitive(false);
                self.window.set_title(Some(tr(APP_NAME)));
            }
        }
        let (can_next, can_prev) = {
            let q = self.queue.borrow();
            (q.has_next(), q.has_previous())
        };
        self.bar.next.set_sensitive(can_next);
        self.bar.prev.set_sensitive(can_prev);
        self.update_position(None);
        self.refresh_rows();
        self.refresh_queue_view();
        let songs_n = self.songs_model.n_items();
        self.songs_model.items_changed(0, songs_n, songs_n);
        if let Some(m) = &self.mpris {
            let meta = track.as_ref().map(|t| mpris::Metadata {
                track_id: mpris::track_id(&t.path),
                title: t.title.clone(),
                artist: t.artist_name().to_string(),
                album: t.album_name().to_string(),
                album_artist: t.album_artist.clone().unwrap_or_default(),
                track_number: t.track,
                length_us: t.duration_ms as i64 * 1000,
                art_url: t
                    .cover
                    .as_ref()
                    .map(|k| model::path_to_uri(&index::cover_path(&Self::covers_dir(), k))),
                url: t.uri(),
            });
            m.update(|s| {
                s.metadata = meta;
                s.can_next = can_next;
                s.can_prev = can_prev;
            });
        }
    }

    pub fn update_mode_buttons(&self) {
        let (shuffle, repeat) = {
            let q = self.queue.borrow();
            (q.shuffle(), q.repeat)
        };
        self.suppress_ui.set(true);
        self.bar.shuffle.set_active(shuffle);
        self.suppress_ui.set(false);
        let (icon, tip) = match repeat {
            Repeat::None => ("media-playlist-consecutive-symbolic", tr("Repeat: Off")),
            Repeat::All => ("media-playlist-repeat-symbolic", tr("Repeat: All")),
            Repeat::One => ("media-playlist-repeat-song-symbolic", tr("Repeat: Song")),
        };
        self.bar.repeat.set_icon_name(icon);
        self.bar.repeat.set_tooltip_text(Some(tip));
        helix_app_common::set_accessible_label(&self.bar.repeat, tip);
        if let Some(m) = &self.mpris {
            m.update(|s| {
                s.shuffle = shuffle;
                s.loop_status = repeat.mpris();
            });
        }
    }

    fn save_mode(&self) {
        let (shuffle, repeat) = {
            let q = self.queue.borrow();
            (q.shuffle(), q.repeat)
        };
        if shuffle {
            self.prefs.set_enum_nick("repeat", "shuffle");
            self.state.borrow_mut().repeat_with_shuffle = repeat.nick().into();
            self.save_state();
        } else {
            self.prefs.set_enum_nick("repeat", repeat.nick());
        }
    }

    pub fn set_shuffle(self: &Rc<Self>, on: bool) {
        self.queue.borrow_mut().set_shuffle(on);
        self.after_mode_change();
    }

    pub fn set_repeat(self: &Rc<Self>, r: Repeat) {
        self.queue.borrow_mut().repeat = r;
        self.after_mode_change();
    }

    fn after_mode_change(self: &Rc<Self>) {
        self.update_mode_buttons();
        self.save_mode();
        self.update_next_uri();
        self.update_now_playing();
    }

    fn on_mpris(self: &Rc<Self>, cmd: Command) {
        match cmd {
            Command::Raise => self.window.present(),
            Command::Quit => self.window.close(),
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::PlayPause => self.play_pause(),
            Command::Stop => self.stop(),
            Command::Next => self.next(),
            Command::Previous => self.previous(),
            Command::Seek(off) => {
                let pos = self
                    .player
                    .as_ref()
                    .and_then(|p| p.position_ms())
                    .unwrap_or(0) as i64;
                let target = (pos + off / 1000).max(0) as u64;
                let dur = self
                    .player
                    .as_ref()
                    .and_then(|p| p.duration_ms())
                    .unwrap_or(u64::MAX);
                if target >= dur {
                    self.next();
                } else {
                    self.seek_ms(target);
                }
            }
            Command::SetPosition(id, pos) => {
                let current = self.current_track().map(|t| mpris::track_id(&t.path));
                if current.as_deref() == Some(id.as_str()) && pos >= 0 {
                    self.seek_ms(pos as u64 / 1000);
                }
            }
            Command::SetVolume(v) => {
                let v = v.clamp(0.0, 1.0);
                self.bar.volume.set_value(v);
            }
            Command::SetShuffle(s) => self.set_shuffle(s),
            Command::SetLoop(l) => {
                if let Some(r) = Repeat::from_mpris(&l) {
                    self.set_repeat(r);
                }
            }
        }
    }

    // ----- favourites and rows ---------------------------------------------

    pub fn set_favorite(self: &Rc<Self>, path: &Path, on: bool) {
        if self.stats.borrow().is_favorite(path) == on {
            return;
        }
        self.stats.borrow_mut().set_favorite(path, on);
        self.save_stats();
        self.refresh_rows();
        if self.current_track().is_some_and(|t| t.path == path) {
            sync_heart(&self.bar.heart, on);
        }
        let n = self.songs_model.n_items();
        self.songs_model.items_changed(0, n, n);
    }

    pub fn register_row(&self, handle: RowHandle) {
        self.rows.borrow_mut().push(handle);
    }

    /// Updates playing indicators and hearts of all live track rows.
    pub fn refresh_rows(&self) {
        let current = self.queue.borrow().current().cloned();
        let stats = self.stats.borrow();
        self.rows.borrow_mut().retain(|r| {
            let Some(ind) = r.indicator.upgrade() else {
                return false;
            };
            show_playing(&ind, current.as_ref() == Some(&r.path));
            if let Some(h) = r.heart.upgrade() {
                sync_heart(&h, stats.is_favorite(&r.path));
            }
            true
        });
    }

    // ----- window plumbing ---------------------------------------------------

    fn connect_close(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(app) = w.upgrade() {
                app.state.borrow_mut().volume = app.bar.volume.value();
                app.save_state();
                app.stop();
                if app.inhibit_cookie.get() != 0 {
                    app.app.uninhibit(app.inhibit_cookie.replace(0));
                }
            }
            glib::Propagation::Proceed
        });
    }

    fn connect_notifications(self: &Rc<Self>) {
        let next = gio::SimpleAction::new("next-track", None);
        let w = Rc::downgrade(self);
        next.connect_activate(move |_, _| {
            if let Some(app) = w.upgrade() {
                app.next();
            }
        });
        self.app.add_action(&next);
        let w = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |win| {
            if let (true, Some(app)) = (win.is_active(), w.upgrade()) {
                app.app.withdraw_notification(NOTIFICATION_ID);
            }
        });
    }

    pub fn set_replaygain(&self, mode: player::ReplayGain) {
        self.prefs
            .set("replaygain", mode != player::ReplayGain::Off);
        if mode != player::ReplayGain::Off {
            self.state.borrow_mut().replaygain_mode = mode.nick().into();
            self.save_state();
        }
        if let Some(p) = &self.player {
            p.set_replaygain(mode);
        }
    }

    pub fn replaygain(&self) -> player::ReplayGain {
        player::ReplayGain::from_settings(
            self.prefs.get("replaygain", false),
            &self.state.borrow().replaygain_mode,
        )
    }

    fn install_actions(self: &Rc<Self>) {
        let win = &self.window;
        let add = |name: &str, f: fn(&Rc<App>)| {
            let a = gio::SimpleAction::new(name, None);
            let w = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(app) = w.upgrade() {
                    f(&app);
                }
            });
            win.add_action(&a);
        };
        add("play-pause", |a| a.play_pause());
        add("next", |a| a.next());
        add("previous", |a| a.previous());
        add("repeat", |a| {
            let r = a.queue.borrow().repeat.cycle();
            a.set_repeat(r);
        });
        add("shuffle", |a| {
            let s = !a.queue.borrow().shuffle();
            a.set_shuffle(s);
        });
        add("search", |a| {
            a.search_bar.set_search_mode(true);
            a.search_entry.grab_focus();
        });
        add("rescan", |a| a.start_scan());
        add("preferences", |a| a.show_preferences());
        add("now-playing", |a| a.show_now_playing());
        add("new-playlist", |a| a.new_playlist_dialog(None));
        add("add-folder", |a| a.add_folder_dialog());
        add("import-playlist", |a| a.import_playlist_dialog());
        add("import-index-playlists", |a| a.import_index_playlists());

        let view = gio::SimpleAction::new("view", Some(glib::VariantTy::STRING));
        let w = Rc::downgrade(self);
        view.connect_activate(move |_, v| {
            if let (Some(app), Some(name)) = (w.upgrade(), v.and_then(|v| v.get::<String>())) {
                app.search_bar.set_search_mode(false);
                app.nav.pop_to_tag("root");
                app.stack.set_visible_child_name(&name);
            }
        });
        win.add_action(&view);

        // Track-targeted actions (target: library index).
        let track_action = |name: &str, f: fn(&Rc<App>, usize)| {
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::UINT32));
            let w = Rc::downgrade(self);
            a.connect_activate(move |_, v| {
                if let (Some(app), Some(i)) = (w.upgrade(), v.and_then(|v| v.get::<u32>())) {
                    if (i as usize) < app.lib().tracks.len() {
                        f(&app, i as usize);
                    }
                }
            });
            win.add_action(&a);
        };
        track_action("queue-add", |app, i| {
            let path = app.lib().tracks[i].path.clone();
            let was_empty = app.queue.borrow().is_empty();
            app.queue.borrow_mut().append(vec![path]);
            if was_empty {
                app.start_current(true);
            } else {
                app.update_next_uri();
                app.update_now_playing();
            }
            app.toasts.add_toast(toast(tr("Added to queue")));
        });
        track_action("play-next", |app, i| {
            let path = app.lib().tracks[i].path.clone();
            let was_empty = app.queue.borrow().is_empty();
            app.queue.borrow_mut().play_next(path);
            if was_empty {
                app.start_current(true);
            } else {
                app.update_next_uri();
                app.update_now_playing();
                app.toasts.add_toast(toast(tr("Will play next")));
            }
        });
        track_action("add-to-playlist", |app, i| {
            app.add_to_playlist_dialog(vec![i])
        });
        track_action("go-album", |app, i| {
            if let Some(a) = app.lib().album_of(i) {
                app.show_album(a);
            }
        });

        let remove = gio::SimpleAction::new(
            "playlist-remove",
            Some(&<(String, u32)>::static_variant_type()),
        );
        let w = Rc::downgrade(self);
        remove.connect_activate(move |_, v| {
            if let (Some(app), Some((file, pos))) =
                (w.upgrade(), v.and_then(|v| v.get::<(String, u32)>()))
            {
                app.remove_from_playlist(Path::new(&file), pos as usize);
            }
        });
        win.add_action(&remove);

        self.window
            .set_help_overlay(Some(&helix_app_common::shortcuts::window(
                crate::shortcuts::GROUPS,
            )));
    }

    /// Space toggles playback, printable keys start a search, Escape goes back.
    fn install_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(app) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let in_text = gtk::prelude::GtkWindowExt::focus(&app.window)
                .is_some_and(|f| f.is::<gtk::Text>() || f.is::<gtk::Editable>());
            if in_text {
                return glib::Propagation::Proceed;
            }
            if key == gdk::Key::Escape {
                if app.search_bar.is_search_mode() {
                    app.search_bar.set_search_mode(false);
                    return glib::Propagation::Stop;
                }
                if app.nav.visible_page().and_then(|p| p.tag()).as_deref() != Some("root") {
                    app.nav.pop();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            if mods.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            if key == gdk::Key::space {
                app.play_pause();
                return glib::Propagation::Stop;
            }
            if let Some(c) = key
                .to_unicode()
                .filter(|c| !c.is_control() && !c.is_whitespace())
            {
                app.nav.pop_to_tag("root");
                app.search_bar.set_search_mode(true);
                app.search_entry.set_text(&c.to_string());
                app.search_entry.grab_focus();
                app.search_entry.set_position(-1);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);
    }

    fn connect_search(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |e| {
            if let Some(app) = w.upgrade() {
                app.show_search(&e.text());
            }
        });
        let w = Rc::downgrade(self);
        self.search_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if let Some(app) = w.upgrade() {
                    if !bar.is_search_mode() {
                        app.search_entry.set_text("");
                        app.update_main_stack();
                    }
                }
            });
    }

    pub fn update_main_stack(&self) {
        let searching =
            self.search_bar.is_search_mode() && !self.search_entry.text().trim().is_empty();
        let name = if searching {
            "search"
        } else if !self.lib().tracks.is_empty() {
            "library"
        } else if self.scanned_once.get() {
            "empty"
        } else {
            "scanning"
        };
        self.main_stack.set_visible_child_name(name);
        self.empty_page.set_description(Some(&trf!(
            "Music files in {} will appear here.",
            glib::markup_escape_text(&friendly_folder_name(&music_dir(), &glib::home_dir()))
        )));
    }

    fn connect_player_bar(self: &Rc<Self>) {
        let b = &self.bar;
        b.play.set_action_name(Some("win.play-pause"));
        b.prev.set_action_name(Some("win.previous"));
        b.next.set_action_name(Some("win.next"));
        b.repeat.set_action_name(Some("win.repeat"));
        let w = Rc::downgrade(self);
        b.shuffle.connect_toggled(move |t| {
            if let Some(app) = w.upgrade() {
                if !app.suppress_ui.get() {
                    app.set_shuffle(t.is_active());
                }
            }
        });
        let w = Rc::downgrade(self);
        b.heart.connect_toggled(move |h| {
            if let Some(app) = w.upgrade() {
                set_heart_icon(h);
                if let Some(t) = app.current_track() {
                    app.set_favorite(&t.path, h.is_active());
                }
            }
        });
        let w = Rc::downgrade(self);
        b.volume.connect_value_changed(move |s| {
            if let Some(app) = w.upgrade() {
                app.apply_volume(s.value());
                app.state.borrow_mut().volume = s.value();
            }
        });
        let w = Rc::downgrade(self);
        b.scale.connect_change_value(move |_, _, value| {
            if let Some(app) = w.upgrade() {
                if !app.suppress_ui.get() {
                    app.seek_ms(value.max(0.0) as u64);
                }
            }
            glib::Propagation::Proceed
        });
    }

    pub fn add_folder_dialog(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Add Music Folder"))
            .modal(true)
            .build();
        let w = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let (Some(app), Ok(folder)) = (w.upgrade(), res) else {
                return;
            };
            let Some(path) = folder.path() else { return };
            app.add_folder(path);
        });
    }

    pub fn add_folder(self: &Rc<Self>, path: PathBuf) {
        let exists = path == music_dir() || self.state.borrow().folders.contains(&path);
        if !exists {
            self.state.borrow_mut().folders.push(path);
            self.save_state();
            self.start_scan();
        }
    }

    pub fn remove_folder(self: &Rc<Self>, path: &Path) {
        self.state.borrow_mut().folders.retain(|p| p != path);
        self.save_state();
        self.start_scan();
    }
}

/// Shows a track row's playing indicator (instead of its number) and
/// says "Now playing" in the row's accessible description.
pub fn show_playing(indicator: &gtk::Stack, playing: bool) {
    indicator.set_visible_child_name(if playing { "playing" } else { "number" });
    if let Some(row) = indicator.ancestor(gtk::ListBoxRow::static_type()) {
        if playing {
            row.update_property(&[gtk::accessible::Property::Description(tr("Now playing"))]);
        } else {
            row.reset_property(gtk::AccessibleProperty::Description);
        }
    }
}

pub fn sync_heart(btn: &gtk::ToggleButton, on: bool) {
    if btn.is_active() != on {
        btn.set_active(on);
    }
    set_heart_icon(btn);
}

pub fn set_heart_icon(btn: &gtk::ToggleButton) {
    let on = btn.is_active();
    btn.set_icon_name(if on {
        "starred-symbolic"
    } else {
        "non-starred-symbolic"
    });
    btn.set_tooltip_text(Some(if on {
        tr("Remove from Favorites")
    } else {
        tr("Add to Favorites")
    }));
    helix_app_common::set_accessible_label(btn, &btn.tooltip_text().unwrap_or_default());
}

fn search_box_scroller(content: &gtk::Box) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder()
        .maximum_size(900)
        .child(content)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    gtk::ScrolledWindow::builder()
        .child(&clamp)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build()
}

fn flat_button(icon: &str, tip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
        .labelled(tip)
}

/// Below this width (in sp) the window uses its narrow layout: the view
/// switcher at the bottom and a two-row player bar.
pub const NARROW_SP: f64 = 550.0;

impl App {
    /// Whether the narrow layout is in use.
    pub fn is_narrow(&self) -> bool {
        self.window.current_breakpoint().as_ref() == Some(&self.narrow)
    }

    /// Lays out the open album and playlist headers for the window width.
    fn layout_headers(&self, narrow: bool) {
        self.headers.borrow_mut().retain(|h| match h.upgrade() {
            Some(h) => {
                crate::pages::layout_header(&h, narrow);
                true
            }
            None => false,
        });
    }
}

impl PlayerBar {
    /// Narrow layout: cover, title and Previous / Play / Next on the first
    /// row; position and the other controls on the second.
    fn add_narrow_setters(&self, bp: &adw::Breakpoint) {
        bp.add_setter(&self.info_box, "width-request", Some(&(-1).to_value()));
        let Some(layout) = self.grid.layout_manager() else {
            return;
        };
        for (widget, column, row) in [(&self.seek_box, 0, 1), (&self.side_box, 1, 1)] {
            let child = layout.layout_child(widget);
            bp.add_setter(&child, "column", Some(&column.to_value()));
            bp.add_setter(&child, "row", Some(&row.to_value()));
        }
    }
}

fn build_player_bar() -> PlayerBar {
    let cover = gtk::Image::builder()
        .pixel_size(48)
        .css_classes(["hm-cover"])
        // Decorative: the album or track title next to it names it.
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let title = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["heading"])
        .build();
    let artist = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label", "caption"])
        .build();
    let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
    info.set_valign(gtk::Align::Center);
    info.append(&title);
    info.append(&artist);
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    left.set_size_request(220, -1);
    left.append(&cover);
    left.append(&info);
    info.set_hexpand(true);
    // The cover and title open Now Playing: a real (flat) button, so it
    // works from the keyboard too.
    let info_button = gtk::Button::builder()
        .child(&left)
        .action_name("win.now-playing")
        .tooltip_text(tr("Show Now Playing"))
        .css_classes(["flat"])
        .build()
        .labelled(tr("Show Now Playing"));

    let prev = flat_button("media-skip-backward-symbolic", tr("Previous"));
    let play = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text(tr("Play"))
        .valign(gtk::Align::Center)
        .css_classes(["circular", "suggested-action"])
        .build()
        .labelled(tr("Play"));
    let next = flat_button("media-skip-forward-symbolic", tr("Next"));
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.append(&prev);
    controls.append(&play);
    controls.append(&next);

    let elapsed = gtk::Label::builder()
        .label("0:00")
        .css_classes(["numeric", "caption"])
        .width_chars(5)
        .build();
    let total = gtk::Label::builder()
        .label("0:00")
        .css_classes(["numeric", "caption"])
        .width_chars(5)
        .build();
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1000.0);
    scale.set_hexpand(true);
    scale.set_draw_value(false);
    scale.set_tooltip_text(Some(tr("Position")));
    helix_app_common::set_accessible_label(&scale, tr("Position"));
    let seek = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    seek.set_hexpand(true);
    seek.append(&elapsed);
    seek.append(&scale);
    seek.append(&total);

    let heart = gtk::ToggleButton::builder()
        .icon_name("non-starred-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
        .labelled(tr("Add to Favorites"));
    let shuffle = gtk::ToggleButton::builder()
        .icon_name("media-playlist-shuffle-symbolic")
        .tooltip_text(tr("Shuffle"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
        .labelled(tr("Shuffle"));
    let repeat = flat_button("media-playlist-consecutive-symbolic", tr("Repeat"));
    let volume = gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, 1.0, 0.05);
    volume.set_inverted(true);
    volume.set_size_request(-1, 140);
    volume.set_draw_value(false);
    helix_app_common::set_accessible_label(&volume, tr("Volume"));
    volume.update_property(&[gtk::accessible::Property::ValueText(&model::volume_text(
        volume.value(),
    ))]);
    let vol_pop = gtk::Popover::builder().child(&volume).build();
    let vol_btn = gtk::MenuButton::builder()
        .icon_name("audio-volume-high-symbolic")
        .tooltip_text(tr("Volume"))
        .popover(&vol_pop)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
        .labelled(tr("Volume"));
    let vb = vol_btn.clone();
    volume.connect_value_changed(move |s| {
        let v = s.value();
        s.update_property(&[gtk::accessible::Property::ValueText(&model::volume_text(v))]);
        vb.set_icon_name(match v {
            v if v <= 0.0 => "audio-volume-muted-symbolic",
            v if v < 0.34 => "audio-volume-low-symbolic",
            v if v < 0.67 => "audio-volume-medium-symbolic",
            _ => "audio-volume-high-symbolic",
        });
    });
    let queue_btn = gtk::Button::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text(tr("Now Playing"))
        .action_name("win.now-playing")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
        .labelled(tr("Now Playing"));
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    for w in [
        heart.upcast_ref::<gtk::Widget>(),
        shuffle.upcast_ref(),
        repeat.upcast_ref(),
        vol_btn.upcast_ref(),
        queue_btn.upcast_ref(),
    ] {
        right.append(w);
    }

    controls.set_halign(gtk::Align::End);
    right.set_halign(gtk::Align::End);
    let grid = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(4)
        .css_classes(["hm-playerbar"])
        .build();
    grid.attach(&info_button, 0, 0, 1, 1);

    grid.attach(&controls, 1, 0, 1, 1);
    grid.attach(&seek, 2, 0, 1, 1);
    grid.attach(&right, 3, 0, 1, 1);
    let revealer = gtk::Revealer::builder()
        .child(&grid)
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .build();
    PlayerBar {
        revealer,
        grid,
        info_box: left,
        seek_box: seek,
        side_box: right,
        cover,
        title,
        artist,
        play,
        prev,
        next,
        elapsed,
        total,
        scale,
        heart,
        shuffle,
        repeat,
        volume,
    }
}

#[cfg(test)]
mod tests {
    use super::friendly_folder_name;
    use std::path::Path;

    #[test]
    fn folder_names() {
        let home = Path::new("/home/ana");
        assert_eq!(
            friendly_folder_name(Path::new("/home/ana/Music"), home),
            "~/Music"
        );
        assert_eq!(friendly_folder_name(Path::new("/home/ana"), home), "~");
        assert_eq!(
            friendly_folder_name(Path::new("/srv/music"), home),
            "/srv/music"
        );
    }
}
