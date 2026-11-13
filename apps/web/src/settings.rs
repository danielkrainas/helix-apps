//! Preferences: `settings.json` plus GNOME GSettings keys when installed.

use crate::session::RestorePolicy;
use crate::urlinput::{default_engines, SearchEngine};
use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "org.gnome.Epiphany";
/// Relocatable in Epiphany; the browser's own settings live at `WEB_PATH`.
pub const WEB_SCHEMA: &str = "org.gnome.Epiphany.web";
pub const WEB_PATH: &str = "/org/gnome/epiphany/web/";
pub const READER_SCHEMA: &str = "org.gnome.Epiphany.reader";
/// Epiphany's value for "show the start page".
pub const OVERVIEW: &str = "about:overview";

pub const ZOOM_MIN: f64 = 0.3;
pub const ZOOM_MAX: f64 = 3.0;
pub const ZOOM_STEPS: [f64; 13] = [
    0.3, 0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.2, 1.33, 1.5, 1.7, 2.0, 3.0,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookiePolicy {
    Always,
    NoThirdParty,
    Never,
}

impl CookiePolicy {
    pub const NICKS: [&'static str; 3] = ["always", "no-third-party", "never"];

    pub fn from_nick(s: &str) -> Self {
        match s {
            "always" => CookiePolicy::Always,
            "never" => CookiePolicy::Never,
            _ => CookiePolicy::NoThirdParty,
        }
    }

    pub fn nick(self) -> &'static str {
        match self {
            CookiePolicy::Always => "always",
            CookiePolicy::NoThirdParty => "no-third-party",
            CookiePolicy::Never => "never",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub homepage: String,
    pub restore_session: String,
    pub default_zoom: f64,
    pub enable_popups: bool,
    pub cookies_policy: String,
    pub javascript: bool,
    pub search_engines: Vec<SearchEngine>,
    pub default_search_engine: String,
    pub remember_passwords: bool,
    /// `sans` | `serif` (`org.gnome.Epiphany.reader font-style`).
    pub reader_font: String,
    /// `light` | `dark` (`org.gnome.Epiphany.reader color-scheme`).
    pub reader_colors: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            homepage: OVERVIEW.into(),
            restore_session: "always".into(),
            default_zoom: 1.0,
            enable_popups: false,
            cookies_policy: "no-third-party".into(),
            javascript: true,
            search_engines: default_engines(),
            default_search_engine: "DuckDuckGo".into(),
            remember_passwords: true,
            reader_font: "sans".into(),
            reader_colors: "light".into(),
        }
    }
}

impl Settings {
    pub fn load_file(path: &Path) -> Self {
        let mut s: Settings = crate::store::load(path);
        s.sanitize();
        s
    }

    pub fn sanitize(&mut self) {
        if self.search_engines.is_empty() {
            self.search_engines = default_engines();
        }
        if !self.default_zoom.is_finite() {
            self.default_zoom = 1.0;
        }
        self.default_zoom = self.default_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.restore_session = self.restore_policy().nick().into();
        self.cookies_policy = self.cookie_policy().nick().into();
        if self.reader_font != "serif" {
            self.reader_font = "sans".into();
        }
        if self.reader_colors != "dark" {
            self.reader_colors = "light".into();
        }
    }

    pub fn reader_font(&self) -> crate::reader::FontStyle {
        if self.reader_font == "serif" {
            crate::reader::FontStyle::Serif
        } else {
            crate::reader::FontStyle::Sans
        }
    }

    pub fn restore_policy(&self) -> RestorePolicy {
        RestorePolicy::from_nick(&self.restore_session)
    }

    pub fn cookie_policy(&self) -> CookiePolicy {
        CookiePolicy::from_nick(&self.cookies_policy)
    }

    /// The default engine (falls back to the first one).
    pub fn engine(&self) -> SearchEngine {
        self.search_engines
            .iter()
            .find(|e| e.name == self.default_search_engine)
            .or(self.search_engines.first())
            .cloned()
            .unwrap_or_else(|| default_engines().remove(0))
    }

    /// Homepage URL; `None` means the start page.
    pub fn home_url(&self) -> Option<String> {
        let h = self.homepage.trim();
        (!h.is_empty() && h != OVERVIEW && h != "about:newtab").then(|| h.to_string())
    }

    /// Adds a custom engine; rejects empty names, duplicates and templates
    /// without `%s`.
    pub fn add_engine(&mut self, name: &str, url: &str) -> bool {
        let name = name.trim();
        if name.is_empty()
            || !crate::urlinput::valid_template(url)
            || self
                .search_engines
                .iter()
                .any(|e| e.name.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.search_engines
            .push(SearchEngine::new(name, url.trim()));
        true
    }

    pub fn remove_engine(&mut self, name: &str) -> bool {
        if self.search_engines.len() <= 1 {
            return false;
        }
        let n = self.search_engines.len();
        self.search_engines.retain(|e| e.name != name);
        if self.default_search_engine == name {
            self.default_search_engine = self.search_engines[0].name.clone();
        }
        n != self.search_engines.len()
    }
}

/// Next zoom step in `dir` (+1 / -1) from `current`.
pub fn zoom_step(current: f64, dir: i32) -> f64 {
    if dir > 0 {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|&z| z > current + 0.001)
            .unwrap_or(ZOOM_MAX)
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&z| z < current - 0.001)
            .unwrap_or(ZOOM_MIN)
    }
}

/// A GSettings object for `schema_id` if installed. Relocatable schemas
/// (such as `org.gnome.Epiphany.web`) are opened at `path`;
/// `helix_app_common::Prefs` cannot do that, hence this local helper.
fn open_settings(schema_id: &str, path: &str) -> Option<(gio::Settings, Vec<String>)> {
    let source = gio::SettingsSchemaSource::default()?;
    let schema = source.lookup(schema_id, true)?;
    let path = schema.path().is_none().then_some(path);
    let keys = schema.list_keys().iter().map(|k| k.to_string()).collect();
    let settings = gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, path);
    Some((settings, keys))
}

/// Typed access to the keys of one schema that actually exist.
struct Gs(Option<(gio::Settings, Vec<String>)>);

impl Gs {
    fn new(schema_id: &str, path: &str) -> Self {
        Gs(open_settings(schema_id, path))
    }

    fn key(&self, key: &str) -> Option<&gio::Settings> {
        self.0
            .as_ref()
            .filter(|(_, keys)| keys.iter().any(|k| k == key))
            .map(|(s, _)| s)
    }

    fn get<T: glib::variant::FromVariant>(&self, key: &str, fallback: T) -> T {
        self.key(key)
            .and_then(|s| s.value(key).get::<T>())
            .unwrap_or(fallback)
    }

    fn set<T: glib::variant::ToVariant>(&self, key: &str, value: T) {
        if let Some(s) = self.key(key) {
            let v = value.to_variant();
            if s.value(key) != v {
                let _ = s.set_value(key, &v);
            }
        }
    }

    fn enum_nick(&self, key: &str, fallback: &str) -> String {
        self.key(key)
            .map(|s| s.string(key).to_string())
            .unwrap_or_else(|| fallback.to_string())
    }

    fn set_enum_nick(&self, key: &str, nick: &str) {
        if let Some(s) = self.key(key) {
            if s.string(key) != nick {
                let _ = s.set_string(key, nick);
            }
        }
    }
}

/// `search-engine-providers` (`aa{sv}`: name, url, bang) to engines.
pub fn engines_from_variant(v: &glib::Variant) -> Vec<SearchEngine> {
    let Some(list) = v.get::<Vec<HashMap<String, glib::Variant>>>() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|d| {
            let get = |k: &str| d.get(k).and_then(|v| v.get::<String>());
            let name = get("name")?;
            let url = get("url")?;
            (!name.trim().is_empty() && crate::urlinput::valid_template(&url)).then(|| {
                SearchEngine::new(&name, &url).with_bang(&get("bang").unwrap_or_default())
            })
        })
        .collect()
}

pub fn engines_to_variant(engines: &[SearchEngine]) -> glib::Variant {
    let list: Vec<HashMap<String, glib::Variant>> = engines
        .iter()
        .map(|e| {
            HashMap::from([
                ("name".to_string(), e.name.to_variant()),
                ("url".to_string(), e.url.to_variant()),
                ("bang".to_string(), e.bang.to_variant()),
            ])
        })
        .collect();
    list.to_variant()
}

/// Settings persisted to JSON and mirrored to GSettings when available.
pub struct Store {
    path: PathBuf,
    main: Gs,
    web: Gs,
    reader: Gs,
}

impl Store {
    pub fn new(config_dir: &Path) -> Self {
        Store {
            path: config_dir.join("settings.json"),
            main: Gs::new(SCHEMA, "/org/gnome/epiphany/"),
            web: Gs::new(WEB_SCHEMA, WEB_PATH),
            reader: Gs::new(READER_SCHEMA, "/org/gnome/epiphany/reader/"),
        }
    }

    pub fn load(&self) -> Settings {
        let mut s = Settings::load_file(&self.path);
        s.homepage = self.main.get("homepage-url", s.homepage.clone());
        // `never` is a Helix extension stored only in settings.json.
        if s.restore_session != "never" {
            s.restore_session = self
                .main
                .enum_nick("restore-session-policy", &s.restore_session);
        }
        s.default_search_engine = self
            .main
            .get("default-search-engine", s.default_search_engine.clone());
        if let Some(v) = self
            .main
            .key("search-engine-providers")
            .map(|g| g.value("search-engine-providers"))
        {
            let engines = engines_from_variant(&v);
            if !engines.is_empty() {
                s.search_engines = engines;
            }
        }
        s.enable_popups = self.web.get("enable-popups", s.enable_popups);
        s.default_zoom = self.web.get("default-zoom-level", s.default_zoom);
        s.remember_passwords = self.web.get("remember-passwords", s.remember_passwords);
        s.reader_font = self.reader.enum_nick("font-style", &s.reader_font);
        s.reader_colors = self.reader.enum_nick("color-scheme", &s.reader_colors);
        s.sanitize();
        s
    }

    pub fn save(&self, s: &Settings) {
        crate::store::write_in_background(self.path.clone(), crate::store::to_json(s));
        // Epiphany's default (empty) means the start page.
        let home = if s.home_url().is_none() { "" } else { s.homepage.as_str() };
        self.main.set("homepage-url", home.to_string());
        if s.restore_session != "never" {
            self.main
                .set_enum_nick("restore-session-policy", &s.restore_session);
        }
        self.main
            .set("default-search-engine", s.default_search_engine.clone());
        if let Some(g) = self.main.key("search-engine-providers") {
            let v = engines_to_variant(&s.search_engines);
            if g.value("search-engine-providers") != v {
                let _ = g.set_value("search-engine-providers", &v);
            }
        }
        self.web.set("enable-popups", s.enable_popups);
        self.web.set("default-zoom-level", s.default_zoom);
        self.web.set("remember-passwords", s.remember_passwords);
        self.reader.set_enum_nick("font-style", &s.reader_font);
        self.reader.set_enum_nick("color-scheme", &s.reader_colors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_sanitize() {
        let s = Settings::default();
        assert_eq!(s.engine().name, "DuckDuckGo");
        assert_eq!(s.cookie_policy(), CookiePolicy::NoThirdParty);
        assert_eq!(s.restore_policy(), RestorePolicy::Always);
        assert_eq!(s.home_url(), None);
        let mut s: Settings = serde_json::from_str(
            r#"{"default_zoom": 9.0, "search_engines": [], "cookies_policy": "x", "homepage": "https://h/"}"#,
        )
        .unwrap();
        s.sanitize();
        assert_eq!(s.default_zoom, ZOOM_MAX);
        assert_eq!(s.search_engines.len(), default_engines().len());
        assert_eq!(s.cookies_policy, "no-third-party");
        assert_eq!(s.home_url().as_deref(), Some("https://h/"));
        assert!(s.javascript);
    }

    #[test]
    fn engines() {
        let mut s = Settings::default();
        assert!(s.add_engine("Mine", "https://mine.example/?q=%s"));
        assert!(!s.add_engine("mine", "https://x/?q=%s"));
        assert!(!s.add_engine("Bad", "https://x/"));
        s.default_search_engine = "Mine".into();
        assert_eq!(s.engine().url, "https://mine.example/?q=%s");
        assert!(s.remove_engine("Mine"));
        assert_eq!(s.engine().name, "DuckDuckGo");
        s.default_search_engine = "Gone".into();
        assert_eq!(s.engine().name, "DuckDuckGo");
    }

    #[test]
    fn engine_variants_roundtrip() {
        let engines = default_engines();
        let v = engines_to_variant(&engines);
        assert_eq!(v.type_().as_str(), "aa{sv}");
        assert_eq!(engines_from_variant(&v), engines);
        // Epiphany's own default value parses, bangs included.
        let epiphany = glib::Variant::parse(
            Some(glib::VariantTy::new("aa{sv}").unwrap()),
            "[{'name': <'DuckDuckGo'>, 'url': <'https://duckduckgo.com/?q=%s&t=epiphany&kd=-1'>, 'bang': <'!ddg'>}, \
              {'name': <'NoBang'>, 'url': <'https://x.example/?q=%s'>}, {'name': <'Bad'>, 'url': <'nope'>}]",
        )
        .unwrap();
        let parsed = engines_from_variant(&epiphany);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].bang, "!ddg");
        assert_eq!(parsed[1].bang, "");
        assert!(engines_from_variant(&1u32.to_variant()).is_empty());
    }

    #[test]
    fn reader_prefs_sanitize() {
        let mut s = Settings {
            reader_font: "comic".into(),
            reader_colors: "x".into(),
            ..Default::default()
        };
        s.sanitize();
        assert_eq!((s.reader_font.as_str(), s.reader_colors.as_str()), ("sans", "light"));
        assert!(s.remember_passwords);
    }

    #[test]
    fn zoom_steps() {
        assert_eq!(zoom_step(1.0, 1), 1.1);
        assert_eq!(zoom_step(1.0, -1), 0.9);
        assert_eq!(zoom_step(3.0, 1), ZOOM_MAX);
        assert_eq!(zoom_step(0.3, -1), ZOOM_MIN);
        assert_eq!(zoom_step(1.05, 1), 1.1);
    }

    #[test]
    fn file_roundtrip() {
        let dir = crate::store::temp_dir("settings");
        let p = dir.join("settings.json");
        let s = Settings {
            javascript: false,
            homepage: "https://example.com/".into(),
            ..Default::default()
        };
        crate::store::write_atomic(&p, &crate::store::to_json(&s)).unwrap();
        assert_eq!(Settings::load_file(&p), s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
