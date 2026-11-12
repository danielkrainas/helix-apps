//! Time formatting for the elapsed / remaining labels and resume toast.

/// Formats whole seconds as `m:ss`, or `h:mm:ss` when `long` is set (used
/// for every label once the duration reaches an hour so widths stay stable)
/// or when the value itself is at least an hour.
pub fn format_secs(secs: f64, long: bool) -> String {
    let total = if secs.is_finite() && secs > 0.0 { secs.floor() as u64 } else { 0 };
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 || long {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Elapsed label for `pos` within `duration` (seconds).
pub fn elapsed(pos: f64, duration: Option<f64>) -> String {
    format_secs(pos, duration.is_some_and(|d| d >= 3600.0))
}

/// Remaining label, `-m:ss`; `--:--` when the duration is unknown.
pub fn remaining(pos: f64, duration: Option<f64>) -> String {
    match duration {
        Some(d) if d.is_finite() && d > 0.0 => {
            // Round the remainder up so elapsed + remaining add up.
            let rem = (d - pos.max(0.0)).max(0.0).ceil();
            format!("-{}", format_secs(rem, d >= 3600.0))
        }
        _ => "--:--".into(),
    }
}

/// File-name friendly timestamp, e.g. `0-01-05` for 65 s.
pub fn file_stamp(secs: f64) -> String {
    format_secs(secs, true).replace(':', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_long() {
        assert_eq!(format_secs(0.0, false), "0:00");
        assert_eq!(format_secs(5.9, false), "0:05");
        assert_eq!(format_secs(65.0, false), "1:05");
        assert_eq!(format_secs(3599.0, false), "59:59");
        assert_eq!(format_secs(3600.0, false), "1:00:00");
        assert_eq!(format_secs(3725.0, false), "1:02:05");
        assert_eq!(format_secs(65.0, true), "0:01:05");
        assert_eq!(format_secs(-3.0, false), "0:00");
        assert_eq!(format_secs(f64::NAN, false), "0:00");
    }

    #[test]
    fn labels() {
        assert_eq!(elapsed(61.2, Some(120.0)), "1:01");
        assert_eq!(elapsed(61.2, Some(4000.0)), "0:01:01");
        assert_eq!(remaining(61.2, Some(120.0)), "-0:59");
        assert_eq!(remaining(0.0, Some(4000.0)), "-1:06:40");
        assert_eq!(remaining(130.0, Some(120.0)), "-0:00");
        assert_eq!(remaining(1.0, None), "--:--");
        assert_eq!(file_stamp(65.0), "0-01-05");
    }
}
