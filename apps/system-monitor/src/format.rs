//! Human-readable formatting of sizes, rates, states and priorities.

use helix_app_common::i18n::{format_number, tr};
use helix_app_common::nc_;
use helix_app_common::trf;
use helix_app_common::trnf;

/// Formats a byte count, SI (kB = 1000) or IEC (KiB = 1024).
pub fn size(bytes: u64, iec: bool) -> String {
    let (base, units): (f64, [&str; 7]) = if iec {
        (
            1024.0,
            [
                "bytes",
                tr("KiB"),
                tr("MiB"),
                tr("GiB"),
                tr("TiB"),
                tr("PiB"),
                tr("EiB"),
            ],
        )
    } else {
        (
            1000.0,
            [
                "bytes",
                tr("kB"),
                tr("MB"),
                tr("GB"),
                tr("TB"),
                tr("PB"),
                tr("EB"),
            ],
        )
    };
    if (bytes as f64) < base {
        return trnf!("{n} byte", "{n} bytes", bytes);
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= base && i < units.len() - 1 {
        v /= base;
        i += 1;
    }
    // Translators: a size, "2.5 MB"; {unit} is one of the units above.
    trf!(
        "{value} {unit}",
        value = format_number(v, 1),
        unit = units[i]
    )
}

/// Formats a transfer rate in bytes per second, or bits per second.
pub fn rate(bytes_per_sec: f64, bits: bool) -> String {
    let v = bytes_per_sec.max(0.0);
    if bits {
        let b = v * 8.0;
        let units = [
            tr("bits/s"),
            tr("kbit/s"),
            tr("Mbit/s"),
            tr("Gbit/s"),
            tr("Tbit/s"),
        ];
        let mut x = b;
        let mut i = 0;
        while x >= 1000.0 && i < units.len() - 1 {
            x /= 1000.0;
            i += 1;
        }
        let value = format_number(x, if i == 0 { 0 } else { 1 });
        trf!("{value} {unit}", value = value, unit = units[i])
    } else {
        // Translators: a transfer rate, "2.5 kB/s".
        trf!("{size}/s", size = size(v.round() as u64, false))
    }
}

/// CPU time in ticks as `h:mm:ss.cc` / `m:ss.cc`.
pub fn cpu_time(ticks: u64, hz: u64) -> String {
    let hz = hz.max(1);
    let centis = ticks * 100 / hz;
    let secs = centis / 100;
    let (h, m, s, c) = (secs / 3600, (secs / 60) % 60, secs % 60, centis % 100);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{c:02}")
    } else {
        format!("{m}:{s:02}.{c:02}")
    }
}

/// Process state letter → user-visible label.
pub fn state(c: char) -> &'static str {
    match c {
        'R' => tr("Running"),
        'S' => tr("Sleeping"),
        'D' => tr("Uninterruptible"),
        'T' => tr("Stopped"),
        't' => tr("Tracing Stop"),
        'Z' => tr("Zombie"),
        'X' | 'x' => tr("Dead"),
        'I' => tr("Idle"),
        'W' => tr("Paging"),
        'K' => tr("Wakekill"),
        'P' => tr("Parked"),
        _ => tr("Unknown"),
    }
}

/// Nice value → priority label.
pub fn priority(nice: i32) -> &'static str {
    match nice {
        i32::MIN..=-15 => tr(nc_!("process priority", "Very High")),
        -14..=-5 => tr(nc_!("process priority", "High")),
        -4..=4 => tr(nc_!("process priority", "Normal")),
        5..=14 => tr(nc_!("process priority", "Low")),
        _ => tr(nc_!("process priority", "Very Low")),
    }
}

/// Preset priorities offered by the Change Priority dialog.
pub const PRIORITY_PRESETS: [(&str, i32); 5] = [
    (nc_!("process priority", "Very High"), -20),
    (nc_!("process priority", "High"), -5),
    (nc_!("process priority", "Normal"), 0),
    (nc_!("process priority", "Low"), 5),
    (nc_!("process priority", "Very Low"), 19),
];

/// Formats a percentage with one decimal.
pub fn percent(p: f64) -> String {
    format!("{p:.1} %")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0, false), "0 bytes");
        assert_eq!(size(999, false), "999 bytes");
        assert_eq!(size(1000, false), "1.0 kB");
        assert_eq!(size(1_500_000, false), "1.5 MB");
        assert_eq!(size(1024, true), "1.0 KiB");
        assert_eq!(size(1023, true), "1023 bytes");
        assert_eq!(size(3 * 1024 * 1024 * 1024, true), "3.0 GiB");
        assert_eq!(size(u64::MAX, false), "18.4 EB");
    }

    #[test]
    fn rates() {
        assert_eq!(rate(0.0, false), "0 bytes/s");
        assert_eq!(rate(2500.0, false), "2.5 kB/s");
        assert_eq!(rate(100.0, true), "800 bits/s");
        assert_eq!(rate(125_000.0, true), "1.0 Mbit/s");
        assert_eq!(rate(-5.0, false), "0 bytes/s");
    }

    #[test]
    fn times() {
        assert_eq!(cpu_time(0, 100), "0:00.00");
        assert_eq!(cpu_time(6150, 100), "1:01.50");
        assert_eq!(cpu_time(3600 * 100 + 5, 100), "1:00:00.05");
    }

    #[test]
    fn labels() {
        assert_eq!(state('R'), "Running");
        assert_eq!(state('T'), "Stopped");
        assert_eq!(state('?'), "Unknown");
        assert_eq!(priority(-20), "Very High");
        assert_eq!(priority(-5), "High");
        assert_eq!(priority(0), "Normal");
        assert_eq!(priority(5), "Low");
        assert_eq!(priority(19), "Very Low");
        for (label, nice) in PRIORITY_PRESETS {
            assert_eq!(priority(nice), tr(label));
        }
        assert_eq!(percent(12.345), "12.3 %");
    }
}
