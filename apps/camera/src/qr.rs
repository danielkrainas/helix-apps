//! QR-code detection on preview frames.
//!
//! The UI offers at most one frame every [`SCAN_INTERVAL`] to a scanner
//! thread (a bounded channel of one: frames are dropped while a scan runs),
//! which converts it to greyscale, downsamples large frames and decodes with
//! `rqrr`. Results come back as events; [`Tracker`] debounces them so the
//! banner does not flicker when a code is missed in a single frame.

use crate::pipeline::Frame;
use std::time::{Duration, Instant};

pub const SCAN_INTERVAL: Duration = Duration::from_millis(400);
/// Consecutive scans without the code before the banner is hidden.
pub const MISSES_TO_HIDE: u32 = 3;
/// Longest frame side handed to the decoder.
const MAX_SIDE: usize = 960;

/// Rate limiter for frame submissions.
#[derive(Debug, Clone, Copy, Default)]
pub struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    pub fn ready(&mut self, now: Instant, interval: Duration) -> bool {
        match self.last {
            Some(t) if now.saturating_duration_since(t) < interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// What the banner should do after a scan result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Show(String),
    Hide,
}

/// Debounces scan results.
#[derive(Debug, Default)]
pub struct Tracker {
    current: Option<String>,
    misses: u32,
    /// Text the user dismissed; not shown again until it leaves the view.
    dismissed: Option<String>,
}

impl Tracker {
    pub fn update(&mut self, result: Option<String>) -> Option<Change> {
        match result {
            Some(text) => {
                self.misses = 0;
                if self.dismissed.as_deref() == Some(text.as_str()) {
                    return None;
                }
                self.dismissed = None;
                if self.current.as_deref() == Some(text.as_str()) {
                    return None;
                }
                self.current = Some(text.clone());
                Some(Change::Show(text))
            }
            None => {
                self.misses += 1;
                if self.misses < MISSES_TO_HIDE {
                    return None;
                }
                self.dismissed = None;
                self.current.take().map(|_| Change::Hide)
            }
        }
    }

    pub fn dismiss(&mut self) {
        self.dismissed = self.current.take();
    }

    pub fn reset(&mut self) -> Option<Change> {
        self.misses = 0;
        self.dismissed = None;
        self.current.take().map(|_| Change::Hide)
    }
}

/// Converts RGBA pixels to luma, downsampling by an integer factor so the
/// longest side is at most `MAX_SIDE`. Returns `(width, height, pixels)`.
pub fn to_luma(width: usize, height: usize, stride: usize, rgba: &[u8]) -> (usize, usize, Vec<u8>) {
    let step = width.max(height).div_ceil(MAX_SIDE).max(1);
    let (w, h) = (width / step, height / step);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = y * step * stride;
        for x in 0..w {
            let i = row + x * step * 4;
            let p = rgba.get(i..i + 3).unwrap_or(&[0, 0, 0]);
            // ITU-R BT.601 luma, integer approximation.
            out.push(((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8);
        }
    }
    (w, h, out)
}

fn decode_luma(w: usize, h: usize, luma: &[u8], mirrored: bool) -> Option<String> {
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let x = if mirrored { w - 1 - x } else { x };
        luma[y * w + x]
    });
    img.detect_grids().into_iter().find_map(|g| g.decode().ok().map(|(_, text)| text))
}

/// Decodes the first QR code in an RGBA image. The preview of front cameras
/// is mirrored, so a mirrored read is tried when the direct one fails.
pub fn decode_rgba(width: usize, height: usize, stride: usize, rgba: &[u8]) -> Option<String> {
    if width == 0 || height == 0 || rgba.len() < stride * (height - 1) + width * 4 {
        return None;
    }
    let (w, h, luma) = to_luma(width, height, stride, rgba);
    if w < 21 || h < 21 {
        return None;
    }
    decode_luma(w, h, &luma, false)
        .or_else(|| decode_luma(w, h, &luma, true))
        .filter(|t| !t.trim().is_empty())
}

pub fn decode_frame(frame: &Frame) -> Option<String> {
    decode_rgba(frame.width.max(0) as usize, frame.height.max(0) as usize, frame.stride, &frame.data)
}

/// URI schemes the "Open" action is offered for. Anything else (file:,
/// javascript:, custom handlers…) can only be copied.
const OPEN_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "sms", "geo", "ftp"];

/// The URI to launch for a decoded text, if it is a web/mail/phone link.
/// A bare `www.` host gets `https://` prepended.
pub fn openable_uri(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) {
        return None;
    }
    if t.len() > 4 && t[..4].eq_ignore_ascii_case("www.") {
        return Some(format!("https://{t}"));
    }
    let (scheme, rest) = t.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    if !OPEN_SCHEMES.contains(&scheme.as_str()) || rest.is_empty() {
        return None;
    }
    if matches!(scheme.as_str(), "http" | "https" | "ftp") && !rest.starts_with("//") {
        return None;
    }
    Some(t.to_string())
}

/// Handle to the scanner thread.
pub struct Scanner {
    requests: async_channel::Sender<Frame>,
    pub results: async_channel::Receiver<Option<String>>,
}

impl Scanner {
    pub fn spawn() -> Scanner {
        let (req_tx, req_rx) = async_channel::bounded::<Frame>(1);
        let (res_tx, res_rx) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("qr-scanner".into())
            .spawn(move || {
                while let Ok(frame) = req_rx.recv_blocking() {
                    if res_tx.send_blocking(decode_frame(&frame)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn qr scanner thread");
        Scanner { requests: req_tx, results: res_rx }
    }

    /// Offers a frame; dropped if a scan is still running.
    pub fn offer(&self, frame: Frame) -> bool {
        self.requests.try_send(frame).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders `text` as a QR code into an RGBA buffer (`scale` px per
    /// module, quiet zone included), optionally mirrored.
    fn qr_rgba(text: &str, scale: usize, mirrored: bool) -> (usize, usize, Vec<u8>) {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let n = code.width();
        let colors = code.to_colors();
        let quiet = 4;
        let side = (n + 2 * quiet) * scale;
        let mut buf = vec![255u8; side * side * 4];
        for y in 0..side {
            for x in 0..side {
                let (mx, my) = ((x / scale) as isize - quiet as isize, (y / scale) as isize - quiet as isize);
                let dark = mx >= 0
                    && my >= 0
                    && (mx as usize) < n
                    && (my as usize) < n
                    && colors[my as usize * n + mx as usize] == qrcode::Color::Dark;
                let px = if mirrored { side - 1 - x } else { x };
                if dark {
                    let i = (y * side + px) * 4;
                    buf[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        (side, side, buf)
    }

    #[test]
    fn decodes_generated_code() {
        let (w, h, buf) = qr_rgba("https://example.org/helix?q=1", 6, false);
        assert_eq!(decode_rgba(w, h, w * 4, &buf).as_deref(), Some("https://example.org/helix?q=1"));
    }

    #[test]
    fn decodes_mirrored_code() {
        let (w, h, buf) = qr_rgba("hello helix", 5, true);
        assert_eq!(decode_rgba(w, h, w * 4, &buf).as_deref(), Some("hello helix"));
    }

    #[test]
    fn decodes_downsampled_large_frame() {
        let (w, h, buf) = qr_rgba("BIG", 60, false);
        assert!(w > MAX_SIDE);
        assert_eq!(decode_rgba(w, h, w * 4, &buf).as_deref(), Some("BIG"));
    }

    #[test]
    fn blank_and_bad_frames() {
        let buf = vec![128u8; 320 * 240 * 4];
        assert_eq!(decode_rgba(320, 240, 320 * 4, &buf), None);
        assert_eq!(decode_rgba(320, 240, 320 * 4, &buf[..100]), None);
        assert_eq!(decode_rgba(0, 0, 0, &[]), None);
    }

    #[test]
    fn luma_downsampling() {
        let (w, h, l) = to_luma(2000, 1000, 8000, &vec![255u8; 8000 * 1000]);
        assert_eq!((w, h), (666, 333));
        assert!(l.iter().all(|&v| v >= 254));
        let (w, h, _) = to_luma(640, 480, 2560, &vec![0u8; 2560 * 480]);
        assert_eq!((w, h), (640, 480));
    }

    #[test]
    fn uri_detection() {
        assert_eq!(openable_uri("https://gnome.org").as_deref(), Some("https://gnome.org"));
        assert_eq!(openable_uri("  HTTP://x.y/z ").as_deref(), Some("HTTP://x.y/z"));
        assert_eq!(openable_uri("www.example.com").as_deref(), Some("https://www.example.com"));
        assert_eq!(openable_uri("mailto:a@b.c").as_deref(), Some("mailto:a@b.c"));
        assert_eq!(openable_uri("tel:+123").as_deref(), Some("tel:+123"));
        assert_eq!(openable_uri("file:///etc/passwd"), None);
        assert_eq!(openable_uri("javascript:alert(1)"), None);
        assert_eq!(openable_uri("WIFI:S:net;T:WPA;P:pw;;"), None);
        assert_eq!(openable_uri("http:nope"), None);
        assert_eq!(openable_uri("just some text"), None);
        assert_eq!(openable_uri(""), None);
    }

    #[test]
    fn throttle() {
        let mut t = Throttle::default();
        let t0 = Instant::now();
        assert!(t.ready(t0, SCAN_INTERVAL));
        assert!(!t.ready(t0 + Duration::from_millis(100), SCAN_INTERVAL));
        assert!(t.ready(t0 + SCAN_INTERVAL, SCAN_INTERVAL));
    }

    #[test]
    fn tracker_debounces_and_dismisses() {
        let mut t = Tracker::default();
        assert_eq!(t.update(None), None);
        assert_eq!(t.update(Some("a".into())), Some(Change::Show("a".into())));
        assert_eq!(t.update(Some("a".into())), None);
        // Short misses keep the banner.
        assert_eq!(t.update(None), None);
        assert_eq!(t.update(None), None);
        assert_eq!(t.update(Some("a".into())), None);
        // A different code replaces it.
        assert_eq!(t.update(Some("b".into())), Some(Change::Show("b".into())));
        for _ in 0..MISSES_TO_HIDE - 1 {
            assert_eq!(t.update(None), None);
        }
        assert_eq!(t.update(None), Some(Change::Hide));
        assert_eq!(t.update(None), None);
        // Dismissed code stays hidden while it remains in view.
        assert_eq!(t.update(Some("c".into())), Some(Change::Show("c".into())));
        t.dismiss();
        assert_eq!(t.update(Some("c".into())), None);
        for _ in 0..MISSES_TO_HIDE {
            assert_eq!(t.update(None), None);
        }
        assert_eq!(t.update(Some("c".into())), Some(Change::Show("c".into())));
        assert_eq!(t.reset(), Some(Change::Hide));
        assert_eq!(t.reset(), None);
    }
}
