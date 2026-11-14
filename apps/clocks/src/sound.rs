//! Alarm and timer tones through GStreamer (PipeWire by default).
//!
//! The tone is the sound theme's event sound (`alarm-clock-elapsed` /
//! `complete`, freedesktop sound theme naming) when a file exists, else a
//! generated beep pattern from `audiotestsrc`. `$HELIX_AUDIO_SINK` (a
//! `gst-launch` description, e.g. `fakesink`) overrides the sink.

use gst::prelude::*;
use gtk::gio::prelude::SettingsExt;
use gtk::glib;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const SINK_ENV: &str = "HELIX_AUDIO_SINK";
const TONE_HZ: u32 = 880;
const TONE_VOLUME: f64 = 0.4;
/// A non-looping (timer) tone stops after this long.
const ONE_SHOT: Duration = Duration::from_millis(2000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Loops until stopped.
    Alarm,
    /// Plays once.
    Timer,
}

impl Sound {
    pub fn event_id(self) -> &'static str {
        match self {
            Sound::Alarm => "alarm-clock-elapsed",
            Sound::Timer => "complete",
        }
    }
    fn looping(self) -> bool {
        self == Sound::Alarm
    }
}

/// Finds `<data dir>/sounds/<theme>/stereo/<id>.{oga,ogg,wav}`, trying each
/// theme (the user's first, `freedesktop` last) in every data directory.
pub fn find_sound(
    id: &str,
    data_dirs: &[PathBuf],
    themes: &[String],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    for theme in themes {
        for dir in data_dirs {
            for ext in ["oga", "ogg", "wav"] {
                let p = dir
                    .join("sounds")
                    .join(theme)
                    .join("stereo")
                    .join(format!("{id}.{ext}"));
                if exists(&p) {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn data_dirs() -> Vec<PathBuf> {
    let mut v = vec![glib::user_data_dir()];
    v.extend(glib::system_data_dirs());
    v
}

fn themes() -> Vec<String> {
    let mut t = Vec::new();
    if let Some(s) = helix_app_common::settings("org.gnome.desktop.sound") {
        let name = s.string("theme-name").to_string();
        if !name.is_empty() && name != "freedesktop" {
            t.push(name);
        }
    }
    t.push("freedesktop".into());
    t
}

/// Generated tone cadence: beep 200 ms, pause 100 ms, beep 200 ms, pause
/// 500 ms.
pub fn tone_on(elapsed: Duration) -> bool {
    let ms = elapsed.as_millis() % 1000;
    ms < 200 || (300..500).contains(&ms)
}

fn sink() -> Option<gst::Element> {
    if let Some(desc) = std::env::var(SINK_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return gst::parse::bin_from_description(&desc, true)
            .ok()
            .map(|b| b.upcast());
    }
    if let Ok(s) = gst::ElementFactory::make("pipewiresink").build() {
        if s.has_property("stream-properties", None) {
            if let Ok(props) = "props,media.role=Alarm".parse::<gst::Structure>() {
                s.set_property("stream-properties", props);
            }
        }
        return Some(s);
    }
    gst::ElementFactory::make("autoaudiosink").build().ok()
}

struct Playing {
    kind: Sound,
    pipeline: gst::Element,
    _bus: Option<gst::bus::BusWatchGuard>,
    source: Option<glib::SourceId>,
}

/// Plays one sound at a time.
#[derive(Default)]
pub struct Player {
    current: Option<Playing>,
    /// Set when GStreamer is unusable or playback failed; callers then fall
    /// back to the display bell.
    failed: Rc<Cell<bool>>,
}

impl Player {
    pub fn playing(&self) -> Option<Sound> {
        self.current.as_ref().map(|p| p.kind)
    }

    pub fn failed(&self) -> bool {
        self.failed.get()
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.current.take() {
            if let Some(s) = p.source {
                s.remove();
            }
            let _ = p.pipeline.set_state(gst::State::Null);
        }
    }

    /// Starts `kind`, replacing whatever plays. Returns false (and marks the
    /// player failed) when nothing could be started.
    pub fn play(&mut self, kind: Sound) -> bool {
        let file = find_sound(kind.event_id(), &data_dirs(), &themes(), |p| p.is_file());
        self.play_file(kind, file)
    }

    /// Plays `file`, or the generated tone when `None`.
    fn play_file(&mut self, kind: Sound, file: Option<PathBuf>) -> bool {
        self.stop();
        self.failed.set(false);
        let ok = gst::init().is_ok() && self.start(kind, file).is_some();
        if !ok {
            self.failed.set(true);
        }
        ok
    }

    fn start(&mut self, kind: Sound, file: Option<PathBuf>) -> Option<()> {
        let (pipeline, volume) = match file {
            Some(path) => {
                let pb = gst::ElementFactory::make("playbin").build().ok()?;
                pb.set_property("uri", glib::filename_to_uri(&path, None).ok()?.as_str());
                if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
                    pb.set_property("video-sink", &fake);
                }
                pb.set_property("audio-sink", sink()?);
                (pb, None)
            }
            None => {
                let bin = gst::Pipeline::new();
                let src = gst::ElementFactory::make("audiotestsrc")
                    .property_from_str("wave", "sine")
                    .property("freq", TONE_HZ as f64)
                    .property("volume", 0.0f64)
                    .property("is-live", true)
                    .build()
                    .ok()?;
                let conv = gst::ElementFactory::make("audioconvert").build().ok()?;
                let res = gst::ElementFactory::make("audioresample").build().ok()?;
                let sink = sink()?;
                bin.add_many([&src, &conv, &res, &sink]).ok()?;
                gst::Element::link_many([&src, &conv, &res, &sink]).ok()?;
                (bin.upcast::<gst::Element>(), Some(src))
            }
        };
        let failed = self.failed.clone();
        let weak = pipeline.downgrade();
        let looping = kind.looping();
        let bus = pipeline.bus()?.add_watch_local(move |_, msg| {
            let Some(p) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            match msg.view() {
                gst::MessageView::Eos(_) if looping => {
                    let _ = p.seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO);
                }
                gst::MessageView::Eos(_) => {
                    let _ = p.set_state(gst::State::Null);
                }
                gst::MessageView::Error(e) => {
                    eprintln!("helix-clocks: sound: {}", e.error());
                    failed.set(true);
                    let _ = p.set_state(gst::State::Null);
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        });
        let started = Instant::now();
        let p2 = pipeline.downgrade();
        let source = (volume.is_some() || !looping).then(|| {
            glib::timeout_add_local(Duration::from_millis(50), move || {
                let Some(p) = p2.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let t = started.elapsed();
                if !looping && t >= ONE_SHOT {
                    let _ = p.set_state(gst::State::Null);
                    return glib::ControlFlow::Break;
                }
                if let Some(src) = &volume {
                    src.set_property("volume", if tone_on(t) { TONE_VOLUME } else { 0.0 });
                }
                glib::ControlFlow::Continue
            })
        });
        if pipeline.set_state(gst::State::Playing).is_err() {
            if let Some(s) = source {
                s.remove();
            }
            let _ = pipeline.set_state(gst::State::Null);
            return None;
        }
        self.current = Some(Playing {
            kind,
            pipeline,
            _bus: bus.ok(),
            source,
        });
        Some(())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_lookup_order() {
        let dirs = vec![
            PathBuf::from("/home/u/.local/share"),
            PathBuf::from("/usr/share"),
        ];
        let themes = vec!["Yaru".to_string(), "freedesktop".to_string()];
        let have = |p: &Path| {
            p == Path::new("/usr/share/sounds/freedesktop/stereo/alarm-clock-elapsed.oga")
                || p == Path::new("/usr/share/sounds/Yaru/stereo/complete.ogg")
        };
        assert_eq!(
            find_sound("alarm-clock-elapsed", &dirs, &themes, have),
            Some(PathBuf::from(
                "/usr/share/sounds/freedesktop/stereo/alarm-clock-elapsed.oga"
            ))
        );
        assert_eq!(
            find_sound("complete", &dirs, &themes, have),
            Some(PathBuf::from("/usr/share/sounds/Yaru/stereo/complete.ogg"))
        );
        assert_eq!(find_sound("bell", &dirs, &themes, have), None);
    }

    #[test]
    fn cadence() {
        let ms = Duration::from_millis;
        assert!(tone_on(ms(0)) && tone_on(ms(199)) && tone_on(ms(350)) && tone_on(ms(1100)));
        assert!(!tone_on(ms(250)) && !tone_on(ms(600)) && !tone_on(ms(999)));
    }

    #[test]
    fn plays_into_fakesink() {
        // Exercises both pipelines without producing sound.
        std::env::set_var(SINK_ENV, "fakesink");
        if gst::init().is_err() {
            return;
        }
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let mut p = Player::default();
            assert!(p.play_file(Sound::Timer, None), "generated tone");
            assert_eq!(p.playing(), Some(Sound::Timer));
            assert!(p.play(Sound::Alarm));
            assert_eq!(p.playing(), Some(Sound::Alarm));
            p.stop();
            assert_eq!(p.playing(), None);
            assert!(!p.failed());
        })
        .unwrap();
    }
}
