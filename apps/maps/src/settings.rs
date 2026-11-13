//! Helix-private settings, stored as JSON under the config dir.
//!
//! `org.gnome.Maps` is not installed on the reference system, so nothing is
//! read from GSettings; see SPEC.md.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
pub const DEFAULT_ATTRIBUTION: &str = "© OpenStreetMap contributors";
pub const DEFAULT_ATTRIBUTION_URL: &str = "https://www.openstreetmap.org/copyright";
pub const DEFAULT_GEOCODER: &str = "https://nominatim.openstreetmap.org";
pub const DEFAULT_ROUTER: &str = "https://router.project-osrm.org";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub tile_url_template: String,
    pub tile_attribution: String,
    pub tile_attribution_url: String,
    pub tile_max_zoom: u32,
    pub geocoder_url: String,
    pub router_url: String,
    pub max_search_results: u32,
    pub units: String,
    pub transportation_type: String,
    pub last_viewed_location: [f64; 2],
    pub zoom_level: f64,
    pub window_width: i32,
    pub window_height: i32,
    pub window_maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            tile_url_template: DEFAULT_TILE_URL.into(),
            tile_attribution: DEFAULT_ATTRIBUTION.into(),
            tile_attribution_url: DEFAULT_ATTRIBUTION_URL.into(),
            tile_max_zoom: 19,
            geocoder_url: DEFAULT_GEOCODER.into(),
            router_url: DEFAULT_ROUTER.into(),
            max_search_results: 10,
            units: "metric".into(),
            transportation_type: "car".into(),
            last_viewed_location: [30.0, 10.0],
            zoom_level: 3.0,
            window_width: 1000,
            window_height: 700,
            window_maximized: false,
        }
    }
}

impl Settings {
    /// Parses settings JSON; malformed input yields defaults and invalid
    /// individual values are replaced by their defaults.
    pub fn from_json(text: &str) -> Settings {
        let mut s: Settings = serde_json::from_str(text).unwrap_or_default();
        s.sanitize();
        s
    }

    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path).map(|t| Settings::from_json(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    pub fn sanitize(&mut self) {
        let d = Settings::default();
        if validate_tile_template(&self.tile_url_template).is_err() {
            self.tile_url_template = d.tile_url_template;
        }
        if !is_http_url(&self.geocoder_url) {
            self.geocoder_url = d.geocoder_url;
        }
        if !is_http_url(&self.router_url) {
            self.router_url = d.router_url;
        }
        self.geocoder_url = self.geocoder_url.trim_end_matches('/').to_string();
        self.router_url = self.router_url.trim_end_matches('/').to_string();
        self.tile_max_zoom = self.tile_max_zoom.clamp(2, 22);
        self.max_search_results = self.max_search_results.clamp(1, 50);
        if !matches!(self.units.as_str(), "metric" | "imperial") {
            self.units = d.units;
        }
        if !matches!(self.transportation_type.as_str(), "car" | "bike" | "walk") {
            self.transportation_type = d.transportation_type;
        }
        let [lat, lon] = self.last_viewed_location;
        if crate::geo::LatLon::new(lat, lon).is_none() {
            self.last_viewed_location = d.last_viewed_location;
        }
        if !self.zoom_level.is_finite() {
            self.zoom_level = d.zoom_level;
        }
        self.zoom_level = self.zoom_level.clamp(2.0, self.tile_max_zoom as f64);
        if self.window_width < 360 || self.window_height < 300 {
            self.window_width = d.window_width;
            self.window_height = d.window_height;
        }
    }

    /// Whether the default (public, policy-restricted) OSRM demo is used.
    pub fn uses_demo_router(&self) -> bool {
        self.router_url == DEFAULT_ROUTER
    }
}

pub fn is_http_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") || s.starts_with("http://")) && s.len() > "https://".len()
}

/// Checks a raster tile URL template.
pub fn validate_tile_template(t: &str) -> Result<(), &'static str> {
    if !is_http_url(t) {
        return Err("The tile URL must start with http:// or https://");
    }
    for p in ["{z}", "{x}", "{y}"] {
        if !t.contains(p) {
            return Err("The tile URL must contain {z}, {x} and {y}");
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

    #[test]
    fn defaults_and_roundtrip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(Settings::from_json(&json), s);
        assert_eq!(Settings::from_json("not json"), s);
        assert!(s.uses_demo_router());
    }

    #[test]
    fn partial_and_invalid_values() {
        let s = Settings::from_json(
            r#"{"tile_url_template":"ftp://x","geocoder_url":"https://geo.example/",
                "max_search_results":500,"units":"furlongs","zoom_level":99,
                "last_viewed_location":[200,0],"transportation_type":"bike"}"#,
        );
        assert_eq!(s.tile_url_template, DEFAULT_TILE_URL);
        assert_eq!(s.geocoder_url, "https://geo.example");
        assert_eq!(s.max_search_results, 50);
        assert_eq!(s.units, "metric");
        assert_eq!(s.zoom_level, 19.0);
        assert_eq!(s.last_viewed_location, [30.0, 10.0]);
        assert_eq!(s.transportation_type, "bike");
    }

    #[test]
    fn save_load() {
        let dir = std::env::temp_dir().join(format!("helix-maps-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let s = Settings { zoom_level: 7.5, units: "imperial".into(), ..Default::default() };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert_eq!(expand_tile_url(DEFAULT_TILE_URL, 0, 0, 0), "https://tile.openstreetmap.org/0/0/0.png");
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(fnv1a("x"), fnv1a("y"));
    }
}
