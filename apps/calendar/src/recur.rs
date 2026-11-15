//! Editor presets for recurrence rules and reminders.

use chrono::{Duration, NaiveDate, Weekday};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;
use helix_app_common::trnf;
use helix_eds::rrule::{Freq, RRule, Until};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    None,
    Daily,
    Weekdays,
    Weekly,
    Monthly,
    Yearly,
    Custom,
}

pub const PRESETS: [(Preset, &str); 7] = [
    (Preset::None, n_("No Repeat")),
    (Preset::Daily, n_("Daily")),
    (Preset::Weekdays, n_("Weekdays")),
    (Preset::Weekly, n_("Weekly")),
    (Preset::Monthly, n_("Monthly")),
    (Preset::Yearly, n_("Yearly")),
    (Preset::Custom, n_("Custom")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ends {
    Forever,
    Count(u32),
    Until(NaiveDate),
}

const WEEKDAYS: [Weekday; 5] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
];

/// Classifies an existing rule.
pub fn classify(rule: Option<&RRule>) -> Preset {
    let Some(r) = rule else { return Preset::None };
    let plain = r.interval == 1
        && r.by_month_day.is_empty()
        && r.by_month.is_empty()
        && r.by_set_pos.is_empty()
        && r.other.is_empty();
    if !plain {
        return Preset::Custom;
    }
    match r.freq {
        Freq::Daily if r.by_day.is_empty() => Preset::Daily,
        Freq::Weekly if r.by_day.is_empty() => Preset::Weekly,
        Freq::Weekly => {
            let mut days: Vec<Weekday> = r
                .by_day
                .iter()
                .filter(|(o, _)| *o == 0)
                .map(|(_, w)| *w)
                .collect();
            days.sort_by_key(|w| w.num_days_from_monday());
            days.dedup();
            if days.len() == r.by_day.len() && days == WEEKDAYS {
                Preset::Weekdays
            } else if days.len() == 1 && r.by_day.len() == 1 {
                Preset::Weekly
            } else {
                Preset::Custom
            }
        }
        Freq::Monthly if r.by_day.is_empty() => Preset::Monthly,
        Freq::Yearly if r.by_day.is_empty() => Preset::Yearly,
        _ => Preset::Custom,
    }
}

pub fn ends_of(rule: Option<&RRule>) -> Ends {
    match rule {
        Some(RRule { count: Some(c), .. }) => Ends::Count(*c),
        Some(RRule {
            until: Some(Until::Date(d)),
            ..
        }) => Ends::Until(*d),
        Some(RRule {
            until: Some(Until::DateTime { value, .. }),
            ..
        }) => Ends::Until(value.date()),
        _ => Ends::Forever,
    }
}

/// Builds a rule for a preset. `Custom` keeps `existing` (with the new end).
/// `until_utc` makes UNTIL a UTC date-time (required for timed events).
pub fn build(preset: Preset, ends: Ends, existing: Option<&RRule>, all_day: bool) -> Option<RRule> {
    let mut r = match preset {
        Preset::None => return None,
        Preset::Daily => RRule::new(Freq::Daily),
        Preset::Weekdays => {
            let mut r = RRule::new(Freq::Weekly);
            r.by_day = WEEKDAYS.iter().map(|w| (0, *w)).collect();
            r
        }
        Preset::Weekly => RRule::new(Freq::Weekly),
        Preset::Monthly => RRule::new(Freq::Monthly),
        Preset::Yearly => RRule::new(Freq::Yearly),
        Preset::Custom => existing.cloned()?,
    };
    if preset == Preset::Weekly {
        if let Some(e) = existing {
            if classify(Some(e)) == Preset::Weekly {
                r.by_day = e.by_day.clone();
            }
        }
    }
    r.count = None;
    r.until = None;
    match ends {
        Ends::Forever => {}
        Ends::Count(c) => r.count = Some(c.max(1)),
        Ends::Until(d) => {
            r.until = Some(if all_day {
                Until::Date(d)
            } else {
                Until::DateTime {
                    value: d.and_hms_opt(23, 59, 59).unwrap(),
                    utc: true,
                }
            })
        }
    }
    Some(r)
}

/// Reminder offsets offered by the editor.
pub const REMINDERS: [(i64, &str); 8] = [
    (5, n_("5 minutes before")),
    (10, n_("10 minutes before")),
    (15, n_("15 minutes before")),
    (30, n_("30 minutes before")),
    (60, n_("1 hour before")),
    (1440, n_("1 day before")),
    (2880, n_("2 days before")),
    (10080, n_("1 week before")),
];

/// Human description of a reminder offset (time before start).
pub fn describe_reminder(before: Option<Duration>) -> String {
    let Some(d) = before else {
        return tr("Custom reminder").into();
    };
    let mins = d.num_minutes();
    if mins == 0 {
        return tr("At the start").into();
    }
    let (n, after) = if mins < 0 {
        (-mins, true)
    } else {
        (mins, false)
    };
    let text = if n % 10080 == 0 {
        trnf!("{n} week", "{n} weeks", n / 10080)
    } else if n % 1440 == 0 {
        trnf!("{n} day", "{n} days", n / 1440)
    } else if n % 60 == 0 {
        trnf!("{n} hour", "{n} hours", n / 60)
    } else {
        trnf!("{n} minute", "{n} minutes", n)
    };
    if after {
        // Translators: a reminder time, e.g. "10 minutes after".
        trf!("{text} after", text = text)
    } else {
        // Translators: a reminder time, e.g. "10 minutes before".
        trf!("{text} before", text = text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> RRule {
        RRule::parse(s).unwrap()
    }

    #[test]
    fn classify_rules() {
        assert_eq!(classify(None), Preset::None);
        assert_eq!(classify(Some(&r("FREQ=DAILY"))), Preset::Daily);
        assert_eq!(
            classify(Some(&r("FREQ=WEEKLY;BYDAY=FR,MO,TU,WE,TH"))),
            Preset::Weekdays
        );
        assert_eq!(classify(Some(&r("FREQ=WEEKLY;BYDAY=TU"))), Preset::Weekly);
        assert_eq!(
            classify(Some(&r("FREQ=WEEKLY;BYDAY=TU,TH"))),
            Preset::Custom
        );
        assert_eq!(classify(Some(&r("FREQ=MONTHLY;COUNT=3"))), Preset::Monthly);
        assert_eq!(
            classify(Some(&r("FREQ=MONTHLY;BYDAY=-1FR"))),
            Preset::Custom
        );
        assert_eq!(classify(Some(&r("FREQ=YEARLY;INTERVAL=2"))), Preset::Custom);
    }

    #[test]
    fn build_rules() {
        assert_eq!(build(Preset::None, Ends::Forever, None, false), None);
        assert_eq!(
            build(Preset::Weekdays, Ends::Count(10), None, false)
                .unwrap()
                .to_string(),
            "FREQ=WEEKLY;COUNT=10;BYDAY=MO,TU,WE,TH,FR"
        );
        let d = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert_eq!(
            build(Preset::Daily, Ends::Until(d), None, true)
                .unwrap()
                .to_string(),
            "FREQ=DAILY;UNTIL=20261231"
        );
        assert_eq!(
            build(Preset::Daily, Ends::Until(d), None, false)
                .unwrap()
                .to_string(),
            "FREQ=DAILY;UNTIL=20261231T235959Z"
        );
        let custom = r("FREQ=MONTHLY;BYDAY=-1FR;COUNT=4");
        assert_eq!(
            build(Preset::Custom, Ends::Forever, Some(&custom), false)
                .unwrap()
                .to_string(),
            "FREQ=MONTHLY;BYDAY=-1FR"
        );
        let weekly = r("FREQ=WEEKLY;BYDAY=TU");
        assert_eq!(
            build(Preset::Weekly, Ends::Forever, Some(&weekly), false)
                .unwrap()
                .to_string(),
            "FREQ=WEEKLY;BYDAY=TU"
        );
        assert_eq!(ends_of(Some(&custom)), Ends::Count(4));
        assert_eq!(
            ends_of(Some(&r("FREQ=DAILY;UNTIL=20261231T235959Z"))),
            Ends::Until(d)
        );
    }

    #[test]
    fn reminder_text() {
        assert_eq!(
            describe_reminder(Some(Duration::minutes(15))),
            "15 minutes before"
        );
        assert_eq!(
            describe_reminder(Some(Duration::minutes(60))),
            "1 hour before"
        );
        assert_eq!(describe_reminder(Some(Duration::days(2))), "2 days before");
        assert_eq!(describe_reminder(Some(Duration::weeks(1))), "1 week before");
        assert_eq!(
            describe_reminder(Some(Duration::minutes(-5))),
            "5 minutes after"
        );
        assert_eq!(describe_reminder(Some(Duration::zero())), "At the start");
        assert_eq!(describe_reminder(None), "Custom reminder");
    }
}
