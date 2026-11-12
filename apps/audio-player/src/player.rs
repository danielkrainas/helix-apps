//! Playback through `playbin` (GTK-independent).

use gst::prelude::*;

use crate::meta::TrackInfo;

/// Environment variable holding a `gst-launch` description of the audio sink.
pub const SINK_ENV: &str = "HELIX_AUDIO_SINK";
const TAGS_CHANGED: &str = "helix-audio-tags-changed";

#[derive(Debug)]
pub enum Event {
    Eos,
    Error(String),
    Tags(TrackInfo),
    DurationChanged,
    /// The pipeline prerolled; carries the number of audio streams.
    Prerolled(i32),
}

pub struct Player {
    playbin: gst::Element,
    /// Whether `scaletempo` keeps pitch on rate changes.
    pub pitch_corrected: bool,
}

fn make_sink(desc: &str) -> Result<gst::Element, String> {
    gst::parse::bin_from_description(desc, true)
        .map(|b| b.upcast())
        .map_err(|e| e.to_string())
}

impl Player {
    /// Creates the player; the audio sink comes from `$HELIX_AUDIO_SINK`
    /// when set.
    pub fn new() -> Result<Self, String> {
        let desc = std::env::var(SINK_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty());
        Self::with_sink(desc.as_deref())
    }

    pub fn with_sink(sink: Option<&str>) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let playbin = gst::ElementFactory::make("playbin")
            .build()
            .map_err(|e| e.to_string())?;
        for prop in ["video-sink", "text-sink"] {
            if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
                playbin.set_property(prop, &fake);
            }
        }
        if let Some(desc) = sink {
            playbin.set_property("audio-sink", make_sink(desc)?);
        }
        let mut pitch_corrected = false;
        if let Ok(tempo) = gst::ElementFactory::make("scaletempo").build() {
            playbin.set_property("audio-filter", &tempo);
            pitch_corrected = true;
        }
        // Stream tags change on a streaming thread; forward to the bus.
        playbin.connect("audio-tags-changed", false, |args| {
            if let Ok(pb) = args[0].get::<gst::Element>() {
                let _ = pb.post_message(gst::message::Application::new(gst::Structure::new_empty(
                    TAGS_CHANGED,
                )));
            }
            None
        });
        Ok(Player {
            playbin,
            pitch_corrected,
        })
    }

    pub fn bus(&self) -> gst::Bus {
        self.playbin.bus().expect("playbin has a bus")
    }

    /// Loads `uri` and prerolls (paused).
    pub fn load(&self, uri: &str) {
        let _ = self.playbin.set_state(gst::State::Null);
        self.playbin.set_property("uri", uri);
        let _ = self.playbin.set_state(gst::State::Paused);
    }

    pub fn unload(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }

    pub fn play(&self) {
        let _ = self.playbin.set_state(gst::State::Playing);
    }

    pub fn pause(&self) {
        let _ = self.playbin.set_state(gst::State::Paused);
    }

    pub fn position(&self) -> Option<u64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds())
    }

    pub fn duration(&self) -> Option<u64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds())
            .filter(|d| *d > 0)
    }

    /// Flushing, accurate seek that also applies `rate`.
    pub fn seek(&self, position_ns: u64, rate: f64) -> bool {
        self.playbin
            .seek(
                rate,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds(position_ns),
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .is_ok()
    }

    /// Linear amplitude 0–1.
    pub fn set_volume(&self, linear: f64) {
        self.playbin.set_property("volume", linear.clamp(0.0, 1.0));
    }

    pub fn set_mute(&self, mute: bool) {
        self.playbin.set_property("mute", mute);
    }

    pub fn n_audio(&self) -> i32 {
        self.playbin.property::<i32>("n-audio")
    }

    /// Tags of the first audio stream as collected by playbin.
    pub fn stream_info(&self) -> TrackInfo {
        let tags = self
            .playbin
            .emit_by_name::<Option<gst::TagList>>("get-audio-tags", &[&0i32]);
        tags.map(|t| info_from_tags(&t)).unwrap_or_default()
    }

    /// Converts a bus message into an [`Event`] the UI cares about.
    pub fn translate(&self, msg: &gst::Message) -> Option<Event> {
        use gst::MessageView;
        match msg.view() {
            MessageView::Eos(_) => Some(Event::Eos),
            MessageView::Error(e) => Some(Event::Error(e.error().to_string())),
            MessageView::Tag(t) => {
                let mut info = info_from_tags(&t.tags());
                info.merge(self.stream_info());
                Some(Event::Tags(info))
            }
            MessageView::Application(a)
                if a.structure().is_some_and(|s| s.name() == TAGS_CHANGED) =>
            {
                Some(Event::Tags(self.stream_info()))
            }
            MessageView::DurationChanged(_) => Some(Event::DurationChanged),
            MessageView::AsyncDone(_) => Some(Event::Prerolled(self.n_audio())),
            _ => None,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

fn tag_string<'a, T: gst::tags::Tag<'a, TagType = &'a str>>(
    tags: &'a gst::TagListRef,
) -> Option<String> {
    tags.get::<T>().map(|v| v.get().to_string())
}

fn image_bytes(sample: &gst::Sample) -> Option<Vec<u8>> {
    let buffer = sample.buffer()?;
    let map = buffer.map_readable().ok()?;
    Some(map.as_slice().to_vec())
}

pub fn info_from_tags(tags: &gst::TagListRef) -> TrackInfo {
    use gst::tags::*;
    let artist = tag_string::<Artist>(tags).or_else(|| tag_string::<AlbumArtist>(tags));
    let cover = tags
        .get::<Image>()
        .and_then(|v| image_bytes(&v.get()))
        .or_else(|| {
            tags.get::<PreviewImage>()
                .and_then(|v| image_bytes(&v.get()))
        });
    TrackInfo::new(
        tag_string::<Title>(tags),
        artist,
        tag_string::<Album>(tags),
        cover,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::tests::{uri, write_wav};

    fn wait_for(player: &Player, want: impl Fn(&Event) -> bool) -> Option<Event> {
        let bus = player.bus();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
                continue;
            };
            if let Some(ev) = player.translate(&msg) {
                if want(&ev) || matches!(ev, Event::Error(_)) {
                    return Some(ev);
                }
            }
        }
        None
    }

    #[test]
    fn prerolls_wav_with_fake_sink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        write_wav(&path, 8000, 1.0, 1.0);
        let player = Player::with_sink(Some("fakesink sync=true")).unwrap();
        player.load(&uri(&path));
        let ev = wait_for(&player, |e| matches!(e, Event::Prerolled(_)));
        assert!(matches!(ev, Some(Event::Prerolled(1))), "{ev:?}");
        assert_eq!(player.stream_info().title, None);
        let d = player.duration().unwrap();
        assert!(
            (d as i64 - 2_000_000_000).abs() < 10_000_000,
            "duration {d}"
        );
        assert!(player.seek(1_000_000_000, 1.5));
        player.unload();
    }

    #[test]
    fn reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.mp3");
        std::fs::write(&path, vec![0x5au8; 4096]).unwrap();
        let player = Player::with_sink(Some("fakesink")).unwrap();
        player.load(&uri(&path));
        let ev = wait_for(&player, |_| false);
        assert!(matches!(ev, Some(Event::Error(_))), "{ev:?}");
    }

    #[test]
    fn tag_extraction() {
        gst::init().unwrap();
        let mut tags = gst::TagList::new();
        {
            let t = tags.get_mut().unwrap();
            t.add::<gst::tags::Title>(&"Song", gst::TagMergeMode::Append);
            t.add::<gst::tags::AlbumArtist>(&"Various", gst::TagMergeMode::Append);
        }
        let info = info_from_tags(&tags);
        assert_eq!(info.title.as_deref(), Some("Song"));
        assert_eq!(info.artist.as_deref(), Some("Various"));
        assert_eq!(info.album, None);
    }
}
