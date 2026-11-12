//! GStreamer `playbin` wrapper with gapless track changes.

use gst::prelude::*;
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
    StreamStart { gapless: bool },
    Eos,
    Error(String),
    State(State),
    DurationChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Paused,
    Playing,
}

pub struct Player {
    playbin: gst::Element,
    next_uri: Arc<Mutex<Option<String>>>,
    gapless: Arc<AtomicBool>,
    watch: Option<gst::bus::BusWatchGuard>,
}

impl Player {
    pub fn new(sink: Option<&str>) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let playbin = gst::ElementFactory::make("playbin").name("helix-music").build().map_err(|e| e.to_string())?;
        // Audio only: no video window for files with attached video streams.
        playbin.set_property_from_str("flags", "audio+soft-volume");
        if let Some(desc) = sink.map(str::trim).filter(|s| !s.is_empty()) {
            let bin = gst::parse::bin_from_description(desc, true).map_err(|e| format!("{SINK_ENV}: {e}"))?;
            playbin.set_property("audio-sink", &bin);
        }
        let next_uri: Arc<Mutex<Option<String>>> = Arc::default();
        let gapless = Arc::new(AtomicBool::new(false));
        {
            let next_uri = next_uri.clone();
            let gapless = gapless.clone();
            // Runs on a streaming thread: hand playbin the next URI right away.
            playbin.connect("about-to-finish", false, move |args| {
                let pb = args[0].get::<gst::Element>().ok()?;
                if let Some(uri) = next_uri.lock().ok().and_then(|mut n| n.take()) {
                    gapless.store(true, Ordering::SeqCst);
                    pb.set_property("uri", uri);
                }
                None
            });
        }
        Ok(Player { playbin, next_uri, gapless, watch: None })
    }

    /// Delivers events on the main context.
    pub fn attach(&mut self, on_event: impl Fn(Event) + 'static) {
        let Some(bus) = self.playbin.bus() else { return };
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
        self.gapless.store(false, Ordering::SeqCst);
        let _ = self.playbin.set_state(gst::State::Ready);
        self.playbin.set_property("uri", uri);
        let _ = self.playbin.set_state(if play { gst::State::Playing } else { gst::State::Paused });
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
        self.gapless.store(false, Ordering::SeqCst);
        let _ = self.playbin.set_state(gst::State::Null);
    }

    pub fn state(&self) -> State {
        let (_, cur, pending) = self.playbin.state(gst::ClockTime::ZERO);
        let s = if pending != gst::State::VoidPending { pending } else { cur };
        map_state(s)
    }

    pub fn position_ms(&self) -> Option<u64> {
        self.playbin.query_position::<gst::ClockTime>().map(|t| t.mseconds())
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.playbin.query_duration::<gst::ClockTime>().map(|t| t.mseconds())
    }

    pub fn seek(&self, ms: u64) {
        let _ = self.playbin.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, gst::ClockTime::from_mseconds(ms));
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

fn translate(playbin: &gst::Element, gapless: &AtomicBool, msg: &gst::Message) -> Option<Event> {
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
        MessageView::StreamStart(_) => Some(Event::StreamStart { gapless: gapless.swap(false, Ordering::SeqCst) }),
        MessageView::StateChanged(s) if s.src().is_some_and(|src| src == playbin) => Some(Event::State(map_state(s.current()))),
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
        let bus = player.bus();
        let mut starts = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            assert!(std::time::Instant::now() < deadline, "timed out; starts = {starts:?}");
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else { continue };
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
}
