//! Playback preferences remembered across sessions
//! (`$XDG_CONFIG_HOME/helix-video-player/settings.json`).
//!
//! `org.gnome.totem` has no volume or playback-speed keys, so these are
//! app-private.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MIN_RATE: f64 = 0.5;
pub const MAX_RATE: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Linear volume, 0..=1.
    pub volume: f64,
    /// Playback speed, `MIN_RATE..=MAX_RATE`.
    pub rate: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { volume: 1.0, rate: 1.0 }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        helix_app_common::config_dir("helix-video-player").join("settings.json")
    }

    /// Clamps out-of-range values and replaces non-finite ones by defaults.
    pub fn sanitized(self) -> Settings {
        let d = Settings::default();
        let volume = if self.volume.is_finite() { self.volume.clamp(0.0, 1.0) } else { d.volume };
        let rate = if self.rate.is_finite() && self.rate > 0.0 { self.rate.clamp(MIN_RATE, MAX_RATE) } else { d.rate };
        Settings { volume, rate }
    }

    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
        let s = Settings { volume: 0.42, rate: 1.5 };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        assert!(!dir.path().join("settings.json.tmp").exists());
    }

    #[test]
    fn sanitizes_bad_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"volume": 3.0, "rate": 9}"#).unwrap();
        assert_eq!(Settings::load_from(&path), Settings { volume: 1.0, rate: MAX_RATE });
        std::fs::write(&path, r#"{"volume": -1, "rate": 0}"#).unwrap();
        assert_eq!(Settings::load_from(&path), Settings { volume: 0.0, rate: 1.0 });
        std::fs::write(&path, r#"{"rate": 0.75}"#).unwrap();
        assert_eq!(Settings::load_from(&path), Settings { volume: 1.0, rate: 0.75 });
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert_eq!(Settings { volume: f64::NAN, rate: f64::INFINITY }.sanitized(), Settings::default());
    }
}
