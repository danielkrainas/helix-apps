//! Date arithmetic and formatting for the calendar views.

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike, Weekday};
use helix_app_common::i18n::tr;
use helix_app_common::trf;

pub fn month_name(m: u32) -> &'static str {
    helix_app_common::i18n::month_name(m)
}

pub fn month_short(m: u32) -> &'static str {
    helix_app_common::i18n::month_abbr(m)
}

pub fn weekday_short(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => tr("Mon"),
        Weekday::Tue => tr("Tue"),
        Weekday::Wed => tr("Wed"),
        Weekday::Thu => tr("Thu"),
        Weekday::Fri => tr("Fri"),
        Weekday::Sat => tr("Sat"),
        Weekday::Sun => tr("Sun"),
    }
}

/// Start of the week containing `d`.
pub fn week_start(d: NaiveDate, first: Weekday) -> NaiveDate {
    let back = (7 + d.weekday().num_days_from_monday() - first.num_days_from_monday()) % 7;
    d - Duration::days(back as i64)
}

/// Days shown in a month grid: whole weeks covering the month (5 or 6).
pub fn month_grid(year: i32, month: u32, first: Weekday) -> Vec<NaiveDate> {
    let first_day = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let start = week_start(first_day, first);
    let last = add_months(first_day, 1) - Duration::days(1);
    let end = week_start(last, first) + Duration::days(7);
    let n = (end - start).num_days();
    (0..n).map(|i| start + Duration::days(i)).collect()
}

/// Adds months, clamping the day to the target month's length.
pub fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + n;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let len = helix_eds::rrule::month_len(y, m);
    NaiveDate::from_ymd_opt(y, m, d.day().min(len)).expect("valid date")
}

pub fn iso_week(d: NaiveDate) -> u32 {
    d.iso_week().week()
}

pub fn month_title(d: NaiveDate) -> String {
    // Translators: a month view's title, "November 2026".
    trf!(
        "{month} {year}",
        month = month_name(d.month()),
        year = d.year()
    )
}

/// "9 – 15 Nov 2026", "30 Nov – 6 Dec 2026", "28 Dec 2026 – 3 Jan 2027".
pub fn range_title(start: NaiveDate, end_inclusive: NaiveDate) -> String {
    let (s, e) = (start, end_inclusive);
    if s.year() != e.year() {
        // Translators: a week spanning two years, "28 Dec 2026 – 3 Jan 2027".
        trf!(
            "{day1} {month1} {year1} – {day2} {month2} {year2}",
            day1 = s.day(),
            month1 = month_short(s.month()),
            year1 = s.year(),
            day2 = e.day(),
            month2 = month_short(e.month()),
            year2 = e.year()
        )
    } else if s.month() != e.month() {
        // Translators: a week spanning two months, "30 Nov – 6 Dec 2026".
        trf!(
            "{day1} {month1} – {day2} {month2} {year}",
            day1 = s.day(),
            month1 = month_short(s.month()),
            day2 = e.day(),
            month2 = month_short(e.month()),
            year = e.year()
        )
    } else {
        // Translators: a week within one month, "9 – 15 Nov 2026".
        trf!(
            "{day1} – {day2} {month} {year}",
            day1 = s.day(),
            day2 = e.day(),
            month = month_short(e.month()),
            year = e.year()
        )
    }
}

/// "Friday, 13 November 2026", with the weekday in the current locale.
pub fn long_date(d: NaiveDate) -> String {
    // Translators: a full date, "Friday, 13 November 2026".
    trf!(
        "{weekday}, {day} {month} {year}",
        weekday = helix_app_common::i18n::weekday_name(d.weekday().num_days_from_monday()),
        day = d.day(),
        month = month_name(d.month()),
        year = d.year()
    )
}

/// "Fri 13 Nov".
pub fn short_date(d: NaiveDate) -> String {
    // Translators: a short date, "Fri 13 Nov".
    trf!(
        "{weekday} {day} {month}",
        weekday = weekday_short(d.weekday()),
        day = d.day(),
        month = month_short(d.month())
    )
}

pub fn format_time(t: NaiveTime, twelve: bool) -> String {
    if twelve {
        let (pm, h) = t.hour12();
        if t.minute() == 0 {
            format!("{h} {}", if pm { "PM" } else { "AM" })
        } else {
            format!("{h}:{:02} {}", t.minute(), if pm { "PM" } else { "AM" })
        }
    } else {
        format!("{:02}:{:02}", t.hour(), t.minute())
    }
}

/// Parses user time input: `9`, `930`, `0930`, `9:30`, `9.30`, `9:30pm`,
/// `9 pm`, `21:30`, `12am`.
pub fn parse_time_input(s: &str) -> Option<NaiveTime> {
    let s = s.trim().to_lowercase();
    let (body, suffix) = if let Some(b) = s
        .strip_suffix("am")
        .or_else(|| s.strip_suffix("a.m."))
        .or_else(|| s.strip_suffix('a'))
    {
        (b.trim().to_string(), Some(false))
    } else if let Some(b) = s
        .strip_suffix("pm")
        .or_else(|| s.strip_suffix("p.m."))
        .or_else(|| s.strip_suffix('p'))
    {
        (b.trim().to_string(), Some(true))
    } else {
        (s.clone(), None)
    };
    let (h, m): (u32, u32) = if let Some((h, m)) = body.split_once([':', '.']) {
        (h.trim().parse().ok()?, m.trim().parse().ok()?)
    } else {
        if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        match body.len() {
            1 | 2 => (body.parse().ok()?, 0),
            3 => (body[..1].parse().ok()?, body[1..].parse().ok()?),
            4 => (body[..2].parse().ok()?, body[2..].parse().ok()?),
            _ => return None,
        }
    };
    let h = match suffix {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            (h % 12) + if pm { 12 } else { 0 }
        }
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

/// Parses `YYYY-MM-DD`.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// First day of the week for the current locale (`LC_ALL`, `LC_TIME`,
/// `LANG`), from glibc's locale sources; Monday when unknown.
pub fn first_weekday() -> Weekday {
    let locale = ["LC_ALL", "LC_TIME", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    let name = locale.split(['.', '@']).next().unwrap_or("").to_string();
    if name.is_empty() || name == "C" || name == "POSIX" {
        return Weekday::Mon;
    }
    first_weekday_from_sources(&name, |n| {
        std::fs::read_to_string(format!("/usr/share/i18n/locales/{n}")).ok()
    })
    .unwrap_or(Weekday::Mon)
}

/// Resolves the first weekday by reading locale definitions through `read`
/// (follows `copy` inside LC_TIME).
pub fn first_weekday_from_sources(
    name: &str,
    read: impl Fn(&str) -> Option<String>,
) -> Option<Weekday> {
    let mut current = name.to_string();
    for _ in 0..8 {
        let text = read(&current)?;
        let mut in_time = false;
        let mut week_start_day: Option<NaiveDate> = None;
        let mut first: Option<u32> = None;
        let mut copy: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            if line == "LC_TIME" {
                in_time = true;
                continue;
            }
            if line == "END LC_TIME" {
                break;
            }
            if !in_time {
                continue;
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("week") => {
                    let rest: String = parts.collect::<Vec<_>>().join("");
                    week_start_day = rest
                        .split(';')
                        .nth(1)
                        .and_then(|d| NaiveDate::parse_from_str(d, "%Y%m%d").ok());
                }
                Some("first_weekday") => first = parts.next().and_then(|n| n.parse().ok()),
                Some("copy") => copy = parts.next().map(|c| c.trim_matches('"').to_string()),
                _ => {}
            }
        }
        if week_start_day.is_some() || first.is_some() {
            let base =
                week_start_day.unwrap_or_else(|| NaiveDate::from_ymd_opt(1997, 11, 30).unwrap());
            let day = base + Duration::days(first.unwrap_or(1) as i64 - 1);
            return Some(day.weekday());
        }
        current = copy?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn grids() {
        // November 2026 starts on a Sunday.
        let g = month_grid(2026, 11, Weekday::Mon);
        assert_eq!(g[0], d(2026, 10, 26));
        assert_eq!(g.len(), 42);
        assert_eq!(*g.last().unwrap(), d(2026, 12, 6));
        let g = month_grid(2026, 11, Weekday::Sun);
        assert_eq!(g[0], d(2026, 11, 1));
        assert_eq!(g.len(), 35);
        // February 2026 with Sunday start fits exactly 4 weeks.
        assert_eq!(month_grid(2026, 2, Weekday::Sun).len(), 28);
    }

    #[test]
    fn week_and_months() {
        assert_eq!(week_start(d(2026, 11, 13), Weekday::Mon), d(2026, 11, 9));
        assert_eq!(week_start(d(2026, 11, 13), Weekday::Sun), d(2026, 11, 8));
        assert_eq!(week_start(d(2026, 11, 8), Weekday::Sun), d(2026, 11, 8));
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 28));
        assert_eq!(add_months(d(2026, 1, 15), -1), d(2025, 12, 15));
        assert_eq!(iso_week(d(2026, 11, 13)), 46);
    }

    #[test]
    fn titles() {
        assert_eq!(month_title(d(2026, 11, 13)), "November 2026");
        assert_eq!(
            range_title(d(2026, 11, 9), d(2026, 11, 15)),
            "9 – 15 Nov 2026"
        );
        assert_eq!(
            range_title(d(2026, 11, 30), d(2026, 12, 6)),
            "30 Nov – 6 Dec 2026"
        );
        assert_eq!(
            range_title(d(2026, 12, 28), d(2027, 1, 3)),
            "28 Dec 2026 – 3 Jan 2027"
        );
        assert_eq!(long_date(d(2026, 11, 13)), "Friday, 13 November 2026");
        assert_eq!(short_date(d(2026, 11, 13)), "Fri 13 Nov");
    }

    #[test]
    fn time_format_and_parse() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(format_time(t(9, 5), false), "09:05");
        assert_eq!(format_time(t(21, 30), true), "9:30 PM");
        assert_eq!(format_time(t(0, 0), true), "12 AM");
        assert_eq!(parse_time_input("9"), Some(t(9, 0)));
        assert_eq!(parse_time_input("930"), Some(t(9, 30)));
        assert_eq!(parse_time_input("0930"), Some(t(9, 30)));
        assert_eq!(parse_time_input("9:30pm"), Some(t(21, 30)));
        assert_eq!(parse_time_input("12 am"), Some(t(0, 0)));
        assert_eq!(parse_time_input("12pm"), Some(t(12, 0)));
        assert_eq!(parse_time_input("21.45"), Some(t(21, 45)));
        assert_eq!(parse_time_input("25:00"), None);
        assert_eq!(parse_time_input("13pm"), None);
        assert_eq!(parse_time_input("abc"), None);
        assert_eq!(parse_date("2026-11-13"), Some(d(2026, 11, 13)));
    }

    #[test]
    fn locale_first_weekday() {
        let files = |n: &str| -> Option<String> {
            Some(
                match n {
                    "en_US" => "LC_TIME\nweek 7;19971130;1\nEND LC_TIME\n",
                    "en_GB" => "LC_CTYPE\ncopy \"x\"\nEND LC_CTYPE\nLC_TIME\nweek    7;19971130;4\nfirst_weekday 2\nEND LC_TIME\n",
                    "de_AT" => "LC_TIME\ncopy \"de_DE\"\nEND LC_TIME\n",
                    "de_DE" => "LC_TIME\nweek    7;19971130;4\nfirst_weekday 2\nEND LC_TIME\n",
                    "ar_EG" => "LC_TIME\nweek 7;19971130;1\nfirst_weekday 7\nEND LC_TIME\n",
                    _ => return None,
                }
                .to_string(),
            )
        };
        assert_eq!(
            first_weekday_from_sources("en_US", files),
            Some(Weekday::Sun)
        );
        assert_eq!(
            first_weekday_from_sources("en_GB", files),
            Some(Weekday::Mon)
        );
        assert_eq!(
            first_weekday_from_sources("de_AT", files),
            Some(Weekday::Mon)
        );
        assert_eq!(
            first_weekday_from_sources("ar_EG", files),
            Some(Weekday::Sat)
        );
        assert_eq!(first_weekday_from_sources("xx_YY", files), None);
    }
}
