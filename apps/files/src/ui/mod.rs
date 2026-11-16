//! GTK user interface.

use helix_app_common::i18n::tr;

mod dbus;
mod dialogs;
mod jobs;
mod properties;
mod shortcuts;
mod sidebar;
mod tab;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use crate::sorting::SortKey;
pub use window::Window;

pub const APP_ID: &str = "org.helixdesktop.Files";
const PREFS: &str = "org.gnome.nautilus.preferences";
const WINDOW_STATE: &str = "org.gnome.nautilus.window-state";
const LIST_VIEW: &str = "org.gnome.nautilus.list-view";
const ICON_VIEW: &str = "org.gnome.nautilus.icon-view";
const CHOOSER: &str = "org.gtk.gtk4.Settings.FileChooser";

/// Pseudo location of the "Other Locations" page.
pub const OTHER_LOCATIONS: &str = "other-locations:";

pub const GRID_ZOOM: [(&str, i32); 5] = [
    ("small", 48),
    ("small-plus", 64),
    ("medium", 96),
    ("large", 128),
    ("extra-large", 256),
];
pub const LIST_ZOOM: [(&str, i32); 3] = [("small", 16), ("medium", 32), ("large", 48)];

/// Attributes queried for every listed file.
pub const FILE_ATTRS: &str = "standard::name,standard::display-name,standard::edit-name,standard::type,\
standard::size,standard::icon,standard::symbolic-icon,standard::content-type,standard::fast-content-type,\
standard::is-hidden,standard::is-backup,standard::is-symlink,standard::symlink-target,standard::target-uri,\
time::modified,unix::mode,access::can-read,access::can-write,access::can-trash,access::can-delete,\
access::can-rename,thumbnail::path,thumbnail::is-valid,trash::orig-path,trash::deletion-date";

/// State of our own clipboard content.
#[derive(Clone)]
pub struct ClipState {
    pub files: Vec<gio::File>,
    pub cut: bool,
}

pub enum Undo {
    Trash(Vec<gio::File>),
    Rename { file: gio::File, old_name: String },
}

/// Settings that influence how folders are shown.
#[derive(Clone, PartialEq)]
pub struct ViewSettings {
    pub show_hidden: bool,
    pub sort_key: SortKey,
    pub reversed: bool,
    pub dirs_first: bool,
    pub single_click: bool,
    pub thumbnails: String,
    pub detailed_dates: bool,
    pub grid_zoom: usize,
    pub list_zoom: usize,
    pub list_view: bool,
    pub columns: Vec<String>,
    pub show_delete: bool,
    pub show_link: bool,
    pub recursive: String,
    pub location_entry: bool,
    pub extra_buttons: bool,
    pub back_button: u32,
    pub forward_button: u32,
}

pub struct Shared {
    pub app: adw::Application,
    pub prefs: Prefs,
    pub window_state: Prefs,
    pub list_prefs: Prefs,
    pub icon_prefs: Prefs,
    pub chooser_prefs: Prefs,
    pub clipboard: RefCell<Option<ClipState>>,
    pub undo: RefCell<Vec<Undo>>,
    pub windows: RefCell<Vec<Rc<Window>>>,
}

fn zoom_index(table: &[(&str, i32)], nick: &str, default: usize) -> usize {
    table
        .iter()
        .position(|(n, _)| *n == nick)
        .unwrap_or(default)
}

impl Shared {
    pub fn view_settings(&self) -> ViewSettings {
        let p = &self.prefs;
        ViewSettings {
            show_hidden: p.get("show-hidden-files", false),
            sort_key: SortKey::from_nick(&p.get_enum_nick("default-sort-order", "name")),
            reversed: p.get("default-sort-in-reverse-order", false),
            dirs_first: self.chooser_prefs.get("sort-directories-first", true),
            single_click: p.get_enum_nick("click-policy", "double") == "single",
            thumbnails: p.get_enum_nick("show-image-thumbnails", "local-only"),
            detailed_dates: p.get_enum_nick("date-time-format", "simple") == "detailed",
            grid_zoom: zoom_index(
                &GRID_ZOOM,
                &self
                    .icon_prefs
                    .get_enum_nick("default-zoom-level", "medium"),
                2,
            ),
            list_zoom: zoom_index(
                &LIST_ZOOM,
                &self
                    .list_prefs
                    .get_enum_nick("default-zoom-level", "medium"),
                1,
            ),
            list_view: p.get_enum_nick("default-folder-viewer", "icon-view") == "list-view",
            columns: self.list_prefs.get(
                "default-visible-columns",
                vec![
                    "name".to_string(),
                    "size".to_string(),
                    "date_modified".to_string(),
                ],
            ),
            show_delete: p.get("show-delete-permanently", false),
            show_link: p.get("show-create-link", false),
            recursive: p.get_enum_nick("recursive-search", "local-only"),
            location_entry: p.get("always-use-location-entry", false),
            extra_buttons: p.get("mouse-use-extra-buttons", true),
            back_button: p.get("mouse-back-button", 8u32),
            forward_button: p.get("mouse-forward-button", 9u32),
        }
    }

    /// Re-applies settings to every open window.
    pub fn settings_changed(&self) {
        let windows: Vec<Rc<Window>> = self.windows.borrow().clone();
        for w in windows {
            w.apply_settings();
        }
    }

    pub fn active_window(&self) -> Option<Rc<Window>> {
        let active = self.app.active_window();
        let windows = self.windows.borrow();
        active
            .and_then(|a| {
                windows
                    .iter()
                    .find(|w| w.window.upcast_ref::<gtk::Window>() == &a)
                    .cloned()
            })
            .or_else(|| windows.last().cloned())
    }

    pub fn remove_window(&self, w: &Window) {
        self.windows
            .borrow_mut()
            .retain(|x| !std::ptr::eq(x.as_ref(), w));
    }
}

/// Opens `files` the way the command line / D-Bus ask: folders open in a
/// new window (further ones as tabs), other files open their parent folder
/// with the file selected (`select` forces that for folders too).
///
/// `startup_id` (from D-Bus callers) is the startup-notification /
/// XDG activation token handed to the first new window.
pub fn open_locations(
    shared: &Rc<Shared>,
    files: Vec<gio::File>,
    select: bool,
    properties: bool,
    startup_id: Option<String>,
) {
    let present = move |w: &gtk::Window| {
        if let Some(id) = startup_id.as_deref().filter(|id| !id.is_empty()) {
            w.set_startup_id(id);
        }
        w.present();
    };
    if files.is_empty() {
        present(Window::new(shared, None, vec![]).window.upcast_ref());
        return;
    }
    let shared = shared.clone();
    let guard = shared.app.hold();
    glib::spawn_future_local(async move {
        let _guard = guard;
        // Group into (folder, items to select).
        let mut groups: Vec<(gio::File, Vec<gio::File>)> = Vec::new();
        for f in files {
            let is_dir = match f
                .query_info_future(
                    "standard::type",
                    gio::FileQueryInfoFlags::NONE,
                    glib::Priority::DEFAULT,
                )
                .await
            {
                Ok(info) => matches!(
                    info.file_type(),
                    gio::FileType::Directory | gio::FileType::Mountable
                ),
                // Unmounted remote locations: treat as folders, the view mounts them.
                Err(_) => f.path().is_none(),
            };
            let (folder, item) = if (select || !is_dir) && f.parent().is_some() {
                (f.parent().unwrap(), Some(f))
            } else {
                (f, None)
            };
            match groups.iter_mut().find(|(g, _)| g.equal(&folder)) {
                Some((_, items)) => items.extend(item),
                None => groups.push((folder, item.into_iter().collect())),
            }
        }
        let mut win: Option<Rc<Window>> = None;
        for (folder, items) in groups {
            match &win {
                None => {
                    let w = Window::new(&shared, Some(folder), items.clone());
                    present(w.window.upcast_ref());
                    if properties && !items.is_empty() {
                        properties::show(&w, items);
                    }
                    win = Some(w);
                }
                Some(w) => w.open_tab(&folder, items, true),
            }
        }
    });
}

fn load_css() {
    // The path separators use the dim-label class. A cut item is faded as
    // a whole (its state, also given in its accessible description), less
    // so in high contrast.
    helix_app_common::style::add_css(
        ".crumb { padding: 2px 6px; min-height: 24px; }
         .grid-item { padding: 6px 4px; }
         .grid-item label { margin-top: 2px; }
         .files-status { padding: 3px 12px; }
         .cut-item { opacity: 0.5; }
         .drop-highlight { background-color: alpha(@accent_bg_color, 0.25); border-radius: 6px; }
         .sidebar-eject { padding: 0; min-height: 24px; min-width: 24px; }",
        ".cut-item { opacity: 0.75; }
         .drop-highlight { background-color: alpha(@accent_bg_color, 0.4); outline: 2px solid @accent_color; outline-offset: -2px; }",
    );
}

fn install_app_actions(shared: &Rc<Shared>) {
    let app = &shared.app;
    let s = shared.clone();
    let new_window = gio::ActionEntry::builder("new-window")
        .activate(move |_: &adw::Application, _, _| {
            let loc = s
                .active_window()
                .and_then(|w| w.current_tab())
                .map(|t| t.location());
            Window::new(&s, loc, vec![]).window.present();
        })
        .build();
    let s = shared.clone();
    let prefs = gio::ActionEntry::builder("preferences")
        .activate(move |_: &adw::Application, _, _| {
            if let Some(w) = s.active_window() {
                dialogs::preferences(&w);
            }
        })
        .build();
    app.add_action_entries([new_window, prefs]);

    for (action, keys) in ACCELS {
        app.set_accels_for_action(action, keys);
    }
}

/// Application accelerators: (action, keys). The Keyboard Shortcuts
/// window lists every one of them (checked by a test in `shortcuts`).
pub const ACCELS: &[(&str, &[&str])] = &[
    ("app.new-window", &["<Control>n"]),
    ("app.preferences", &["<Control>comma"]),
    ("win.new-tab", &["<Control>t"]),
    ("win.close-tab", &["<Control>w"]),
    ("win.back", &["<Alt>Left", "Back"]),
    ("win.forward", &["<Alt>Right", "Forward"]),
    ("win.up", &["<Alt>Up"]),
    ("win.home", &["<Alt>Home"]),
    ("win.reload", &["F5", "<Control>r"]),
    ("win.location", &["<Control>l"]),
    ("win.search", &["<Control>f"]),
    ("win.view-mode::grid", &["<Control>1"]),
    ("win.view-mode::list", &["<Control>2"]),
    (
        "win.zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
    ),
    ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
    ("win.zoom-reset", &["<Control>0", "<Control>KP_0"]),
    ("win.show-hidden", &["<Control>h"]),
    ("win.bookmark", &["<Control>d"]),
    ("win.new-folder", &["<Shift><Control>n"]),
    ("win.properties", &["<Alt>Return", "<Control>i"]),
    ("win.toggle-sidebar", &["F9"]),
    ("win.invert-selection", &["<Shift><Control>i"]),
    ("win.show-shortcuts", &["<Control>question"]),
];

fn arg_strings(cmd: &gio::ApplicationCommandLine) -> Vec<String> {
    cmd.arguments()
        .iter()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.add_main_option(
        "new-window",
        b'w'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        tr("Always open a new window"),
        None,
    );
    app.add_main_option(
        "select",
        b's'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        tr("Select the given items in their parent folders"),
        None,
    );
    app.add_main_option(
        "quit",
        b'q'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        tr("Quit the running instance"),
        None,
    );
    helix_app_common::install_standard_actions(&app, tr("Files"), tr("Access and organize files"));
    // Ctrl+W closes a tab instead of the whole window.
    app.set_accels_for_action("window.close", &[]);

    let shared: Rc<OnceCell<Rc<Shared>>> = Rc::default();
    let cell = shared.clone();
    app.connect_startup(move |app| {
        load_css();
        let s = Rc::new(Shared {
            app: app.clone(),
            prefs: Prefs::new(PREFS),
            window_state: Prefs::new(WINDOW_STATE),
            list_prefs: Prefs::new(LIST_VIEW),
            icon_prefs: Prefs::new(ICON_VIEW),
            chooser_prefs: Prefs::new(CHOOSER),
            clipboard: RefCell::default(),
            undo: RefCell::default(),
            windows: RefCell::default(),
        });
        // External changes (e.g. `gsettings set`) apply live.
        for p in [&s.prefs, &s.list_prefs, &s.icon_prefs, &s.chooser_prefs] {
            if let Some(g) = p.gsettings() {
                let weak = Rc::downgrade(&s);
                g.connect_changed(None, move |_, _| {
                    if let Some(s) = weak.upgrade() {
                        // Defer: our own writes trigger this synchronously.
                        glib::idle_add_local_once(move || s.settings_changed());
                    }
                });
            }
        }
        install_app_actions(&s);
        dbus::register(&s);
        // Started by D-Bus activation (`--gapplication-service`, see
        // data/org.helixdesktop.Files.FileManager1.service): no window is opened
        // until a method call arrives; linger briefly once idle.
        if app.flags().contains(gio::ApplicationFlags::IS_SERVICE) {
            app.set_inactivity_timeout(10_000);
        }
        let _ = cell.set(s);
    });

    let cell = shared.clone();
    app.connect_command_line(move |app, cmd| {
        let Some(s) = cell.get() else { return 1 };
        let dict = cmd.options_dict();
        let mut opts = match crate::cli::parse(arg_strings(cmd)) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("{e}\n{}", crate::cli::USAGE);
                return 1;
            }
        };
        opts.new_window |= dict.contains("new-window");
        opts.select |= dict.contains("select");
        opts.quit |= dict.contains("quit");
        if opts.help {
            println!("{}", crate::cli::USAGE);
            return 0;
        }
        if opts.quit {
            app.quit();
            return 0;
        }
        let files: Vec<gio::File> = opts
            .locations
            .iter()
            .map(|l| cmd.create_file_for_arg(l.as_str()))
            .collect();
        if files.is_empty() && !opts.new_window {
            if let Some(w) = s.active_window() {
                w.window.present();
                return 0;
            }
        }
        open_locations(s, files, opts.select, false, None);
        0
    });
    app.connect_activate(move |_| {
        if let Some(s) = shared.get() {
            match s.active_window() {
                Some(w) => w.window.present(),
                None => Window::new(s, None, vec![]).window.present(),
            }
        }
    });
    app.run()
}
