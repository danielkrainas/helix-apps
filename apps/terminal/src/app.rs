//! Application: single instance, settings, and the list of windows.

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use helix_app_common::Prefs;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use vte4::prelude::*;

use crate::cli::{self, Parsed};
use crate::palette::{self, Palette, Theme};
use crate::search;
use crate::transparency::{self, Transparency};
use crate::window::Window;

pub const APP_ID: &str = "org.helix.Terminal";
pub const SCHEMA: &str = "org.gnome.Console";
const IFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const DEFAULT_SIZE: (i32, i32) = (800, 520);

const CSS: &str = "
vte-terminal.bell-flash { opacity: 0.6; }
/* Transparent terminal background: the window paints nothing behind the
   terminal, so the bars (which otherwise rely on it) get their own opaque
   background. Before the tints so those still win. */
window.transparent-background,
window.transparent-background taboverview,
window.transparent-background toolbarview,
window.transparent-background tabview,
window.transparent-background tabview scrolledwindow,
window.transparent-background tabview vte-terminal {
  background: none;
}
window.transparent-background .top-bar,
window.transparent-background searchbar > revealer > box {
  background-color: @headerbar_bg_color;
}
window.root-tint .top-bar, window.root-tint headerbar {
  background-color: mix(@headerbar_bg_color, #e01b24, 0.22);
}
window.remote-tint .top-bar, window.remote-tint headerbar {
  background-color: mix(@headerbar_bg_color, #9141ac, 0.22);
}
";

/// What a new tab should run and where.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub cwd: Option<PathBuf>,
    pub command: Vec<String>,
}

pub struct App {
    pub this: Weak<App>,
    pub gtk: adw::Application,
    pub prefs: Prefs,
    /// `org.helix.Terminal`: settings Console's schema has no key for.
    pub helix_prefs: Prefs,
    iface: Option<gio::Settings>,
    pub windows: RefCell<Vec<Rc<Window>>>,
}

pub fn run() -> glib::ExitCode {
    let gtk_app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let holder: Rc<RefCell<Option<Rc<App>>>> = Rc::default();

    gtk_app.connect_startup({
        let holder = holder.clone();
        move |gtk_app| {
            let app = App::new(gtk_app);
            app.install_actions();
            app.setup();
            *holder.borrow_mut() = Some(app);
        }
    });
    gtk_app.connect_activate({
        let holder = holder.clone();
        move |_| {
            if let Some(app) = holder.borrow().clone() {
                app.open(false, Launch::default());
            }
        }
    });
    gtk_app.connect_command_line({
        let holder = holder.clone();
        move |_, cmdline| {
            let Some(app) = holder.borrow().clone() else {
                return 1;
            };
            let args: Vec<String> = cmdline
                .arguments()
                .iter()
                .skip(1)
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let cwd = cmdline.cwd().unwrap_or_else(glib::home_dir);
            // --help, --version and invalid options were already handled by
            // the invoking process in main().
            match cli::parse(args, &cwd) {
                Ok(Parsed::Run(opts)) => {
                    let launch = Launch {
                        cwd: Some(opts.working_directory.unwrap_or(cwd)),
                        command: opts.command,
                    };
                    app.open(opts.tab, launch);
                    0
                }
                Ok(_) => 0,
                Err(_) => 2,
            }
        }
    });
    gtk_app.run()
}

impl App {
    fn new(gtk_app: &adw::Application) -> Rc<App> {
        Rc::new_cyclic(|this| App {
            this: this.clone(),
            gtk: gtk_app.clone(),
            prefs: Prefs::new(SCHEMA),
            helix_prefs: Prefs::new(transparency::SCHEMA),
            iface: helix_app_common::settings(IFACE_SCHEMA),
            windows: RefCell::default(),
        })
    }

    fn setup(&self) {
        // Some backends (e.g. Broadway) leave the DPI unset (-1); AdwTabButton
        // derives its counter's font size from it, so fall back to 96 DPI.
        if let Some(settings) = gtk::Settings::default() {
            if settings.gtk_xft_dpi() <= 0 {
                settings.set_gtk_xft_dpi(96 * 1024);
            }
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let wk = self.this.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(app) = wk.upgrade() {
                app.refresh_all();
            }
        });
        if let Some(s) = self.prefs.gsettings() {
            let wk = self.this.clone();
            s.connect_changed(None, move |_, key| {
                if let Some(app) = wk.upgrade() {
                    if !key.starts_with("last-window") {
                        app.refresh_all();
                    }
                }
            });
        }
        if let Some(s) = self.helix_prefs.gsettings() {
            let wk = self.this.clone();
            s.connect_changed(None, move |_, _| {
                if let Some(app) = wk.upgrade() {
                    app.refresh_all();
                }
            });
        }
        if let Some(s) = &self.iface {
            let wk = self.this.clone();
            s.connect_changed(Some("monospace-font-name"), move |_, _| {
                if let Some(app) = wk.upgrade() {
                    app.refresh_all();
                }
            });
        }
        self.apply_style();
    }

    fn install_actions(&self) {
        helix_app_common::install_standard_actions(&self.gtk, "Terminal", "Use the command line");
        // Ctrl+W / Ctrl+Q belong to programs running in the terminal.
        self.gtk.set_accels_for_action("app.quit", &[]);
        self.gtk
            .set_accels_for_action("window.close", &["<Control><Shift>q"]);

        let wk = self.this.clone();
        let prefs = gio::ActionEntry::builder("preferences")
            .activate(move |_: &adw::Application, _, _| {
                if let Some(app) = wk.upgrade() {
                    if let Some(w) = app.active_window() {
                        crate::prefs_dialog::show(&app, &w.win);
                    }
                }
            })
            .build();
        self.gtk.add_action_entries([prefs]);

        let accels: &[(&str, &[&str])] = &[
            ("app.preferences", &["<Control>comma"]),
            ("win.new-tab", &["<Control><Shift>t"]),
            ("win.new-window", &["<Control><Shift>n"]),
            ("win.close-tab", &["<Control><Shift>w"]),
            ("win.copy", &["<Control><Shift>c"]),
            ("win.paste", &["<Control><Shift>v"]),
            ("win.select-all", &["<Control><Shift>a"]),
            ("win.find", &["<Control><Shift>f"]),
            ("win.find-next", &["<Control><Shift>g"]),
            (
                "win.zoom-in",
                &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
            ),
            ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
            ("win.zoom-reset", &["<Control>0", "<Control>KP_0"]),
            ("win.tab-overview", &["<Control><Shift>o"]),
            ("win.fullscreen", &["F11"]),
            ("win.show-shortcuts", &["<Control>question"]),
        ];
        for (action, keys) in accels {
            self.gtk.set_accels_for_action(action, keys);
        }
    }

    pub fn active_window(&self) -> Option<Rc<Window>> {
        let active = self.gtk.active_window();
        let windows = self.windows.borrow();
        active
            .and_then(|a| {
                windows
                    .iter()
                    .find(|w| w.win.upcast_ref::<gtk::Window>() == &a)
                    .cloned()
            })
            .or_else(|| windows.last().cloned())
    }

    /// Opens `launch` in a new window, or in a new tab of the active window.
    pub fn open(&self, tab: bool, launch: Launch) {
        let win = match self.active_window() {
            Some(w) if tab => w,
            _ => self.new_window(),
        };
        win.new_tab(launch);
        win.win.present();
    }

    pub fn new_window(&self) -> Rc<Window> {
        let app = self.this.upgrade().expect("app alive");
        let w = Window::new(&app);
        if self.prefs.get("restore-window-size", true) {
            let (width, height) = self.prefs.get("last-window-size", (-1i32, -1i32));
            if width > 0 && height > 0 {
                w.win.set_default_size(width, height);
            } else {
                w.win.set_default_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
            }
            if self.prefs.get("last-window-maximised", false) {
                w.win.maximize();
            }
        } else {
            w.win.set_default_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
        }
        self.windows.borrow_mut().push(w.clone());
        w
    }

    pub fn remove_window(&self, w: &Rc<Window>) {
        if self.prefs.get("restore-window-size", true) {
            let (width, height) = w.win.default_size();
            if !w.win.is_maximized() && width > 0 && height > 0 {
                self.prefs.set("last-window-size", (width, height));
            }
            self.prefs
                .set("last-window-maximised", w.win.is_maximized());
        }
        self.windows.borrow_mut().retain(|x| !Rc::ptr_eq(x, w));
    }

    // --- settings -------------------------------------------------------

    pub fn theme(&self) -> Theme {
        Theme::from_nick(&self.prefs.get_enum_nick("theme", "night"))
    }

    pub fn set_theme(&self, t: Theme) {
        self.prefs.set_enum_nick("theme", t.nick());
        self.apply_style();
        self.refresh_all();
    }

    fn apply_style(&self) {
        let scheme = match self.theme() {
            Theme::Auto => adw::ColorScheme::Default,
            Theme::Day => adw::ColorScheme::ForceLight,
            Theme::Night | Theme::Hacker => adw::ColorScheme::ForceDark,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    pub fn font_scale(&self) -> f64 {
        search::clamp_scale(self.prefs.get("font-scale", 1.0f64))
    }

    pub fn set_font_scale(&self, s: f64) {
        self.prefs.set("font-scale", search::clamp_scale(s));
        self.refresh_all();
    }

    pub fn font(&self) -> pango::FontDescription {
        let name = if self.prefs.get("use-system-font", true) {
            self.iface
                .as_ref()
                .map(|s| s.string("monospace-font-name").to_string())
                .unwrap_or_else(|| "Monospace 11".into())
        } else {
            self.prefs.get("custom-font", "Monospace 11".to_string())
        };
        let mut desc = pango::FontDescription::from_string(&name);
        if desc.family().is_none() {
            desc.set_family("Monospace");
        }
        if desc.size() <= 0 {
            desc.set_size(11 * pango::SCALE);
        }
        desc
    }

    pub fn transparency(&self) -> Transparency {
        Transparency::load(&self.helix_prefs)
    }

    pub fn palette(&self) -> &'static Palette {
        palette::palette_for(self.theme(), adw::StyleManager::default().is_dark())
    }

    /// Applies every setting to `term`.
    pub fn apply_to(&self, term: &vte4::Terminal) {
        term.set_font(Some(&self.font()));
        term.set_font_scale(self.font_scale());
        // `scrollback-lines` < 0 means unlimited (VTE's convention).
        let lines: i64 = self.prefs.get("scrollback-lines", 10000i64).max(-1);
        term.set_scrollback_lines(lines as _);
        term.set_audible_bell(self.prefs.get("audible-bell", true));

        let p = self.palette();
        let rgba = |s: &str| {
            let (r, g, b) = palette::parse_hex(s).unwrap_or((0.5, 0.5, 0.5));
            gdk::RGBA::new(r, g, b, 1.0)
        };
        let colors: Vec<gdk::RGBA> = p.colors.iter().map(|c| rgba(c)).collect();
        let refs: Vec<&gdk::RGBA> = colors.iter().collect();
        // Only the default background gets alpha; text stays opaque.
        let mut background = rgba(p.background);
        background.set_alpha(self.transparency().background_alpha());
        term.set_colors(Some(&rgba(p.foreground)), Some(&background), &refs);
        term.set_color_cursor(Some(&rgba(p.cursor)));
        term.set_color_cursor_foreground(Some(&rgba(p.cursor_foreground)));
        term.set_color_highlight(Some(&rgba(p.highlight)));
        term.set_color_highlight_foreground(Some(&rgba(p.highlight_foreground)));
    }

    pub fn visual_bell(&self) -> bool {
        self.prefs.get("visual-bell", true)
    }

    /// Lets the window background show the terminal's alpha through.
    pub fn apply_to_window(&self, win: &adw::ApplicationWindow) {
        if self.transparency().is_see_through() {
            win.add_css_class("transparent-background");
        } else {
            win.remove_css_class("transparent-background");
        }
    }

    pub fn refresh_all(&self) {
        let windows = self.windows.borrow().clone();
        for w in windows {
            self.apply_to_window(&w.win);
            for t in w.tabs.borrow().iter() {
                self.apply_to(&t.term);
            }
        }
    }
}
