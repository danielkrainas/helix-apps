//! Starred files: GNOME Files' (Nautilus's) stars (`crate::stars`),
//! read and written on a worker thread, and read again whenever the
//! store changes (also when Nautilus writes it).

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use super::Shared;
use crate::stars::{self, Access};

/// The Starred place's location.
pub const STARRED: &str = "starred:///";

/// How often the store is looked at for changes (two `stat`s).
const WATCH_EVERY: u32 = 2;

enum Req {
    Reload,
    Set(Vec<String>, bool),
    Moved(Vec<(String, String)>),
}

/// The store's state: access, why (when not read-write), the stars.
type State = (Access, String, Result<Vec<String>, String>);

enum Reply {
    State(State),
    Failed(String),
}

pub struct Stars {
    uris: RefCell<HashSet<String>>,
    access: Cell<Access>,
    loaded: Cell<bool>,
    home: PathBuf,
    tx: RefCell<Option<std::sync::mpsc::Sender<Req>>>,
}

impl Stars {
    pub fn new() -> Rc<Stars> {
        Rc::new(Stars {
            uris: RefCell::default(),
            access: Cell::new(Access::ReadOnly),
            loaded: Cell::new(false),
            home: glib::home_dir(),
            tx: RefCell::default(),
        })
    }

    /// Starts the worker and the watch; `shared` is told of changes.
    pub fn start(self: &Rc<Self>, shared: Weak<Shared>) {
        let data = glib::user_data_dir();
        let (tx, rx) = std::sync::mpsc::channel::<Req>();
        let (reply_tx, reply_rx) = async_channel::unbounded::<Reply>();
        let worker_data = data.clone();
        std::thread::spawn(move || worker(&worker_data, rx, reply_tx));
        let _ = tx.send(Req::Reload);
        *self.tx.borrow_mut() = Some(tx);

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(reply) = reply_rx.recv().await {
                let Some(s) = weak.upgrade() else { return };
                match reply {
                    Reply::State((access, note, uris)) => {
                        let uris = match uris {
                            Ok(u) => u.into_iter().collect(),
                            Err(e) => {
                                glib::g_warning!("helix-files", "starred files: {e}");
                                HashSet::new()
                            }
                        };
                        glib::g_debug!("helix-files", "starred files: {} ({access:?})", uris.len());
                        if access != Access::ReadWrite {
                            glib::g_info!("helix-files", "starred files {access:?}: {note}");
                        }
                        // (Each replaced: no `||` cutting the others short.)
                        let first = !s.loaded.replace(true);
                        let access_changed = s.access.replace(access) != access;
                        let changed = first || access_changed || *s.uris.borrow() != uris;
                        *s.uris.borrow_mut() = uris;
                        if changed {
                            if let Some(shared) = shared.upgrade() {
                                shared.stars_changed();
                            }
                        }
                    }
                    Reply::Failed(e) => {
                        glib::g_warning!("helix-files", "starring failed: {e}");
                        if let Some(w) = shared.upgrade().and_then(|s| s.active_window()) {
                            w.toast(&helix_app_common::trf!("Could not change stars: {}", e));
                        }
                    }
                }
            }
        });

        // Changes by anyone (Nautilus too) show in the store's files.
        let weak = Rc::downgrade(self);
        let mut last = stars::stamp(&data);
        glib::timeout_add_seconds_local(WATCH_EVERY, move || {
            let Some(s) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let now = stars::stamp(&data);
            if now != last {
                last = now;
                s.send(Req::Reload);
            }
            glib::ControlFlow::Continue
        });
    }

    fn send(&self, req: Req) {
        if let Some(tx) = self.tx.borrow().as_ref() {
            let _ = tx.send(req);
        }
    }

    /// Whether the stars could be read (or there are none yet).
    pub fn available(&self) -> bool {
        self.access.get() != Access::Off
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded.get()
    }

    pub fn is_starred(&self, file: &gio::File) -> bool {
        self.uris.borrow().contains(file.uri().as_str())
    }

    /// Whether `file` may be starred or unstarred here: a local file
    /// inside the home folder, and a store that may be written.
    pub fn can_star(&self, file: &gio::File) -> bool {
        self.access.get() == Access::ReadWrite
            && super::window::local_path(file).is_some_and(|p| stars::can_star(&self.home, &p))
    }

    /// Stars (`on`) or unstars `files` (those that may be).
    pub fn set(&self, shared: &Shared, files: &[gio::File], on: bool) {
        let files: Vec<&gio::File> = files.iter().filter(|f| self.can_star(f)).collect();
        if files.is_empty() {
            return;
        }
        let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
        // Shown at once; the store's state follows.
        {
            let mut set = self.uris.borrow_mut();
            for u in &uris {
                if on {
                    set.insert(u.clone());
                } else {
                    set.remove(u);
                }
            }
        }
        self.send(Req::Set(uris, on));
        shared.stars_changed();
    }

    /// Files were renamed or moved (from, to): their stars follow, as
    /// those of files inside them.
    pub fn moved(&self, pairs: &[(gio::File, gio::File)]) {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .filter(|(from, _)| {
                let inside = format!("{}/", from.uri());
                let uri = from.uri();
                self.uris
                    .borrow()
                    .iter()
                    .any(|u| *u == uri.as_str() || u.starts_with(&inside))
            })
            .map(|(a, b)| (a.uri().to_string(), b.uri().to_string()))
            .collect();
        if !pairs.is_empty() && self.access.get() == Access::ReadWrite {
            self.send(Req::Moved(pairs));
        }
    }

    /// The starred files Starred lists: local ones inside the home
    /// folder.
    pub fn listed(&self) -> Vec<gio::File> {
        let mut uris: Vec<String> = self
            .uris
            .borrow()
            .iter()
            .filter(|u| u.starts_with("file://"))
            .cloned()
            .collect();
        uris.sort();
        uris.iter()
            .map(|u| gio::File::for_uri(u))
            .filter(|f| f.path().is_some_and(|p| stars::can_star(&self.home, &p)))
            .collect()
    }
}

fn worker(
    data: &std::path::Path,
    rx: std::sync::mpsc::Receiver<Req>,
    tx: async_channel::Sender<Reply>,
) {
    let mut store = stars::Store::open(data, true);
    while let Ok(req) = rx.recv() {
        match req {
            Req::Reload => {
                // The store made, or its schema changed, since: open it
                // again.
                if store.access() != Access::ReadWrite || (!store.is_open() && stars::exists(data))
                {
                    store = stars::Store::open(data, true);
                }
            }
            Req::Set(uris, on) => {
                if let Err(e) = store.set_starred(&uris, on) {
                    let _ = tx.send_blocking(Reply::Failed(e));
                }
            }
            Req::Moved(pairs) => {
                for (from, to) in pairs {
                    if let Err(e) = store.moved(&from, &to) {
                        let _ = tx.send_blocking(Reply::Failed(e));
                    }
                }
            }
        }
        let state = (store.access(), store.note().to_owned(), store.starred());
        if tx.send_blocking(Reply::State(state)).is_err() {
            return;
        }
    }
}
