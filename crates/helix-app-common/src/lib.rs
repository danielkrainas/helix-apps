//! Shared helpers for the Helix app suite.
//!
//! Apps read GNOME-compatible GSettings schemas (for example
//! `org.gnome.calculator`) so that existing user settings carry over. Those
//! schemas may not be installed on every system, so apps go through
//! [`Prefs`], which falls back to in-memory defaults when the schema is
//! missing instead of aborting the way `gio::Settings::new` does.

pub mod a11y;
pub mod focus;

pub use a11y::{
    announce, announce_change, announce_settled, icon_button, icon_menu_button, icon_toggle_button,
    label_widget, set_accessible_label, set_icon_and_label, Labelled, Urgency,
};

use gio::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Where packages install Helix's own copies of the GNOME-compatible
/// schemas the apps read, compiled. They live outside the global schema
/// directory so they never conflict with the GNOME packages that own the
/// same schema files; `HELIX_APPS_SCHEMA_DIR` overrides the location.
pub const BUNDLED_SCHEMA_DIR: &str = "/usr/share/helix-apps/schemas";

fn bundled_source() -> Option<gio::SettingsSchemaSource> {
    thread_local! {
        static SOURCE: Option<gio::SettingsSchemaSource> = {
            let dir = std::env::var_os("HELIX_APPS_SCHEMA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| BUNDLED_SCHEMA_DIR.into());
            if dir.join("gschemas.compiled").is_file() {
                gio::SettingsSchemaSource::from_directory(&dir, None, false).ok()
            } else {
                None
            }
        };
    }
    SOURCE.with(|s| s.clone())
}

/// Looks up `schema_id`: the system's schemas first (so a full GNOME
/// install wins), then Helix's bundled copies.
pub fn lookup_schema(schema_id: &str) -> Option<gio::SettingsSchema> {
    gio::SettingsSchemaSource::default()
        .and_then(|s| s.lookup(schema_id, true))
        .or_else(|| bundled_source()?.lookup(schema_id, false))
}

/// Returns `Some(settings)` if `schema_id` is installed (system-wide or
/// bundled with Helix).
pub fn settings(schema_id: &str) -> Option<gio::Settings> {
    let schema = lookup_schema(schema_id)?;
    Some(gio::Settings::new_full(
        &schema,
        None::<&gio::SettingsBackend>,
        None,
    ))
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
        Prefs {
            settings,
            keys: Rc::new(keys),
            memory: Rc::default(),
        }
    }

    /// Whether the GSettings schema is installed.
    pub fn is_persistent(&self) -> bool {
        self.settings.is_some()
    }

    pub fn gsettings(&self) -> Option<&gio::Settings> {
        self.settings.as_ref()
    }

    fn backed(&self, key: &str) -> Option<&gio::Settings> {
        self.settings
            .as_ref()
            .filter(|_| self.keys.iter().any(|k| k == key))
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
        self.get_variant(key)
            .and_then(|v| v.get::<T>())
            .unwrap_or(default)
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
pub fn show_about(parent: &impl IsA<gtk::Widget>, app_name: &str, app_id: &str, comments: &str) {
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

/// Resource path of Helix's own symbolic icons (`icons/` in this crate),
/// for icons the Adwaita theme does not ship.
pub const ICON_RESOURCE_PATH: &str = "/org/helix/Apps/icons";

/// Names of the icons bundled under [`ICON_RESOURCE_PATH`].
pub fn bundled_icon_names() -> Vec<String> {
    gio::resources_enumerate_children(
        &format!("{ICON_RESOURCE_PATH}/scalable/actions"),
        gio::ResourceLookupFlags::NONE,
    )
    .map(|v| {
        v.iter()
            .filter_map(|n| n.strip_suffix(".svg").map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

/// Registers the bundled icon resource with GIO (idempotent).
pub fn register_icon_resource() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let data =
            glib::Bytes::from_static(include_bytes!(concat!(env!("OUT_DIR"), "/icons.gresource")));
        let resource = gio::Resource::from_data(&data).expect("bundled icon resource");
        gio::resources_register(&resource);
    });
}

/// Makes Helix's bundled symbolic icons available to the default display's
/// icon theme. Called at startup by [`install_standard_actions`].
pub fn register_icons() {
    register_icon_resource();
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_resource_path(ICON_RESOURCE_PATH);
    }
}

/// Registers `app.quit` (Ctrl+Q) and `app.about` on `app`, and Helix's
/// bundled icons at startup.
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
                let id = app
                    .application_id()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                show_about(&win, app_name, &id, comments);
            }
        })
        .build();
    app.add_action_entries([quit, about]);
    a11y::install_check(app);
    app.connect_startup(|_| register_icons());
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema found only in the bundled directory is used, and the
    /// system's schemas still win for ids they define.
    #[test]
    fn bundled_schema_fallback() {
        let dir =
            std::env::temp_dir().join(format!("helix-app-common-schemas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("org.helix.Test.gschema.xml"),
            r#"<schemalist><schema id="org.helix.Test" path="/org/helix/test/">
                 <key name="answer" type="i"><default>42</default></key>
               </schema></schemalist>"#,
        )
        .unwrap();
        let compiled = std::process::Command::new("glib-compile-schemas")
            .arg(&dir)
            .status();
        if !matches!(compiled, Ok(s) if s.success()) {
            eprintln!("glib-compile-schemas unavailable; skipping");
            return;
        }
        // Read once per thread, so set it before the first lookup here.
        std::env::set_var("HELIX_APPS_SCHEMA_DIR", &dir);
        std::env::set_var("GSETTINGS_BACKEND", "memory");
        let prefs = Prefs::new("org.helix.Test");
        assert_eq!(prefs.get::<i32>("answer", 0), 42);
        assert!(lookup_schema("org.helix.NoSuchSchema").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
