//! App-private window geometry (`org.gnome.Characters` has no such keys),
//! stored as `$XDG_CONFIG_HOME/helix-characters/window.json`.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const DEFAULT_WIDTH: i32 = 860;
pub const DEFAULT_HEIGHT: i32 = 620;
/// Sizes outside this range are ignored on load (corrupt or stale values).
const MIN: i32 = 200;
const MAX: i32 = 16384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowState {
    pub width: i32,
    pub height: i32,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState { width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT, maximized: false }
    }
}

impl WindowState {
    /// Loads state; a missing or corrupt file, or an out-of-range size,
    /// yields the defaults (an out-of-range size keeps `maximized`).
    pub fn load(path: &Path) -> WindowState {
        let Some(s) = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<WindowState>(&s).ok())
        else {
            return WindowState::default();
        };
        let ok = |v: i32| (MIN..=MAX).contains(&v);
        if ok(s.width) && ok(s.height) {
            s
        } else {
            WindowState { maximized: s.maximized, ..WindowState::default() }
        }
    }

    /// Writes atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(self).map_err(io::Error::other)? + "\n")?;
        std::fs::rename(&tmp, path)
    }
}

pub fn default_path() -> PathBuf {
    helix_app_common::config_dir("helix-characters").join("window.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_fallbacks() {
        let dir = std::env::temp_dir().join(format!("helix-characters-win-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("window.json");
        assert_eq!(WindowState::load(&path), WindowState::default());

        let s = WindowState { width: 1000, height: 700, maximized: true };
        s.save(&path).unwrap();
        assert_eq!(WindowState::load(&path), s);
        assert!(!path.with_extension("json.tmp").exists());

        std::fs::write(&path, "{garbage").unwrap();
        assert_eq!(WindowState::load(&path), WindowState::default());

        std::fs::write(&path, r#"{"width": 5, "height": 99999, "maximized": true}"#).unwrap();
        assert_eq!(
            WindowState::load(&path),
            WindowState { maximized: true, ..WindowState::default() }
        );

        std::fs::write(&path, r#"{"width": 900, "height": 640}"#).unwrap();
        assert_eq!(WindowState::load(&path), WindowState { width: 900, height: 640, maximized: false });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
