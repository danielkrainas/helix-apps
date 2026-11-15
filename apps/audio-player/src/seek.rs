//! Seek, rate and volume math.

use helix_app_common::trf;

pub const SECOND: u64 = 1_000_000_000;
/// Step of the skip buttons / arrow keys.
pub const SKIP_NS: i64 = 10 * SECOND as i64;
pub const MIN_RATE: f64 = 0.5;
pub const MAX_RATE: f64 = 2.0;
pub const RATES: [f64; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

/// Position after moving `delta_ns` from `position`, clamped to the file.
pub fn offset(position: u64, delta_ns: i64, duration: Option<u64>) -> u64 {
    let target = (position as i128 + delta_ns as i128).max(0) as u64;
    match duration {
        Some(d) => target.min(d),
        None => target,
    }
}

/// Small step of the focused waveform's arrow keys.
pub const KEY_STEP_NS: i64 = 5 * SECOND as i64;
/// Large step of the focused waveform's Page Up/Page Down keys.
pub const KEY_PAGE_NS: i64 = 60 * SECOND as i64;

/// A seek requested with the keyboard on the waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySeek {
    By(i64),
    Start,
    End,
}

/// Where a keyboard seek on the waveform goes.
pub fn key_target(seek: KeySeek, position: u64, duration: Option<u64>) -> u64 {
    match seek {
        KeySeek::By(d) => offset(position, d, duration),
        KeySeek::Start => 0,
        KeySeek::End => duration.unwrap_or(position),
    }
}

/// Alpha applied to the foreground colour for the not-yet-played bars:
/// enough for 3:1 against the window background in the light and dark
/// styles (see the test), more in high contrast.
pub fn unplayed_alpha(high_contrast: bool) -> f32 {
    if high_contrast {
        0.85
    } else {
        0.6
    }
}

/// Position under `x` on a widget `width` pixels wide.
pub fn position_at(x: f64, width: f64, duration: u64) -> u64 {
    if width <= 0.0 || !x.is_finite() {
        return 0;
    }
    let frac = (x / width).clamp(0.0, 1.0);
    (frac * duration as f64).round() as u64
}

/// Fraction of the file played, for drawing the playhead.
pub fn fraction(position: u64, duration: Option<u64>) -> f64 {
    match duration {
        Some(d) if d > 0 => (position as f64 / d as f64).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

pub fn clamp_rate(rate: f64) -> f64 {
    if rate.is_finite() {
        rate.clamp(MIN_RATE, MAX_RATE)
    } else {
        1.0
    }
}

/// Label for a rate menu entry / button, e.g. `1.25×`.
pub fn rate_label(rate: f64) -> String {
    let s = format!("{rate:.2}");
    let s = s.trim_end_matches('0');
    let s = if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    };
    format!("{s}\u{d7}")
}

/// Slider (perceptual, 0–1) to linear amplitude.
pub fn cubic_to_linear(v: f64) -> f64 {
    v.clamp(0.0, 1.0).powi(3)
}

pub fn linear_to_cubic(v: f64) -> f64 {
    v.clamp(0.0, 1.0).cbrt()
}

/// Accessible name of the speed button: "Playback Speed: 1.25×".
pub fn rate_name(rate: f64) -> String {
    trf!("Playback Speed: {}", rate_label(rate))
}

/// Accessible value of the volume slider (its position, 0 – 100 %).
pub fn volume_text(slider: f64, muted: bool) -> String {
    let pct = (slider.clamp(0.0, 1.0) * 100.0).round() as u32;
    if muted {
        trf!("{pct}%, muted", pct = pct)
    } else {
        // Translators: the volume, as a percentage.
        trf!("{pct}%", pct = pct)
    }
}

pub fn volume_icon(slider: f64, muted: bool) -> &'static str {
    if muted || slider <= 0.0 {
        "audio-volume-muted-symbolic"
    } else if slider < 0.34 {
        "audio-volume-low-symbolic"
    } else if slider < 0.67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_seeks() {
        let d = Some(100 * SECOND);
        assert_eq!(key_target(KeySeek::By(KEY_STEP_NS), 0, d), 5 * SECOND);
        assert_eq!(key_target(KeySeek::By(-KEY_STEP_NS), 3 * SECOND, d), 0);
        assert_eq!(
            key_target(KeySeek::By(KEY_PAGE_NS), 50 * SECOND, d),
            100 * SECOND
        );
        assert_eq!(key_target(KeySeek::Start, 50 * SECOND, d), 0);
        assert_eq!(key_target(KeySeek::End, 50 * SECOND, d), 100 * SECOND);
        assert_eq!(key_target(KeySeek::End, 7, None), 7);
    }

    /// WCAG contrast of the unplayed bars (foreground at `unplayed_alpha`)
    /// against Adwaita's window background, light and dark.
    #[test]
    fn unplayed_bars_contrast() {
        fn lin(v: f64) -> f64 {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        fn lum(c: (f64, f64, f64)) -> f64 {
            0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
        }
        fn over(fg: (f64, f64, f64), a: f64, bg: (f64, f64, f64)) -> (f64, f64, f64) {
            (
                fg.0 * a + bg.0 * (1.0 - a),
                fg.1 * a + bg.1 * (1.0 - a),
                fg.2 * a + bg.2 * (1.0 - a),
            )
        }
        fn ratio(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        }
        // Adwaita: light fg rgb(0 0 6 / 80%) on #fafafb; dark fg #ffffff on
        // #222226.
        let light_bg = (0.98, 0.98, 0.984);
        let light_fg = over(
            (0.0, 0.0, 0.024),
            0.8 * unplayed_alpha(false) as f64,
            light_bg,
        );
        assert!(
            ratio(light_fg, light_bg) >= 3.0,
            "{}",
            ratio(light_fg, light_bg)
        );
        let dark_bg = (0.133, 0.133, 0.149);
        let dark_fg = over((1.0, 1.0, 1.0), unplayed_alpha(false) as f64, dark_bg);
        assert!(
            ratio(dark_fg, dark_bg) >= 3.0,
            "{}",
            ratio(dark_fg, dark_bg)
        );
        assert!(unplayed_alpha(true) > unplayed_alpha(false));
    }

    #[test]
    fn readable_rate_and_volume() {
        assert_eq!(rate_name(1.25), "Playback Speed: 1.25\u{d7}");
        assert_eq!(rate_name(1.0), "Playback Speed: 1.0\u{d7}");
        assert_eq!(volume_text(0.5, false), "50%");
        assert_eq!(volume_text(1.0, true), "100%, muted");
    }

    #[test]
    fn skip_clamps() {
        let d = Some(30 * SECOND);
        assert_eq!(offset(5 * SECOND, -SKIP_NS, d), 0);
        assert_eq!(offset(15 * SECOND, -SKIP_NS, d), 5 * SECOND);
        assert_eq!(offset(15 * SECOND, SKIP_NS, d), 25 * SECOND);
        assert_eq!(offset(25 * SECOND, SKIP_NS, d), 30 * SECOND);
        assert_eq!(offset(25 * SECOND, SKIP_NS, None), 35 * SECOND);
    }

    #[test]
    fn click_position() {
        assert_eq!(position_at(50.0, 200.0, 100), 25);
        assert_eq!(position_at(-10.0, 200.0, 100), 0);
        assert_eq!(position_at(500.0, 200.0, 100), 100);
        assert_eq!(position_at(10.0, 0.0, 100), 0);
        assert_eq!(position_at(f64::NAN, 100.0, 100), 0);
        assert_eq!(position_at(100.0, 400.0, 60 * SECOND), 15 * SECOND);
    }

    #[test]
    fn fractions() {
        assert_eq!(fraction(5, Some(10)), 0.5);
        assert_eq!(fraction(20, Some(10)), 1.0);
        assert_eq!(fraction(5, None), 0.0);
        assert_eq!(fraction(5, Some(0)), 0.0);
    }

    #[test]
    fn rates() {
        assert_eq!(clamp_rate(3.0), 2.0);
        assert_eq!(clamp_rate(0.1), 0.5);
        assert_eq!(clamp_rate(f64::NAN), 1.0);
        assert_eq!(rate_label(1.0), "1.0\u{d7}");
        assert_eq!(rate_label(0.75), "0.75\u{d7}");
        assert_eq!(rate_label(1.5), "1.5\u{d7}");
        assert_eq!(rate_label(2.0), "2.0\u{d7}");
    }

    #[test]
    fn volume_curve() {
        assert_eq!(cubic_to_linear(0.5), 0.125);
        assert!((linear_to_cubic(0.125) - 0.5).abs() < 1e-12);
        assert_eq!(cubic_to_linear(2.0), 1.0);
        assert_eq!(volume_icon(0.5, true), "audio-volume-muted-symbolic");
        assert_eq!(volume_icon(0.0, false), "audio-volume-muted-symbolic");
        assert_eq!(volume_icon(0.2, false), "audio-volume-low-symbolic");
        assert_eq!(volume_icon(0.9, false), "audio-volume-high-symbolic");
    }
}
