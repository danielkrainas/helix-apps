//! Time formatting for the elapsed / remaining labels.

/// Formats `ns` as `m:ss`, or `h:mm:ss` when `long` (duration ≥ 1 h).
pub fn format(ns: u64, long: bool) -> String {
    let secs = ns / 1_000_000_000;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if long || h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Whether a duration should use the hour format.
pub fn is_long(duration_ns: Option<u64>) -> bool {
    duration_ns.is_some_and(|d| d >= 3600 * 1_000_000_000)
}

/// Elapsed and remaining strings for a position within a duration.
pub fn elapsed_remaining(position_ns: u64, duration_ns: Option<u64>) -> (String, String) {
    let long = is_long(duration_ns);
    match duration_ns {
        Some(d) => {
            let pos = position_ns.min(d);
            // Whole-second difference, so elapsed + remaining always add up
            // to the duration's whole seconds.
            let rem = (d / 1_000_000_000).saturating_sub(pos / 1_000_000_000) * 1_000_000_000;
            (format(pos, long), format!("\u{2212}{}", format(rem, long)))
        }
        None => (
            format(position_ns, long),
            format!("\u{2212}{}", format(0, long)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const S: u64 = 1_000_000_000;

    #[test]
    fn short_format() {
        assert_eq!(format(0, false), "0:00");
        assert_eq!(format(9 * S + 999_999_999, false), "0:09");
        assert_eq!(format(61 * S, false), "1:01");
        assert_eq!(format(59 * 60 * S + 59 * S, false), "59:59");
    }

    #[test]
    fn long_format() {
        assert_eq!(format(3600 * S, false), "1:00:00");
        assert_eq!(format(5 * S, true), "0:00:05");
        assert_eq!(format((2 * 3600 + 3 * 60 + 4) * S, false), "2:03:04");
    }

    #[test]
    fn elapsed_and_remaining() {
        assert_eq!(
            elapsed_remaining(0, Some(125 * S)),
            ("0:00".into(), "\u{2212}2:05".into())
        );
        assert_eq!(
            elapsed_remaining(5 * S + S / 2, Some(125 * S)),
            ("0:05".into(), "\u{2212}2:00".into())
        );
        assert_eq!(
            elapsed_remaining(200 * S, Some(125 * S)),
            ("2:05".into(), "\u{2212}0:00".into())
        );
        assert_eq!(
            elapsed_remaining(S, Some(3600 * S)),
            ("0:00:01".into(), "\u{2212}0:59:59".into())
        );
        assert_eq!(
            elapsed_remaining(3 * S, None),
            ("0:03".into(), "\u{2212}0:00".into())
        );
    }
}
