//! Keyboard navigation of the month and week grids, and the accessible
//! descriptions of their cells. Pure functions, unit-tested.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::dates;

/// A navigation key, independent of GDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

impl NavKey {
    pub fn from_gdk(key: gtk::gdk::Key) -> Option<NavKey> {
        use gtk::gdk::Key;
        Some(match key {
            Key::Left | Key::KP_Left => NavKey::Left,
            Key::Right | Key::KP_Right => NavKey::Right,
            Key::Up | Key::KP_Up => NavKey::Up,
            Key::Down | Key::KP_Down => NavKey::Down,
            Key::Home | Key::KP_Home => NavKey::Home,
            Key::End | Key::KP_End => NavKey::End,
            Key::Page_Up | Key::KP_Page_Up => NavKey::PageUp,
            Key::Page_Down | Key::KP_Page_Down => NavKey::PageDown,
            _ => return None,
        })
    }
}

/// Same day of the month `delta` months away, clamped to the month's end.
pub fn add_months(d: NaiveDate, delta: i32) -> NaiveDate {
    let m0 = d.year() * 12 + d.month0() as i32 + delta;
    let (y, m) = (m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1);
    (1..=d.day())
        .rev()
        .find_map(|day| NaiveDate::from_ymd_opt(y, m, day))
        .unwrap_or(d)
}

/// Month grid: arrows move by a day / a week, Home and End go to the start
/// and end of the week, Page Up / Page Down to the same day of the previous
/// / next month.
pub fn month_target(d: NaiveDate, key: NavKey, first: Weekday) -> NaiveDate {
    match key {
        NavKey::Left => d - Duration::days(1),
        NavKey::Right => d + Duration::days(1),
        NavKey::Up => d - Duration::days(7),
        NavKey::Down => d + Duration::days(7),
        NavKey::Home => dates::week_start(d, first),
        NavKey::End => dates::week_start(d, first) + Duration::days(6),
        NavKey::PageUp => add_months(d, -1),
        NavKey::PageDown => add_months(d, 1),
    }
}

/// Granularity of the week grid's slot cursor, in minutes.
pub const SLOT: i64 = 30;
const LAST_SLOT: i64 = 24 * 60 - SLOT;

/// Week grid slot cursor: Up / Down move by half an hour, Left / Right by a
/// day, Home / End to the first / last slot of the day, Page Up / Page
/// Down by a week.
pub fn week_target(d: NaiveDate, minutes: i64, key: NavKey) -> (NaiveDate, i64) {
    let m = minutes.clamp(0, LAST_SLOT) / SLOT * SLOT;
    match key {
        NavKey::Up => (d, (m - SLOT).max(0)),
        NavKey::Down => (d, (m + SLOT).min(LAST_SLOT)),
        NavKey::Left => (d - Duration::days(1), m),
        NavKey::Right => (d + Duration::days(1), m),
        NavKey::Home => (d, 0),
        NavKey::End => (d, LAST_SLOT),
        NavKey::PageUp => (d - Duration::days(7), m),
        NavKey::PageDown => (d + Duration::days(7), m),
    }
}

/// "Friday, 14 November 2026, today, selected, 3 events: A, B, C".
pub fn day_label(d: NaiveDate, today: NaiveDate, selected: bool, titles: &[&str]) -> String {
    let mut s = dates::long_date(d);
    if d == today {
        s.push_str(", today");
    }
    if selected {
        s.push_str(", selected");
    }
    match titles.len() {
        0 => s.push_str(", no events"),
        1 => s.push_str(&format!(", 1 event: {}", titles[0])),
        n => s.push_str(&format!(", {n} events: {}", titles.join(", "))),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn month_navigation() {
        let fri = d(2026, 11, 13);
        assert_eq!(
            month_target(fri, NavKey::Left, Weekday::Mon),
            d(2026, 11, 12)
        );
        assert_eq!(
            month_target(fri, NavKey::Right, Weekday::Mon),
            d(2026, 11, 14)
        );
        assert_eq!(month_target(fri, NavKey::Up, Weekday::Mon), d(2026, 11, 6));
        assert_eq!(
            month_target(fri, NavKey::Down, Weekday::Mon),
            d(2026, 11, 20)
        );
        assert_eq!(
            month_target(fri, NavKey::Home, Weekday::Mon),
            d(2026, 11, 9)
        );
        assert_eq!(
            month_target(fri, NavKey::End, Weekday::Mon),
            d(2026, 11, 15)
        );
        assert_eq!(
            month_target(fri, NavKey::Home, Weekday::Sun),
            d(2026, 11, 8)
        );
        assert_eq!(
            month_target(fri, NavKey::PageUp, Weekday::Mon),
            d(2026, 10, 13)
        );
        assert_eq!(
            month_target(fri, NavKey::PageDown, Weekday::Mon),
            d(2026, 12, 13)
        );
        // Crossing a year and clamping to the month's last day.
        assert_eq!(
            month_target(d(2027, 1, 31), NavKey::PageUp, Weekday::Mon),
            d(2026, 12, 31)
        );
        assert_eq!(
            month_target(d(2026, 1, 31), NavKey::PageDown, Weekday::Mon),
            d(2026, 2, 28)
        );
        assert_eq!(
            month_target(d(2026, 12, 31), NavKey::Right, Weekday::Mon),
            d(2027, 1, 1)
        );
    }

    #[test]
    fn week_slot_cursor() {
        let day = d(2026, 11, 13);
        assert_eq!(
            week_target(day, 9 * 60 + 10, NavKey::Down),
            (day, 9 * 60 + 30)
        );
        assert_eq!(week_target(day, 0, NavKey::Up), (day, 0));
        assert_eq!(week_target(day, LAST_SLOT, NavKey::Down), (day, LAST_SLOT));
        assert_eq!(week_target(day, 600, NavKey::Right), (d(2026, 11, 14), 600));
        assert_eq!(week_target(day, 600, NavKey::PageUp), (d(2026, 11, 6), 600));
        assert_eq!(week_target(day, 600, NavKey::End), (day, LAST_SLOT));
    }

    #[test]
    fn cell_labels() {
        let day = d(2026, 11, 13);
        assert_eq!(
            day_label(day, day, true, &["Standup", "Lunch"]),
            "Friday, 13 November 2026, today, selected, 2 events: Standup, Lunch"
        );
        assert_eq!(
            day_label(day, d(2026, 11, 1), false, &[]),
            "Friday, 13 November 2026, no events"
        );
    }
}
