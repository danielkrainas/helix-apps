//! Helix-private settings that have no `org.gnome.SimpleScan` key.

use crate::options::ColorMode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorChoice {
    /// Grey for text documents, colour for photos.
    #[default]
    Auto,
    Color,
    Gray,
    Lineart,
}

impl ColorChoice {
    pub const ALL: [ColorChoice; 4] = [
        ColorChoice::Auto,
        ColorChoice::Color,
        ColorChoice::Gray,
        ColorChoice::Lineart,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColorChoice::Auto => "Automatic",
            ColorChoice::Color => "Color",
            ColorChoice::Gray => "Grayscale",
            ColorChoice::Lineart => "Black & White",
        }
    }

    pub fn resolve(self, text: bool) -> ColorMode {
        match self {
            ColorChoice::Auto if text => ColorMode::Gray,
            ColorChoice::Auto | ColorChoice::Color => ColorMode::Color,
            ColorChoice::Gray => ColorMode::Gray,
            ColorChoice::Lineart => ColorMode::Lineart,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Private {
    /// SANE `source` value; empty = automatic.
    pub source: String,
    pub color_mode: ColorChoice,
}

impl Private {
    pub fn path() -> PathBuf {
        helix_app_common::config_dir("helix-document-scanner").join("settings.json")
    }

    pub fn load_from(path: &Path) -> Private {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let data = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        assert_eq!(Private::load_from(&path), Private::default());
        let p = Private {
            source: "Flatbed".into(),
            color_mode: ColorChoice::Lineart,
        };
        p.save_to(&path).unwrap();
        assert_eq!(Private::load_from(&path), p);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"color-mode\": \"lineart\""));
        std::fs::write(&path, "{\"source\": \"ADF\"}").unwrap();
        assert_eq!(Private::load_from(&path).color_mode, ColorChoice::Auto);
    }

    #[test]
    fn resolve_auto() {
        assert_eq!(ColorChoice::Auto.resolve(true), ColorMode::Gray);
        assert_eq!(ColorChoice::Auto.resolve(false), ColorMode::Color);
        assert_eq!(ColorChoice::Lineart.resolve(false), ColorMode::Lineart);
    }
}
