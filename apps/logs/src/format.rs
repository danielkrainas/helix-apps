//! Text formatting of times, rows, boots and exports.

use crate::journal::{Boot, Entry};
use gtk::glib::{DateTime, TimeZone};

fn datetime(usec: i64, tz: &TimeZone) -> Option<DateTime> {
    DateTime::from_unix_utc(usec.div_euclid(1_000_000))
        .ok()?
        .to_timezone(tz)
        .ok()
}

fn fmt(dt: &DateTime, pattern: &str) -> String {
    dt.format(pattern)
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Compact time for a list row, relative to `now`.
pub fn row_time(usec: Option<i64>, now: &DateTime, tz: &TimeZone) -> String {
    let Some(dt) = usec.and_then(|u| datetime(u, tz)) else {
        return String::new();
    };
    let now = now.to_timezone(tz).unwrap_or_else(|_| now.clone());
    if dt.ymd() == now.ymd() {
        fmt(&dt, "%H:%M:%S")
    } else if dt.year() == now.year() {
        fmt(&dt, "%b %e %H:%M")
    } else {
        fmt(&dt, "%Y-%m-%d")
    }
}

/// Full timestamp with microseconds, for the detail page.
pub fn full_time(usec: Option<i64>, tz: &TimeZone) -> String {
    let Some(u) = usec else { return String::new() };
    let Some(dt) = datetime(u, tz) else {
        return String::new();
    };
    format!(
        "{}.{:06} {}",
        fmt(&dt, "%Y-%m-%d %H:%M:%S"),
        u.rem_euclid(1_000_000),
        fmt(&dt, "%Z")
    )
}

/// One line of an exported view (continuation lines indented with a tab).
pub fn export_line(e: &Entry, tz: &TimeZone) -> String {
    let time = e
        .timestamp_usec()
        .and_then(|u| datetime(u, tz))
        .map(|d| fmt(&d, "%Y-%m-%d %H:%M:%S"))
        .unwrap_or_default();
    let mut head = time;
    let process = e.process();
    if !process.is_empty() {
        head.push(' ');
        head.push_str(process);
        if let Some(pid) = e.pid() {
            head.push('[');
            head.push_str(pid);
            head.push(']');
        }
        head.push(':');
    }
    let msg = e.message().trim_end_matches('\n').replace('\n', "\n\t");
    format!("{head} {msg}")
}

pub fn export_text<'a>(entries: impl IntoIterator<Item = &'a Entry>, tz: &TimeZone) -> String {
    let mut out = String::new();
    for e in entries {
        out.push_str(&export_line(e, tz));
        out.push('\n');
    }
    out
}

/// Label for the boot selector.
pub fn boot_label(b: &Boot, tz: &TimeZone) -> String {
    match b.index {
        0 => return "Current Boot".into(),
        -1 => return "Previous Boot".into(),
        _ => {}
    }
    let range = match (
        b.first.and_then(|u| datetime(u, tz)),
        b.last.and_then(|u| datetime(u, tz)),
    ) {
        (Some(f), Some(l)) => Some(format!(
            "{} – {}",
            fmt(&f, "%b %e %H:%M"),
            fmt(&l, "%b %e %H:%M")
        )),
        (Some(f), None) => Some(fmt(&f, "%b %e %Y %H:%M")),
        _ => b.range_text.clone(),
    };
    range
        .map(|r| r.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| format!("Boot {}", b.index))
}

/// A message squashed to a single line for list rows.
pub fn one_line(msg: &str) -> String {
    let first = msg.trim().lines().next().unwrap_or("");
    let s: String = first
        .chars()
        .take(500)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if msg.trim().lines().nth(1).is_some() {
        format!("{s} …")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> TimeZone {
        TimeZone::utc()
    }

    #[test]
    fn row_times() {
        let now = DateTime::from_utc(2026, 11, 12, 18, 0, 0.0).unwrap();
        let today = 1794506400_i64 * 1_000_000; // 2026-11-12 18:00:00 UTC
        assert_eq!(
            row_time(Some(today - 3_600_000_000), &now, &utc()),
            "17:00:00"
        );
        assert_eq!(
            row_time(Some(today - 86_400_000_000 * 2), &now, &utc()),
            "Nov 10 18:00"
        );
        assert_eq!(
            row_time(Some(today - 86_400_000_000 * 400), &now, &utc()),
            "2025-10-08"
        );
        assert_eq!(row_time(None, &now, &utc()), "");
    }

    #[test]
    fn full_times() {
        assert_eq!(
            full_time(Some(1_794_506_400_000_123), &utc()),
            "2026-11-12 18:00:00.000123 UTC"
        );
    }

    #[test]
    fn export_lines() {
        let e = Entry::from_fields([
            ("MESSAGE", "first\nsecond\n"),
            ("SYSLOG_IDENTIFIER", "app"),
            ("_PID", "12"),
            ("__REALTIME_TIMESTAMP", "1794506400000000"),
        ]);
        assert_eq!(
            export_line(&e, &utc()),
            "2026-11-12 18:00:00 app[12]: first\n\tsecond"
        );
        let k = Entry::from_fields([("MESSAGE", "x"), ("_TRANSPORT", "kernel")]);
        assert_eq!(export_text([&k, &k], &utc()), " kernel: x\n kernel: x\n");
    }

    #[test]
    fn boot_labels() {
        let mk = |index, first, last, text: Option<&str>| Boot {
            index,
            id: "x".into(),
            first,
            last,
            range_text: text.map(String::from),
        };
        assert_eq!(boot_label(&mk(0, None, None, None), &utc()), "Current Boot");
        assert_eq!(
            boot_label(&mk(-1, None, None, None), &utc()),
            "Previous Boot"
        );
        assert_eq!(
            boot_label(
                &mk(-2, Some(1794506400000000), Some(1794510000000000), None),
                &utc()
            ),
            "Nov 12 18:00 – Nov 12 19:00"
        );
        assert_eq!(
            boot_label(&mk(-3, None, None, Some("Tue  2026-10-13")), &utc()),
            "Tue 2026-10-13"
        );
        assert_eq!(boot_label(&mk(-4, None, None, None), &utc()), "Boot -4");
    }

    #[test]
    fn one_lines() {
        assert_eq!(one_line("  a\tb\nc"), "a b …");
        assert_eq!(one_line("plain"), "plain");
    }
}
