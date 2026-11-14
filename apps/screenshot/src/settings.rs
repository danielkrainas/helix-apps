//! The remembered choices, in `$XDG_CONFIG_HOME/helix-screenshot/settings.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::args::MAX_DELAY;
use crate::shell::Mode;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    pub delay: u32,
    pub pointer: bool,
    pub last_folder: Option<PathBuf>,
}

impl Settings {
    pub fn path() -> PathBuf {
        helix_app_common::config_dir("helix-screenshot").join("settings.json")
    }

    /// Reads `path`; anything missing or broken gives the defaults.
    pub fn load(path: &Path) -> Self {
        let mut s: Settings = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        s.delay = s.delay.min(MAX_DELAY);
        s
    }

    /// Writes `path` atomically.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_fallbacks() {
        let dir =
            std::env::temp_dir().join(format!("helix-screenshot-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        let s = Settings {
            mode: Mode::Area,
            delay: 7,
            pointer: true,
            last_folder: Some("/home/me/Pictures".into()),
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"area\""), "{text}");

        std::fs::write(&path, "{\"delay\": 500, \"mode\": \"window\"}").unwrap();
        let s = Settings::load(&path);
        assert_eq!(
            (s.mode, s.delay, s.pointer),
            (Mode::Window, MAX_DELAY, false)
        );

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }
}
