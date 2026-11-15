//! Remembering a main window's size and maximized state.
//!
//! [`persist`] restores the last size and maximized state before the
//! window is shown and saves them while it is used (shortly after each
//! change, so quitting with Ctrl+Q keeps them too) and when it closes.
//!
//! Apps whose GNOME schema has window keys keep using them, so the
//! GNOME app's saved size carries over ([`Store::settings`]); the others
//! write `window.json` (`{"width":…,"height":…,"maximized":…}`) in their
//! config directory ([`Store::file`]). A settings store whose keys are
//! missing from the installed schema falls back to that file.

use crate::Prefs;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// Sizes outside this range are ignored on load (corrupt or stale values).
const SANE: std::ops::RangeInclusive<i32> = 200..=16384;

/// How a schema stores the window size.
#[derive(Clone, Copy, Debug)]
pub enum SizeKeys {
    /// One key holding `(width, height)`, typed `(ii)` or `ai`.
    Pair(&'static str),
    /// Separate `i` keys for width and height.
    Split(&'static str, &'static str),
}

/// Where the state is kept.
#[derive(Clone)]
pub enum Store {
    Settings {
        prefs: Prefs,
        size: SizeKeys,
        maximized: &'static str,
        /// The file used when the schema lacks these keys.
        fallback: PathBuf,
    },
    File(PathBuf),
}

impl Store {
    /// `window.json` in `app`'s config directory (`helix-<app>`).
    pub fn file(app: &str) -> Store {
        Store::File(file_path(app))
    }

    /// GSettings keys of `prefs`, or `window.json` in `app`'s config
    /// directory when the schema does not have them.
    pub fn settings(prefs: &Prefs, size: SizeKeys, maximized: &'static str, app: &str) -> Store {
        Store::Settings {
            prefs: prefs.clone(),
            size,
            maximized,
            fallback: file_path(app),
        }
    }

    /// The store that is actually used.
    fn resolved(&self) -> Store {
        match self {
            Store::Settings {
                prefs,
                size,
                maximized,
                fallback,
            } => {
                let keys = match size {
                    SizeKeys::Pair(k) => vec![*k, maximized],
                    SizeKeys::Split(w, h) => vec![*w, *h, maximized],
                };
                if keys.iter().all(|k| prefs.is_backed(k)) {
                    self.clone()
                } else {
                    Store::File(fallback.clone())
                }
            }
            Store::File(_) => self.clone(),
        }
    }

    /// The saved state, if there is a sane one.
    pub fn load(&self) -> Option<State> {
        let state = match self.resolved() {
            Store::File(path) => State::parse(&std::fs::read_to_string(path).ok()?)?,
            Store::Settings {
                prefs,
                size,
                maximized,
                ..
            } => {
                let (width, height) = match size {
                    SizeKeys::Split(w, h) => (prefs.get(w, 0i32), prefs.get(h, 0i32)),
                    SizeKeys::Pair(k) => {
                        let v = prefs.get_variant(k)?;
                        v.get::<(i32, i32)>()
                            .or_else(|| match v.get::<Vec<i32>>()?.as_slice() {
                                [w, h] => Some((*w, *h)),
                                _ => None,
                            })
                            .unwrap_or((0, 0))
                    }
                };
                State {
                    width,
                    height,
                    maximized: prefs.get(maximized, false),
                }
            }
        };
        Some(state)
    }

    /// Saves `state`. The size is left alone when it is not sane (for
    /// example a window that was never shown).
    pub fn save(&self, state: State) {
        let sane = state.size_is_sane();
        match self.resolved() {
            Store::File(path) => {
                let mut state = state;
                if !sane {
                    match self.load() {
                        Some(old) => (state.width, state.height) = (old.width, old.height),
                        None => (state.width, state.height) = (0, 0),
                    }
                }
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, state.to_json()).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
            Store::Settings {
                prefs,
                size,
                maximized,
                ..
            } => {
                if sane {
                    match size {
                        SizeKeys::Split(w, h) => {
                            prefs.set(w, state.width);
                            prefs.set(h, state.height);
                        }
                        SizeKeys::Pair(k) => {
                            let is_array = prefs
                                .get_variant(k)
                                .is_some_and(|v| v.type_().as_str() == "ai");
                            if is_array {
                                prefs.set(k, vec![state.width, state.height]);
                            } else {
                                prefs.set(k, (state.width, state.height));
                            }
                        }
                    }
                }
                prefs.set(maximized, state.maximized);
            }
        }
    }
}

fn file_path(app: &str) -> PathBuf {
    glib::user_config_dir().join(app).join("window.json")
}

/// A window's unmaximized size and whether it is maximized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl State {
    fn size_is_sane(&self) -> bool {
        SANE.contains(&self.width) && SANE.contains(&self.height)
    }

    fn to_json(self) -> String {
        format!(
            "{{\"width\":{},\"height\":{},\"maximized\":{}}}\n",
            self.width, self.height, self.maximized
        )
    }

    /// Reads the small JSON object written by [`State::to_json`] (and by
    /// earlier per-app code); unknown fields are ignored.
    fn parse(text: &str) -> Option<State> {
        let body = text.trim().strip_prefix('{')?.strip_suffix('}')?;
        let mut state = State::default();
        for field in body.split(',') {
            let (key, value) = field.split_once(':')?;
            let value = value.trim();
            match key.trim().trim_matches('"') {
                "width" => state.width = value.parse().ok()?,
                "height" => state.height = value.parse().ok()?,
                "maximized" => state.maximized = value == "true",
                _ => {}
            }
        }
        Some(state)
    }
}

/// Restores `window`'s saved size and maximized state from `store` (call
/// before presenting it) and keeps them saved from then on. A saved size
/// smaller than the window's minimum is raised to it.
pub fn persist(window: &impl IsA<gtk::Window>, store: Store) {
    let window = window.as_ref();
    if let Some(state) = store.load() {
        if state.size_is_sane() {
            let w = state.width.max(window.width_request());
            let h = state.height.max(window.height_request());
            window.set_default_size(w, h);
        }
        if state.maximized {
            window.maximize();
        }
    }
    let store = Rc::new(store);
    let save = {
        let store = store.clone();
        move |w: &gtk::Window| {
            let (width, height) = w.default_size();
            let maximized = w.is_maximized();
            // A maximized or fullscreen window's size is not the one to
            // come back to.
            let (width, height) = if maximized || w.is_fullscreen() {
                (0, 0)
            } else {
                (width, height)
            };
            store.save(State {
                width,
                height,
                maximized,
            });
        }
    };
    let pending: Rc<Cell<Option<glib::SourceId>>> = Rc::default();
    let schedule = {
        let save = save.clone();
        let pending = pending.clone();
        move |w: &gtk::Window| {
            if let Some(id) = pending.take() {
                id.remove();
            }
            let weak = w.downgrade();
            let save = save.clone();
            let p = pending.clone();
            let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
                p.set(None);
                if let Some(w) = weak.upgrade() {
                    save(&w);
                }
            });
            pending.set(Some(id));
        }
    };
    for prop in ["default-width", "default-height", "maximized"] {
        let schedule = schedule.clone();
        window.connect_notify_local(Some(prop), move |w, _| {
            if w.is_visible() {
                schedule(w);
            }
        });
    }
    window.connect_close_request(move |w| {
        if let Some(id) = pending.take() {
            id.remove();
        }
        save(w);
        glib::Propagation::Proceed
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let s = State {
            width: 800,
            height: 600,
            maximized: true,
        };
        assert_eq!(State::parse(&s.to_json()), Some(s));
        // The format earlier per-app code wrote (serde, pretty or not).
        assert_eq!(
            State::parse("{\n  \"width\": 640,\n  \"height\": 480,\n  \"maximized\": false\n}"),
            Some(State {
                width: 640,
                height: 480,
                maximized: false
            })
        );
        assert_eq!(State::parse("not json"), None);
    }

    #[test]
    fn file_store_keeps_old_size_when_new_one_is_not_sane() {
        let dir = std::env::temp_dir().join(format!("helix-ws-{}", std::process::id()));
        let store = Store::File(dir.join("window.json"));
        store.save(State {
            width: 700,
            height: 500,
            maximized: false,
        });
        store.save(State {
            width: 0,
            height: 0,
            maximized: true,
        });
        assert_eq!(
            store.load(),
            Some(State {
                width: 700,
                height: 500,
                maximized: true
            })
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn settings_store_without_schema_uses_the_file() {
        std::env::set_var("GSETTINGS_BACKEND", "memory");
        let dir = std::env::temp_dir().join(format!("helix-ws2-{}", std::process::id()));
        let prefs = Prefs::new("org.helix.NoSuchSchema");
        let store = Store::Settings {
            prefs,
            size: SizeKeys::Pair("window-size"),
            maximized: "window-maximized",
            fallback: dir.join("window.json"),
        };
        store.save(State {
            width: 900,
            height: 700,
            maximized: false,
        });
        assert!(dir.join("window.json").is_file());
        assert_eq!(store.load().map(|s| s.width), Some(900));
        std::fs::remove_dir_all(&dir).ok();
    }
}
