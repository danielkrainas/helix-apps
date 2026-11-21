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
    ("win.view::genres", &["<Alt>5"]),
    ("win.toggle-favorite", &["<Control>d"]),
    ("win.mute", &["<Control>m"]),
    ("win.now-playing", &["<Control>l"]),
    ("win.rescan", &["F5"]),
    ("win.preferences", &["<Control>comma"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

/// Shortcuts that run only when the focused widget does not use the key
/// (Ctrl+arrows move by words in the search entry).
pub(crate) const BUBBLE_ACCELS: &[(&str, &[&str])] = &[
    ("win.seek-backward", &["<Control>Left"]),
    ("win.seek-forward", &["<Control>Right"]),
    ("win.volume-up", &["<Control>Up"]),
    ("win.volume-down", &["<Control>Down"]),
];

/// Ctrl+Left / Ctrl+Right seek by this much.
const SEEK_STEP_MS: i64 = 10_000;
const VOLUME_STEP: f64 = 0.05;

fn load_css() {
    helix_app_common::style::add_css(
        ".hm-cover { border-radius: 8px; box-shadow: 0 1px 4px alpha(black, 0.18); }
         .hm-cover.hm-cover-small { border-radius: 6px; box-shadow: none; }
         .hm-cover-empty { background-color: alpha(@window_fg_color, 0.07);
             color: alpha(@window_fg_color, 0.4); }
         gridview.hm-albums { padding: 12px 6px 24px; }
         flowbox.hm-album-tiles > flowboxchild { padding: 6px; border-radius: 12px; }
         button.hm-link { padding: 2px 6px; margin-left: -6px; min-height: 0; }
         .hm-drop-target { background-color: alpha(@accent_bg_color, 0.2); }
         .hm-queue-bar { padding: 6px 6px 6px 12px; }
         listview.hm-track-list { background: none; }
         button.hm-np-link { padding: 2px 8px; min-height: 0; }
         button.hm-np-link:disabled { color: inherit; opacity: 1; }
         .hm-np-artist { font-weight: bold; }
         .hm-lyrics { margin: 12px 0; }
         .hm-lyric-current { color: @accent_color; }
         gridview.hm-albums > child { padding: 12px 6px; border-radius: 12px; }
         .hm-playerbar { padding: 6px 12px; border-top: 1px solid alpha(currentColor, 0.15); }
         .hm-num { min-width: 2.5em; }
         .hm-disc { margin-top: 12px; }
         .hm-header-buttons > flowboxchild,
         .hm-header-buttons > flowboxchild:hover,
         .hm-header-buttons > flowboxchild:active,
         .hm-header-buttons > flowboxchild:selected {
             background: none; padding: 0; box-shadow: none; outline: none; }",
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
    /// Keep playing with the window closed (Helix only; off like GNOME
    /// Music).
    pub background_playback: bool,
    /// Albums view order (`model::AlbumSort` nick).
    pub album_sort: String,
    /// Artists view order (`model::ArtistSort` nick).
    pub artist_sort: String,
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
            background_playback: false,
            album_sort: "title".into(),
            artist_sort: "name".into(),
        }
    }
}

const NOTIFICATION_ID: &str = "now-playing";

/// `queue.json`: the queue and where the current song was.
#[derive(Serialize, Deserialize, Default)]
struct QueueFile {
    #[serde(flatten)]
    queue: crate::queue::Saved<PathBuf>,
    #[serde(default)]
    position_ms: u64,
}

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

/// A widget that shows something about a track that can change while it
/// is on screen.
pub enum Live {
    /// Track row: the number, or a play icon while the track plays.
    Indicator(glib::WeakRef<gtk::Stack>),
    /// Songs table: a play icon while the track plays.
    PlayingIcon(glib::WeakRef<gtk::Image>),
    /// Favourite toggle.
    Heart(glib::WeakRef<gtk::ToggleButton>),
    /// Play count.
    Plays(glib::WeakRef<gtk::Label>),
    /// A list view row: "Now playing" as its description while it plays.
    ItemDescription(glib::WeakRef<gtk::ListItem>),
}

/// A registered track widget, updated when the playing track, a favourite
/// or a play count changes. Recycled list cells change `path` whenever
/// they are bound to another track.
pub struct RowHandle {
    pub path: RefCell<Option<PathBuf>>,
    pub live: Live,
}

impl RowHandle {
    pub fn new(path: Option<PathBuf>, live: Live) -> Rc<RowHandle> {
        Rc::new(RowHandle {
            path: RefCell::new(path),
            live,
        })
    }

    fn alive(&self) -> bool {
        match &self.live {
            Live::Indicator(w) => w.upgrade().is_some(),
            Live::PlayingIcon(w) => w.upgrade().is_some(),
            Live::Heart(w) => w.upgrade().is_some(),
            Live::Plays(w) => w.upgrade().is_some(),
            Live::ItemDescription(w) => w.upgrade().is_some(),
        }
    }

    /// Shows the current state; false once the widget is gone.
    fn update(&self, current: Option<&PathBuf>, stats: &Stats) -> bool {
        let path = self.path.borrow();
        let playing = path.is_some() && path.as_ref() == current;
        match &self.live {
            Live::Indicator(w) => match w.upgrade() {
                Some(ind) => show_playing(&ind, playing),
                None => return false,
            },
            Live::PlayingIcon(w) => match w.upgrade() {
                Some(img) => show_playing_icon(&img, playing),
                None => return false,
            },
            Live::Heart(w) => match w.upgrade() {
                Some(h) => sync_heart(&h, path.as_deref().is_some_and(|p| stats.is_favorite(p))),
                None => return false,
            },
            Live::Plays(w) => match w.upgrade() {
                Some(l) => {
                    let n = path.as_deref().map_or(0, |p| stats.plays(p));
                    l.set_label(&n.to_string());
                }
                None => return false,
            },
            Live::ItemDescription(w) => match w.upgrade() {
                Some(item) => {
                    item.set_accessible_description(if playing { tr("Now playing") } else { "" })
                }
                None => return false,
            },
        }
        true
    }
}

pub struct PlayerBar {
    pub revealer: gtk::Revealer,
    /// The bar's layout: one row when wide, two when narrow.
    grid: gtk::Grid,
    info_box: gtk::Box,
    seek_box: gtk::Box,
    side_box: gtk::Box,
    pub cover: gtk::Overlay,
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
    pub repeat: gtk::ToggleButton,
    /// Narrow windows: opens the controls that do not fit (`side_box`).
    more: gtk::MenuButton,
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
    /// Shown during the first scan, with a running count.
    pub scanning_page: adw::StatusPage,
    /// In the header while a later scan runs.
    pub scan_spinner: gtk::Spinner,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub search_box: gtk::Box,
    /// Header buttons for one view (by view name), shown while it is.
    pub view_buttons: Vec<(&'static str, gtk::Widget)>,
    pub albums_model: gio::ListStore,
    pub songs_model: gio::ListStore,
    pub songs_sorted: gtk::SortListModel,
    /// Songs selection mode: the header's Select button and the selection.
    pub songs_select: gtk::ToggleButton,
    pub songs_selection: gtk::MultiSelection,
    pub artists_list: gtk::ListBox,
    /// Artist index of each Artists row.
    pub artist_order: RefCell<Vec<usize>>,
    pub playlists_list: gtk::ListBox,
    pub genres_list: gtk::ListBox,
    pub bar: PlayerBar,

    pub lib: RefCell<Rc<Library>>,
    pub stats: RefCell<Stats>,
    pub playlists: RefCell<Vec<Playlist>>,
    pub queue: RefCell<Queue<PathBuf>>,
    pub player: Option<Player>,
    pub mpris: Option<Mpris>,
    pub state: RefCell<AppState>,
    pub textures: RefCell<HashMap<(String, i32), Option<gdk::Texture>>>,
    pub rows: RefCell<Vec<Rc<RowHandle>>>,
    /// The open Now Playing page's widgets.
    pub now_playing: RefCell<Option<Rc<crate::nowplaying::NowPlaying>>>,
    /// The queue in play order: `BoxedAnyObject`s of (item index, path).
    pub queue_model: gio::ListStore,
    pub playlist_view: RefCell<Option<(glib::WeakRef<gtk::Box>, crate::pages::PlRef)>>,
    /// Track lists built now get all their rows at once (a page being
    /// rebuilt in place, whose scroll position must survive).
    pub all_rows_now: Cell<bool>,

    pub playing: Cell<bool>,
    pub scanning: Cell<bool>,
    pub scan_again: Cell<bool>,
    pub scanned_once: Cell<bool>,
    /// The running scan was asked for with Rescan Library (toast when done).
    pub user_rescan: Cell<bool>,
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
    /// Restored from the last run: the position to continue the current
    /// song from when it is first played (shown meanwhile, "Paused").
    pub resume_ms: Cell<Option<u64>>,
    /// Seek here once the song just loaded has started.
    pub pending_seek: Cell<Option<u64>>,
    pub queue_save_timer: RefCell<Option<glib::SourceId>>,
    /// Hidden window keeping the app alive for background playback.
    pub background_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// The window is closing for real (Quit), even with background playback.
    pub quitting: Cell<bool>,
    /// The volume before Mute (Ctrl+M), restored by the next Mute.
    pub unmuted_volume: Cell<f64>,
    /// Now Playing shows the lyrics (when the song has some).
    pub show_lyrics: Cell<bool>,
    pub suppress_ui: Cell<bool>,
}

impl App {
    pub fn data_dir() -> PathBuf {
        helix_app_common::data_dir(DATA)
    }

    /// Cached covers (see `index::COVER_SIZE`). Covers used to be cached
    /// at 256 px in `covers/`; tracks whose cover is missing here are read
    /// again on the next scan, and the old folder is then removed.
    pub fn covers_dir() -> PathBuf {
        glib::user_cache_dir().join(DATA).join("covers-512")
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
        s1.append(Some(tr("_New Playlist…")), Some("win.new-playlist"));
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
        // Buttons for one view, shown while it is visible.
        let new_playlist_btn = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(tr("New Playlist…"))
            .action_name("win.new-playlist")
            .visible(false)
            .build()
            .labelled(tr("New Playlist…"));
        header.pack_end(&new_playlist_btn);
        let songs_select = gtk::ToggleButton::builder()
            .icon_name("selection-mode-symbolic")
            .tooltip_text(tr("Select"))
            .visible(false)
            .build()
            .labelled(tr("Select"));
        header.pack_end(&songs_select);
        let sort_button = |action: &str, items: Vec<(&str, &str)>| {
            let menu = gio::Menu::new();
            for (label, nick) in items {
                let it = gio::MenuItem::new(Some(label), None);
                it.set_action_and_target_value(Some(action), Some(&nick.to_variant()));
                menu.append_item(&it);
            }
            let b = gtk::MenuButton::builder()
                .icon_name("view-sort-descending-symbolic")
                .menu_model(&menu)
                .tooltip_text(tr("Sort"))
                .visible(false)
                .build()
                .labelled(tr("Sort"));
            header.pack_end(&b);
            b.upcast::<gtk::Widget>()
        };
        let album_sort = sort_button(
            "win.album-sort",
            model::AlbumSort::ALL
                .iter()
                .map(|s| (s.label(), s.nick()))
                .collect(),
        );
        let artist_sort = sort_button(
            "win.artist-sort",
            model::ArtistSort::ALL
                .iter()
                .map(|s| (s.label(), s.nick()))
                .collect(),
        );
        let view_buttons: Vec<(&'static str, gtk::Widget)> = vec![
            ("playlists", new_playlist_btn.upcast()),
            ("songs", songs_select.clone().upcast()),
            ("albums", album_sort),
            ("artists", artist_sort),
        ];

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
        // Only as tall as its rows, not the whole view.
        let artists_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk::Align::Start)
            .build();
        let genres_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk::Align::Start)
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
        // Later scans run in the background: a spinner in the header.
        let scan_spinner = gtk::Spinner::builder()
            .spinning(true)
            .visible(false)
            .tooltip_text(tr("Scanning Library…"))
            .build()
            .labelled(tr("Scanning Library…"));
        header.pack_end(&scan_spinner);
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
                scanning_page,
                scan_spinner,
                search_bar,
                search_entry,
                search_box,
                view_buttons,
                albums_model,
                songs_model,
                songs_sorted,
                songs_select,
                songs_selection: gtk::MultiSelection::new(None::<gio::ListModel>),
                artists_list,
                artist_order: RefCell::default(),
                playlists_list,
                genres_list,
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
                now_playing: RefCell::default(),
                queue_model: gio::ListStore::new::<glib::BoxedAnyObject>(),
                playlist_view: RefCell::default(),
                all_rows_now: Cell::new(false),
                playing: Cell::new(false),
                scanning: Cell::new(false),
                scan_again: Cell::new(false),
                scanned_once: Cell::new(false),
                user_rescan: Cell::new(false),
                monitors: RefCell::default(),
                rescan_timer: RefCell::default(),
                inhibit_cookie: Cell::new(0),
                tracker_watching: Cell::new(false),
                using_tracker: Cell::new(false),
                played_ms: Cell::new(0),
                counted: Cell::new(false),
                last_tick: Cell::new(None),
                seeking: Cell::new(false),
                resume_ms: Cell::new(None),
                pending_seek: Cell::new(None),
                queue_save_timer: RefCell::default(),
                background_hold: RefCell::default(),
                quitting: Cell::new(false),
                unmuted_volume: Cell::new(1.0),
                show_lyrics: Cell::new(false),
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
                    app.bar.set_narrow(applied);
                    app.layout_now_playing(applied);
                }
            };
            if applied {
                app.narrow.connect_apply(relayout);
            } else {
                app.narrow.connect_unapply(relayout);
            }
        }
        // Covers are loaded for the scale they are shown at.
        let w = Rc::downgrade(&app);
        app.window.connect_scale_factor_notify(move |_| {
            if let Some(app) = w.upgrade() {
                app.textures.borrow_mut().clear();
            }
        });
        app.build_views();
        app.connect_player_bar();
        app.install_actions();
        app.install_keys();
        app.connect_search();
        let w = Rc::downgrade(&app);
        app.stack.connect_visible_child_name_notify(move |_| {
            if let Some(app) = w.upgrade() {
                app.update_view_buttons();
            }
        });
        app.connect_close();
        app.connect_notifications();
        let volume = app.state.borrow().volume;
        app.bar.volume.set_value(volume);
        app.apply_volume(volume);
        app.update_mode_buttons();
        app.restore_queue();
        app.refresh_library_views();
        app.update_now_playing();
        app.set_playing(false);

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

    /// Cover texture for a cover key, at least `px` device pixels wide
    /// (cached per size step, so small covers do not hold large images).
    pub fn texture(&self, key: Option<&str>, px: i32) -> Option<gdk::Texture> {
        let key = key?;
        let step = cover_step(px);
        let id = (key.to_string(), step);
        if let Some(t) = self.textures.borrow().get(&id) {
            return t.clone();
        }
        let tex = load_texture(&index::cover_path(&Self::covers_dir(), key), step);
        self.textures.borrow_mut().insert(id, tex.clone());
        tex
    }

    /// Shows a cover (from [`cover_widget`]): the art, sharp at the
    /// window's scale, or a placeholder tile with a small note icon.
    pub fn set_cover(&self, cover: &gtk::Overlay, key: Option<&str>) {
        let size = cover.width_request().max(1);
        let tex = self.texture(key, size * self.window.scale_factor().max(1));
        let mut child = cover.first_child();
        while let Some(c) = child {
            if let Some(p) = c.downcast_ref::<gtk::Picture>() {
                p.set_paintable(tex.as_ref());
                p.set_visible(tex.is_some());
            } else if let Some(img) = c.downcast_ref::<gtk::Image>() {
                img.set_pixel_size((size * 2 / 5).clamp(16, 64));
                img.set_icon_name(tex.is_none().then_some("audio-x-generic-symbolic"));
            }
            child = c.next_sibling();
        }
        if tex.is_some() {
            cover.remove_css_class("hm-cover-empty");
        } else {
            cover.add_css_class("hm-cover-empty");
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
        let (ptx, prx) = async_channel::unbounded::<usize>();
        if self.scanned_once.get() {
            self.scan_spinner.set_visible(true);
        }
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
                // A running count for the first scan's status page.
                let mut last = Instant::now();
                let mut progress = |n: usize| {
                    if last.elapsed() >= Duration::from_millis(200) {
                        last = Instant::now();
                        let _ = ptx.try_send(n);
                    }
                };
                let (tracks, report) = index::scan_with_progress(
                    &roots,
                    indexed,
                    previous,
                    Some(&covers),
                    now_secs(),
                    &mut progress,
                );
                drop(ptx);
                if report.changed() || !App::data_dir().join("library.json").exists() {
                    if let Err(e) = index::save(&App::data_dir().join("library.json"), &tracks) {
                        eprintln!("helix-music: cannot save index: {e}");
                    }
                }
                let old_covers = glib::user_cache_dir().join(DATA).join("covers");
                if old_covers.is_dir() {
                    let _ = std::fs::remove_dir_all(old_covers);
                }
                let ep = ep.filter(|_| used && !watching);
                let _ = tx.send_blocking((tracks, report, ep));
            })
            .expect("spawn scanner");
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(n) = prx.recv().await {
                let Some(app) = w.upgrade() else { return };
                app.scanning_page.set_description(Some(&trnf!(
                    "{n} song found",
                    "{n} songs found",
                    n
                )));
            }
        });
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok((tracks, report, ep)) = rx.recv().await else {
                return;
            };
            let Some(app) = w.upgrade() else { return };
            app.scanning.set(false);
            app.scan_spinner.set_visible(false);
            let asked = app.user_rescan.replace(false);
            app.using_tracker
                .set(report.from_tracker > 0 || ep.is_some());
            if let Some(ep) = ep {
                app.watch_tracker(ep);
            }
            let first = !app.scanned_once.replace(true);
            let text = trnf!(
                "Library updated: {n} song",
                "Library updated: {n} songs",
                tracks.len()
            );
            if asked {
                // Rescan Library: say what it found (the toast is also
                // spoken by libadwaita).
                app.toasts.add_toast(toast(if report.changed() {
                    &text
                } else {
                    tr("No changes found")
                }));
            } else if report.changed() && !first {
                // A rescan found changes while the user may be listening
                // or browsing; say so without moving focus.
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

    /// Adds tracks to the end of the queue, or (`next`) right after the
    /// current one; starts playing when the queue was empty.
    pub fn queue_tracks(self: &Rc<Self>, tracks: &[usize], next: bool) {
        let lib = self.lib();
        let paths: Vec<PathBuf> = tracks.iter().map(|&i| lib.tracks[i].path.clone()).collect();
        let was_empty = self.queue.borrow().is_empty();
        if next {
            self.queue.borrow_mut().play_next_all(paths);
        } else {
            self.queue.borrow_mut().append(paths);
        }
        if was_empty {
            self.start_current(true);
        } else {
            self.update_next_uri();
            self.update_now_playing();
            self.toasts.add_toast(toast(if next {
                tr("Will play next")
            } else {
                tr("Added to queue")
            }));
        }
    }

    /// Loads the queue's current track.
    pub fn start_current(self: &Rc<Self>, play: bool) {
        let Some(path) = self.queue.borrow().current().cloned() else {
            self.stop();
            return;
        };
        self.played_ms.set(0);
        self.counted.set(false);
        self.resume_ms.set(None);
        self.pending_seek.set(None);
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
            player::State::Stopped => {
                // A queue restored from the last run continues where it
                // stopped.
                let resume = self.resume_ms.take();
                self.start_current(true);
                self.pending_seek.set(resume);
            }
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

    /// Seeks by `delta_ms` from the current position (to the next song
    /// past the end).
    pub fn seek_by(self: &Rc<Self>, delta_ms: i64) {
        let Some(p) = &self.player else { return };
        if p.state() == player::State::Stopped {
            return;
        }
        let target = (self.position_ms() as i64 + delta_ms).max(0) as u64;
        match p.duration_ms() {
            Some(d) if target >= d => self.next(),
            _ => self.seek_ms(target),
        }
    }

    /// Changes the volume slider (and so the volume) by `delta`.
    pub fn change_volume(&self, delta: f64) {
        let v = (self.bar.volume.value() + delta).clamp(0.0, 1.0);
        self.bar.volume.set_value(v);
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
            Event::State(s) => {
                if s != player::State::Stopped {
                    if let (Some(ms), Some(p)) = (self.pending_seek.take(), &self.player) {
                        p.seek(ms);
                    }
                }
                self.set_playing(s == player::State::Playing);
            }
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
        let paused = self
            .player
            .as_ref()
            .is_some_and(|p| p.state() == player::State::Paused);
        let status = if playing {
            "Playing"
        } else if paused || self.resume_ms.get().is_some() {
            "Paused"
        } else {
            "Stopped"
        };
        if paused {
            // Remember where (a stopped player has no position to save).
            self.save_queue();
        }
        if !playing
            && !paused
            && !self.quitting.get()
            && !self.window.is_visible()
            && self.background_hold.borrow().is_some()
        {
            // Background playback ended: quit for real.
            self.quitting.set(true);
            self.window.close();
        }
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
                    self.refresh_rows();
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
        let pos = pos
            .or(self.resume_ms.get())
            .or_else(|| p.and_then(|p| p.position_ms()))
            .unwrap_or(0);
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
        self.update_lyrics_position(pos);
    }

    /// The playing position (0 when nothing plays).
    pub fn position_ms(&self) -> u64 {
        self.player
            .as_ref()
            .and_then(|p| p.position_ms())
            .unwrap_or(0)
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
                self.set_cover(&self.bar.cover, None);
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
        self.schedule_queue_save();
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
        // Off looks inactive; All and One are checked, like shuffle.
        let (icon, tip) = match repeat {
            Repeat::None => ("media-playlist-repeat-symbolic", tr("Repeat: Off")),
            Repeat::All => ("media-playlist-repeat-symbolic", tr("Repeat: All")),
            Repeat::One => ("media-playlist-repeat-song-symbolic", tr("Repeat: Song")),
        };
        self.suppress_ui.set(true);
        self.bar.repeat.set_active(repeat != Repeat::None);
        self.suppress_ui.set(false);
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
            Command::Quit => {
                self.quitting.set(true);
                self.window.close();
            }
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
    }

    /// Registers a track widget and shows its current state.
    pub fn register_row(&self, handle: Rc<RowHandle>) {
        self.refresh_row(&handle);
        self.rows.borrow_mut().push(handle);
    }

    /// Shows the current state in one registered widget (after a list
    /// cell was bound to another track).
    pub fn refresh_row(&self, handle: &RowHandle) {
        let current = self.queue.borrow().current().cloned();
        handle.update(current.as_ref(), &self.stats.borrow());
    }

    /// Updates playing indicators, hearts and play counts of all live
    /// track widgets. Toggling a heart here re-enters `set_favorite`,
    /// which returns early, so the registry is not borrowed meanwhile.
    pub fn refresh_rows(&self) {
        let current = self.queue.borrow().current().cloned();
        let rows: Vec<Rc<RowHandle>> = self.rows.borrow().clone();
        let mut dead = false;
        {
            let stats = self.stats.borrow();
            for r in &rows {
                dead |= !r.update(current.as_ref(), &stats);
            }
        }
        if dead {
            self.rows.borrow_mut().retain(|r| r.alive());
        }
    }

    // ----- window plumbing ---------------------------------------------------

    fn connect_close(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.window.connect_close_request(move |win| {
            if let Some(app) = w.upgrade() {
                app.state.borrow_mut().volume = app.bar.volume.value();
                app.save_state();
                app.save_queue();
                if app.state.borrow().background_playback
                    && app.playing.get()
                    && !app.quitting.get()
                {
                    // Keep playing: hide the window, keep the app (and MPRIS)
                    // running until playback stops, Quit, or it is shown.
                    win.set_visible(false);
                    app.background_hold.replace(Some(app.app.hold()));
                    return glib::Propagation::Stop;
                }
                // Closing for real: shutdown must not save again (the
                // player is stopped by then).
                app.quitting.set(true);
                app.stop();
                if app.inhibit_cookie.get() != 0 {
                    app.app.uninhibit(app.inhibit_cookie.replace(0));
                }
                app.background_hold.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        // Shown again (activation, MPRIS Raise): no longer held.
        let w = Rc::downgrade(self);
        self.window.connect_visible_notify(move |win| {
            if let (true, Some(app)) = (win.is_visible(), w.upgrade()) {
                app.background_hold.borrow_mut().take();
            }
        });
        // Ctrl+Q quits without closing the window first.
        let w = Rc::downgrade(self);
        self.app.connect_shutdown(move |_| {
            if let Some(app) = w.upgrade().filter(|a| !a.quitting.get()) {
                app.state.borrow_mut().volume = app.bar.volume.value();
                app.save_state();
                app.save_queue();
            }
        });
    }

    fn queue_file() -> PathBuf {
        Self::data_dir().join("queue.json")
    }

    /// Saves the queue and the position in the current song.
    pub fn save_queue(&self) {
        if let Some(id) = self.queue_save_timer.borrow_mut().take() {
            id.remove();
        }
        let file = QueueFile {
            queue: self.queue.borrow().save(),
            position_ms: self.resume_ms.get().unwrap_or_else(|| self.position_ms()),
        };
        if let Ok(data) = serde_json::to_vec(&file) {
            if let Err(e) = playlist::write_atomic(&Self::queue_file(), &data) {
                eprintln!("helix-music: cannot save the queue: {e}");
            }
        }
    }

    /// Saves the queue a moment after it changed (changes come in bursts).
    fn schedule_queue_save(self: &Rc<Self>) {
        if let Some(id) = self.queue_save_timer.borrow_mut().take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(app) = w.upgrade() {
                app.queue_save_timer.borrow_mut().take();
                app.save_queue();
            }
        });
        *self.queue_save_timer.borrow_mut() = Some(id);
    }

    /// Restores the last run's queue, paused at the saved position; songs
    /// no longer in the library are dropped.
    fn restore_queue(&self) {
        let Some(mut file) = std::fs::read(Self::queue_file())
            .ok()
            .and_then(|d| serde_json::from_slice::<QueueFile>(&d).ok())
        else {
            return;
        };
        let lib = self.lib();
        let kept = file.queue.retain(|p| lib.index_of(p).is_some());
        if self.queue.borrow_mut().restore(file.queue) && kept && file.position_ms > 0 {
            self.resume_ms.set(Some(file.position_ms));
        }
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
        add("seek-backward", |a| a.seek_by(-SEEK_STEP_MS));
        add("seek-forward", |a| a.seek_by(SEEK_STEP_MS));
        add("volume-up", |a| a.change_volume(VOLUME_STEP));
        add("volume-down", |a| a.change_volume(-VOLUME_STEP));
        add("mute", |a| {
            let v = a.bar.volume.value();
            if v > 0.0 {
                a.unmuted_volume.set(v);
                a.bar.volume.set_value(0.0);
            } else {
                a.bar
                    .volume
                    .set_value(a.unmuted_volume.get().max(VOLUME_STEP));
            }
        });
        add("toggle-favorite", |a| {
            if let Some(t) = a.current_track() {
                let on = !a.stats.borrow().is_favorite(&t.path);
                a.set_favorite(&t.path, on);
            }
        });
        add("rescan", |a| {
            a.user_rescan.set(true);
            a.start_scan();
        });
        add("preferences", |a| a.show_preferences());
        add("now-playing", |a| a.show_now_playing());
        add("new-playlist", |a| a.new_playlist_dialog(None));
        add("queue-clear", |a| {
            a.queue.borrow_mut().clear();
            a.stop();
            a.update_next_uri();
            a.update_now_playing();
        });
        add("queue-save", |a| {
            let lib = a.lib();
            let tracks: Vec<usize> = a
                .queue
                .borrow()
                .in_play_order()
                .into_iter()
                .filter_map(|(_, p)| lib.index_of(p))
                .collect();
            if !tracks.is_empty() {
                a.save_as_playlist_dialog(tracks);
            }
        });
        add("add-folder", |a| a.add_folder_dialog());
        add("import-playlist", |a| a.import_playlist_dialog());
        add("import-index-playlists", |a| a.import_index_playlists());

        // Sort orders, remembered in the state file.
        for (name, current) in [
            ("album-sort", self.state.borrow().album_sort.clone()),
            ("artist-sort", self.state.borrow().artist_sort.clone()),
        ] {
            let a = gio::SimpleAction::new_stateful(
                name,
                Some(glib::VariantTy::STRING),
                &current.to_variant(),
            );
            let w = Rc::downgrade(self);
            let albums = name == "album-sort";
            a.connect_change_state(move |a, v| {
                let (Some(app), Some(nick)) = (w.upgrade(), v.and_then(|v| v.get::<String>()))
                else {
                    return;
                };
                let valid = if albums {
                    model::AlbumSort::from_nick(&nick).is_some()
                } else {
                    model::ArtistSort::from_nick(&nick).is_some()
                };
                if !valid {
                    return;
                }
                a.set_state(&nick.to_variant());
                if albums {
                    app.state.borrow_mut().album_sort = nick;
                    app.sort_albums();
                } else {
                    app.state.borrow_mut().artist_sort = nick;
                    app.sort_artists();
                }
                app.save_state();
            });
            win.add_action(&a);
        }

        let view = gio::SimpleAction::new("view", Some(glib::VariantTy::STRING));
        let w = Rc::downgrade(self);
        view.connect_activate(move |_, v| {
            if let (Some(app), Some(name)) = (w.upgrade(), v.and_then(|v| v.get::<String>())) {
                app.search_bar.set_search_mode(false);
                app.nav.pop_to_tag("root");
                if name == "genres" && app.is_narrow() {
                    // No Genres view in the narrow bottom bar: its page.
                    app.stack.set_visible_child_name("artists");
                    app.show_genres_page();
                } else {
                    app.stack.set_visible_child_name(&name);
                }
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
        track_action("queue-add", |app, i| app.queue_tracks(&[i], false));
        track_action("play-next", |app, i| app.queue_tracks(&[i], true));
        track_action("add-to-playlist", |app, i| {
            app.add_to_playlist_dialog(vec![i])
        });
        track_action("go-album", |app, i| {
            if let Some(a) = app.lib().album_of(i) {
                app.show_album(a);
            }
        });
        track_action("song-details", |app, i| app.show_song_details(i));
        track_action("show-in-files", |app, i| {
            let path = app.lib().tracks[i].path.clone();
            app.show_in_files(&path);
        });
        track_action("go-artist", |app, i| {
            let lib = app.lib();
            if let Some(ar) = lib.artist_index(lib.tracks[i].artist_name()) {
                app.show_artist(ar);
            }
        });
        let a = gio::SimpleAction::new("show-artist", Some(glib::VariantTy::UINT32));
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, v| {
            if let (Some(app), Some(i)) = (w.upgrade(), v.and_then(|v| v.get::<u32>())) {
                if (i as usize) < app.lib().artists.len() {
                    app.show_artist(i as usize);
                }
            }
        });
        win.add_action(&a);

        // Actions on several tracks (target: library indices), from album,
        // artist and playlist menus and the Songs selection.
        let tracks_action = |name: &str, f: fn(&Rc<App>, Vec<usize>)| {
            let a = gio::SimpleAction::new(name, Some(&<Vec<u32>>::static_variant_type()));
            let w = Rc::downgrade(self);
            a.connect_activate(move |_, v| {
                let (Some(app), Some(t)) = (w.upgrade(), v.and_then(|v| v.get::<Vec<u32>>()))
                else {
                    return;
                };
                let n = app.lib().tracks.len();
                let t: Vec<usize> = t
                    .into_iter()
                    .map(|i| i as usize)
                    .filter(|&i| i < n)
                    .collect();
                if !t.is_empty() {
                    f(&app, t);
                }
            });
            win.add_action(&a);
        };
        tracks_action("tracks-play-next", |app, t| app.queue_tracks(&t, true));
        tracks_action("tracks-queue", |app, t| app.queue_tracks(&t, false));
        tracks_action("tracks-add-to-playlist", |app, t| {
            app.add_to_playlist_dialog(t)
        });
        tracks_action("play-tracks", |app, t| app.play_tracks(&t, 0, Some(false)));
        tracks_action("shuffle-tracks", |app, t| {
            app.play_tracks(&t, 0, Some(true))
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
                if app.songs_select.is_active() {
                    app.songs_select.set_active(false);
                    return glib::Propagation::Stop;
                }
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
        let bubble: Vec<(&str, &str)> = BUBBLE_ACCELS
            .iter()
            .flat_map(|(action, keys)| keys.iter().map(move |k| (*k, *action)))
            .collect();
        helix_app_common::add_bubble_shortcuts(&self.window, &bubble);
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

    /// Shows the header buttons of the visible view (none while searching
    /// or before the library is shown).
    pub fn update_view_buttons(&self) {
        let library = self.main_stack.visible_child_name().as_deref() == Some("library");
        let view = self.stack.visible_child_name();
        for (name, button) in &self.view_buttons {
            button.set_visible(library && view.as_deref() == Some(*name));
        }
        if !(library && view.as_deref() == Some("songs")) {
            self.songs_select.set_active(false);
        }
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
        self.update_view_buttons();
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
        // While the knob is dragged (or the trough is held) it follows the
        // pointer and the labels preview the time; one seek happens on
        // release. Keyboard steps seek at once. A legacy controller sees
        // the press and release without taking them from the scale's own
        // gestures.
        let held = gtk::EventControllerLegacy::new();
        held.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        held.connect_event(move |c, ev| {
            use gdk::EventType as E;
            let Some(app) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match ev.event_type() {
                E::ButtonPress | E::TouchBegin => app.seeking.set(true),
                E::ButtonRelease | E::TouchEnd | E::TouchCancel if app.seeking.replace(false) => {
                    if let Some(scale) = c.widget().and_downcast::<gtk::Scale>() {
                        app.seek_ms(scale.value().max(0.0) as u64);
                    }
                }
                _ => {}
            }
            glib::Propagation::Proceed
        });
        b.scale.add_controller(held);
        let w = Rc::downgrade(self);
        b.scale.connect_change_value(move |s, _, value| {
            if let Some(app) = w.upgrade() {
                if app.suppress_ui.get() {
                    return glib::Propagation::Proceed;
                }
                let (lo, hi) = (s.adjustment().lower(), s.adjustment().upper());
                let ms = value.clamp(lo, hi).max(0.0) as u64;
                if app.seeking.get() {
                    app.preview_position(ms, hi as u64);
                } else {
                    app.seek_ms(ms);
                }
            }
            glib::Propagation::Proceed
        });
    }

    /// Shows a position while the seek slider is dragged, without seeking.
    fn preview_position(&self, pos: u64, dur: u64) {
        self.bar.elapsed.set_label(&model::format_duration(pos));
        self.bar
            .scale
            .update_property(
                &[gtk::accessible::Property::ValueText(&model::position_text(
                    pos, dur,
                ))],
            );
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

/// The Songs table's playing cell: a play icon named "Now Playing".
pub fn show_playing_icon(img: &gtk::Image, playing: bool) {
    img.set_icon_name(playing.then_some("media-playback-start-symbolic"));
    if playing {
        img.update_property(&[gtk::accessible::Property::Label(tr("Now Playing"))]);
    } else {
        img.reset_property(gtk::AccessibleProperty::Label);
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

/// A square cover `size` logical pixels wide, with rounded corners:
/// the art fills it (cropped to the square), or a tinted tile with a
/// small note icon stands in for missing art. Decorative: the album or
/// track title next to it names it. Fill it with [`App::set_cover`].
pub fn cover_widget(size: i32) -> gtk::Overlay {
    // The tile: an image draws its icon centred in whatever size it has.
    let tile = gtk::Image::builder()
        .icon_name("audio-x-generic-symbolic")
        .width_request(size)
        .height_request(size)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .visible(false)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let cover = gtk::Overlay::builder()
        .child(&tile)
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["hm-cover", "hm-cover-empty"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    if size < 96 {
        cover.add_css_class("hm-cover-small");
    }
    // Overlay children do not count for its size: the tile sets it.
    cover.add_overlay(&picture);
    cover
}

/// Resizes a cover made by [`cover_widget`] (the caller shows it again).
pub fn resize_cover(cover: &gtk::Overlay, size: i32) {
    cover.set_size_request(size, size);
    if let Some(tile) = cover.child() {
        tile.set_size_request(size, size);
    }
    if size < 96 {
        cover.add_css_class("hm-cover-small");
    } else {
        cover.remove_css_class("hm-cover-small");
    }
}

/// The size a cover texture is loaded at for `px` device pixels.
pub fn cover_step(px: i32) -> i32 {
    match px {
        ..=64 => 64,
        65..=128 => 128,
        129..=256 => 256,
        _ => index::COVER_SIZE,
    }
}

/// Loads a cached cover scaled down to at most `size` pixels.
fn load_texture(path: &Path, size: i32) -> Option<gdk::Texture> {
    use gtk::gdk_pixbuf::{InterpType, Pixbuf};
    let pb = Pixbuf::from_file(path).ok()?;
    let (w, h) = (pb.width(), pb.height());
    let scale = (size as f64 / w.max(h).max(1) as f64).min(1.0);
    let pb = if scale < 1.0 {
        pb.scale_simple(
            ((w as f64 * scale).round() as i32).max(1),
            ((h as f64 * scale).round() as i32).max(1),
            InterpType::Bilinear,
        )?
    } else {
        pb
    };
    let format = if pb.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    Some(
        gdk::MemoryTexture::new(
            pb.width(),
            pb.height(),
            format,
            &pb.read_pixel_bytes(),
            pb.rowstride() as usize,
        )
        .upcast(),
    )
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
    /// row; a full-width seek slider on the second. Favourite, shuffle,
    /// repeat, volume and queue move into a "More Controls" popover.
    fn add_narrow_setters(&self, bp: &adw::Breakpoint) {
        bp.add_setter(&self.info_box, "width-request", Some(&(-1).to_value()));
        bp.add_setter(&self.more, "visible", Some(&true.to_value()));
    }

    fn set_narrow(&self, narrow: bool) {
        let pop = self.more.popover();
        if self.seek_box.parent().is_some() {
            self.grid.remove(&self.seek_box);
        }
        if narrow {
            if self.side_box.parent().as_ref() == Some(self.grid.upcast_ref()) {
                self.grid.remove(&self.side_box);
            }
            self.grid.attach(&self.seek_box, 0, 1, 2, 1);
            if let Some(pop) = pop {
                pop.set_child(Some(&self.side_box));
            }
        } else {
            if let Some(pop) = pop {
                pop.popdown();
                pop.set_child(None::<&gtk::Widget>);
            }
            self.grid.attach(&self.seek_box, 2, 0, 1, 1);
            if self.side_box.parent().is_none() {
                self.grid.attach(&self.side_box, 3, 0, 1, 1);
            }
        }
    }
}

fn build_player_bar() -> PlayerBar {
    let cover = cover_widget(48);
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
    let repeat = gtk::ToggleButton::builder()
        .icon_name("media-playlist-repeat-symbolic")
        .tooltip_text(tr("Repeat"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
        .labelled(tr("Repeat"));
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

    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text(tr("More Controls"))
        .popover(&gtk::Popover::new())
        .valign(gtk::Align::Center)
        .visible(false)
        .css_classes(["flat"])
        .build()
        .labelled(tr("More Controls"));
    controls.append(&more);
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
        more,
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
