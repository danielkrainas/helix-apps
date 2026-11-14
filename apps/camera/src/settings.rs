//! Persistent preferences.
//!
//! Settings that have a key in GNOME Snapshot's `org.gnome.Snapshot` schema
//! are stored there so they carry over between the two apps: `countdown`,
//! `capture-mode`, `play-shutter-sound`, `last-camera-id`, `window-width`,
//! `window-height`, `is-maximized`. Everything else (the video format, whose
//! Snapshot enum only knows `vp8webm`) lives in
//! `$XDG_CONFIG_HOME/helix-camera/settings.json`. When the Snapshot schema
//! is not installed the JSON file holds every setting instead.

use crate::countdown;
use crate::encoders::VideoFormat;
use glib::prelude::ToVariant;
use helix_app_common::Prefs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "org.gnome.Snapshot";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub countdown: u32,
    pub capture_mode: String,
    pub video_format: String,
    pub play_shutter_sound: bool,
    pub last_camera: Option<String>,
    pub window_width: i32,
    pub window_height: i32,
    pub is_maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            countdown: 0,
            capture_mode: "photo".into(),
            video_format: VideoFormat::WebmVp8.id().into(),
            play_shutter_sound: true,
            last_camera: None,
            window_width: 800,
            window_height: 640,
            is_maximized: false,
        }
    }
}

/// The private file when GSettings holds the rest.
#[derive(Serialize)]
struct PrivateOnly<'a> {
    video_format: &'a str,
}

/// Snapshot's `capture-mode` nick for our mode id.
pub fn mode_to_nick(mode: &str) -> &'static str {
    if mode == "video" {
        "video"
    } else {
        "picture"
    }
}

pub fn mode_from_nick(nick: &str) -> &'static str {
    if nick == "video" {
        "video"
    } else {
        "photo"
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        helix_app_common::config_dir("helix-camera").join("settings.json")
    }

    fn sanitize(mut self) -> Settings {
        self.countdown = countdown::normalize_delay(self.countdown);
        if self.capture_mode != "video" {
            self.capture_mode = "photo".into();
        }
        if VideoFormat::from_id(&self.video_format).is_none() {
            self.video_format = VideoFormat::WebmVp8.id().into();
        }
        if self.last_camera.as_deref() == Some("") {
            self.last_camera = None;
        }
        if self.window_width < 200 || self.window_height < 200 {
            let d = Settings::default();
            (self.window_width, self.window_height) = (d.window_width, d.window_height);
        }
        self
    }

    pub fn load_from(path: &Path) -> Settings {
        let s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.sanitize()
    }

    /// Loads from `prefs` (Snapshot keys) when persistent, the file otherwise.
    pub fn load(prefs: &Prefs, path: &Path) -> Settings {
        let mut s = Settings::load_from(path);
        if prefs.is_persistent() {
            s.countdown = prefs.get::<i32>("countdown", 0).max(0) as u32;
            s.capture_mode = mode_from_nick(&prefs.get_enum_nick("capture-mode", "picture")).into();
            s.play_shutter_sound = prefs.get("play-shutter-sound", true);
            let cam: String = prefs.get("last-camera-id", String::new());
            s.last_camera = Some(cam);
            s.window_width = prefs.get("window-width", 800);
            s.window_height = prefs.get("window-height", 640);
            s.is_maximized = prefs.get("is-maximized", false);
        }
        s.sanitize()
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Writes Snapshot keys to `prefs` and the rest to `path` (everything to
    /// `path` when the schema is missing).
    pub fn save(&self, prefs: &Prefs, path: &Path) -> std::io::Result<()> {
        if !prefs.is_persistent() {
            return self.save_to(path);
        }
        let set_if = |key: &str, v: glib::Variant| {
            if prefs.get_variant(key).as_ref() != Some(&v) {
                prefs.set_variant(key, v);
            }
        };
        set_if("countdown", (self.countdown as i32).to_variant());
        if prefs.get_enum_nick("capture-mode", "picture") != mode_to_nick(&self.capture_mode) {
            prefs.set_enum_nick("capture-mode", mode_to_nick(&self.capture_mode));
        }
        set_if("play-shutter-sound", self.play_shutter_sound.to_variant());
        set_if(
            "last-camera-id",
            self.last_camera.clone().unwrap_or_default().to_variant(),
        );
        set_if("window-width", self.window_width.to_variant());
        set_if("window-height", self.window_height.to_variant());
        set_if("is-maximized", self.is_maximized.to_variant());
        let json = serde_json::to_string_pretty(&PrivateOnly {
            video_format: &self.video_format,
        })
        .map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn video_format(&self) -> VideoFormat {
        VideoFormat::from_id(&self.video_format).unwrap_or(VideoFormat::WebmVp8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helix-camera-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_and_sanitize() {
        let dir = temp("settings");
        let path = dir.join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
        let s = Settings {
            countdown: 5,
            capture_mode: "video".into(),
            video_format: "mp4-h264".into(),
            play_shutter_sound: false,
            last_camera: Some("Cam".into()),
            window_width: 1000,
            window_height: 700,
            is_maximized: true,
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        std::fs::write(
            &path,
            r#"{"countdown": 7, "capture_mode": "x", "video_format": "avi"}"#,
        )
        .unwrap();
        let s = Settings::load_from(&path);
        assert_eq!((s.countdown, s.capture_mode.as_str()), (0, "photo"));
        assert_eq!(s.video_format(), VideoFormat::WebmVp8);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn capture_mode_nicks() {
        assert_eq!(mode_to_nick("photo"), "picture");
        assert_eq!(mode_to_nick("video"), "video");
        assert_eq!(mode_from_nick("picture"), "photo");
        assert_eq!(mode_from_nick("video"), "video");
        assert_eq!(mode_from_nick("bogus"), "photo");
    }

    #[test]
    fn missing_schema_falls_back_to_file() {
        let dir = temp("fallback");
        let path = dir.join("settings.json");
        let prefs = Prefs::new("org.helix.Camera.DoesNotExist");
        assert!(!prefs.is_persistent());
        let s = Settings {
            countdown: 3,
            play_shutter_sound: false,
            ..Settings::default()
        };
        s.save(&prefs, &path).unwrap();
        assert_eq!(Settings::load(&prefs, &path), s);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
