//! Application state shared by all windows.

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use webkit6::prelude::*;

use crate::bookmarks::Bookmarks;
use crate::history::History;
use crate::permissions::Permissions;
use crate::session::{Session, TabState, WindowState};
use crate::settings::{self, Settings};
use crate::store;
use crate::tab::{Change, Tab};
use crate::urlinput;
use crate::webapp::WebApp;
use crate::window::Window;

pub const APP_ID: &str = "org.helix.Web";
pub const APP_DIR: &str = "helix-web";
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Saved {
    History,
    Bookmarks,
    Permissions,
    Session,
}

pub struct App {
    pub this: Weak<App>,
    pub gtk: adw::Application,
    pub settings: RefCell<Settings>,
    store: settings::Store,
    pub history: RefCell<History>,
    pub bookmarks: RefCell<Bookmarks>,
    pub permissions: RefCell<Permissions>,
    pub session: webkit6::NetworkSession,
    pub web_settings: webkit6::Settings,
    /// Set when running as an installed web application.
    pub web_app: Option<WebApp>,
    /// The web app's own preferences (web-app mode only).
    pub web_app_prefs: RefCell<crate::webapp::Prefs>,
    pub windows: RefCell<Vec<Rc<Window>>>,
    pub tabs: RefCell<Vec<Rc<Tab>>>,
    data_dir: PathBuf,
    /// Download destinations currently in flight.
    reserved: RefCell<HashSet<PathBuf>>,
    pending_saves: RefCell<HashSet<Saved>>,
    save_scheduled: Cell<bool>,
    started: Cell<bool>,
    /// Session to keep when the last normal window closes.
    final_windows: RefCell<Option<Vec<WindowState>>>,
}

#[derive(Default)]
struct Args {
    private: bool,
    new_window: bool,
    uris: Vec<String>,
}

/// What the primary instance is doing.
enum State {
    Running(Rc<App>),
    /// WebKit's sandbox cannot start; only the explanation is shown.
    Blocked(crate::sandbox::Cause),
}

fn show_blocked(gtk_app: &adw::Application, cause: crate::sandbox::Cause) {
    match gtk_app.windows().first() {
        Some(w) => w.present(),
        None => {
            crate::sandbox::failure_window(gtk_app, "Web", APP_DIR, cause).present();
        }
    }
}

pub fn run(web_app: Option<WebApp>) -> glib::ExitCode {
    let app_id = web_app
        .as_ref()
        .map(|w| w.app_id())
        .unwrap_or_else(|| APP_ID.to_string());
    let gtk_app = adw::Application::builder()
        .application_id(app_id.as_str())
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let holder: Rc<RefCell<Option<State>>> = Rc::default();

    gtk_app.connect_startup({
        let holder = holder.clone();
        move |gtk_app| {
            // Checked before any WebView exists: WebKit aborts the whole
            // process when its sandbox cannot start.
            let state = match crate::sandbox::check() {
                Some(cause) => {
                    eprintln!("helix-web: WebKit sandbox unavailable ({cause:?}); not showing web content");
                    helix_app_common::install_standard_actions(gtk_app, "Web", "Browse the web");
                    State::Blocked(cause)
                }
                None => State::Running(App::new(gtk_app, web_app.clone())),
            };
            *holder.borrow_mut() = Some(state);
        }
    });
    gtk_app.connect_command_line({
        let holder = holder.clone();
        move |gtk_app, cmdline| {
            match holder.borrow().as_ref() {
                Some(State::Running(app)) => {
                    let args = app.parse_args(cmdline);
                    app.handle_args(args);
                }
                Some(State::Blocked(cause)) => show_blocked(gtk_app, *cause),
                None => {}
            }
            0
        }
    });
    gtk_app.connect_activate({
        let holder = holder.clone();
        move |gtk_app| match holder.borrow().as_ref() {
            Some(State::Running(app)) => app.handle_args(Args::default()),
            Some(State::Blocked(cause)) => show_blocked(gtk_app, *cause),
            None => {}
        }
    });
    gtk_app.connect_shutdown({
        let holder = holder.clone();
        move |_| {
            if let Some(State::Running(app)) = holder.borrow_mut().take() {
                app.shutdown();
            }
        }
    });
    gtk_app.run()
}

fn webkit_dirs(data_dir: &Path, web_app: Option<&WebApp>) -> (String, String) {
    let data = data_dir.join("webkit");
    let mut cache = glib::user_cache_dir().join(APP_DIR);
    if let Some(w) = web_app {
        cache = cache.join("web-apps").join(&w.id);
    }
    let cache = cache.join("webkit");
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);
    (
        data.to_string_lossy().into_owned(),
        cache.to_string_lossy().into_owned(),
    )
}

impl App {
    fn new(gtk_app: &adw::Application, web_app: Option<WebApp>) -> Rc<App> {
        let mut data_dir = helix_app_common::data_dir(APP_DIR);
        if let Some(w) = &web_app {
            // Each web app keeps its own cookies, history and permissions.
            data_dir = data_dir.join("web-apps").join(&w.id);
            let _ = std::fs::create_dir_all(&data_dir);
        }
        let store = settings::Store::new(&helix_app_common::config_dir(APP_DIR));
        let settings = store.load();

        let (wk_data, wk_cache) = webkit_dirs(&data_dir, web_app.as_ref());
        let session = webkit6::NetworkSession::new(Some(&wk_data), Some(&wk_cache));
        if let Some(cm) = session.cookie_manager() {
            let cookies = data_dir.join("webkit").join("cookies.sqlite");
            cm.set_persistent_storage(
                &cookies.to_string_lossy(),
                webkit6::CookiePersistentStorage::Sqlite,
            );
        }
        if let Some(dm) = session.website_data_manager() {
            dm.set_favicons_enabled(true);
        }

        let web_settings = webkit6::Settings::new();
        web_settings.set_enable_developer_extras(true);
        web_settings.set_enable_back_forward_navigation_gestures(true);
        web_settings.set_enable_smooth_scrolling(true);
        web_settings.set_enable_fullscreen(true);
        web_settings.set_enable_media_stream(true);
        web_settings.set_enable_webrtc(true);
        web_settings.set_javascript_can_access_clipboard(false);

        let app = Rc::new_cyclic(|this| App {
            this: this.clone(),
            gtk: gtk_app.clone(),
            settings: RefCell::new(settings),
            store,
            history: RefCell::new(History::load(&data_dir.join("history.json"))),
            bookmarks: RefCell::new(Bookmarks::load(&data_dir.join("bookmarks.json"))),
            permissions: RefCell::new(Permissions::load(&data_dir.join("permissions.json"))),
            session,
            web_settings,
            web_app_prefs: RefCell::new(if web_app.is_some() {
                store::load(&data_dir.join(crate::webapp::PREFS_FILE))
            } else {
                Default::default()
            }),
            web_app,
            windows: RefCell::default(),
            tabs: RefCell::default(),
            data_dir,
            reserved: RefCell::default(),
            pending_saves: RefCell::default(),
            save_scheduled: Cell::new(false),
            started: Cell::new(false),
            final_windows: RefCell::new(None),
        });
        app.watch_downloads(&app.session);
        app.register_scheme();
        app.install_actions();
        app.apply_settings();
        crate::window::install_css();
        app
    }

    fn rc(&self) -> Rc<App> {
        self.this.upgrade().expect("app alive")
    }

    // ---- command line -----------------------------------------------------

    fn parse_args(&self, cmdline: &gio::ApplicationCommandLine) -> Args {
        let mut args = Args::default();
        let cwd = cmdline.cwd();
        let mut only_uris = false;
        for arg in cmdline.arguments().into_iter().skip(1) {
            let arg = arg.to_string_lossy().into_owned();
            if !only_uris {
                match arg.as_str() {
                    "--private" | "-p" | "--incognito-mode" | "-i" => {
                        args.private = true;
                        continue;
                    }
                    "--new-window" | "-n" => {
                        args.new_window = true;
                        continue;
                    }
                    "--" => {
                        only_uris = true;
                        continue;
                    }
                    s if s.starts_with('-') && s.len() > 1 => {
                        eprintln!("helix-web: ignoring unknown option {s}");
                        continue;
                    }
                    _ => {}
                }
            }
            let path = match &cwd {
                Some(cwd) => cwd.join(&arg),
                None => PathBuf::from(&arg),
            };
            let uri = if !arg.contains("://") && path.exists() {
                Some(gio::File::for_path(&path).uri().to_string())
            } else {
                self.resolve_input(&arg)
            };
            args.uris.extend(uri);
        }
        args
    }

    fn handle_args(&self, args: Args) {
        if let Some(wa) = &self.web_app {
            match self.windows.borrow().last() {
                Some(w) => w.win.present(),
                None => {
                    let w = self.new_window(false);
                    w.open_uri(&wa.url, true);
                    w.win.present();
                }
            }
            return;
        }
        let first = !self.started.replace(true);
        let mut restored = false;
        if first {
            restored = self.restore_session();
            self.mark_running();
        }
        if args.private {
            let w = self.new_window(true);
            if args.uris.is_empty() {
                w.open_home(true);
            }
            for (i, u) in args.uris.iter().enumerate() {
                w.open_uri(u, i == args.uris.len() - 1);
            }
            w.win.present();
            return;
        }
        if !args.uris.is_empty() {
            let w = match (args.new_window, self.active_window(false)) {
                (false, Some(w)) => w,
                _ => self.new_window(false),
            };
            for (i, u) in args.uris.iter().enumerate() {
                w.open_uri(u, i == args.uris.len() - 1);
            }
            w.win.present();
            return;
        }
        if first && restored {
            if let Some(w) = self.active_window(false) {
                w.win.present();
            }
            return;
        }
        let w = self.new_window(false);
        w.open_home(true);
        w.win.present();
    }

    // ---- windows & tabs ---------------------------------------------------

    pub fn active_window(&self, private: bool) -> Option<Rc<Window>> {
        let windows = self.windows.borrow();
        if let Some(active) = self.gtk.active_window() {
            if let Some(w) = windows
                .iter()
                .find(|w| w.win.upcast_ref::<gtk::Window>() == &active && w.private == private)
            {
                return Some(w.clone());
            }
        }
        windows.iter().rev().find(|w| w.private == private).cloned()
    }

    pub fn new_window(&self, private: bool) -> Rc<Window> {
        let session = if private {
            let s = webkit6::NetworkSession::new_ephemeral();
            if let Some(dm) = s.website_data_manager() {
                dm.set_favicons_enabled(true);
            }
            self.watch_downloads(&s);
            self.apply_cookie_policy(&s);
            s
        } else {
            self.session.clone()
        };
        let w = Window::new(&self.rc(), private, session);
        self.windows.borrow_mut().push(w.clone());
        w
    }

    pub fn window_closed(&self, w: &Rc<Window>) {
        // Keep the last normal window's tabs for the next start.
        let others = self
            .windows
            .borrow()
            .iter()
            .filter(|o| !o.private && !Rc::ptr_eq(o, w))
            .count();
        if !w.private && others == 0 {
            let state = w.session_state();
            let keep = if state.tabs.is_empty() {
                Vec::new()
            } else {
                vec![state]
            };
            self.final_windows.replace(Some(keep));
        }
        self.windows.borrow_mut().retain(|o| !Rc::ptr_eq(o, w));
        let gone: Vec<Rc<Tab>> = self
            .tabs
            .borrow()
            .iter()
            .filter(|t| w.contains(t))
            .cloned()
            .collect();
        for t in gone {
            t.deny_all_permissions();
            self.forget_tab(&t);
        }
        self.schedule_save(Saved::Session);
    }

    /// Creates a tab (not yet placed in a window).
    /// A content manager for one view. Normal views get the password
    /// form script (in an isolated world); private views get nothing.
    fn content_manager(&self, private: bool) -> webkit6::UserContentManager {
        let ucm = webkit6::UserContentManager::new();
        if !private {
            let script = webkit6::UserScript::for_world(
                crate::passwords::FORM_SCRIPT,
                webkit6::UserContentInjectedFrames::TopFrame,
                webkit6::UserScriptInjectionTime::End,
                crate::passwords::WORLD,
                &[],
                &[],
            );
            ucm.add_script(&script);
            ucm.register_script_message_handler(
                crate::passwords::HANDLER,
                Some(crate::passwords::WORLD),
            );
        }
        ucm
    }

    pub fn create_tab(&self, private: bool, session: &webkit6::NetworkSession) -> Rc<Tab> {
        let view = webkit6::WebView::builder()
            .network_session(session)
            .settings(&self.web_settings)
            .user_content_manager(&self.content_manager(private))
            .build();
        view.set_zoom_level(self.settings.borrow().default_zoom);
        let tab = Tab::new(&self.rc(), view, private);
        self.tabs.borrow_mut().push(tab.clone());
        tab
    }

    pub fn forget_tab(&self, tab: &Rc<Tab>) {
        self.tabs.borrow_mut().retain(|t| !Rc::ptr_eq(t, tab));
    }

    pub fn window_of(&self, tab: &Tab) -> Option<Rc<Window>> {
        self.windows
            .borrow()
            .iter()
            .find(|w| w.contains(tab))
            .cloned()
    }

    pub fn tab_of_view(&self, view: &webkit6::WebView) -> Option<Rc<Tab>> {
        self.tabs.borrow().iter().find(|t| &t.view == view).cloned()
    }

    pub fn tab_changed(&self, tab: &Rc<Tab>, what: Change) {
        if let Some(w) = self.window_of(tab) {
            w.tab_changed(tab, what);
        }
    }

    pub fn open_in_new_tab(&self, from: &Rc<Tab>, uri: &str, foreground: bool) {
        if let Some(w) = self.window_of(from) {
            let tab = w.new_tab_after(from);
            tab.load(uri);
            if foreground {
                w.select(&tab);
            }
        }
    }

    /// Handles WebKit's `create` signal: a related view in a new tab.
    pub fn create_related_tab(&self, from: &Rc<Tab>) -> webkit6::WebView {
        let view = webkit6::WebView::builder()
            .related_view(&from.view)
            .settings(&self.web_settings)
            .user_content_manager(&self.content_manager(from.private))
            .build();
        view.set_zoom_level(self.settings.borrow().default_zoom);
        let tab = Tab::new(&self.rc(), view.clone(), from.private);
        self.tabs.borrow_mut().push(tab.clone());
        if let Some(w) = self.window_of(from) {
            w.insert_tab_after(from, &tab);
        }
        let weak = Rc::downgrade(&tab);
        let app = self.this.clone();
        view.connect_ready_to_show(move |_| {
            if let (Some(t), Some(app)) = (weak.upgrade(), app.upgrade()) {
                if let Some(w) = app.window_of(&t) {
                    w.select(&t);
                }
            }
        });
        view
    }

    pub fn close_tab(&self, tab: &Rc<Tab>) {
        if let Some(w) = self.window_of(tab) {
            w.close_tab(tab);
        }
    }

    pub fn page_fullscreen(&self, tab: &Rc<Tab>, on: bool) {
        if let Some(w) = self.window_of(tab) {
            w.set_page_fullscreen(on);
        }
    }

    pub fn find_result(&self, tab: &Rc<Tab>, matches: Option<u32>) {
        if let Some(w) = self.window_of(tab) {
            w.find_result(tab, matches);
        }
    }

    pub fn bookmarks_changed(&self) {
        self.schedule_save(Saved::Bookmarks);
        for w in self.windows.borrow().iter() {
            w.sync_bookmark_state();
        }
    }

    // ---- address input ----------------------------------------------------

    pub fn resolve_input(&self, text: &str) -> Option<String> {
        let home = glib::home_dir().to_string_lossy().into_owned();
        let s = self.settings.borrow();
        urlinput::resolve_with_bangs(text, &home, &s.search_engines, &s.engine())
    }

    pub fn home_uri(&self) -> String {
        self.settings
            .borrow()
            .home_url()
            .unwrap_or_else(|| settings::OVERVIEW.to_string())
    }

    // ---- history ----------------------------------------------------------

    pub fn record_visit(&self, uri: &str, title: &str) {
        if self
            .history
            .borrow_mut()
            .add_visit(uri, title, store::now())
        {
            self.schedule_save(Saved::History);
        }
    }

    pub fn update_history_title(&self, uri: &str, title: &str) {
        if self.history.borrow_mut().set_title(uri, title) {
            self.schedule_save(Saved::History);
        }
    }

    /// Writes the web app's preferences (web-app mode only).
    pub fn save_web_app_prefs(&self) {
        if self.web_app.is_some() {
            let json = store::to_json(&*self.web_app_prefs.borrow());
            store::write_in_background(self.data_dir.join(crate::webapp::PREFS_FILE), json);
        }
    }

    /// Whether a link to `uri` stays in this instance: always in the
    /// browser; in a web app, only for its own or extra allowed sites.
    pub fn keeps_link(&self, uri: &str) -> bool {
        match &self.web_app {
            Some(wa) => wa.keeps(uri, &self.web_app_prefs.borrow()),
            None => true,
        }
    }

    pub fn save_permissions(&self) {
        self.schedule_save(Saved::Permissions);
    }

    pub fn schedule_session_save(&self) {
        self.schedule_save(Saved::Session);
    }

    pub fn schedule_save(&self, what: Saved) {
        self.pending_saves.borrow_mut().insert(what);
        if self.save_scheduled.replace(true) {
            return;
        }
        let weak = self.this.clone();
        glib::timeout_add_local_once(SAVE_DELAY, move || {
            if let Some(app) = weak.upgrade() {
                app.save_scheduled.set(false);
                app.flush_saves(false);
            }
        });
    }

    fn flush_saves(&self, clean_exit: bool) {
        let pending: Vec<Saved> = self.pending_saves.borrow_mut().drain().collect();
        for what in pending {
            let data = match what {
                Saved::History => self.history.borrow().serialize(),
                Saved::Bookmarks => self.bookmarks.borrow().serialize(),
                Saved::Permissions => self.permissions.borrow().serialize(),
                Saved::Session => Some(self.session_json(clean_exit)),
            };
            if let Some((path, json)) = data {
                store::write_in_background(path, json);
            }
        }
    }

    fn session_path(&self) -> PathBuf {
        self.data_dir.join("session.json")
    }

    fn session_json(&self, clean_exit: bool) -> (PathBuf, String) {
        let windows: Vec<WindowState> = self
            .windows
            .borrow()
            .iter()
            .filter(|w| !w.private)
            .map(|w| w.session_state())
            .filter(|s| !s.tabs.is_empty())
            .collect();
        let windows = if self.windows.borrow().iter().any(|w| !w.private) {
            windows
        } else {
            self.final_windows.borrow().clone().unwrap_or(windows)
        };
        (
            self.session_path(),
            store::to_json(&Session {
                windows,
                clean_exit,
            }),
        )
    }

    fn restore_session(&self) -> bool {
        let session: Session = store::load(&self.session_path());
        let policy = self.settings.borrow().restore_policy();
        let windows = session.to_restore(policy);
        for ws in &windows {
            let w = self.new_window(false);
            w.restore(ws);
        }
        !windows.is_empty()
    }

    /// Records that we are running, so a crash can be detected next time.
    fn mark_running(&self) {
        let mut s: Session = store::load(&self.session_path());
        s.clean_exit = false;
        store::write_in_background(self.session_path(), store::to_json(&s));
    }

    fn shutdown(&self) {
        self.pending_saves.borrow_mut().extend([
            Saved::History,
            Saved::Bookmarks,
            Saved::Permissions,
            Saved::Session,
        ]);
        self.flush_saves(true);
        store::flush();
    }

    // ---- settings ---------------------------------------------------------

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        {
            let mut s = self.settings.borrow_mut();
            f(&mut s);
            s.sanitize();
            self.store.save(&s);
        }
        self.apply_settings();
    }

    fn apply_cookie_policy(&self, session: &webkit6::NetworkSession) {
        let policy = match self.settings.borrow().cookie_policy() {
            settings::CookiePolicy::Always => webkit6::CookieAcceptPolicy::Always,
            settings::CookiePolicy::NoThirdParty => webkit6::CookieAcceptPolicy::NoThirdParty,
            settings::CookiePolicy::Never => webkit6::CookieAcceptPolicy::Never,
        };
        if let Some(cm) = session.cookie_manager() {
            cm.set_accept_policy(policy);
        }
    }

    pub fn apply_settings(&self) {
        {
            let s = self.settings.borrow();
            self.web_settings.set_enable_javascript(s.javascript);
            self.web_settings
                .set_javascript_can_open_windows_automatically(s.enable_popups);
        }
        self.apply_cookie_policy(&self.session);
        for w in self.windows.borrow().iter().filter(|w| w.private) {
            self.apply_cookie_policy(&w.session);
        }
        for w in self.windows.borrow().iter() {
            w.sync();
        }
    }

    /// Clears the selected kinds of browsing data (all time).
    pub fn clear_data(&self, site_data: bool, cache: bool, history: bool) {
        let mut types = webkit6::WebsiteDataTypes::empty();
        if site_data {
            types |= webkit6::WebsiteDataTypes::COOKIES
                | webkit6::WebsiteDataTypes::LOCAL_STORAGE
                | webkit6::WebsiteDataTypes::SESSION_STORAGE
                | webkit6::WebsiteDataTypes::INDEXEDDB_DATABASES
                | webkit6::WebsiteDataTypes::OFFLINE_APPLICATION_CACHE
                | webkit6::WebsiteDataTypes::SERVICE_WORKER_REGISTRATIONS
                | webkit6::WebsiteDataTypes::DOM_CACHE
                | webkit6::WebsiteDataTypes::HSTS_CACHE
                | webkit6::WebsiteDataTypes::DEVICE_ID_HASH_SALT
                | webkit6::WebsiteDataTypes::ITP;
        }
        if cache {
            types |=
                webkit6::WebsiteDataTypes::MEMORY_CACHE | webkit6::WebsiteDataTypes::DISK_CACHE;
        }
        if !types.is_empty() {
            if let Some(dm) = self.session.website_data_manager() {
                dm.clear(
                    types,
                    glib::TimeSpan::from_seconds(0),
                    None::<&gio::Cancellable>,
                    |r| {
                        if let Err(e) = r {
                            eprintln!("helix-web: clearing website data failed: {e}");
                        }
                    },
                );
            }
        }
        if history {
            self.history.borrow_mut().clear();
            self.schedule_save(Saved::History);
        }
    }

    // ---- start page -------------------------------------------------------

    fn register_scheme(&self) {
        let Some(ctx) = webkit6::WebContext::default() else {
            return;
        };
        if let Some(sm) = ctx.security_manager() {
            sm.register_uri_scheme_as_local("helix-about");
            sm.register_uri_scheme_as_secure("helix-about");
        }
        let weak = self.this.clone();
        ctx.register_uri_scheme("helix-about", move |req| {
            let private = req
                .web_view()
                .and_then(|v| v.network_session())
                .is_some_and(|s| s.is_ephemeral());
            let most = match weak.upgrade() {
                Some(app) if !private => app.history.borrow().most_visited(12, store::now()),
                _ => Vec::new(),
            };
            let html = crate::overview::html(&most, private);
            let bytes = glib::Bytes::from_owned(html.into_bytes());
            let len = bytes.len() as i64;
            let stream = gio::MemoryInputStream::from_bytes(&bytes);
            req.finish(&stream, len, Some("text/html"));
        });
    }

    // ---- downloads --------------------------------------------------------

    fn watch_downloads(&self, session: &webkit6::NetworkSession) {
        let weak = self.this.clone();
        session.connect_download_started(move |_, download| {
            if let Some(app) = weak.upgrade() {
                app.handle_download(download);
            }
        });
    }

    pub fn download_dir() -> PathBuf {
        glib::user_special_dir(glib::UserDirectory::Downloads)
            .unwrap_or_else(|| glib::home_dir().join("Downloads"))
    }

    fn handle_download(&self, download: &webkit6::Download) {
        let weak = self.this.clone();
        download.connect_decide_destination(move |d, suggested| {
            let Some(app) = weak.upgrade() else {
                return false;
            };
            let dir = App::download_dir();
            let _ = std::fs::create_dir_all(&dir);
            let name = if suggested.trim().is_empty() {
                d.request()
                    .and_then(|r| r.uri())
                    .map(|u| crate::downloads::name_from_uri(&u))
                    .unwrap_or_default()
            } else {
                suggested.to_string()
            };
            let path = {
                let reserved = app.reserved.borrow();
                crate::downloads::unique_path(&dir, &name, |p| p.exists() || reserved.contains(p))
            };
            app.reserved.borrow_mut().insert(path.clone());
            d.set_allow_overwrite(false);
            d.set_destination(&path.to_string_lossy());
            if let Some(w) = app.window_for_download(d) {
                w.add_download(d, &path);
            }
            true
        });
        let weak = self.this.clone();
        let release = move |d: &webkit6::Download| {
            if let (Some(app), Some(dest)) = (weak.upgrade(), d.destination()) {
                app.reserved.borrow_mut().remove(Path::new(dest.as_str()));
            }
        };
        let r2 = release.clone();
        download.connect_finished(move |d| release(d));
        download.connect_failed(move |d, _| r2(d));
    }

    fn window_for_download(&self, d: &webkit6::Download) -> Option<Rc<Window>> {
        let from_view = d
            .web_view()
            .and_then(|v| self.tab_of_view(&v))
            .and_then(|t| self.window_of(&t));
        from_view
            .or_else(|| self.active_window(false))
            .or_else(|| self.windows.borrow().last().cloned())
    }

    // ---- actions ----------------------------------------------------------

    fn install_actions(&self) {
        helix_app_common::install_standard_actions(&self.gtk, "Web", "Browse the web");
        let weak = self.this.clone();
        let action = |name: &str, f: fn(&Rc<App>)| {
            let a = gio::SimpleAction::new(name, None);
            let weak = weak.clone();
            a.connect_activate(move |_, _| {
                if let Some(app) = weak.upgrade() {
                    f(&app);
                }
            });
            self.gtk.add_action(&a);
        };
        action("new-window", |app| {
            let w = app.new_window(false);
            w.open_home(true);
            w.win.present();
        });
        action("new-private-window", |app| {
            let w = app.new_window(true);
            w.open_home(true);
            w.win.present();
        });
        action("preferences", crate::dialogs::show_preferences);
        action("history", crate::dialogs::show_history);
        action("import-bookmarks", crate::dialogs::import_bookmarks);
        action("export-bookmarks", crate::dialogs::export_bookmarks);
        action("passwords", crate::dialogs::show_passwords);
        action("uninstall-web-app", crate::dialogs::uninstall_web_app);
        action(
            "web-app-preferences",
            crate::dialogs::show_web_app_preferences,
        );

        let accels: &[(&str, &[&str])] = &[
            ("app.new-window", &["<Control>n"]),
            ("app.new-private-window", &["<Control><Shift>n"]),
            ("app.preferences", &["<Control>comma"]),
            ("app.history", &["<Control>h"]),
            ("win.new-tab", &["<Control>t"]),
            ("win.close-tab", &["<Control>w"]),
            ("win.reopen-tab", &["<Control><Shift>t"]),
            ("win.location", &["<Control>l", "F6", "<Alt>d"]),
            ("win.back", &["<Alt>Left", "Back"]),
            ("win.forward", &["<Alt>Right", "Forward"]),
            ("win.reload", &["<Control>r", "F5", "Reload"]),
            ("win.reload-bypass", &["<Control><Shift>r", "<Shift>F5"]),
            ("win.home", &["<Alt>Home", "HomePage"]),
            ("win.bookmark", &["<Control>d"]),
            ("win.overview", &["<Control><Shift>o"]),
            ("win.find", &["<Control>f"]),
            ("win.find-next", &["<Control>g"]),
            ("win.find-prev", &["<Control><Shift>g"]),
            ("win.print", &["<Control>p"]),
            (
                "win.zoom-in",
                &[
                    "<Control>plus",
                    "<Control>equal",
                    "<Control>KP_Add",
                    "ZoomIn",
                ],
            ),
            (
                "win.zoom-out",
                &["<Control>minus", "<Control>KP_Subtract", "ZoomOut"],
            ),
            ("win.zoom-reset", &["<Control>0", "<Control>KP_0"]),
            ("win.fullscreen", &["F11"]),
            ("win.reader", &["<Control><Alt>r"]),
        ];
        // Ctrl+W closes the tab here; closing the window moves to Ctrl+Shift+W.
        self.gtk
            .set_accels_for_action("window.close", &["<Control><Shift>w"]);
        for (action, keys) in accels {
            self.gtk.set_accels_for_action(action, keys);
        }
    }
}

/// Session state for one tab.
pub fn tab_state(tab: &Tab) -> Option<TabState> {
    if tab.private {
        return None;
    }
    let uri = tab.uri();
    let uri = if uri.starts_with("helix-about:") {
        settings::OVERVIEW.to_string()
    } else {
        uri
    };
    crate::session::savable(&uri).then(|| TabState {
        uri,
        title: tab.title(),
    })
}
