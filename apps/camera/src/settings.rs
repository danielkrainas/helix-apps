//! Persistent preferences (`$XDG_CONFIG_HOME/helix-camera/settings.json`).

use crate::countdown;
use crate::encoders::VideoFormat;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub countdown: u32,
    pub capture_mode: String,
    pub video_format: String,
    pub play_shutter_sound: bool,
    pub last_camera: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            countdown: 0,
            capture_mode: "photo".into(),
            video_format: VideoFormat::WebmVp8.id().into(),
            play_shutter_sound: true,
            last_camera: None,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        helix_app_common::config_dir("helix-camera").join("settings.json")
    }

    pub fn load_from(path: &Path) -> Settings {
        let mut s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.countdown = countdown::normalize_delay(s.countdown);
        if s.capture_mode != "video" {
            s.capture_mode = "photo".into();
        }
        if VideoFormat::from_id(&s.video_format).is_none() {
            s.video_format = VideoFormat::WebmVp8.id().into();
        }
        s
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn video_format(&self) -> VideoFormat {
        VideoFormat::from_id(&self.video_format).unwrap_or(VideoFormat::WebmVp8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_sanitize() {
        let dir = std::env::temp_dir().join(format!("helix-camera-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
        let s = Settings {
            countdown: 5,
            capture_mode: "video".into(),
            video_format: "mp4-h264".into(),
            play_shutter_sound: false,
            last_camera: Some("Cam".into()),
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        std::fs::write(&path, r#"{"countdown": 7, "capture_mode": "x", "video_format": "avi"}"#).unwrap();
        let s = Settings::load_from(&path);
        assert_eq!((s.countdown, s.capture_mode.as_str()), (0, "photo"));
        assert_eq!(s.video_format(), VideoFormat::WebmVp8);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
