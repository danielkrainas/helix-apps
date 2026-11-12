//! Preferences: `settings.json` plus GNOME GSettings keys when installed.

use crate::session::RestorePolicy;
use crate::urlinput::{default_engines, SearchEngine};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "org.gnome.Epiphany";
pub const WEB_SCHEMA: &str = "org.gnome.Epiphany.web";
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

/// Settings persisted to JSON and mirrored to GSettings when available.
pub struct Store {
    path: PathBuf,
    prefs: helix_app_common::Prefs,
    web: helix_app_common::Prefs,
}

impl Store {
    pub fn new(config_dir: &Path) -> Self {
        Store {
            path: config_dir.join("settings.json"),
            prefs: helix_app_common::Prefs::new(SCHEMA),
            web: helix_app_common::Prefs::new(WEB_SCHEMA),
        }
    }

    pub fn load(&self) -> Settings {
        let mut s = Settings::load_file(&self.path);
        if self.prefs.is_persistent() {
            s.homepage = self.prefs.get("homepage-url", s.homepage.clone());
            s.restore_session = self
                .prefs
                .get_enum_nick("restore-session-policy", &s.restore_session);
        }
        if self.web.is_persistent() {
            s.enable_popups = self.web.get("enable-popups", s.enable_popups);
            s.cookies_policy = self.web.get("cookies-policy", s.cookies_policy.clone());
            s.default_zoom = self.web.get("default-zoom-level", s.default_zoom);
        }
        s.sanitize();
        s
    }

    pub fn save(&self, s: &Settings) {
        crate::store::write_in_background(self.path.clone(), crate::store::to_json(s));
        if self.prefs.is_persistent() {
            self.prefs.set("homepage-url", s.homepage.clone());
            self.prefs
                .set_enum_nick("restore-session-policy", &s.restore_session);
        }
        if self.web.is_persistent() {
            self.web.set("enable-popups", s.enable_popups);
            self.web.set("cookies-policy", s.cookies_policy.clone());
            self.web.set("default-zoom-level", s.default_zoom);
        }
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
