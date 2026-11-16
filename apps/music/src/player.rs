//! GStreamer `playbin` wrapper with gapless track changes.

use gst::prelude::*;
use helix_app_common::i18n::tr;
use helix_app_common::nc_;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Environment variable overriding the audio sink (a `gst-launch` bin
/// description, e.g. `fakesink sync=true`). Tests and smoke runs set it so
/// nothing reaches the speakers.
pub const SINK_ENV: &str = "HELIX_AUDIO_SINK";

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A new stream started. `gapless` is true when it was queued from
    /// `about-to-finish` (the queue should advance).
    StreamStart {
        gapless: bool,
    },
    Eos,
    Error(String),
    State(State),
    DurationChanged,
}

/// Which ReplayGain value `rgvolume` applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplayGain {
    #[default]
    Off,
    Track,
    Album,
}

impl ReplayGain {
    pub const ALL: [ReplayGain; 3] = [ReplayGain::Off, ReplayGain::Track, ReplayGain::Album];

    pub fn nick(self) -> &'static str {
        match self {
            ReplayGain::Off => "off",
            ReplayGain::Track => "track",
            ReplayGain::Album => "album",
        }
    }

    pub fn from_nick(s: &str) -> Option<ReplayGain> {
        ReplayGain::ALL.into_iter().find(|m| m.nick() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            ReplayGain::Off => tr("Off"),
            ReplayGain::Track => tr(nc_!("ReplayGain mode", "Track")),
            ReplayGain::Album => tr(nc_!("ReplayGain mode", "Album")),
        }
    }

    /// From GNOME Music's boolean `replaygain` key plus Helix's remembered
    /// track/album choice (GNOME's "on" means album gain).
    pub fn from_settings(enabled: bool, mode: &str) -> ReplayGain {
        match (enabled, ReplayGain::from_nick(mode)) {
            (false, _) => ReplayGain::Off,
            (true, Some(ReplayGain::Track)) => ReplayGain::Track,
            (true, _) => ReplayGain::Album,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Paused,
    Playing,
}

/// Tracks which stream starts were queued from `about-to-finish`.
///
/// The flag is read when GStreamer posts each `stream-start`, not when the
/// main loop gets to the message: by then the next `about-to-finish` may
/// already have fired (short tracks, or a busy main loop).
#[derive(Default)]
struct Gapless {
    /// Set by `about-to-finish` when it hands playbin the next URI.
    queued: AtomicBool,
    /// Per posted `stream-start` message: whether it was gapless.
    starts: Mutex<Vec<(gst::Seqnum, bool)>>,
}

impl Gapless {
    fn reset(&self) {
        self.queued.store(false, Ordering::SeqCst);
    }

    fn take(&self, msg: &gst::Message) -> bool {
        let Ok(mut starts) = self.starts.lock() else {
            return false;
        };
        match starts.iter().position(|(n, _)| *n == msg.seqnum()) {
            Some(i) => starts.remove(i).1,
            None => false,
        }
    }
}

pub struct Player {
    playbin: gst::Element,
    next_uri: Arc<Mutex<Option<String>>>,
    gapless: Arc<Gapless>,
    watch: Option<gst::bus::BusWatchGuard>,
    /// `rgvolume` (when the plugin is available) and the mode it should use.
    rgvolume: Option<gst::Element>,
    replaygain: Cell<ReplayGain>,
    /// Whether `rgvolume` is currently plugged in as playbin's audio filter.
    filter_on: Cell<bool>,
}

impl Player {
    pub fn new(sink: Option<&str>) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let playbin = gst::ElementFactory::make("playbin")
            .name("helix-music")
            .build()
            .map_err(|e| e.to_string())?;
        // Audio only: no video window for files with attached video streams.
        playbin.set_property_from_str("flags", "audio+soft-volume");
        if let Some(desc) = sink.map(str::trim).filter(|s| !s.is_empty()) {
            let bin = gst::parse::bin_from_description(desc, true)
                .map_err(|e| format!("{SINK_ENV}: {e}"))?;
            playbin.set_property("audio-sink", &bin);
        }
        let next_uri: Arc<Mutex<Option<String>>> = Arc::default();
        let gapless = Arc::new(Gapless::default());
        if let Some(bus) = playbin.bus() {
            let gapless = gapless.clone();
            // Runs on the posting thread, in order with `about-to-finish`.
            bus.set_sync_handler(move |_, msg| {
                if let gst::MessageView::StreamStart(_) = msg.view() {
                    let was = gapless.queued.swap(false, Ordering::SeqCst);
                    if let Ok(mut starts) = gapless.starts.lock() {
                        // Messages dropped unread (e.g. a bus flush) must not
                        // pile up.
                        if starts.len() > 64 {
                            starts.clear();
                        }
                        starts.push((msg.seqnum(), was));
                    }
                }
                gst::BusSyncReply::Pass
            });
        }
        {
            let next_uri = next_uri.clone();
            let gapless = gapless.clone();
            // Runs on a streaming thread: hand playbin the next URI right away.
            playbin.connect("about-to-finish", false, move |args| {
                let pb = args[0].get::<gst::Element>().ok()?;
                if let Some(uri) = next_uri.lock().ok().and_then(|mut n| n.take()) {
                    gapless.queued.store(true, Ordering::SeqCst);
                    pb.set_property("uri", uri);
                }
                None
            });
        }
        let rgvolume = gst::ElementFactory::make("rgvolume").build().ok();
        if rgvolume.is_none() {
            eprintln!("helix-music: rgvolume not available; ReplayGain disabled");
        }
        Ok(Player {
            playbin,
            next_uri,
            gapless,
            watch: None,
            rgvolume,
            replaygain: Cell::new(ReplayGain::Off),
            filter_on: Cell::new(false),
        })
    }

    /// Selects the ReplayGain mode. Switching between track and album gain
    /// takes effect immediately; turning it on or off (which changes the
    /// pipeline) applies from the next track that is loaded.
    pub fn set_replaygain(&self, mode: ReplayGain) {
        self.replaygain.set(mode);
        if let Some(rg) = &self.rgvolume {
            rg.set_property("album-mode", mode == ReplayGain::Album);
        }
        let (_, cur, _) = self.playbin.state(gst::ClockTime::ZERO);
        if cur <= gst::State::Ready {
            self.apply_filter();
        }
    }

    /// Plugs `rgvolume` in or out; only valid in the NULL/READY state.
    fn apply_filter(&self) {
        let Some(rg) = &self.rgvolume else { return };
        let want = self.replaygain.get() != ReplayGain::Off;
        if want != self.filter_on.get() {
            self.playbin.set_property(
                "audio-filter",
                if want {
                    Some(rg)
                } else {
                    None::<&gst::Element>
                },
            );
            self.filter_on.set(want);
        }
    }

    /// Delivers events on the main context.
    pub fn attach(&mut self, on_event: impl Fn(Event) + 'static) {
        let Some(bus) = self.playbin.bus() else {
            return;
        };
        let playbin = self.playbin.clone();
        let gapless = self.gapless.clone();
        self.watch = bus
            .add_watch_local(move |_, msg| {
                if let Some(ev) = translate(&playbin, &gapless, msg) {
                    on_event(ev);
                }
                glib::ControlFlow::Continue
            })
            .ok();
    }

    /// Starts `uri` from the beginning.
    pub fn load(&self, uri: &str, play: bool) {
        self.gapless.reset();
        let _ = self.playbin.set_state(gst::State::Ready);
        self.apply_filter();
        self.playbin.set_property("uri", uri);
        let _ = self.playbin.set_state(if play {
            gst::State::Playing
        } else {
            gst::State::Paused
        });
    }

    /// The URI to continue with gaplessly when the current one ends.
    pub fn set_next(&self, uri: Option<String>) {
        if let Ok(mut n) = self.next_uri.lock() {
            *n = uri;
        }
    }

    pub fn play(&self) {
        let _ = self.playbin.set_state(gst::State::Playing);
    }

    pub fn pause(&self) {
        let _ = self.playbin.set_state(gst::State::Paused);
    }

    pub fn stop(&self) {
        self.gapless.reset();
        let _ = self.playbin.set_state(gst::State::Null);
    }

    pub fn state(&self) -> State {
        let (_, cur, pending) = self.playbin.state(gst::ClockTime::ZERO);
        let s = if pending != gst::State::VoidPending {
            pending
        } else {
            cur
        };
        map_state(s)
    }

    pub fn position_ms(&self) -> Option<u64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.mseconds())
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.mseconds())
    }

    pub fn seek(&self, ms: u64) {
        let _ = self.playbin.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::ClockTime::from_mseconds(ms),
        );
    }

    /// `level` is the slider position 0..=1; mapped cubically.
    pub fn set_volume(&self, level: f64) {
        self.playbin.set_property("volume", slider_to_linear(level));
    }

    #[cfg(test)]
    fn bus(&self) -> gst::Bus {
        self.playbin.bus().unwrap()
    }

    #[cfg(test)]
    fn translate(&self, msg: &gst::Message) -> Option<Event> {
        translate(&self.playbin, &self.gapless, msg)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

pub fn slider_to_linear(level: f64) -> f64 {
    level.clamp(0.0, 1.0).powi(3)
}

fn map_state(s: gst::State) -> State {
    match s {
        gst::State::Playing => State::Playing,
        gst::State::Paused => State::Paused,
        _ => State::Stopped,
    }
}

fn translate(playbin: &gst::Element, gapless: &Gapless, msg: &gst::Message) -> Option<Event> {
    use gst::MessageView;
    match msg.view() {
        MessageView::Eos(_) => Some(Event::Eos),
        MessageView::Error(e) => {
            let mut text = e.error().to_string();
            if let Some(d) = e.debug() {
                text.push_str(&format!(" ({d})"));
            }
            Some(Event::Error(text))
        }
        MessageView::StreamStart(_) => Some(Event::StreamStart {
            gapless: gapless.take(msg),
        }),
        MessageView::StateChanged(s) if s.src().is_some_and(|src| src == playbin) => {
            Some(Event::State(map_state(s.current())))
        }
        MessageView::DurationChanged(_) => Some(Event::DurationChanged),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::testutil::write_wav;

    #[test]
    fn volume_curve() {
        assert_eq!(slider_to_linear(0.5), 0.125);
        assert_eq!(slider_to_linear(2.0), 1.0);
    }

    /// Plays two short files through a fake sink and checks that the second
    /// one starts gaplessly (no EOS in between).
    #[test]
    fn gapless_transition_with_fakesink() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        write_wav(&a, 300);
        write_wav(&b, 300);
        let player = match Player::new(Some("fakesink sync=false")) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        player.set_next(Some(crate::model::path_to_uri(&b)));
        player.load(&crate::model::path_to_uri(&a), true);
        // Read the bus only after both files have played (fakesink with
        // sync=false takes milliseconds): the first stream-start must still
        // be reported as not gapless although `about-to-finish` has fired.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let bus = player.bus();
        let mut starts = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out; starts = {starts:?}"
            );
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
                continue;
            };
            match player.translate(&msg) {
                Some(Event::StreamStart { gapless }) => starts.push(gapless),
                Some(Event::Eos) => break,
                Some(Event::Error(e)) => panic!("{e}"),
                _ => {}
            }
        }
        assert_eq!(starts, [false, true]);
        player.stop();
    }

    #[test]
    fn replaygain_settings() {
        assert_eq!(ReplayGain::from_settings(false, "track"), ReplayGain::Off);
        assert_eq!(ReplayGain::from_settings(true, "track"), ReplayGain::Track);
        assert_eq!(ReplayGain::from_settings(true, ""), ReplayGain::Album);
        for m in ReplayGain::ALL {
            assert_eq!(ReplayGain::from_nick(m.nick()), Some(m));
        }
    }

    /// Plays a file through `rgvolume` into a fake sink.
    #[test]
    fn replaygain_filter_plays() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        write_wav(&a, 200);
        let player = match Player::new(Some("fakesink sync=false")) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        if player.rgvolume.is_none() {
            eprintln!("skipping: no rgvolume");
            return;
        }
        player.set_replaygain(ReplayGain::Track);
        player.load(&crate::model::path_to_uri(&a), true);
        assert!(player.filter_on.get());
        let rg = player.rgvolume.as_ref().unwrap();
        assert!(!rg.property::<bool>("album-mode"));
        let bus = player.bus();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            assert!(std::time::Instant::now() < deadline, "timed out");
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
                continue;
            };
            match player.translate(&msg) {
                Some(Event::Eos) => break,
                Some(Event::Error(e)) => panic!("{e}"),
                _ => {}
            }
        }
        // Switching to album mode is live; turning it off waits for a reload.
        player.set_replaygain(ReplayGain::Album);
        assert!(rg.property::<bool>("album-mode"));
        player.stop();
        player.set_replaygain(ReplayGain::Off);
        assert!(!player.filter_on.get());
    }
}
