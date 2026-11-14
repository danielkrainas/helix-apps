//! App-private settings that have no `org.gnome.Disks` key, stored as JSON
//! in `$XDG_CONFIG_HOME/helix-disks/settings.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    /// Hide loop devices that back `/snap/*` mounts (default: true).
    pub hide_snap_loops: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hide_snap_loops: true,
        }
    }
}

pub fn path() -> PathBuf {
    helix_app_common::config_dir("helix-disks").join("settings.json")
}

impl Settings {
    /// Reads `path`; a missing or malformed file gives the defaults.
    pub fn load_from(path: &Path) -> Settings {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(
            path,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("helix-disks-settings-{}", std::process::id()));
        let p = dir.join("sub").join("settings.json");
        assert_eq!(
            Settings::load_from(&p),
            Settings {
                hide_snap_loops: true
            }
        );
        let s = Settings {
            hide_snap_loops: false,
        };
        s.save_to(&p).unwrap();
        assert_eq!(Settings::load_from(&p), s);
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains("\"hide-snap-loops\": false"));
        // Unknown/missing keys fall back to defaults; garbage too.
        std::fs::write(&p, "{\"other\": 1}").unwrap();
        assert!(Settings::load_from(&p).hide_snap_loops);
        std::fs::write(&p, "not json").unwrap();
        assert!(Settings::load_from(&p).hide_snap_loops);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
