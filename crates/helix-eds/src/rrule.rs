//! RFC 5545 recurrence rules: the subset used by desktop calendars (DAILY,
//! WEEKLY, MONTHLY, YEARLY with INTERVAL, COUNT, UNTIL, BYDAY, BYMONTHDAY,
//! BYMONTH, BYSETPOS, WKST). Expansion works on wall-clock times in the
//! event's own time zone, as the RFC requires.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Freq {
    fn parse(s: &str) -> Option<Freq> {
        Some(match s {
            "SECONDLY" => Freq::Secondly,
            "MINUTELY" => Freq::Minutely,
            "HOURLY" => Freq::Hourly,
            "DAILY" => Freq::Daily,
            "WEEKLY" => Freq::Weekly,
            "MONTHLY" => Freq::Monthly,
            "YEARLY" => Freq::Yearly,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Freq::Secondly => "SECONDLY",
            Freq::Minutely => "MINUTELY",
            Freq::Hourly => "HOURLY",
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        }
    }
}

/// UNTIL bound: either a date or a date-time (UTC flag kept so callers can
/// convert into the event's frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    Date(NaiveDate),
    DateTime { value: NaiveDateTime, utc: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRule {
    pub freq: Freq,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<Until>,
    /// (ordinal, weekday); ordinal 0 means "every".
    pub by_day: Vec<(i32, Weekday)>,
    pub by_month_day: Vec<i32>,
    pub by_month: Vec<u32>,
    pub by_set_pos: Vec<i32>,
    pub wkst: Weekday,
    /// Parts this implementation does not interpret, kept for round trips.
    pub other: Vec<(String, String)>,
}

pub fn weekday_code(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

impl RRule {
    pub fn new(freq: Freq) -> RRule {
        RRule {
            freq,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            wkst: Weekday::Mon,
            other: Vec::new(),
        }
    }

    pub fn parse(s: &str) -> Option<RRule> {
        let mut freq = None;
        let mut r = RRule::new(Freq::Daily);
        for part in s.trim().split(';').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=')?;
            let k = k.trim().to_ascii_uppercase();
            let v = v.trim().to_ascii_uppercase();
            match k.as_str() {
                "FREQ" => freq = Freq::parse(&v),
                "INTERVAL" => r.interval = v.parse().ok().filter(|i| *i > 0)?,
                "COUNT" => r.count = v.parse().ok(),
                "UNTIL" => r.until = parse_until(&v),
                "BYDAY" => {
                    for d in v.split(',') {
                        let d = d.trim();
                        if d.len() < 2 {
                            return None;
                        }
                        let (ord, code) = d.split_at(d.len() - 2);
                        let ord = if ord.is_empty() {
                            0
                        } else {
                            ord.trim_start_matches('+').parse().ok()?
                        };
                        r.by_day.push((ord, parse_weekday(code)?));
                    }
                }
                "BYMONTHDAY" => {
                    r.by_month_day = v.split(',').filter_map(|x| x.parse().ok()).collect()
                }
                "BYMONTH" => r.by_month = v.split(',').filter_map(|x| x.parse().ok()).collect(),
                "BYSETPOS" => r.by_set_pos = v.split(',').filter_map(|x| x.parse().ok()).collect(),
                "WKST" => r.wkst = parse_weekday(&v)?,
                _ => r.other.push((k, v)),
            }
        }
        r.freq = freq?;
        Some(r)
    }

    /// Whether the expansion below understands every part of the rule.
    pub fn is_supported(&self) -> bool {
        matches!(
            self.freq,
            Freq::Daily | Freq::Weekly | Freq::Monthly | Freq::Yearly
        ) && self.other.is_empty()
    }

    /// Generates occurrence start times (wall clock, event frame) from
    /// `dtstart` up to and including `horizon`, at most `max` items. `until`
    /// must already be converted into the event frame. DTSTART is always the
    /// first occurrence.
    pub fn occurrences(
        &self,
        dtstart: NaiveDateTime,
        until: Option<NaiveDateTime>,
        horizon: NaiveDateTime,
        max: usize,
    ) -> Vec<NaiveDateTime> {
        let mut out = vec![dtstart];
        if !self.is_supported() {
            return out;
        }
        let limit_count = self
            .count
            .map(|c| c as usize)
            .unwrap_or(usize::MAX)
            .min(max);
        let time = dtstart.time();
        let interval = self.interval.max(1) as i64;
        let mut period: i64 = 0;
        let mut guard = 0;
        'outer: loop {
            guard += 1;
            if guard > 100_000 || out.len() >= limit_count {
                break;
            }
            let (period_start, mut cands) = match self.freq {
                Freq::Daily => {
                    let d = dtstart.date() + Duration::days(period * interval);
                    let ok = self.month_ok(d)
                        && (self.by_month_day.is_empty() || self.month_day_ok(d))
                        && (self.by_day.is_empty()
                            || self.by_day.iter().any(|(_, w)| *w == d.weekday()));
                    (d, if ok { vec![d] } else { vec![] })
                }
                Freq::Weekly => {
                    let offset = (7 + dtstart.weekday().num_days_from_monday() as i64
                        - self.wkst.num_days_from_monday() as i64)
                        % 7;
                    let week_start = dtstart.date() - Duration::days(offset)
                        + Duration::weeks(period * interval);
                    let days: Vec<Weekday> = if self.by_day.is_empty() {
                        vec![dtstart.weekday()]
                    } else {
                        self.by_day.iter().map(|(_, w)| *w).collect()
                    };
                    let mut c: Vec<NaiveDate> = (0..7)
                        .map(|i| week_start + Duration::days(i))
                        .filter(|d| days.contains(&d.weekday()) && self.month_ok(*d))
                        .collect();
                    c.sort();
                    (week_start, c)
                }
                Freq::Monthly => {
                    let total =
                        dtstart.year() as i64 * 12 + dtstart.month0() as i64 + period * interval;
                    let (y, m) = (
                        (total.div_euclid(12)) as i32,
                        (total.rem_euclid(12)) as u32 + 1,
                    );
                    let first = NaiveDate::from_ymd_opt(y, m, 1).unwrap();
                    let c = if self.by_month.is_empty() || self.by_month.contains(&m) {
                        self.month_candidates(y, m, dtstart.day())
                    } else {
                        vec![]
                    };
                    (first, c)
                }
                Freq::Yearly => {
                    let y = dtstart.year() + (period * interval) as i32;
                    let Some(first) = NaiveDate::from_ymd_opt(y, 1, 1) else {
                        break;
                    };
                    let mut c = Vec::new();
                    if self.by_month.is_empty()
                        && self.by_month_day.is_empty()
                        && !self.by_day.is_empty()
                    {
                        // BYDAY relative to the whole year.
                        let days: Vec<NaiveDate> =
                            first.iter_days().take_while(|d| d.year() == y).collect();
                        c = select_by_day(&days, &self.by_day);
                    } else {
                        let months = if self.by_month.is_empty() {
                            vec![dtstart.month()]
                        } else {
                            self.by_month.clone()
                        };
                        for m in months {
                            c.extend(self.month_candidates(y, m, dtstart.day()));
                        }
                    }
                    c.sort();
                    (first, c)
                }
                _ => break,
            };
            if !self.by_set_pos.is_empty() && !cands.is_empty() {
                let n = cands.len() as i32;
                let mut picked: Vec<NaiveDate> = self
                    .by_set_pos
                    .iter()
                    .filter_map(|p| {
                        let idx = if *p > 0 { p - 1 } else { n + p };
                        (0..n).contains(&idx).then(|| cands[idx as usize])
                    })
                    .collect();
                picked.sort();
                picked.dedup();
                cands = picked;
            }
            for d in cands {
                let t = d.and_time(time);
                if t <= dtstart {
                    continue;
                }
                if until.is_some_and(|u| t > u) || t > horizon {
                    break 'outer;
                }
                out.push(t);
                if out.len() >= limit_count {
                    break 'outer;
                }
            }
            if period_start.and_time(time) > horizon
                || until.is_some_and(|u| period_start.and_time(time) > u)
            {
                break;
            }
            period += 1;
        }
        out
    }

    fn month_ok(&self, d: NaiveDate) -> bool {
        self.by_month.is_empty() || self.by_month.contains(&d.month())
    }

    fn month_day_ok(&self, d: NaiveDate) -> bool {
        let len = month_len(d.year(), d.month()) as i32;
        self.by_month_day.iter().any(|md| {
            let target = if *md > 0 { *md } else { len + md + 1 };
            target == d.day() as i32
        })
    }

    fn month_candidates(&self, y: i32, m: u32, default_day: u32) -> Vec<NaiveDate> {
        let Some(first) = NaiveDate::from_ymd_opt(y, m, 1) else {
            return vec![];
        };
        let days: Vec<NaiveDate> = first.iter_days().take_while(|d| d.month() == m).collect();
        let mut c: Vec<NaiveDate> = if self.by_day.is_empty() && self.by_month_day.is_empty() {
            NaiveDate::from_ymd_opt(y, m, default_day)
                .into_iter()
                .collect()
        } else if self.by_day.is_empty() {
            days.into_iter().filter(|d| self.month_day_ok(*d)).collect()
        } else {
            let sel = select_by_day(&days, &self.by_day);
            if self.by_month_day.is_empty() {
                sel
            } else {
                sel.into_iter().filter(|d| self.month_day_ok(*d)).collect()
            }
        };
        c.sort();
        c.dedup();
        c
    }
}

fn select_by_day(days: &[NaiveDate], by_day: &[(i32, Weekday)]) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    for (ord, wd) in by_day {
        let matching: Vec<NaiveDate> = days
            .iter()
            .copied()
            .filter(|d| d.weekday() == *wd)
            .collect();
        if *ord == 0 {
            out.extend(matching);
        } else {
            let n = matching.len() as i32;
            let idx = if *ord > 0 { ord - 1 } else { n + ord };
            if (0..n).contains(&idx) {
                out.push(matching[idx as usize]);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn month_len(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|n| n.pred_opt())
        .map(|d| d.day())
        .unwrap_or(30)
}

fn parse_until(v: &str) -> Option<Until> {
    if v.len() == 8 {
        return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(Until::Date);
    }
    let utc = v.ends_with('Z');
    let v = v.trim_end_matches('Z');
    NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .ok()
        .map(|value| Until::DateTime { value, utc })
}

impl std::fmt::Display for RRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FREQ={}", self.freq.as_str())?;
        if self.interval > 1 {
            write!(f, ";INTERVAL={}", self.interval)?;
        }
        if let Some(c) = self.count {
            write!(f, ";COUNT={c}")?;
        }
        match self.until {
            Some(Until::Date(d)) => write!(f, ";UNTIL={}", d.format("%Y%m%d"))?,
            Some(Until::DateTime { value, utc }) => write!(
                f,
                ";UNTIL={}{}",
                value.format("%Y%m%dT%H%M%S"),
                if utc { "Z" } else { "" }
            )?,
            None => {}
        }
        if !self.by_day.is_empty() {
            let v: Vec<String> = self
                .by_day
                .iter()
                .map(|(o, w)| {
                    if *o == 0 {
                        weekday_code(*w).to_string()
                    } else {
                        format!("{o}{}", weekday_code(*w))
                    }
                })
                .collect();
            write!(f, ";BYDAY={}", v.join(","))?;
        }
        let join = |v: &[i32]| {
            v.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        if !self.by_month_day.is_empty() {
            write!(f, ";BYMONTHDAY={}", join(&self.by_month_day))?;
        }
        if !self.by_month.is_empty() {
            let v: Vec<i32> = self.by_month.iter().map(|m| *m as i32).collect();
            write!(f, ";BYMONTH={}", join(&v))?;
        }
        if !self.by_set_pos.is_empty() {
            write!(f, ";BYSETPOS={}", join(&self.by_set_pos))?;
        }
        if self.wkst != Weekday::Mon {
            write!(f, ";WKST={}", weekday_code(self.wkst))?;
        }
        for (k, v) in &self.other {
            write!(f, ";{k}={v}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn fmt(v: &[NaiveDateTime]) -> Vec<String> {
        v.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect()
    }

    fn far() -> NaiveDateTime {
        dt("2100-01-01 00:00")
    }

    #[test]
    fn daily_with_count_and_interval() {
        let r = RRule::parse("FREQ=DAILY;INTERVAL=2;COUNT=3").unwrap();
        assert_eq!(
            fmt(&r.occurrences(dt("2026-01-30 09:00"), None, far(), 1000)),
            ["2026-01-30", "2026-02-01", "2026-02-03"]
        );
    }

    #[test]
    fn weekly_byday_until() {
        let r = RRule::parse("FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL=20261113T235959Z").unwrap();
        let o = r.occurrences(
            dt("2026-11-02 10:00"),
            Some(dt("2026-11-13 23:59")),
            far(),
            1000,
        );
        assert_eq!(
            fmt(&o),
            [
                "2026-11-02",
                "2026-11-04",
                "2026-11-06",
                "2026-11-09",
                "2026-11-11",
                "2026-11-13"
            ]
        );
    }

    #[test]
    fn biweekly_respects_wkst() {
        let r = RRule::parse("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,SU;WKST=SU;COUNT=4").unwrap();
        // RFC 5545 example: starting Tue 1997-08-05.
        let o = r.occurrences(dt("1997-08-05 09:00"), None, far(), 100);
        assert_eq!(
            fmt(&o),
            ["1997-08-05", "1997-08-17", "1997-08-19", "1997-08-31"]
        );
    }

    #[test]
    fn monthly_by_month_day_skips_short_months() {
        let r = RRule::parse("FREQ=MONTHLY;COUNT=4").unwrap();
        let o = r.occurrences(dt("2026-01-31 08:00"), None, far(), 100);
        assert_eq!(
            fmt(&o),
            ["2026-01-31", "2026-03-31", "2026-05-31", "2026-07-31"]
        );
    }

    #[test]
    fn monthly_last_friday_and_setpos() {
        let r = RRule::parse("FREQ=MONTHLY;BYDAY=-1FR;COUNT=3").unwrap();
        let o = r.occurrences(dt("2026-01-30 08:00"), None, far(), 100);
        assert_eq!(fmt(&o), ["2026-01-30", "2026-02-27", "2026-03-27"]);
        let r = RRule::parse("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=3").unwrap();
        let o = r.occurrences(dt("2026-01-30 08:00"), None, far(), 100);
        assert_eq!(fmt(&o), ["2026-01-30", "2026-02-27", "2026-03-31"]);
    }

    #[test]
    fn monthly_negative_month_day() {
        let r = RRule::parse("FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3").unwrap();
        let o = r.occurrences(dt("2026-01-31 08:00"), None, far(), 100);
        assert_eq!(fmt(&o), ["2026-01-31", "2026-02-28", "2026-03-31"]);
    }

    #[test]
    fn yearly_leap_day_and_nth_weekday() {
        let r = RRule::parse("FREQ=YEARLY;COUNT=3").unwrap();
        let o = r.occurrences(dt("2024-02-29 00:00"), None, far(), 100);
        assert_eq!(fmt(&o), ["2024-02-29", "2028-02-29", "2032-02-29"]);
        let r = RRule::parse("FREQ=YEARLY;BYMONTH=11;BYDAY=4TH;COUNT=2").unwrap();
        let o = r.occurrences(dt("2026-11-26 12:00"), None, far(), 100);
        assert_eq!(fmt(&o), ["2026-11-26", "2027-11-25"]);
    }

    #[test]
    fn horizon_limits_infinite_rules() {
        let r = RRule::parse("FREQ=DAILY").unwrap();
        let o = r.occurrences(dt("2026-01-01 09:00"), None, dt("2026-01-10 00:00"), 10_000);
        assert_eq!(o.len(), 9);
    }

    #[test]
    fn round_trip_string() {
        let s = "FREQ=MONTHLY;INTERVAL=2;COUNT=5;BYDAY=2TU,-1FR;BYSETPOS=1;WKST=SU;X-FOO=BAR";
        let r = RRule::parse(s).unwrap();
        assert_eq!(r.to_string(), s);
        assert!(!r.is_supported());
        assert!(RRule::parse("INTERVAL=2").is_none());
        let u = RRule::parse("FREQ=DAILY;UNTIL=20260105").unwrap();
        assert_eq!(
            u.until,
            Some(Until::Date(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()))
        );
        assert_eq!(u.to_string(), "FREQ=DAILY;UNTIL=20260105");
    }

    #[test]
    fn unsupported_yields_only_dtstart() {
        let r = RRule::parse("FREQ=HOURLY;COUNT=5").unwrap();
        assert_eq!(
            r.occurrences(dt("2026-01-01 09:00"), None, far(), 100)
                .len(),
            1
        );
    }
}
