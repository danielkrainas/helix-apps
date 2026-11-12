//! Video format / encoder selection based on what the GStreamer registry has.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoFormat {
    WebmVp8,
    WebmVp9,
    Mp4H264,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 3] = [VideoFormat::WebmVp8, VideoFormat::WebmVp9, VideoFormat::Mp4H264];

    pub fn id(self) -> &'static str {
        match self {
            VideoFormat::WebmVp8 => "webm-vp8",
            VideoFormat::WebmVp9 => "webm-vp9",
            VideoFormat::Mp4H264 => "mp4-h264",
        }
    }

    pub fn from_id(id: &str) -> Option<VideoFormat> {
        VideoFormat::ALL.into_iter().find(|f| f.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::WebmVp8 => "WebM (VP8)",
            VideoFormat::WebmVp9 => "WebM (VP9)",
            VideoFormat::Mp4H264 => "MP4 (H.264)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            VideoFormat::WebmVp8 | VideoFormat::WebmVp9 => "webm",
            VideoFormat::Mp4H264 => "mp4",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            VideoFormat::WebmVp8 | VideoFormat::WebmVp9 => "webmmux",
            VideoFormat::Mp4H264 => "mp4mux",
        }
    }

    fn video_candidates(self) -> &'static [&'static str] {
        match self {
            VideoFormat::WebmVp8 => &["vp8enc"],
            VideoFormat::WebmVp9 => &["vp9enc"],
            VideoFormat::Mp4H264 => &["x264enc", "openh264enc"],
        }
    }

    fn audio_candidates(self) -> &'static [&'static str] {
        match self {
            VideoFormat::WebmVp8 | VideoFormat::WebmVp9 => &["opusenc"],
            VideoFormat::Mp4H264 => &["avenc_aac", "voaacenc", "fdkaacenc"],
        }
    }

    fn extra_required(self) -> &'static [&'static str] {
        match self {
            VideoFormat::Mp4H264 => &["h264parse"],
            _ => &[],
        }
    }
}

/// Concrete elements to build a recording branch from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderPlan {
    pub format: VideoFormat,
    pub video_encoder: &'static str,
    /// `None` → record without sound.
    pub audio_encoder: Option<&'static str>,
    pub muxer: &'static str,
    /// Parser inserted between the video encoder and the muxer.
    pub parser: Option<&'static str>,
}

/// Plan for `format`, or `None` when it cannot be encoded.
pub fn plan(format: VideoFormat, has: &impl Fn(&str) -> bool) -> Option<EncoderPlan> {
    if !has(format.muxer()) || !format.extra_required().iter().all(|e| has(e)) {
        return None;
    }
    let video_encoder = *format.video_candidates().iter().find(|e| has(e))?;
    let audio_encoder = format.audio_candidates().iter().copied().find(|e| has(e));
    Some(EncoderPlan {
        format,
        video_encoder,
        audio_encoder,
        muxer: format.muxer(),
        parser: format.extra_required().first().copied(),
    })
}

/// Every format with whether it is usable.
pub fn availability(has: &impl Fn(&str) -> bool) -> Vec<(VideoFormat, bool)> {
    VideoFormat::ALL.into_iter().map(|f| (f, plan(f, has).is_some())).collect()
}

/// The preferred format if available, otherwise the first one that is.
pub fn choose(preferred: VideoFormat, has: &impl Fn(&str) -> bool) -> Option<EncoderPlan> {
    plan(preferred, has).or_else(|| VideoFormat::ALL.into_iter().find_map(|f| plan(f, has)))
}

/// Registry lookup (requires `gst::init`).
pub fn gst_has(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// Whether a JPEG encoder exists (photo mode needs it).
pub fn can_take_photos(has: &impl Fn(&str) -> bool) -> bool {
    ["appsrc", "jpegenc", "videoconvert"].iter().all(|e| has(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |n| names.contains(&n)
    }

    #[test]
    fn full_registry_prefers_requested() {
        let has = reg(&[
            "vp8enc", "vp9enc", "opusenc", "webmmux", "x264enc", "avenc_aac", "mp4mux", "h264parse",
        ]);
        let p = choose(VideoFormat::Mp4H264, &has).unwrap();
        assert_eq!(p.video_encoder, "x264enc");
        assert_eq!(p.audio_encoder, Some("avenc_aac"));
        assert_eq!(p.muxer, "mp4mux");
        assert_eq!(p.parser, Some("h264parse"));
        let p = choose(VideoFormat::WebmVp9, &has).unwrap();
        assert_eq!((p.video_encoder, p.audio_encoder, p.parser), ("vp9enc", Some("opusenc"), None));
    }

    #[test]
    fn falls_back_to_available_format() {
        let has = reg(&["openh264enc", "voaacenc", "mp4mux", "h264parse"]);
        let p = choose(VideoFormat::WebmVp8, &has).unwrap();
        assert_eq!(p.format, VideoFormat::Mp4H264);
        assert_eq!(p.video_encoder, "openh264enc");
        assert_eq!(p.audio_encoder, Some("voaacenc"));
        assert_eq!(
            availability(&has),
            vec![(VideoFormat::WebmVp8, false), (VideoFormat::WebmVp9, false), (VideoFormat::Mp4H264, true)]
        );
    }

    #[test]
    fn missing_audio_encoder_records_silently() {
        let has = reg(&["vp8enc", "webmmux"]);
        let p = choose(VideoFormat::WebmVp8, &has).unwrap();
        assert_eq!(p.audio_encoder, None);
    }

    #[test]
    fn missing_muxer_or_parser_disables_format() {
        assert!(plan(VideoFormat::WebmVp8, &reg(&["vp8enc", "opusenc"])).is_none());
        assert!(plan(VideoFormat::Mp4H264, &reg(&["x264enc", "mp4mux"])).is_none());
        assert!(choose(VideoFormat::WebmVp8, &reg(&[])).is_none());
    }

    #[test]
    fn ids_round_trip() {
        for f in VideoFormat::ALL {
            assert_eq!(VideoFormat::from_id(f.id()), Some(f));
        }
        assert_eq!(VideoFormat::from_id("avi"), None);
    }

    #[test]
    fn photo_requirements() {
        assert!(can_take_photos(&reg(&["appsrc", "jpegenc", "videoconvert"])));
        assert!(!can_take_photos(&reg(&["appsrc", "videoconvert"])));
    }
}
