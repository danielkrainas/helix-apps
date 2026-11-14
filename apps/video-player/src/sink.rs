//! `appsink` based video sink delivering raw frames to the main thread.
//!
//! There is no `gtk4paintablesink` available, so playbin renders into an
//! `appsink` that only accepts packed 32-bit RGB formats (playsink's own
//! conversion bin inserts `videoconvert`/`videoscale`). Every sample
//! (including the preroll sample shown while paused or after a seek) is put
//! into a single-slot mailbox; the main thread is woken through `notify` and
//! always takes the newest one, so frames that arrive while it is busy are
//! dropped instead of queued.

use std::sync::{Arc, Mutex};

/// Formats accepted by the sink, in order of preference.
pub const FORMATS: &[gst_video::VideoFormat] = &[
    gst_video::VideoFormat::Bgrx,
    gst_video::VideoFormat::Bgra,
    gst_video::VideoFormat::Rgba,
    gst_video::VideoFormat::Rgbx,
];

/// Latest-sample mailbox shared between the streaming thread and the UI.
#[derive(Default)]
pub struct FrameSlot {
    latest: Mutex<Option<gst::Sample>>,
}

impl FrameSlot {
    pub fn put(&self, sample: gst::Sample) -> bool {
        let mut slot = self.latest.lock().unwrap();
        let was_empty = slot.is_none();
        *slot = Some(sample);
        was_empty
    }

    pub fn take(&self) -> Option<gst::Sample> {
        self.latest.lock().unwrap().take()
    }
}

/// Builds the appsink. `notify` runs on a streaming thread after a new
/// sample was stored.
pub fn make_video_sink(
    slot: Arc<FrameSlot>,
    notify: impl Fn() + Send + Sync + 'static,
) -> gst_app::AppSink {
    let caps = gst_video::VideoCapsBuilder::new()
        .format_list(FORMATS.iter().copied())
        .build();
    let notify = Arc::new(notify);
    let (slot2, notify2) = (slot.clone(), notify.clone());
    let sink = gst_app::AppSink::builder()
        .name("helix-video-sink")
        .caps(&caps)
        .sync(true)
        .qos(true)
        .max_buffers(1)
        .drop(true)
        .enable_last_sample(false)
        .build();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                slot.put(sample);
                notify();
                Ok(gst::FlowSuccess::Ok)
            })
            .new_preroll(move |sink| {
                let sample = sink.pull_preroll().map_err(|_| gst::FlowError::Eos)?;
                slot2.put(sample);
                notify2();
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    sink
}

/// A decoded frame ready to be turned into a texture.
#[derive(Debug, Clone)]
pub struct Frame {
    pub format: gst_video::VideoFormat,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub offset: usize,
    pub par: (i32, i32),
    pub fps: (i32, i32),
    pub pts: Option<gst::ClockTime>,
    pub buffer: gst::Buffer,
}

impl Frame {
    pub fn from_sample(sample: &gst::Sample) -> Option<Frame> {
        let caps = sample.caps()?;
        let info = gst_video::VideoInfo::from_caps(caps).ok()?;
        let buffer = sample.buffer_owned()?;
        let (mut stride, mut offset) = (info.stride()[0] as usize, info.offset()[0]);
        if let Some(meta) = buffer.meta::<gst_video::VideoMeta>() {
            stride = meta.stride()[0] as usize;
            offset = meta.offset()[0];
        }
        let (width, height) = (info.width(), info.height());
        if width == 0
            || height == 0
            || buffer.size() < offset + stride * (height as usize - 1) + width as usize * 4
        {
            return None;
        }
        Some(Frame {
            format: info.format(),
            width,
            height,
            stride,
            offset,
            par: (info.par().numer(), info.par().denom()),
            fps: (info.fps().numer(), info.fps().denom()),
            pts: buffer.pts(),
            buffer,
        })
    }

    /// Duration of one frame in seconds (1/25 s when unknown).
    pub fn frame_duration(&self) -> f64 {
        let (n, d) = self.fps;
        if n > 0 && d > 0 {
            d as f64 / n as f64
        } else {
            1.0 / 25.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gst::prelude::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Encodes a tiny VP8/WebM clip with `videotestsrc`.
    fn make_clip(path: &std::path::Path, w: u32, h: u32, par: &str) {
        let desc = format!(
            "videotestsrc num-buffers=15 ! video/x-raw,width={w},height={h},framerate=15/1,pixel-aspect-ratio={par} \
             ! videoconvert ! vp8enc deadline=1 ! webmmux ! filesink location=\"{}\"",
            path.display()
        );
        let pipeline = gst::parse::launch(&desc).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        let msg = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(20),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("encoder timed out");
        assert_eq!(msg.type_(), gst::MessageType::Eos, "{msg:?}");
        pipeline.set_state(gst::State::Null).unwrap();
    }

    #[test]
    fn decode_tiny_video_through_appsink() {
        gst::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("tiny.webm");
        make_clip(&clip, 64, 48, "1/1");
        assert!(std::fs::metadata(&clip).unwrap().len() > 0);

        let slot = Arc::new(FrameSlot::default());
        let (tx, rx) = mpsc::channel::<()>();
        let tx = Mutex::new(tx);
        let sink = make_video_sink(slot.clone(), move || {
            let _ = tx.lock().unwrap().send(());
        });
        let playbin =
            crate::player::make_playbin(sink.upcast_ref(), Some("fakesink sync=false")).unwrap();
        let uri = gtk::glib::filename_to_uri(&clip, None).unwrap();
        playbin.set_property("uri", uri.as_str());
        playbin.set_state(gst::State::Playing).unwrap();

        let mut frames = 0;
        let mut last = None;
        while rx.recv_timeout(Duration::from_secs(10)).is_ok() {
            if let Some(sample) = slot.take() {
                let frame = Frame::from_sample(&sample).expect("valid frame");
                assert_eq!((frame.width, frame.height), (64, 48));
                assert!(FORMATS.contains(&frame.format));
                assert!(frame.stride >= 64 * 4);
                assert!((frame.frame_duration() - 1.0 / 15.0).abs() < 1e-9);
                last = Some(frame);
                frames += 1;
                if frames >= 3 {
                    break;
                }
            }
        }
        playbin.set_state(gst::State::Null).unwrap();
        assert!(frames >= 1, "no frame arrived");
        let frame = last.unwrap();
        assert_eq!(frame.par, (1, 1));
    }

    #[test]
    fn slot_keeps_latest_only() {
        gst::init().unwrap();
        let slot = FrameSlot::default();
        let s1 = gst::Sample::builder()
            .buffer(&gst::Buffer::with_size(1).unwrap())
            .build();
        let s2 = gst::Sample::builder()
            .buffer(&gst::Buffer::with_size(2).unwrap())
            .build();
        assert!(slot.put(s1));
        assert!(!slot.put(s2));
        assert_eq!(slot.take().unwrap().buffer().unwrap().size(), 2);
        assert!(slot.take().is_none());
    }
}
