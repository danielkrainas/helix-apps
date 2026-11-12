//! Seek, rate and volume math.

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
