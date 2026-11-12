//! Shared helpers for the Helix app suite.
//!
//! Apps read GNOME-compatible GSettings schemas (for example
//! `org.gnome.calculator`) so that existing user settings carry over. Those
//! schemas may not be installed on every system, so apps go through
//! [`Prefs`], which falls back to in-memory defaults when the schema is
//! missing instead of aborting the way `gio::Settings::new` does.

use gio::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Returns `Some(settings)` if `schema_id` is installed.
pub fn settings(schema_id: &str) -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    let schema = source.lookup(schema_id, true)?;
    Some(gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None))
}

/// GSettings-backed preferences with an in-memory fallback.
///
/// Only keys present in the installed schema are read from or written to
/// GSettings; anything else (or everything, when the schema is absent) lives
/// in memory for the lifetime of the process.
#[derive(Clone)]
pub struct Prefs {
    settings: Option<gio::Settings>,
    keys: Rc<Vec<String>>,
    memory: Rc<RefCell<HashMap<String, glib::Variant>>>,
}

impl Prefs {
    pub fn new(schema_id: &str) -> Self {
        let settings = settings(schema_id);
        let keys = settings
            .as_ref()
            .and_then(|s| s.settings_schema())
            .map(|schema| schema.list_keys().iter().map(|k| k.to_string()).collect())
            .unwrap_or_default();
        Prefs { settings, keys: Rc::new(keys), memory: Rc::default() }
    }

    /// Whether the GSettings schema is installed.
    pub fn is_persistent(&self) -> bool {
        self.settings.is_some()
    }

    pub fn gsettings(&self) -> Option<&gio::Settings> {
        self.settings.as_ref()
    }

    fn backed(&self, key: &str) -> Option<&gio::Settings> {
        self.settings.as_ref().filter(|_| self.keys.iter().any(|k| k == key))
    }

    pub fn get_variant(&self, key: &str) -> Option<glib::Variant> {
        match self.backed(key) {
            Some(s) => Some(s.value(key)),
            None => self.memory.borrow().get(key).cloned(),
        }
    }

    pub fn set_variant(&self, key: &str, value: glib::Variant) {
        match self.backed(key) {
            Some(s) => {
                // Type mismatches (schema drift) fall back to memory.
                if s.set_value(key, &value).is_err() {
                    self.memory.borrow_mut().insert(key.into(), value);
                }
            }
            None => {
                self.memory.borrow_mut().insert(key.into(), value);
            }
        }
    }

    pub fn get<T: glib::variant::FromVariant>(&self, key: &str, default: T) -> T {
        self.get_variant(key).and_then(|v| v.get::<T>()).unwrap_or(default)
    }

    pub fn set<T: glib::variant::ToVariant>(&self, key: &str, value: T) {
        self.set_variant(key, value.to_variant());
    }

    /// Reads an enum-typed key as its string nick.
    pub fn get_enum_nick(&self, key: &str, default: &str) -> String {
        match self.backed(key) {
            Some(s) => s.string(key).to_string(),
            None => self.get(key, default.to_string()),
        }
    }

    pub fn set_enum_nick(&self, key: &str, nick: &str) {
        match self.backed(key) {
            Some(s) => {
                let _ = s.set_string(key, nick);
            }
            None => self.set(key, nick.to_string()),
        }
    }
}

/// Per-user data directory for an app, e.g. `~/.local/share/helix-calculator`,
/// created on demand.
pub fn data_dir(app: &str) -> std::path::PathBuf {
    let dir = glib::user_data_dir().join(app);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Per-user config directory for an app, created on demand.
pub fn config_dir(app: &str) -> std::path::PathBuf {
    let dir = glib::user_config_dir().join(app);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Shows a standard About dialog.
pub fn show_about(
    parent: &impl IsA<gtk::Widget>,
    app_name: &str,
    app_id: &str,
    comments: &str,
) {
    use adw::prelude::*;
    let about = adw::AboutDialog::builder()
        .application_name(app_name)
        .application_icon(app_id)
        .developer_name("The Helix Project")
        .version(env!("CARGO_PKG_VERSION"))
        .license_type(gtk::License::Custom)
        .license("CC0-1.0")
        .website("https://github.com/danielkrainas/helix-apps")
        .comments(comments)
        .build();
    about.present(Some(parent));
}

/// Registers `app.quit` (Ctrl+Q) and `app.about` on `app`.
pub fn install_standard_actions(
    app: &adw::Application,
    app_name: &'static str,
    comments: &'static str,
) {
    use gtk::prelude::*;
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    let about = gio::ActionEntry::builder("about")
        .activate(move |app: &adw::Application, _, _| {
            if let Some(win) = app.active_window() {
                let id = app.application_id().map(|s| s.to_string()).unwrap_or_default();
                show_about(&win, app_name, &id, comments);
            }
        })
        .build();
    app.add_action_entries([quit, about]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
}
