//! Minimal civil date/time handling for Open-Meteo's ISO-8601 local times
//! (`YYYY-MM-DDTHH:MM` / `YYYY-MM-DD`), without a timezone database.

use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_app_common::trnf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

impl DateTime {
    /// Parses `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM[:SS]`.
    pub fn parse(s: &str) -> Option<DateTime> {
        let (date, time) = match s.split_once('T') {
            Some((d, t)) => (d, Some(t)),
            None => (s, None),
        };
        let mut dp = date.splitn(3, '-');
        let year = dp.next()?.parse().ok()?;
        let month: u32 = dp.next()?.parse().ok()?;
        let day: u32 = dp.next()?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let (hour, minute) = match time {
            Some(t) => {
                let mut tp = t.split(':');
                let h: u32 = tp.next()?.parse().ok()?;
                let m: u32 = tp.next()?.parse().ok()?;
                if h > 23 || m > 59 {
                    return None;
                }
                (h, m)
            }
            None => (0, 0),
        };
        Some(DateTime {
            year,
            month,
            day,
            hour,
            minute,
        })
    }

    /// Civil time at `unix` seconds shifted by `offset` seconds.
    pub fn from_unix(unix: i64, offset: i64) -> DateTime {
        let t = unix + offset;
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        DateTime {
            year,
            month,
            day,
            hour: (secs / 3600) as u32,
            minute: ((secs % 3600) / 60) as u32,
        }
    }

    pub fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// 0 = Monday … 6 = Sunday.
    pub fn weekday(&self) -> usize {
        // 1970-01-01 was a Thursday.
        ((self.days() + 3).rem_euclid(7)) as usize
    }

    pub fn weekday_name(&self) -> &'static str {
        helix_app_common::i18n::weekday_name(self.weekday() as u32)
    }

    pub fn weekday_short(&self) -> &'static str {
        helix_app_common::i18n::weekday_abbr(self.weekday() as u32)
    }

    pub fn month_short(&self) -> &'static str {
        helix_app_common::i18n::month_abbr(self.month)
    }

    /// Truncated to the hour.
    pub fn hour_floor(&self) -> DateTime {
        DateTime { minute: 0, ..*self }
    }

    pub fn same_day(&self, other: &DateTime) -> bool {
        self.days() == other.days()
    }
}

/// Formats a clock time, honouring the `clock-format` preference.
pub fn format_time(t: &DateTime, use_12h: bool) -> String {
    if use_12h {
        let h = match t.hour % 12 {
            0 => 12,
            h => h,
        };
        let ampm = if t.hour < 12 { tr("AM") } else { tr("PM") };
        format!("{h}:{:02} {ampm}", t.minute)
    } else {
        format!("{:02}:{:02}", t.hour, t.minute)
    }
}

/// Formats an hour mark for the hourly strip (`14:00` or `2 PM`).
pub fn format_hour(t: &DateTime, use_12h: bool) -> String {
    if use_12h {
        let h = match t.hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{h} {}", if t.hour < 12 { tr("AM") } else { tr("PM") })
    } else {
        format!("{:02}:00", t.hour)
    }
}

/// Day label for the daily forecast relative to `today`.
pub fn day_label(day: &DateTime, today: &DateTime) -> String {
    match day.days() - today.days() {
        0 => tr("Today").into(),
        1 => tr("Tomorrow").into(),
        _ => day.weekday_name().into(),
    }
}

/// "Updated …" text for a fetch at `fetched` (Unix seconds), seen at `now`.
/// `fetched_local` is the fetch time in the viewer's local time.
pub fn updated_text(fetched: i64, now: i64, fetched_local: &DateTime, use_12h: bool) -> String {
    let age = (now - fetched).max(0);
    if age < 60 {
        tr("Updated just now").into()
    } else if age < 3600 {
        let m = age / 60;
        trnf!("Updated {n} minute ago", "Updated {n} minutes ago", m)
    } else if age < 6 * 3600 {
        let h = age / 3600;
        trnf!("Updated {n} hour ago", "Updated {n} hours ago", h)
    } else {
        trf!(
            "Updated {} {} {}, {}",
            fetched_local.weekday_short(),
            fetched_local.day,
            fetched_local.month_short(),
            format_time(fetched_local, use_12h)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_forms() {
        let t = DateTime::parse("2026-11-12T23:30").unwrap();
        assert_eq!(
            (t.year, t.month, t.day, t.hour, t.minute),
            (2026, 11, 12, 23, 30)
        );
        let d = DateTime::parse("2026-11-12").unwrap();
        assert_eq!((d.hour, d.minute), (0, 0));
        assert!(DateTime::parse("2026-13-01").is_none());
        assert!(DateTime::parse("garbage").is_none());
        assert!(DateTime::parse("2026-11-12T25:00").is_none());
    }

    #[test]
    fn civil_roundtrip_and_weekday() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for z in [-800_000, -1, 0, 59, 10_957, 20_404, 400_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        // 2026-11-12 is a Thursday.
        assert_eq!(
            DateTime::parse("2026-11-12").unwrap().weekday_name(),
            "Thursday"
        );
        assert_eq!(
            DateTime::parse("2024-02-29").unwrap().weekday_name(),
            "Thursday"
        );
        assert_eq!(
            DateTime::parse("2000-01-01").unwrap().weekday_short(),
            "Sat"
        );
    }

    #[test]
    fn from_unix_with_offset() {
        // 2026-11-12T22:30:00Z = 1794522600
        let unix = days_from_civil(2026, 11, 12) * 86_400 + 22 * 3600 + 30 * 60;
        let t = DateTime::from_unix(unix, 3600);
        assert_eq!(t, DateTime::parse("2026-11-12T23:30").unwrap());
        let t = DateTime::from_unix(unix, 7200);
        assert_eq!(t, DateTime::parse("2026-11-13T00:30").unwrap());
        let t = DateTime::from_unix(unix, -5 * 3600);
        assert_eq!(t, DateTime::parse("2026-11-12T17:30").unwrap());
    }

    #[test]
    fn formatting() {
        let t = DateTime::parse("2026-11-12T00:05").unwrap();
        assert_eq!(format_time(&t, false), "00:05");
        assert_eq!(format_time(&t, true), "12:05 AM");
        let t = DateTime::parse("2026-11-12T13:00").unwrap();
        assert_eq!(format_hour(&t, false), "13:00");
        assert_eq!(format_hour(&t, true), "1 PM");
        let today = DateTime::parse("2026-11-12").unwrap();
        assert_eq!(day_label(&today, &today), "Today");
        assert_eq!(
            day_label(&DateTime::parse("2026-11-13").unwrap(), &today),
            "Tomorrow"
        );
        assert_eq!(
            day_label(&DateTime::parse("2026-11-14").unwrap(), &today),
            "Saturday"
        );
    }

    #[test]
    fn updated() {
        let lt = DateTime::parse("2026-11-12T09:15").unwrap();
        assert_eq!(updated_text(1000, 1030, &lt, false), "Updated just now");
        assert_eq!(
            updated_text(1000, 1000 + 60, &lt, false),
            "Updated 1 minute ago"
        );
        assert_eq!(
            updated_text(1000, 1000 + 25 * 60, &lt, false),
            "Updated 25 minutes ago"
        );
        assert_eq!(
            updated_text(1000, 1000 + 2 * 3600, &lt, false),
            "Updated 2 hours ago"
        );
        assert_eq!(
            updated_text(1000, 1000 + 86_400, &lt, false),
            "Updated Thu 12 Nov, 09:15"
        );
    }
}
