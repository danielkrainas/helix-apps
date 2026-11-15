//! Settings.
//!
//! * Keys that `org.gnome.Maps` defines (window state, last viewed
//!   location, zoom, rotation, transportation type, search result count,
//!   scale) live in GSettings under their GNOME names; this module only
//!   holds the pure helpers that validate those values ([`gnome`]).
//! * Service endpoints are layered: built-in defaults, then the system-wide
//!   `helix-maps/endpoints.toml` files found in `$XDG_CONFIG_DIRS`, then the
//!   user's overrides in `$XDG_CONFIG_HOME/helix-maps/settings.json`
//!   ([`resolve`]).
//! * Other Helix-only settings (units) also live in `settings.json`.

use helix_app_common::i18n::{n_, tr};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
pub const DEFAULT_ATTRIBUTION: &str = n_("© OpenStreetMap contributors");
pub const DEFAULT_ATTRIBUTION_URL: &str = "https://www.openstreetmap.org/copyright";
pub const DEFAULT_GEOCODER: &str = "https://nominatim.openstreetmap.org";
pub const DEFAULT_ROUTER: &str = "https://router.project-osrm.org";
pub const DEFAULT_MAX_ZOOM: u32 = 19;

/// File name of the system-wide endpoint configuration, relative to each
/// directory in `$XDG_CONFIG_DIRS`.
pub const SYSTEM_FILE: &str = "helix-maps/endpoints.toml";
/// Version written to `settings.json` (absent in the pre-layering format).
const USER_FILE_VERSION: u32 = 2;

/// The effective service configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Services {
    pub tile_url_template: String,
    pub tile_attribution: String,
    pub tile_attribution_url: String,
    pub tile_max_zoom: u32,
    pub geocoder_url: String,
    pub router_url: String,
}

impl Default for Services {
    fn default() -> Self {
        Services {
            tile_url_template: DEFAULT_TILE_URL.into(),
            tile_attribution: tr(DEFAULT_ATTRIBUTION).into(),
            tile_attribution_url: DEFAULT_ATTRIBUTION_URL.into(),
            tile_max_zoom: DEFAULT_MAX_ZOOM,
            geocoder_url: DEFAULT_GEOCODER.into(),
            router_url: DEFAULT_ROUTER.into(),
        }
    }
}

impl Services {
    /// Applies one configuration layer; invalid values are ignored so the
    /// lower layer's value stays in effect.
    pub fn apply(&mut self, o: &Overrides) {
        if let Some(t) = o.tile_url_template.as_deref().map(str::trim) {
            if validate_tile_template(t).is_ok() {
                self.tile_url_template = t.to_string();
            }
        }
        if let Some(t) = &o.tile_attribution {
            self.tile_attribution = t.trim().to_string();
        }
        if let Some(t) = o.tile_attribution_url.as_deref().map(str::trim) {
            if t.is_empty() || is_http_url(t) {
                self.tile_attribution_url = t.to_string();
            }
        }
        if let Some(z) = o.tile_max_zoom {
            if (2..=22).contains(&z) {
                self.tile_max_zoom = z;
            }
        }
        if let Some(u) = o.geocoder_url.as_deref().and_then(normalize_base_url) {
            self.geocoder_url = u;
        }
        if let Some(u) = o.router_url.as_deref().and_then(normalize_base_url) {
            self.router_url = u;
        }
    }

    /// Whether the default (public, policy-restricted) OSRM demo is used.
    pub fn uses_demo_router(&self) -> bool {
        self.router_url == DEFAULT_ROUTER
    }
}

/// One configuration layer: `None` inherits from the layer below.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Overrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_url_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_attribution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_attribution_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_max_zoom: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geocoder_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub router_url: Option<String>,
}

impl Overrides {
    /// Reads the known keys through typed getters; keys with the wrong type
    /// are skipped individually instead of rejecting the whole layer.
    fn from_getters(
        s: impl Fn(&str) -> Option<String>,
        i: impl Fn(&str) -> Option<i64>,
    ) -> Overrides {
        Overrides {
            tile_url_template: s("tile_url_template"),
            tile_attribution: s("tile_attribution"),
            tile_attribution_url: s("tile_attribution_url"),
            tile_max_zoom: i("tile_max_zoom").and_then(|v| u32::try_from(v).ok()),
            geocoder_url: s("geocoder_url"),
            router_url: s("router_url"),
        }
    }

    /// Parses a system-wide `endpoints.toml`.
    pub fn from_toml(text: &str) -> Result<Overrides, String> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| e.message().to_string())?;
        Ok(Overrides::from_getters(
            |k| table.get(k).and_then(|v| v.as_str()).map(str::to_string),
            |k| table.get(k).and_then(|v| v.as_integer()),
        ))
    }

    fn from_json(v: &serde_json::Value) -> Overrides {
        Overrides::from_getters(
            |k| v.get(k).and_then(|v| v.as_str()).map(str::to_string),
            |k| v.get(k).and_then(|v| v.as_i64()),
        )
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        *self == Overrides::default()
    }

    /// Drops values identical to the built-in defaults. Used to migrate the
    /// old `settings.json`, which stored every endpoint even when unchanged
    /// and would otherwise shadow the system-wide configuration.
    fn without_builtin_defaults(mut self) -> Overrides {
        let d = Services::default();
        fn drop_eq<T: PartialEq>(slot: &mut Option<T>, d: &T) {
            if slot.as_ref() == Some(d) {
                *slot = None;
            }
        }
        drop_eq(&mut self.tile_url_template, &d.tile_url_template);
        drop_eq(&mut self.tile_attribution, &d.tile_attribution);
        drop_eq(&mut self.tile_attribution_url, &d.tile_attribution_url);
        drop_eq(&mut self.tile_max_zoom, &d.tile_max_zoom);
        if self
            .geocoder_url
            .as_deref()
            .and_then(normalize_base_url)
            .as_deref()
            == Some(DEFAULT_GEOCODER)
        {
            self.geocoder_url = None;
        }
        if self
            .router_url
            .as_deref()
            .and_then(normalize_base_url)
            .as_deref()
            == Some(DEFAULT_ROUTER)
        {
            self.router_url = None;
        }
        self
    }

    /// Records the user's choice for every endpoint relative to `inherited`
    /// (the system configuration): equal values are stored as "inherit" so
    /// later changes by the administrator still reach this user.
    pub fn set_from(&mut self, wanted: &Services, inherited: &Services) {
        fn pick<T: PartialEq + Clone>(w: &T, i: &T) -> Option<T> {
            (w != i).then(|| w.clone())
        }
        self.tile_url_template = pick(&wanted.tile_url_template, &inherited.tile_url_template);
        self.tile_attribution = pick(&wanted.tile_attribution, &inherited.tile_attribution);
        self.tile_attribution_url = pick(
            &wanted.tile_attribution_url,
            &inherited.tile_attribution_url,
        );
        self.tile_max_zoom = pick(&wanted.tile_max_zoom, &inherited.tile_max_zoom);
        self.geocoder_url = pick(&wanted.geocoder_url, &inherited.geocoder_url);
        self.router_url = pick(&wanted.router_url, &inherited.router_url);
    }
}

/// Applies `layers` (least important first) on top of the built-in defaults.
pub fn resolve<'a>(layers: impl IntoIterator<Item = &'a Overrides>) -> Services {
    let mut s = Services::default();
    for l in layers {
        s.apply(l);
    }
    s
}

/// Splits `$XDG_CONFIG_DIRS` (default `/etc/xdg`); relative entries are
/// ignored as the Base Directory spec requires.
pub fn config_dirs(var: Option<&str>) -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = var
        .unwrap_or("")
        .split(':')
        .filter(|d| d.starts_with('/'))
        .map(PathBuf::from)
        .collect();
    if dirs.is_empty() {
        vec![PathBuf::from("/etc/xdg")]
    } else {
        dirs
    }
}

/// System-wide configuration: every `helix-maps/endpoints.toml` found in
/// the config dirs, least important first (the first directory in
/// `$XDG_CONFIG_DIRS` wins, key by key).
#[derive(Debug, Clone, Default)]
pub struct SystemConfig {
    pub layers: Vec<(PathBuf, Overrides)>,
}

impl SystemConfig {
    pub fn load(dirs: &[PathBuf]) -> SystemConfig {
        let mut layers = Vec::new();
        for dir in dirs.iter().rev() {
            let path = dir.join(SYSTEM_FILE);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match Overrides::from_toml(&text) {
                Ok(o) => layers.push((path, o)),
                Err(e) => eprintln!("helix-maps: ignoring {}: {e}", path.display()),
            }
        }
        SystemConfig { layers }
    }

    pub fn from_env() -> SystemConfig {
        SystemConfig::load(&config_dirs(
            std::env::var("XDG_CONFIG_DIRS").ok().as_deref(),
        ))
    }

    /// Defaults plus the system layers (what a user without overrides gets).
    pub fn services(&self) -> Services {
        resolve(self.layers.iter().map(|(_, o)| o))
    }

    pub fn files(&self) -> Vec<&Path> {
        self.layers.iter().map(|(p, _)| p.as_path()).collect()
    }
}

/// The user's Helix-only settings (`settings.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct UserSettings {
    pub endpoints: Overrides,
    /// `"metric"` or `"imperial"`.
    pub units: String,
}

impl Default for UserSettings {
    fn default() -> Self {
        UserSettings {
            endpoints: Overrides::default(),
            units: "metric".into(),
        }
    }
}

#[derive(Serialize)]
struct UserFile<'a> {
    version: u32,
    #[serde(flatten)]
    endpoints: &'a Overrides,
    units: &'a str,
}

impl UserSettings {
    /// Parses `settings.json`; malformed input yields defaults and invalid
    /// individual values are ignored.
    pub fn from_json(text: &str) -> UserSettings {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
            return UserSettings::default();
        };
        let mut endpoints = Overrides::from_json(&v);
        if v.get("version").is_none() {
            endpoints = endpoints.without_builtin_defaults();
        }
        let units = match v.get("units").and_then(|u| u.as_str()) {
            Some(u @ ("metric" | "imperial")) => u.to_string(),
            _ => "metric".to_string(),
        };
        UserSettings { endpoints, units }
    }

    pub fn to_json(&self) -> String {
        let file = UserFile {
            version: USER_FILE_VERSION,
            endpoints: &self.endpoints,
            units: &self.units,
        };
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    pub fn load(path: &Path) -> UserSettings {
        std::fs::read_to_string(path)
            .map(|t| UserSettings::from_json(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, path)
    }

    /// Effective services: defaults, then `system`, then this user's overrides.
    pub fn services(&self, system: &SystemConfig) -> Services {
        resolve(
            system
                .layers
                .iter()
                .map(|(_, o)| o)
                .chain(std::iter::once(&self.endpoints)),
        )
    }
}

/// Validation of values read from the `org.gnome.Maps` GSettings keys.
pub mod gnome {
    use crate::geo::LatLon;

    pub const MIN_ZOOM: f64 = 2.0;
    pub const DEFAULT_WIDTH: i32 = 1000;
    pub const DEFAULT_HEIGHT: i32 = 700;

    /// `last-viewed-location` (`ad`): two valid coordinates or the schema
    /// default `[0, 0]`.
    pub fn location(v: &[f64]) -> LatLon {
        match v {
            [lat, lon] => LatLon::new(*lat, *lon),
            _ => None,
        }
        .unwrap_or(LatLon { lat: 0.0, lon: 0.0 })
    }

    /// `zoom-level` (`i`) clamped to the zoom range the map supports.
    pub fn zoom(v: i32, max_zoom: u32) -> f64 {
        (v as f64).clamp(MIN_ZOOM, max_zoom as f64)
    }

    /// `window-size` (`ai`, default `[]`): `(width, height)` when set and sane.
    pub fn window_size(v: &[i32]) -> (i32, i32) {
        match v {
            [w, h] if *w >= 360 && *h >= 300 && *w <= 16384 && *h <= 16384 => (*w, *h),
            _ => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        }
    }

    /// `max-search-results` (`i`, default 20) limited to what the geocoder
    /// accepts.
    pub fn max_results(v: i32) -> u32 {
        v.clamp(1, 50) as u32
    }

    /// `rotation` (`d`, radians) normalised to `[0, 2π)`.
    pub fn rotation(v: f64) -> f64 {
        if v.is_finite() {
            v.rem_euclid(std::f64::consts::TAU)
        } else {
            0.0
        }
    }
}

pub fn is_http_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") || s.starts_with("http://")) && s.len() > "https://".len()
}

/// Trims whitespace and trailing slashes; `None` unless an http(s) URL.
pub fn normalize_base_url(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('/');
    is_http_url(s).then(|| s.to_string())
}

/// Checks a raster tile URL template.
pub fn validate_tile_template(t: &str) -> Result<(), &'static str> {
    if !is_http_url(t) {
        return Err("The tile URL must start with http:// or https://");
    }
    for p in ["{z}", "{x}", "{y}"] {
        if !t.contains(p) {
            return Err(tr("The tile URL must contain {z}, {x} and {y}"));
        }
    }
    Ok(())
}

/// Expands a tile URL template.
pub fn expand_tile_url(template: &str, z: i32, x: i32, y: i32) -> String {
    let sub = ["a", "b", "c"][((x + y).rem_euclid(3)) as usize];
    template
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
        .replace("{s}", sub)
}

/// Stable 64-bit FNV-1a hash (used to name per-template cache dirs).
pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let d =
                std::env::temp_dir().join(format!("helix-maps-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            TempDir(d)
        }
        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            p
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn xdg_config_dirs() {
        assert_eq!(config_dirs(None), vec![PathBuf::from("/etc/xdg")]);
        assert_eq!(config_dirs(Some("")), vec![PathBuf::from("/etc/xdg")]);
        assert_eq!(
            config_dirs(Some("/a:relative:/b/")),
            vec![PathBuf::from("/a"), PathBuf::from("/b/")]
        );
    }

    #[test]
    fn toml_layer() {
        let o = Overrides::from_toml(
            "# site config\ngeocoder_url = \"https://geo.example/\"\ntile_max_zoom = 18\nrouter_url = 5\nunknown = true\n",
        )
        .unwrap();
        assert_eq!(o.geocoder_url.as_deref(), Some("https://geo.example/"));
        assert_eq!(o.tile_max_zoom, Some(18));
        assert_eq!(o.router_url, None, "wrong type is skipped, not fatal");
        assert!(Overrides::from_toml("geocoder_url = ").is_err());
    }

    #[test]
    fn precedence_defaults_system_user() {
        let low = TempDir::new("low");
        let high = TempDir::new("high");
        low.write(
            SYSTEM_FILE,
            r#"
            tile_url_template = "https://low.example/{z}/{x}/{y}.png"
            geocoder_url = "https://geo-low.example"
            router_url = "https://route-low.example"
            tile_max_zoom = 17
            "#,
        );
        high.write(
            SYSTEM_FILE,
            r#"
            geocoder_url = "https://geo-high.example/"
            router_url = "not a url"
            "#,
        );
        // $XDG_CONFIG_DIRS lists the most important directory first.
        let dirs = vec![
            high.0.clone(),
            PathBuf::from("/nonexistent-helix-test"),
            low.0.clone(),
        ];
        let system = SystemConfig::load(&dirs);
        assert_eq!(
            system.files(),
            vec![
                low.0.join(SYSTEM_FILE).as_path(),
                high.0.join(SYSTEM_FILE).as_path()
            ]
        );

        // Built-in defaults only where no layer sets a value.
        let sys = system.services();
        assert_eq!(sys.tile_attribution, DEFAULT_ATTRIBUTION);
        assert_eq!(sys.tile_url_template, "https://low.example/{z}/{x}/{y}.png");
        assert_eq!(sys.tile_max_zoom, 17);
        // The first dir in XDG_CONFIG_DIRS beats later ones…
        assert_eq!(sys.geocoder_url, "https://geo-high.example");
        // …but an invalid value there falls through to the lower layer.
        assert_eq!(sys.router_url, "https://route-low.example");
        assert!(!sys.uses_demo_router());

        // No user overrides: the system configuration applies.
        let user = UserSettings::default();
        assert_eq!(user.services(&system), sys);

        // User overrides beat every system layer, key by key.
        let user = UserSettings::from_json(
            r#"{"version":2,"router_url":"https://my-router.example/","tile_max_zoom":99}"#,
        );
        let eff = user.services(&system);
        assert_eq!(eff.router_url, "https://my-router.example");
        assert_eq!(eff.geocoder_url, "https://geo-high.example");
        assert_eq!(eff.tile_max_zoom, 17, "invalid user value is ignored");

        // Without any system file the built-in defaults apply.
        let none = SystemConfig::load(&[PathBuf::from("/nonexistent-helix-test")]);
        assert!(none.layers.is_empty());
        assert_eq!(none.services(), Services::default());
        assert!(UserSettings::default().services(&none).uses_demo_router());
    }

    #[test]
    fn broken_system_file_is_ignored() {
        let d = TempDir::new("broken");
        d.write(SYSTEM_FILE, "geocoder_url = [");
        let system = SystemConfig::load(std::slice::from_ref(&d.0));
        assert!(system.layers.is_empty());
        assert_eq!(system.services(), Services::default());
    }

    #[test]
    fn user_overrides_track_system_values() {
        let system = Services {
            geocoder_url: "https://geo.site".into(),
            ..Services::default()
        };
        let mut o = Overrides::default();
        // Choosing exactly the inherited value stores nothing.
        o.set_from(&system, &system);
        assert!(o.is_empty());
        let wanted = Services {
            router_url: "https://r.example".into(),
            ..system.clone()
        };
        o.set_from(&wanted, &system);
        assert_eq!(
            o,
            Overrides {
                router_url: Some("https://r.example".into()),
                ..Default::default()
            }
        );
        // Reset = clear overrides.
        o = Overrides::default();
        assert_eq!(resolve([&o]), Services::default());
    }

    #[test]
    fn user_file_roundtrip_and_legacy_migration() {
        let u = UserSettings {
            endpoints: Overrides {
                geocoder_url: Some("https://g.example".into()),
                ..Default::default()
            },
            units: "imperial".into(),
        };
        let json = u.to_json();
        assert!(json.contains("\"version\": 2"));
        assert!(!json.contains("router_url"));
        assert_eq!(UserSettings::from_json(&json), u);
        assert_eq!(UserSettings::from_json("not json"), UserSettings::default());
        assert_eq!(
            UserSettings::from_json(r#"{"version":2,"units":"furlongs"}"#).units,
            "metric"
        );

        // The pre-layering file stored every endpoint; unchanged defaults
        // must not shadow the system configuration.
        let legacy = UserSettings::from_json(
            r#"{"tile_url_template":"https://tile.openstreetmap.org/{z}/{x}/{y}.png",
                "tile_attribution":"© OpenStreetMap contributors","tile_max_zoom":19,
                "geocoder_url":"https://nominatim.openstreetmap.org","router_url":"https://osrm.mine/",
                "units":"imperial","zoom_level":3.0,"window_width":1000}"#,
        );
        assert_eq!(
            legacy.endpoints,
            Overrides {
                router_url: Some("https://osrm.mine/".into()),
                ..Default::default()
            }
        );
        assert_eq!(legacy.units, "imperial");

        let d = TempDir::new("user");
        let path = d.0.join("settings.json");
        u.save(&path).unwrap();
        assert_eq!(UserSettings::load(&path), u);
        assert_eq!(
            UserSettings::load(&d.0.join("missing.json")),
            UserSettings::default()
        );
    }

    #[test]
    fn gnome_key_values() {
        use crate::geo::LatLon;
        assert_eq!(gnome::location(&[0.0, 0.0]), LatLon { lat: 0.0, lon: 0.0 });
        assert_eq!(
            gnome::location(&[52.5, 13.4]),
            LatLon {
                lat: 52.5,
                lon: 13.4
            }
        );
        assert_eq!(
            gnome::location(&[200.0, 0.0]),
            LatLon { lat: 0.0, lon: 0.0 }
        );
        assert_eq!(gnome::location(&[1.0]), LatLon { lat: 0.0, lon: 0.0 });
        assert_eq!(gnome::zoom(0, 19), 2.0);
        assert_eq!(gnome::zoom(12, 19), 12.0);
        assert_eq!(gnome::zoom(25, 19), 19.0);
        assert_eq!(gnome::window_size(&[]), (1000, 700));
        assert_eq!(gnome::window_size(&[1280, 800]), (1280, 800));
        assert_eq!(gnome::window_size(&[10, 800]), (1000, 700));
        assert_eq!(gnome::max_results(20), 20);
        assert_eq!(gnome::max_results(500), 50);
        assert_eq!(gnome::max_results(-3), 1);
        assert_eq!(gnome::rotation(f64::NAN), 0.0);
        assert!((gnome::rotation(-1.0) - (std::f64::consts::TAU - 1.0)).abs() < 1e-12);
    }

    #[test]
    fn templates() {
        assert!(validate_tile_template(DEFAULT_TILE_URL).is_ok());
        assert!(validate_tile_template("https://t/{z}/{x}.png").is_err());
        assert!(validate_tile_template("file:///{z}/{x}/{y}").is_err());
        assert_eq!(
            expand_tile_url("https://{s}.t/{z}/{x}/{y}.png", 3, 1, 1),
            "https://c.t/3/1/1.png"
        );
        assert_eq!(
            expand_tile_url(DEFAULT_TILE_URL, 0, 0, 0),
            "https://tile.openstreetmap.org/0/0/0.png"
        );
        assert_eq!(
            normalize_base_url(" https://x.example// ").as_deref(),
            Some("https://x.example")
        );
        assert_eq!(normalize_base_url("ftp://x"), None);
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(fnv1a("x"), fnv1a("y"));
    }
}
