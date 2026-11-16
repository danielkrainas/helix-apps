//! Human-readable formatting of sizes, durations and temperatures.

use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;
use helix_app_common::trnf;

const UNITS: [&str; 6] = [n_("kB"), n_("MB"), n_("GB"), n_("TB"), n_("PB"), n_("EB")];

/// Short decimal (SI) size: `512 bytes`, `1.0 kB`, `60 GB`, `2.0 TB`.
///
/// Values below 10 of a unit get one decimal, others none, so the result
/// has two or three significant digits.
pub fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return trnf!("{n} byte", "{n} bytes", bytes);
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    loop {
        // Round the way it will be printed, then decide whether it still fits.
        let rounded = if value < 9.95 {
            (value * 10.0).round() / 10.0
        } else {
            value.round()
        };
        if rounded < 1000.0 || unit == UNITS.len() - 1 {
            let value = if rounded < 10.0 {
                format!("{rounded:.1}")
            } else {
                format!("{rounded:.0}")
            };
            // Translators: a size, "1.5" "MB".
            return trf!("{value} {unit}", value = value, unit = tr(UNITS[unit]));
        }
        value /= 1000.0;
        unit += 1;
    }
}

/// `1,234,567`.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Long form with the exact byte count: `2.0 TB (2,000,398,934,016 bytes)`.
pub fn size_long(bytes: u64) -> String {
    if bytes < 1000 {
        return size(bytes);
    }
    trnf!(
        "{size} ({bytes} byte)",
        "{size} ({bytes} bytes)",
        bytes,
        size = size(bytes),
        bytes = group_thousands(bytes)
    )
}

/// `n` of the unit at `index` in `duration`'s table.
fn count(index: usize, n: u64) -> String {
    match index {
        0 => trnf!("{n} year", "{n} years", n),
        1 => trnf!("{n} month", "{n} months", n),
        2 => trnf!("{n} day", "{n} days", n),
        3 => trnf!("{n} hour", "{n} hours", n),
        4 => trnf!("{n} minute", "{n} minutes", n),
        _ => trnf!("{n} second", "{n} seconds", n),
    }
}

/// Coarse duration using the two most significant units:
/// `4 years, 11 months`, `3 days, 2 hours`, `5 minutes`.
pub fn duration(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    let units: [u64; 6] = [YEAR, MONTH, DAY, HOUR, MIN, 1];
    for (i, &len) in units.iter().enumerate() {
        if secs >= len || len == 1 {
            let major = secs / len;
            let rest = secs % len;
            if let Some(&len2) = units.get(i + 1) {
                let minor = rest / len2;
                if minor > 0 && len2 >= MIN {
                    // Translators: a duration in two units, "4 years" "11 months".
                    return trf!(
                        "{major}, {minor}",
                        major = count(i, major),
                        minor = count(i + 1, minor)
                    );
                }
            }
            return count(i, major);
        }
    }
    unreachable!()
}

/// `30° C / 86° F` from kelvin.
pub fn temperature_kelvin(k: f64) -> String {
    let c = k - 273.15;
    let f = c * 9.0 / 5.0 + 32.0;
    trf!(
        "{celsius}° C / {fahrenheit}° F",
        celsius = format!("{c:.0}"),
        fahrenheit = format!("{f:.0}")
    )
}

/// Relative time of a past event: `Less than a minute ago`, `5 minutes ago`.
pub fn ago(secs: u64) -> String {
    if secs < 60 {
        tr("Less than a minute ago").into()
    } else {
        trf!("{} ago", duration(secs))
    }
}

/// Data rate: `85.3 MB/s`.
pub fn rate(bytes_per_sec: f64) -> String {
    // Translators: a data rate, "85.3 MB" per second.
    trf!("{size}/s", size = size(bytes_per_sec.max(0.0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_sizes() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(999), "999 bytes");
    }

    #[test]
    fn decimal_units() {
        assert_eq!(size(1000), "1.0 kB");
        assert_eq!(size(1536), "1.5 kB");
        assert_eq!(size(1_048_576), "1.0 MB");
        assert_eq!(size(60_022_480_896), "60 GB");
        assert_eq!(size(2_000_398_934_016), "2.0 TB");
        assert_eq!(size(500_107_862_016), "500 GB");
        assert_eq!(size(6_001_175_126_016), "6.0 TB");
        assert_eq!(size(15_931_539_456), "16 GB");
    }

    #[test]
    fn rounding_carries_to_next_unit() {
        assert_eq!(size(999_600), "1.0 MB");
        assert_eq!(size(9_960), "10 kB");
        assert_eq!(size(999_499), "999 kB");
    }

    #[test]
    fn long_form() {
        assert_eq!(
            size_long(2_000_398_934_016),
            "2.0 TB (2,000,398,934,016 bytes)"
        );
        assert_eq!(size_long(512), "512 bytes");
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(123_456_789), "123,456,789");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0 seconds");
        assert_eq!(duration(45), "45 seconds");
        assert_eq!(duration(60), "1 minute");
        assert_eq!(duration(3 * 3600 + 120), "3 hours, 2 minutes");
        assert_eq!(duration(430_092_000), "13 years, 7 months");
        assert_eq!(ago(10), "Less than a minute ago");
        assert_eq!(ago(300), "5 minutes ago");
    }

    #[test]
    fn temperatures() {
        assert_eq!(temperature_kelvin(303.15), "30° C / 86° F");
        assert_eq!(temperature_kelvin(273.15), "0° C / 32° F");
    }
}
