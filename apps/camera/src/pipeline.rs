//! GStreamer capture pipelines: viewfinder, stills and recording.

use crate::devices::Location;
use crate::encoders::EncoderPlan;
use gst::prelude::*;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, String>;

/// Where a camera's video comes from.
#[derive(Debug, Clone)]
pub enum SourceSpec {
    /// `videotestsrc` with the given pattern (tests / `HELIX_CAMERA_SOURCE=test`).
    Test { pattern: &'static str, width: i32, height: i32 },
    /// A GStreamer device, optionally on a portal PipeWire remote `fd`.
    Device { device: gst::Device, fd: Option<RawFd> },
}

#[derive(Debug, Clone)]
pub struct Camera {
    pub name: String,
    pub location: Location,
    pub spec: SourceSpec,
}

/// A tightly described RGBA frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub data: glib::Bytes,
}

pub type FrameSink = Arc<dyn Fn(Frame) + Send + Sync>;

/// Audio input for recordings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioInput {
    /// `audiotestsrc` (silence).
    Test,
    /// The default microphone.
    Microphone,
    None,
}

#[derive(Debug, Clone)]
pub struct RecordSpec {
    pub path: PathBuf,
    pub plan: EncoderPlan,
    pub audio: AudioInput,
}

fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .build()
        .map_err(|_| format!("Missing GStreamer element “{factory}”"))
}

/// Sets a property only if the element has it (encoders differ between
/// versions).
fn try_set_str(e: &gst::Element, name: &str, value: &str) {
    if e.find_property(name).is_some() {
        e.set_property_from_str(name, value);
    }
}

fn source_element(spec: &SourceSpec) -> Result<gst::Element> {
    match spec {
        SourceSpec::Test { pattern, width, height } => {
            let bin = gst::Bin::new();
            let src = make("videotestsrc")?;
            src.set_property("is-live", true);
            src.set_property_from_str("pattern", pattern);
            let caps = make("capsfilter")?;
            caps.set_property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", *width)
                    .field("height", *height)
                    .field("framerate", gst::Fraction::new(30, 1))
                    .build(),
            );
            bin.add_many([&src, &caps]).map_err(|e| e.to_string())?;
            src.link(&caps).map_err(|e| e.to_string())?;
            let pad = caps.static_pad("src").unwrap();
            let ghost = gst::GhostPad::with_target(&pad).map_err(|e| e.to_string())?;
            bin.add_pad(&ghost).map_err(|e| e.to_string())?;
            Ok(bin.upcast())
        }
        SourceSpec::Device { device, fd } => {
            let e = device
                .create_element(None)
                .map_err(|_| format!("Could not open camera “{}”", device.display_name()))?;
            if let Some(fd) = fd {
                if e.find_property("fd").is_some() {
                    e.set_property("fd", *fd);
                }
            }
            Ok(e)
        }
    }
}

/// A running camera pipeline.
pub struct Session {
    pipeline: gst::Pipeline,
    still: gst::Element,
    flip: gst::Element,
    recording: Option<PathBuf>,
}

impl Session {
    /// Builds and starts a pipeline for `camera`. With `record` set, the
    /// pipeline also encodes to that file.
    pub fn start(
        camera: &Camera,
        mirror: bool,
        on_frame: FrameSink,
        record: Option<&RecordSpec>,
    ) -> Result<Session> {
        let pipeline = gst::Pipeline::new();
        let src = source_element(&camera.spec)?;
        let decode = make("decodebin")?;
        let convert = make("videoconvert")?;
        let tee = make("tee")?;
        tee.set_property("allow-not-linked", true);

        // Viewfinder branch.
        let pq = make("queue")?;
        pq.set_property_from_str("leaky", "downstream");
        pq.set_property("max-size-buffers", 2u32);
        let flip = make("videoflip")?;
        let pconv = make("videoconvert")?;
        let pcaps = make("capsfilter")?;
        pcaps.set_property("caps", gst::Caps::builder("video/x-raw").field("format", "RGBA").build());
        let appsink = gst_app::AppSink::builder()
            .max_buffers(1)
            .drop(true)
            .sync(false)
            .enable_last_sample(false)
            .build();

        // Still branch: keeps the newest raw frame.
        let sq = make("queue")?;
        sq.set_property_from_str("leaky", "downstream");
        sq.set_property("max-size-buffers", 1u32);
        let still = make("fakesink")?;
        still.set_property("sync", false);
        still.set_property("async", false);
        still.set_property("enable-last-sample", true);

        pipeline
            .add_many([&src, &decode, &convert, &tee, &pq, &flip, &pconv, &pcaps, &sq, &still])
            .map_err(|e| e.to_string())?;
        pipeline.add(&appsink).map_err(|e| e.to_string())?;
        src.link(&decode).map_err(|_| "Could not link the camera source".to_string())?;
        gst::Element::link_many([&convert, &tee]).map_err(|e| e.to_string())?;
        gst::Element::link_many([&tee, &pq, &flip, &pconv, &pcaps, appsink.upcast_ref()])
            .map_err(|e| e.to_string())?;
        gst::Element::link_many([&tee, &sq, &still]).map_err(|e| e.to_string())?;

        let convert_weak = convert.downgrade();
        decode.connect_pad_added(move |_, pad| {
            let Some(convert) = convert_weak.upgrade() else { return };
            let sink = convert.static_pad("sink").unwrap();
            if sink.is_linked() {
                return;
            }
            let is_video = pad
                .current_caps()
                .unwrap_or_else(|| pad.query_caps(None))
                .structure(0)
                .is_some_and(|s| s.name().starts_with("video/"));
            if is_video {
                let _ = pad.link(&sink);
            }
        });

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(frame) = frame_from_sample(&sample) {
                        on_frame(frame);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        if let Some(rec) = record {
            add_recording_branch(&pipeline, &tee, rec)?;
        }

        let session = Session { pipeline, still, flip, recording: record.map(|r| r.path.clone()) };
        session.set_mirror(mirror);
        if session.pipeline.set_state(gst::State::Playing).is_err() {
            let err = session.pop_error().unwrap_or_else(|| "Could not start the camera".into());
            session.stop();
            return Err(err);
        }
        Ok(session)
    }

    pub fn set_mirror(&self, mirror: bool) {
        self.flip
            .set_property_from_str("video-direction", if mirror { "horiz" } else { "identity" });
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Newest raw camera frame.
    pub fn last_sample(&self) -> Option<gst::Sample> {
        self.still.property::<Option<gst::Sample>>("last-sample")
    }

    /// Pops a pending error message, if any.
    pub fn pop_error(&self) -> Option<String> {
        let bus = self.pipeline.bus()?;
        while let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(e) = msg.view() {
                return Some(e.error().to_string());
            }
        }
        None
    }

    /// Sends EOS and waits (up to `timeout`) until every sink has finished,
    /// so muxers write their headers / indexes. Returns the recording path.
    pub fn finish(self, timeout: Duration) -> Result<Option<PathBuf>> {
        self.pipeline.send_event(gst::event::Eos::new());
        let bus = self.pipeline.bus().expect("pipeline bus");
        let result = match bus.timed_pop_filtered(
            gst::ClockTime::from_nseconds(timeout.as_nanos() as u64),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        ) {
            Some(msg) => match msg.view() {
                gst::MessageView::Error(e) => Err(e.error().to_string()),
                _ => Ok(()),
            },
            None => Err("Timed out finishing the recording".into()),
        };
        let path = self.recording.clone();
        self.stop();
        result.map(|_| path)
    }

    pub fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn add_recording_branch(pipeline: &gst::Pipeline, tee: &gst::Element, rec: &RecordSpec) -> Result<()> {
    let plan = &rec.plan;
    let vq = make("queue")?;
    let vconv = make("videoconvert")?;
    let venc = make(plan.video_encoder)?;
    configure_video_encoder(&venc, plan.video_encoder);
    let mux = make(plan.muxer)?;
    let sink = make("filesink")?;
    sink.set_property("location", rec.path.to_string_lossy().to_string());
    sink.set_property("async", false);

    pipeline.add_many([&vq, &vconv, &venc, &mux, &sink]).map_err(|e| e.to_string())?;
    gst::Element::link_many([tee, &vq, &vconv, &venc]).map_err(|e| e.to_string())?;
    if let Some(parser) = plan.parser {
        let p = make(parser)?;
        pipeline.add(&p).map_err(|e| e.to_string())?;
        gst::Element::link_many([&venc, &p, &mux]).map_err(|e| e.to_string())?;
    } else {
        venc.link(&mux).map_err(|e| e.to_string())?;
    }
    mux.link(&sink).map_err(|e| e.to_string())?;

    if let (Some(aenc_name), true) = (plan.audio_encoder, rec.audio != AudioInput::None) {
        let asrc = match rec.audio {
            AudioInput::Test => {
                let s = make("audiotestsrc")?;
                s.set_property("is-live", true);
                s.set_property_from_str("wave", "silence");
                s
            }
            _ => make("pulsesrc").or_else(|_| make("autoaudiosrc"))?,
        };
        let aconv = make("audioconvert")?;
        let ares = make("audioresample")?;
        let aenc = make(aenc_name)?;
        let aq = make("queue")?;
        pipeline.add_many([&asrc, &aconv, &ares, &aenc, &aq]).map_err(|e| e.to_string())?;
        gst::Element::link_many([&asrc, &aconv, &ares, &aenc, &aq, &mux]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn configure_video_encoder(e: &gst::Element, name: &str) {
    match name {
        "vp8enc" | "vp9enc" => {
            try_set_str(e, "deadline", "1");
            try_set_str(e, "cpu-used", "8");
            try_set_str(e, "end-usage", "cbr");
            try_set_str(e, "target-bitrate", "2500000");
            try_set_str(e, "keyframe-max-dist", "60");
            try_set_str(e, "threads", "4");
            try_set_str(e, "row-mt", "true");
        }
        "x264enc" => {
            try_set_str(e, "tune", "zerolatency");
            try_set_str(e, "speed-preset", "veryfast");
            try_set_str(e, "bitrate", "4000");
            try_set_str(e, "key-int-max", "60");
        }
        _ => {}
    }
}

/// RGBA frame from an RGBA sample.
pub fn frame_from_sample(sample: &gst::Sample) -> Option<Frame> {
    let caps = sample.caps()?;
    let info = gst_video::VideoInfo::from_caps(caps).ok()?;
    let buffer = sample.buffer_owned()?;
    let map = buffer.into_mapped_buffer_readable().ok()?;
    Some(Frame {
        width: info.width() as i32,
        height: info.height() as i32,
        stride: info.stride()[0] as usize,
        data: glib::Bytes::from_owned(map),
    })
}

/// Encodes a raw camera sample as JPEG and writes it to `path`.
pub fn save_jpeg(sample: &gst::Sample, path: &Path) -> Result<()> {
    let caps = gst::Caps::builder("image/jpeg").build();
    let jpeg = gst_video::convert_sample(sample, &caps, gst::ClockTime::from_seconds(10))
        .map_err(|e| format!("Could not encode the photo: {e}"))?;
    let buffer = jpeg.buffer().ok_or("Empty photo")?;
    let map = buffer.map_readable().map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, map.as_slice()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Small RGBA thumbnail (longest side `size`) of a raw sample.
pub fn thumbnail(sample: &gst::Sample, size: i32) -> Option<Frame> {
    let info = gst_video::VideoInfo::from_caps(sample.caps()?).ok()?;
    let (w, h) = (info.width() as i32, info.height() as i32);
    if w <= 0 || h <= 0 {
        return None;
    }
    let (tw, th) = if w >= h {
        (size, (size * h / w).max(1))
    } else {
        ((size * w / h).max(1), size)
    };
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", tw)
        .field("height", th)
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build();
    let out = gst_video::convert_sample(sample, &caps, gst::ClockTime::from_seconds(5)).ok()?;
    frame_from_sample(&out)
}

/// Plays the camera shutter sound in the background. `silent` routes it to
/// a fake sink (tests).
pub fn play_shutter_sound(silent: bool) {
    const SOUND: &str = "/usr/share/sounds/freedesktop/stereo/camera-shutter.oga";
    if !Path::new(SOUND).exists() {
        return;
    }
    std::thread::spawn(move || {
        let Ok(playbin) = gst::ElementFactory::make("playbin").build() else { return };
        playbin.set_property("uri", format!("file://{SOUND}"));
        if silent {
            if let Ok(sink) = gst::ElementFactory::make("fakesink").build() {
                playbin.set_property("audio-sink", &sink);
            }
        }
        if playbin.set_state(gst::State::Playing).is_ok() {
            if let Some(bus) = playbin.bus() {
                let _ = bus.timed_pop_filtered(
                    gst::ClockTime::from_seconds(3),
                    &[gst::MessageType::Eos, gst::MessageType::Error],
                );
            }
        }
        let _ = playbin.set_state(gst::State::Null);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoders::{self, VideoFormat};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_camera() -> Camera {
        Camera {
            name: "Test".into(),
            location: Location::Front,
            spec: SourceSpec::Test { pattern: "smpte", width: 320, height: 240 },
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("helix-camera-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn wait_for(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn preview_and_photo_end_to_end() {
        gst::init().unwrap();
        let frames = Arc::new(AtomicUsize::new(0));
        let f2 = frames.clone();
        let sink: FrameSink = Arc::new(move |frame: Frame| {
            assert_eq!((frame.width, frame.height), (320, 240));
            assert!(frame.data.len() >= frame.stride * 240);
            f2.fetch_add(1, Ordering::SeqCst);
        });
        let session = Session::start(&test_camera(), true, sink, None).unwrap();
        assert!(wait_for(|| frames.load(Ordering::SeqCst) >= 3), "no viewfinder frames");
        assert!(wait_for(|| session.last_sample().is_some()), "no still sample");
        let sample = session.last_sample().unwrap();

        let dir = temp_dir("photo");
        let path = dir.join("Photo.jpg");
        save_jpeg(&sample, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "not a JPEG");

        let thumb = thumbnail(&sample, 64).unwrap();
        assert_eq!((thumb.width, thumb.height), (64, 48));
        assert!(session.pop_error().is_none());
        drop(session);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn short_recording_end_to_end() {
        gst::init().unwrap();
        let Some(plan) = encoders::choose(VideoFormat::WebmVp8, &encoders::gst_has) else {
            eprintln!("no video encoder available; skipping");
            return;
        };
        let dir = temp_dir("video");
        let path = dir.join(format!("Video.{}", plan.format.extension()));
        let spec = RecordSpec { path: path.clone(), plan: plan.clone(), audio: AudioInput::Test };
        let sink: FrameSink = Arc::new(|_| {});
        let session = Session::start(&test_camera(), false, sink, Some(&spec)).unwrap();
        assert!(session.is_recording());
        std::thread::sleep(Duration::from_millis(1200));
        let saved = session.finish(Duration::from_secs(10)).unwrap();
        assert_eq!(saved.as_deref(), Some(path.as_path()));
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 1000, "recording too small: {}", bytes.len());
        if plan.format.extension() == "webm" {
            assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3], "not EBML/WebM");
        }

        // The file must be readable back with a video and an audio stream.
        let kinds = decoded_stream_kinds(&path);
        assert!(kinds.iter().any(|k| k.starts_with("video/")), "{kinds:?}");
        assert!(kinds.iter().any(|k| k.starts_with("audio/")), "{kinds:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Decodes `path` to the end and returns the media types of its streams.
    fn decoded_stream_kinds(path: &Path) -> Vec<String> {
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("filesrc").property("location", path.to_str().unwrap()).build().unwrap();
        let decode = gst::ElementFactory::make("decodebin").build().unwrap();
        pipeline.add_many([&src, &decode]).unwrap();
        src.link(&decode).unwrap();
        let kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
        let k2 = kinds.clone();
        let pw = pipeline.downgrade();
        decode.connect_pad_added(move |_, pad| {
            let Some(pipeline) = pw.upgrade() else { return };
            let caps = pad.current_caps().unwrap();
            k2.lock().unwrap().push(caps.structure(0).unwrap().name().to_string());
            let sink = gst::ElementFactory::make("fakesink").property("sync", false).build().unwrap();
            pipeline.add(&sink).unwrap();
            sink.sync_state_with_parent().unwrap();
            pad.link(&sink.static_pad("sink").unwrap()).unwrap();
        });
        pipeline.set_state(gst::State::Playing).unwrap();
        let msg = pipeline
            .bus()
            .unwrap()
            .timed_pop_filtered(gst::ClockTime::from_seconds(10), &[gst::MessageType::Eos, gst::MessageType::Error])
            .expect("playback finished");
        assert!(matches!(msg.view(), gst::MessageView::Eos(_)), "playback error: {msg:?}");
        pipeline.set_state(gst::State::Null).unwrap();
        let kinds = kinds.lock().unwrap().clone();
        kinds
    }

    #[test]
    fn mp4_recording_when_available() {
        gst::init().unwrap();
        let Some(plan) = encoders::plan(VideoFormat::Mp4H264, &encoders::gst_has) else {
            return;
        };
        let dir = temp_dir("mp4");
        let path = dir.join("Video.mp4");
        let spec = RecordSpec { path: path.clone(), plan, audio: AudioInput::Test };
        let session = Session::start(&test_camera(), false, Arc::new(|_| {}), Some(&spec)).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        session.finish(Duration::from_secs(10)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        let kinds = decoded_stream_kinds(&path);
        assert!(kinds.iter().any(|k| k.starts_with("video/")), "{kinds:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
