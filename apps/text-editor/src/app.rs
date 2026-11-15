//! Application state: settings, windows, session and drafts.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Prefs;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::doc::Doc;
use crate::encoding::{LineEnding, TextFormat};
use crate::session::{self, Session, Store, TabState, WindowState};
use crate::settings::{self, FontSpec, Settings, SCHEMA};
use crate::window::Window;

pub const APP_ID: &str = "org.helix.TextEditor";
pub const APP_DIR: &str = "helix-text-editor";
const AUTOSAVE_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

pub struct App {
    pub this: Weak<App>,
    pub gtk: adw::Application,
    pub prefs: Prefs,
    pub settings: RefCell<Settings>,
    iface: Option<gio::Settings>,
    zoom: Cell<i32>,
    css: gtk::CssProvider,
    pub store: Store,
    pub standalone: bool,
    pub windows: RefCell<Vec<Rc<Window>>>,
    pub docs: RefCell<Vec<Rc<Doc>>>,
    restored: Cell<bool>,
    pub recent: RefCell<Vec<String>>,
    autosave_pending: Cell<bool>,
    /// Set while quitting so windows close without prompting.
    pub quitting: Cell<bool>,
}

pub fn run(standalone: bool, files: Vec<String>) -> glib::ExitCode {
    let mut flags = gio::ApplicationFlags::HANDLES_OPEN;
    if standalone {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let gtk_app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(flags)
        .build();
    let holder: Rc<RefCell<Option<Rc<App>>>> = Rc::default();

    gtk_app.connect_startup({
        let holder = holder.clone();
        move |gtk_app| {
            let app = App::new(gtk_app, standalone);
            app.install_actions();
            app.apply_settings();
            *holder.borrow_mut() = Some(app);
        }
    });
    gtk_app.connect_activate({
        let holder = holder.clone();
        move |_| {
            let Some(app) = holder.borrow().clone() else {
                return;
            };
            match app.active_window() {
                Some(w) => w.win.present(),
                None => {
                    let w = app.new_window(true);
                    if w.tab_view.n_pages() == 0 {
                        w.new_tab();
                    }
                    w.win.present();
                }
            }
        }
    });
    gtk_app.connect_open({
        let holder = holder.clone();
        move |_, files, _| {
            let Some(app) = holder.borrow().clone() else {
                return;
            };
            let win = app.active_window().unwrap_or_else(|| app.new_window(true));
            for f in files {
                win.open_file(f.clone());
            }
            win.win.present();
        }
    });
    gtk_app.connect_shutdown({
        let holder = holder.clone();
        move |_| {
            if let Some(app) = holder.borrow_mut().take() {
                app.shutdown();
            }
        }
    });
    let mut argv = vec![std::env::args()
        .next()
        .unwrap_or_else(|| "helix-text-editor".into())];
    argv.extend(files);
    gtk_app.run_with_args(&argv)
}

/// Application accelerators: (action, keys). The shortcuts window lists
/// every one of them (checked by a test in `shortcuts`).
pub const ACCELS: &[(&str, &[&str])] = &[
    ("app.quit", &["<Control>q"]),
    ("app.new-window", &["<Control>n"]),
    ("app.preferences", &["<Control>comma"]),
    (
        "app.zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
    ),
    ("app.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
    ("app.zoom-reset", &["<Control>0", "<Control>KP_0"]),
    ("window.close", &["<Control><Shift>w"]),
    ("win.new-tab", &["<Control>t"]),
    ("win.open", &["<Control>o"]),
    ("win.save", &["<Control>s"]),
    ("win.save-as", &["<Control><Shift>s"]),
    ("win.close-tab", &["<Control>w"]),
    ("win.find", &["<Control>f"]),
    ("win.replace", &["<Control>h"]),
    ("win.find-next", &["<Control>g"]),
    ("win.find-previous", &["<Control><Shift>g"]),
    ("win.goto-line", &["<Control>i"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

impl App {
    fn new(gtk_app: &adw::Application, standalone: bool) -> Rc<App> {
        let prefs = Prefs::new(SCHEMA);
        let settings = Settings::load(&prefs);
        let store = Store::new(helix_app_common::data_dir(APP_DIR));
        let recent = store.load_recent();
        let css = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            helix_app_common::style::add_css(
                ".change-banner { padding: 6px 12px; background: alpha(@accent_bg_color, 0.15); }
                 .find-bar { padding: 6px; border-bottom: 1px solid alpha(currentColor, 0.15); }
                 .statusbar { padding: 0 6px; border-top: 1px solid alpha(currentColor, 0.12); }
                 .statusbar button { min-height: 24px; padding: 2px 8px; }
                 linegutter { background: alpha(currentColor, 0.03); }",
                // Separators reach 3:1; the gutter is set apart by a line.
                ".change-banner { background: alpha(@accent_bg_color, 0.3); }
                 .find-bar { border-bottom-color: alpha(currentColor, 0.6); }
                 .statusbar { border-top-color: alpha(currentColor, 0.6); }
                 linegutter { background: none; border-right: 1px solid alpha(currentColor, 0.6); }",
            );
        }
        let app = Rc::new_cyclic(|this| App {
            this: this.clone(),
            gtk: gtk_app.clone(),
            prefs,
            settings: RefCell::new(settings),
            iface: helix_app_common::settings("org.gnome.desktop.interface"),
            zoom: Cell::new(0),
            css,
            store,
            standalone,
            windows: RefCell::default(),
            docs: RefCell::default(),
            restored: Cell::new(false),
            recent: RefCell::new(recent),
            autosave_pending: Cell::new(false),
            quitting: Cell::new(false),
        });
        if let Some(gs) = app.prefs.gsettings() {
            let w = app.this.clone();
            gs.connect_changed(None, move |_, _| {
                if let Some(app) = w.upgrade() {
                    app.reload_settings();
                }
            });
        }
        if let Some(iface) = &app.iface {
            let w = app.this.clone();
            iface.connect_changed(Some("monospace-font-name"), move |_, _| {
                if let Some(app) = w.upgrade() {
                    app.apply_settings();
                }
            });
        }
        app
    }

    pub fn rc(&self) -> Rc<App> {
        self.this.upgrade().expect("app alive")
    }

    fn install_actions(&self) {
        helix_app_common::install_standard_actions(&self.gtk, "Text Editor", "Edit text files");
        // Replace the standard quit so sessions are saved and prompts shown.
        let w = self.this.clone();
        let quit = gio::SimpleAction::new("quit", None);
        quit.connect_activate(move |_, _| {
            if let Some(app) = w.upgrade() {
                app.quit();
            }
        });
        self.gtk.add_action(&quit);
        let w = self.this.clone();
        let new_window = gio::SimpleAction::new("new-window", None);
        new_window.connect_activate(move |_, _| {
            if let Some(app) = w.upgrade() {
                let win = app.new_window(false);
                win.new_tab();
                win.win.present();
            }
        });
        self.gtk.add_action(&new_window);
        let w = self.this.clone();
        let prefs = gio::SimpleAction::new("preferences", None);
        prefs.connect_activate(move |_, _| {
            if let Some(app) = w.upgrade() {
                if let Some(win) = app.active_window() {
                    crate::prefs::show(&app, &win.win);
                }
            }
        });
        self.gtk.add_action(&prefs);
        for (name, step) in [("zoom-in", 1), ("zoom-out", -1), ("zoom-reset", 0)] {
            let w = self.this.clone();
            let a = gio::SimpleAction::new(name, None);
            a.connect_activate(move |_, _| {
                if let Some(app) = w.upgrade() {
                    app.zoom(step);
                }
            });
            self.gtk.add_action(&a);
        }
        for (action, keys) in ACCELS {
            self.gtk.set_accels_for_action(action, keys);
        }
    }

    // ------------------------------------------------------------ settings

    pub fn reload_settings(&self) {
        let new = Settings::load(&self.prefs);
        if *self.settings.borrow() != new {
            *self.settings.borrow_mut() = new;
            self.apply_settings();
        }
    }

    fn font(&self, s: &Settings) -> FontSpec {
        let name = if s.use_system_font {
            self.iface
                .as_ref()
                .map(|i| i.string("monospace-font-name").to_string())
                .unwrap_or_else(|| "Monospace 11".into())
        } else {
            s.custom_font.clone()
        };
        let desc = gtk::pango::FontDescription::from_string(&name);
        use glib::translate::IntoGlib;
        FontSpec {
            family: desc
                .family()
                .map(|f| f.to_string())
                .unwrap_or_else(|| "Monospace".into()),
            size: desc.size() as f64 / gtk::pango::SCALE as f64,
            size_px: desc.is_size_absolute(),
            weight: desc.weight().into_glib().clamp(100, 1000) as u32,
            italic: matches!(
                desc.style(),
                gtk::pango::Style::Italic | gtk::pango::Style::Oblique
            ),
        }
    }

    pub fn apply_settings(&self) {
        let s = self.settings.borrow().clone();
        let scheme = match s.style_variant.as_str() {
            "light" => adw::ColorScheme::ForceLight,
            "dark" => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
        self.css.load_from_string(&settings::font_css(
            "textview.helix-editor",
            &self.font(&s),
            settings::zoom_factor(self.zoom.get()),
        ));
        for d in self.docs.borrow().iter() {
            d.apply_settings(&s);
        }
        for w in self.windows.borrow().iter() {
            w.update_status();
        }
    }

    pub fn set_pref<T: glib::variant::ToVariant>(&self, key: &str, value: T) {
        self.prefs.set(key, value);
        self.reload_settings();
    }

    pub fn set_pref_nick(&self, key: &str, nick: &str) {
        self.prefs.set_enum_nick(key, nick);
        self.reload_settings();
    }

    /// The editor zoom level in percent.
    pub fn zoom_percent(&self) -> i32 {
        (settings::zoom_factor(self.zoom.get()) * 100.0).round() as i32
    }

    pub fn zoom(&self, step: i32) {
        let z = if step == 0 {
            0
        } else {
            (self.zoom.get() + step).clamp(settings::ZOOM_MIN, settings::ZOOM_MAX)
        };
        self.zoom.set(z);
        self.apply_settings();
        if let Some(w) = self.active_window() {
            w.toast(&format!("Zoom {:.0}%", settings::zoom_factor(z) * 100.0));
        }
    }

    // ------------------------------------------------------------ windows / docs

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

    pub fn new_window(&self, restore: bool) -> Rc<Window> {
        let win = Window::new(&self.rc());
        self.windows.borrow_mut().push(win.clone());
        if restore && !self.restored.replace(true) && !self.standalone {
            self.restore_session(&win);
        }
        win
    }

    pub fn window_removed(&self, win: &Window) {
        self.windows
            .borrow_mut()
            .retain(|w| !std::ptr::eq(w.as_ref(), win));
    }

    pub fn window_of(&self, doc: &Doc) -> Option<Rc<Window>> {
        let tv = doc.root.ancestor(adw::TabView::static_type())?;
        self.windows
            .borrow()
            .iter()
            .find(|w| w.tab_view.upcast_ref::<gtk::Widget>() == &tv)
            .cloned()
    }

    pub fn doc_for_child(&self, child: &gtk::Widget) -> Option<Rc<Doc>> {
        self.docs
            .borrow()
            .iter()
            .find(|d| d.root.upcast_ref::<gtk::Widget>() == child)
            .cloned()
    }

    pub fn find_doc_by_file(&self, file: &gio::File) -> Option<Rc<Doc>> {
        self.docs
            .borrow()
            .iter()
            .find(|d| d.file.borrow().as_ref().is_some_and(|f| f.equal(file)))
            .cloned()
    }

    pub fn register_doc(&self, doc: &Rc<Doc>) {
        self.docs.borrow_mut().push(doc.clone());
    }

    pub fn unregister_doc(&self, doc: &Doc) {
        doc.dispose();
        self.docs
            .borrow_mut()
            .retain(|d| !std::ptr::eq(d.as_ref(), doc));
        self.schedule_autosave();
    }

    /// Lowest unused "New Document N" number.
    pub fn next_untitled(&self) -> u32 {
        let used: Vec<u32> = self
            .docs
            .borrow()
            .iter()
            .filter(|d| d.file.borrow().is_none())
            .map(|d| d.untitled.get())
            .collect();
        (1..).find(|n| !used.contains(n)).unwrap_or(1)
    }

    pub fn doc_state_changed(&self, doc: &Rc<Doc>) {
        if let Some(w) = self.window_of(doc) {
            w.refresh_doc(doc);
        }
    }

    pub fn doc_cursor_moved(&self, doc: &Rc<Doc>) {
        if let Some(w) = self.window_of(doc) {
            if w.current_doc().is_some_and(|d| Rc::ptr_eq(&d, doc)) {
                w.update_status();
            }
        }
    }

    pub fn toast_for(&self, doc: &Doc, msg: &str) {
        if let Some(w) = self.window_of(doc).or_else(|| self.active_window()) {
            w.toast(msg);
        }
    }

    pub fn add_recent(&self, uri: &str) {
        let mut r = self.recent.borrow_mut();
        session::push_recent(&mut r, uri);
        let _ = self.store.save_recent(&r);
    }

    // ------------------------------------------------------------ session

    /// Snapshot of all windows plus the draft texts to write.
    fn build_session(&self) -> (Session, Vec<(String, String)>) {
        let mut drafts = Vec::new();
        let mut session = Session::default();
        for w in self.windows.borrow().iter() {
            let mut ws = WindowState::default();
            let selected = w.tab_view.selected_page();
            for i in 0..w.tab_view.n_pages() {
                let page = w.tab_view.nth_page(i);
                let Some(doc) = self.doc_for_child(&page.child()) else {
                    continue;
                };
                let uri = doc.uri();
                let modified = doc.is_modified();
                if uri.is_none() && !modified {
                    continue;
                }
                if selected.as_ref() == Some(&page) {
                    ws.active = ws.tabs.len();
                }
                let fmt = doc.format.get();
                let (line, column) = doc.cursor();
                if modified {
                    drafts.push((doc.id.clone(), doc.text()));
                }
                ws.tabs.push(TabState {
                    id: doc.id.clone(),
                    uri,
                    draft: modified,
                    encoding: fmt.encoding_name().into(),
                    bom: fmt.bom,
                    line_ending: fmt.line_ending.nick().into(),
                    line,
                    column,
                });
            }
            if !ws.tabs.is_empty() {
                session.windows.push(ws);
            }
        }
        (session, drafts)
    }

    fn write_session(store: &Store, session: &Session, drafts: &[(String, String)]) {
        for (id, text) in drafts {
            if let Err(e) = store.write_draft(id, text) {
                eprintln!("helix-text-editor: could not write draft: {e}");
            }
        }
        if let Err(e) = store.save_session(session) {
            eprintln!("helix-text-editor: could not write session: {e}");
        }
        store.prune_drafts(session);
    }

    /// Debounced background autosave of drafts + session (crash recovery).
    pub fn schedule_autosave(&self) {
        if self.standalone || self.autosave_pending.replace(true) {
            return;
        }
        let w = self.this.clone();
        glib::timeout_add_local_once(AUTOSAVE_DELAY, move || {
            let Some(app) = w.upgrade() else { return };
            app.autosave_pending.set(false);
            if app.quitting.get() {
                return;
            }
            let (session, drafts) = app.build_session();
            let store = app.store.clone();
            std::thread::spawn(move || Self::write_session(&store, &session, &drafts));
        });
    }

    /// Writes the session synchronously (used when closing).
    pub fn save_session_now(&self) {
        if self.standalone {
            return;
        }
        let (session, drafts) = self.build_session();
        Self::write_session(&self.store, &session, &drafts);
    }

    fn restore_session(&self, first: &Rc<Window>) {
        let session = self.store.load_session();
        let restore_all = self.settings.borrow().restore_session;
        if !restore_all && !session.has_drafts() {
            return;
        }
        let mut target = Some(first.clone());
        for ws in &session.windows {
            let tabs: Vec<&TabState> = ws.tabs.iter().filter(|t| restore_all || t.draft).collect();
            if tabs.is_empty() {
                continue;
            }
            let win = match target.take() {
                Some(w) => w,
                None => {
                    let w = Window::new(&self.rc());
                    self.windows.borrow_mut().push(w.clone());
                    w.win.present();
                    w
                }
            };
            let active = ws.tabs.get(ws.active).map(|t| t.id.clone());
            for t in tabs {
                let fmt = TextFormat {
                    encoding: TextFormat::encoding_by_name(&t.encoding),
                    bom: t.bom,
                    line_ending: LineEnding::from_nick(&t.line_ending),
                };
                let file = t.uri.as_deref().map(gio::File::for_uri);
                let select = active.as_deref() == Some(t.id.as_str());
                if t.draft {
                    let Some(text) = self.store.read_draft(&t.id) else {
                        continue;
                    };
                    let doc = win.add_doc(Some(t.id.clone()), select);
                    doc.restore_draft(&text, file, fmt);
                    doc.goto(t.line + 1, Some(t.column + 1));
                } else if let Some(file) = file {
                    win.restore_file(&t.id, file, t.line, t.column, select);
                }
            }
        }
    }

    fn quit(&self) {
        let keep = !self.standalone && self.settings.borrow().restore_session;
        if keep {
            self.save_session_now();
            self.quitting.set(true);
            for w in self.windows.borrow().clone() {
                w.force_close();
            }
            self.gtk.quit();
        } else {
            // Close windows one by one so unsaved changes are confirmed.
            let windows = self.windows.borrow().clone();
            for w in windows {
                w.win.close();
            }
        }
    }

    fn shutdown(&self) {
        if self.standalone {
            return;
        }
        if !self.quitting.get()
            && self.settings.borrow().restore_session
            && !self.docs.borrow().is_empty()
        {
            self.save_session_now();
        }
    }

    /// Called when the last window closes without keeping the session.
    pub fn clear_session(&self) {
        if !self.standalone {
            self.quitting.set(true);
            self.store.clear();
        }
    }
}
