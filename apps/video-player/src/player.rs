//! Thin wrapper around `playbin`.

use gst::glib;
use gst::prelude::*;
use std::cell::Cell;
use std::sync::Arc;

use crate::sink::{self, FrameSlot};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

/// Environment variable naming the audio sink (a bin description).
pub const AUDIO_SINK_ENV: &str = "HELIX_AUDIO_SINK";

/// Name of the application message posted when the track lists change.
pub const TRACKS_CHANGED: &str = "helix-tracks-changed";

/// Builds the audio sink from `desc` (bin description). When a description
/// is given but cannot be parsed, a `fakesink` is used rather than falling
/// back to real speakers.
pub fn make_audio_sink(desc: Option<&str>) -> Option<gst::Element> {
    let desc = desc.map(str::trim).filter(|d| !d.is_empty())?;
    match gst::parse::bin_from_description(desc, true) {
        Ok(bin) => Some(bin.upcast()),
        Err(err) => {
            eprintln!("helix-video-player: bad {AUDIO_SINK_ENV} ({err}); using fakesink");
            gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
                .ok()
        }
    }
}

/// Creates a playbin rendering into `video_sink`.
pub fn make_playbin(
    video_sink: &gst::Element,
    audio_sink_desc: Option<&str>,
) -> Result<gst::Element, glib::BoolError> {
    let playbin = gst::ElementFactory::make("playbin")
        .name("helix-playbin")
        .build()?;
    playbin.set_property("video-sink", video_sink);
    if let Some(audio) = make_audio_sink(audio_sink_desc) {
        playbin.set_property("audio-sink", &audio);
    }
    if let Ok(tempo) = gst::ElementFactory::make("scaletempo").build() {
        playbin.set_property("audio-filter", &tempo);
    }
    for signal in [
        "audio-changed",
        "text-changed",
        "audio-tags-changed",
        "text-tags-changed",
    ] {
        let weak = playbin.downgrade();
        let handler = move |_: &[glib::Value]| -> Option<glib::Value> {
            if let Some(pb) = weak.upgrade() {
                let s = gst::Structure::builder(TRACKS_CHANGED).build();
                let _ = pb.post_message(gst::message::Application::builder(s).src(&pb).build());
            }
            None
        };
        playbin.connect(signal, false, handler);
    }
    Ok(playbin)
}

/// Description of one audio or subtitle stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub index: i32,
    pub label: String,
}

/// Human-readable label for a track.
pub fn track_label(
    index: i32,
    title: Option<&str>,
    language: Option<&str>,
    codec: Option<&str>,
) -> String {
    let title = title.map(str::trim).filter(|s| !s.is_empty());
    let lang = language
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "und");
    match (title, lang) {
        (Some(t), Some(l)) => format!("{t} ({l})"),
        (Some(t), None) => t.to_string(),
        (None, Some(l)) => format!(
            "{} — {}",
            l,
            codec
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .unwrap_or(tr("Track"))
        ),
        (None, None) => trf!("Track {}", index + 1),
    }
}

pub struct Player {
    pub playbin: gst::Element,
    pub slot: Arc<FrameSlot>,
    rate: Cell<f64>,
}

impl Player {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Result<Player, glib::BoolError> {
        let slot = Arc::new(FrameSlot::default());
        let sink = sink::make_video_sink(slot.clone(), notify);
        let env = std::env::var(AUDIO_SINK_ENV).ok();
        let playbin = make_playbin(sink.upcast_ref(), env.as_deref())?;
        Ok(Player {
            playbin,
            slot,
            rate: Cell::new(1.0),
        })
    }

    pub fn bus(&self) -> gst::Bus {
        self.playbin.bus().expect("playbin has a bus")
    }

    /// Loads `uri` (and an optional external subtitle) and starts it in
    /// the given state.
    pub fn load(&self, uri: &str, suburi: Option<&str>, play: bool) {
        let _ = self.playbin.set_state(gst::State::Null);
        self.slot.take();
        self.rate.set(1.0);
        self.playbin.set_property("uri", uri);
        self.playbin.set_property("suburi", suburi);
        self.set_flag("text", true);
        self.set_state(play);
    }

    pub fn set_state(&self, play: bool) {
        let state = if play {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        let _ = self.playbin.set_state(state);
    }

    pub fn stop(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
        self.slot.take();
    }

    /// Target state (pending if a transition is in progress).
    pub fn target_state(&self) -> gst::State {
        let (_, current, pending) = self.playbin.state(gst::ClockTime::ZERO);
        if pending == gst::State::VoidPending {
            current
        } else {
            pending
        }
    }

    pub fn is_playing(&self) -> bool {
        self.target_state() == gst::State::Playing
    }

    pub fn position(&self) -> Option<f64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
    }

    pub fn duration(&self) -> Option<f64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
            .filter(|d| *d > 0.0)
    }

    pub fn is_seekable(&self) -> bool {
        let mut q = gst::query::Seeking::new(gst::Format::Time);
        self.playbin.query(&mut q) && q.result().0
    }

    fn seek_with(&self, secs: f64, rate: f64, accurate: bool) -> bool {
        let pos = gst::ClockTime::from_nseconds((secs.max(0.0) * 1e9) as u64);
        let mut flags = gst::SeekFlags::FLUSH;
        flags |= if accurate {
            gst::SeekFlags::ACCURATE
        } else {
            gst::SeekFlags::KEY_UNIT
        };
        self.playbin
            .seek(
                rate,
                flags,
                gst::SeekType::Set,
                pos,
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .is_ok()
    }

    pub fn seek(&self, secs: f64, accurate: bool) -> bool {
        let secs = match self.duration() {
            Some(d) => secs.min(d),
            None => secs,
        };
        self.seek_with(secs, self.rate.get(), accurate)
    }

    pub fn rate(&self) -> f64 {
        self.rate.get()
    }

    pub fn set_rate(&self, rate: f64) {
        let rate = rate.clamp(0.25, 4.0);
        self.rate.set(rate);
        if let Some(pos) = self.position() {
            self.seek_with(pos, rate, true);
        }
    }

    /// Linear volume 0..=1.
    pub fn volume(&self) -> f64 {
        self.playbin.property::<f64>("volume")
    }

    pub fn set_volume(&self, v: f64) {
        self.playbin.set_property("volume", v.clamp(0.0, 1.0));
    }

    pub fn muted(&self) -> bool {
        self.playbin.property::<bool>("mute")
    }

    pub fn set_muted(&self, m: bool) {
        self.playbin.set_property("mute", m);
    }

    fn set_flag(&self, nick: &str, on: bool) {
        let value = self.playbin.property_value("flags");
        let Some(class) = glib::FlagsClass::with_type(value.type_()) else {
            return;
        };
        let Some(builder) = class.builder_with_value(value) else {
            return;
        };
        let builder = if on {
            builder.set_by_nick(nick)
        } else {
            builder.unset_by_nick(nick)
        };
        if let Some(v) = builder.build() {
            self.playbin.set_property_from_value("flags", &v);
        }
    }

    fn flag(&self, nick: &str) -> bool {
        let value = self.playbin.property_value("flags");
        glib::FlagsClass::with_type(value.type_())
            .map(|c| c.is_set_by_nick(&value, nick))
            .unwrap_or(false)
    }

    fn tracks(&self, count_prop: &str, signal: &str) -> Vec<Track> {
        let n = self.playbin.property::<i32>(count_prop);
        (0..n)
            .map(|i| {
                let tags = self
                    .playbin
                    .emit_by_name::<Option<gst::TagList>>(signal, &[&i]);
                let get =
                    |f: fn(&gst::TagListRef) -> Option<String>| tags.as_ref().and_then(|t| f(t));
                let title = get(|t| t.get::<gst::tags::Title>().map(|v| v.get().to_string()));
                let lang = get(|t| {
                    t.get::<gst::tags::LanguageCode>()
                        .map(|v| v.get().to_string())
                })
                .or_else(|| {
                    get(|t| {
                        t.get::<gst::tags::LanguageName>()
                            .map(|v| v.get().to_string())
                    })
                });
                let codec = get(|t| {
                    t.get::<gst::tags::AudioCodec>()
                        .map(|v| v.get().to_string())
                        .or_else(|| {
                            t.get::<gst::tags::SubtitleCodec>()
                                .map(|v| v.get().to_string())
                        })
                });
                Track {
                    index: i,
                    label: track_label(i, title.as_deref(), lang.as_deref(), codec.as_deref()),
                }
            })
            .collect()
    }

    pub fn audio_tracks(&self) -> Vec<Track> {
        self.tracks("n-audio", "get-audio-tags")
    }

    pub fn text_tracks(&self) -> Vec<Track> {
        self.tracks("n-text", "get-text-tags")
    }

    pub fn current_audio(&self) -> i32 {
        self.playbin.property::<i32>("current-audio")
    }

    pub fn set_audio(&self, i: i32) {
        self.playbin.set_property("current-audio", i);
    }

    /// Current subtitle stream, `-1` when subtitles are off.
    pub fn current_text(&self) -> i32 {
        if self.flag("text") {
            self.playbin.property::<i32>("current-text")
        } else {
            -1
        }
    }

    pub fn set_text(&self, i: i32) {
        if i < 0 {
            self.set_flag("text", false);
        } else {
            self.set_flag("text", true);
            self.playbin.set_property("current-text", i);
        }
    }

    /// Sets an external subtitle file; the pipeline is restarted at the
    /// current position (the seek happens once it prerolled again; the
    /// position to restore is returned).
    pub fn set_suburi(&self, suburi: &str) -> Option<f64> {
        let pos = self.position();
        let play = self.is_playing();
        let _ = self.playbin.set_state(gst::State::Ready);
        self.playbin.set_property("suburi", suburi);
        self.set_flag("text", true);
        self.set_state(play);
        pos
    }

    /// Advances one frame (only meaningful while paused).
    pub fn step_forward(&self) {
        let ev = gst::event::Step::new(gst::format::Buffers::from_u64(1), 1.0, true, false);
        self.playbin.send_event(ev);
    }

    /// Goes back one frame with an accurate seek.
    pub fn step_back(&self, frame_duration: f64) {
        if let Some(pos) = self.position() {
            self.seek_with((pos - frame_duration).max(0.0), self.rate.get(), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(track_label(0, None, None, None), "Track 1");
        assert_eq!(
            track_label(1, Some("Commentary"), Some("en"), None),
            "Commentary (en)"
        );
        assert_eq!(
            track_label(1, Some("Commentary"), Some("und"), None),
            "Commentary"
        );
        assert_eq!(track_label(2, None, Some("de"), Some("AAC")), "de — AAC");
        assert_eq!(track_label(2, Some("  "), Some("de"), None), "de — Track");
    }

    #[test]
    fn audio_sink_from_env_desc() {
        gst::init().unwrap();
        assert!(make_audio_sink(None).is_none());
        assert!(make_audio_sink(Some("  ")).is_none());
        assert!(make_audio_sink(Some("fakesink sync=true")).is_some());
        // Garbage still yields a (fake) sink, never the real speakers.
        let sink = make_audio_sink(Some("definitely-not-an-element")).unwrap();
        assert_eq!(sink.factory().unwrap().name(), "fakesink");
    }
}
