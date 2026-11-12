//! Whole-file waveform extraction with a dedicated GStreamer pipeline.
//!
//! `uridecodebin → audioconvert → appsink (F32LE mono)`; runs on a worker
//! thread and reduces samples to peaks with [`PeakAccumulator`].

use gst::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::peaks::PeakAccumulator;

/// How often partial results are reported.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum DecodeError {
    Cancelled,
    Gst(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Cancelled => write!(f, "cancelled"),
            DecodeError::Gst(e) => write!(f, "{e}"),
        }
    }
}

fn gst_err(e: impl std::fmt::Display) -> DecodeError {
    DecodeError::Gst(e.to_string())
}

/// Decodes `uri` and returns its peaks (see [`crate::peaks::PEAKS_PER_SECOND`]).
/// `progress` receives the partial peak list periodically; `cancel` aborts.
pub fn decode(
    uri: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&[f32]),
) -> Result<Vec<f32>, DecodeError> {
    gst::init().map_err(gst_err)?;
    let pipeline = gst::Pipeline::new();
    let src = gst::ElementFactory::make("uridecodebin")
        .property("uri", uri)
        .build()
        .map_err(gst_err)?;
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(gst_err)?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("layout", "interleaved")
        .field("channels", 1i32)
        .build();
    let sink = gst_app::AppSink::builder()
        .caps(&caps)
        .sync(false)
        .max_buffers(64)
        .build();
    pipeline
        .add_many([&src, &convert, sink.upcast_ref()])
        .map_err(gst_err)?;
    convert.link(&sink).map_err(gst_err)?;

    let convert_weak = convert.downgrade();
    src.connect_pad_added(move |_, pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let is_audio = pad
            .current_caps()
            .or_else(|| Some(pad.query_caps(None)))
            .and_then(|c| c.structure(0).map(|s| s.name().starts_with("audio/")))
            .unwrap_or(false);
        let Some(sinkpad) = convert.static_pad("sink") else {
            return;
        };
        if is_audio && !sinkpad.is_linked() {
            let _ = pad.link(&sinkpad);
        }
    });

    let result = run(&pipeline, &sink, cancel, &mut progress);
    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn run(
    pipeline: &gst::Pipeline,
    sink: &gst_app::AppSink,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&[f32]),
) -> Result<Vec<f32>, DecodeError> {
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| bus_error(pipeline).unwrap_or_else(|| gst_err("Could not start decoder")))?;
    let mut acc: Option<PeakAccumulator> = None;
    let mut last_report = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DecodeError::Cancelled);
        }
        if let Some(err) = bus_error(pipeline) {
            return Err(err);
        }
        match sink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
            Some(sample) => {
                let acc = acc.get_or_insert_with(|| {
                    let rate = sample
                        .caps()
                        .and_then(|c| c.structure(0).and_then(|s| s.get::<i32>("rate").ok()))
                        .unwrap_or(44100);
                    PeakAccumulator::new(rate.max(1) as u32)
                });
                if let Some(buffer) = sample.buffer() {
                    if let Ok(map) = buffer.map_readable() {
                        let samples: Vec<f32> = map
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|b| f32::from_le_bytes(*b))
                            .collect();
                        acc.push(&samples);
                    }
                }
                if last_report.elapsed() >= PROGRESS_INTERVAL {
                    progress(&acc.peaks);
                    last_report = Instant::now();
                }
            }
            None => {
                if sink.is_eos() {
                    if let Some(err) = bus_error(pipeline) {
                        return Err(err);
                    }
                    return match acc {
                        Some(acc) => Ok(acc.finish()),
                        None => Err(gst_err("No audio stream")),
                    };
                }
            }
        }
    }
}

fn bus_error(pipeline: &gst::Pipeline) -> Option<DecodeError> {
    let bus = pipeline.bus()?;
    let msg = bus.pop_filtered(&[gst::MessageType::Error])?;
    match msg.view() {
        gst::MessageView::Error(e) => Some(gst_err(e.error())),
        _ => None,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;

    /// Writes a 16-bit mono PCM WAV: `loud_secs` of a ±half-scale square
    /// wave followed by `quiet_secs` of silence.
    pub fn write_wav(path: &std::path::Path, rate: u32, loud_secs: f32, quiet_secs: f32) {
        let loud = (rate as f32 * loud_secs) as usize;
        let quiet = (rate as f32 * quiet_secs) as usize;
        let mut data = Vec::with_capacity((loud + quiet) * 2);
        for i in 0..loud {
            let v: i16 = if (i / 20) % 2 == 0 { 16384 } else { -16384 };
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.extend(std::iter::repeat_n(0u8, quiet * 2));
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + data.len() as u32).to_le_bytes())
            .unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
        f.write_all(&1u16.to_le_bytes()).unwrap(); // mono
        f.write_all(&rate.to_le_bytes()).unwrap();
        f.write_all(&(rate * 2).to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        f.write_all(&data).unwrap();
    }

    pub fn uri(path: &std::path::Path) -> String {
        gst::glib::filename_to_uri(path, None).unwrap().to_string()
    }

    #[test]
    fn decodes_generated_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone file.wav");
        write_wav(&path, 8000, 0.5, 0.5);
        let cancel = AtomicBool::new(false);
        let peaks = decode(&uri(&path), &cancel, |_| {}).expect("decode");
        assert!(
            (99..=101).contains(&peaks.len()),
            "got {} peaks",
            peaks.len()
        );
        for p in &peaks[2..48] {
            assert!((p - 0.5).abs() < 0.02, "loud peak {p}");
        }
        for p in &peaks[52..98] {
            assert!(*p < 0.01, "quiet peak {p}");
        }
    }

    #[test]
    fn rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("junk.wav");
        std::fs::write(&path, b"this is definitely not audio data at all").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            decode(&uri(&path), &cancel, |_| {}),
            Err(DecodeError::Gst(_))
        ));
    }

    #[test]
    fn honours_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path, 8000, 1.0, 0.0);
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            decode(&uri(&path), &cancel, |_| {}),
            Err(DecodeError::Cancelled)
        ));
    }
}
