//! Alarms: model, next-ring computation and the ringing state machine.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDateTime, TimeZone, Utc, Weekday};

/// A set of weekdays; bit 0 = Monday … bit 6 = Sunday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Weekdays(pub u8);

impl Weekdays {
    pub const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    pub fn is_empty(self) -> bool {
        self.0 & 0x7f == 0
    }
    pub fn contains(self, d: Weekday) -> bool {
        self.0 & (1 << d.num_days_from_monday()) != 0
    }
    pub fn set(&mut self, d: Weekday, on: bool) {
        let bit = 1 << d.num_days_from_monday();
        if on {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }
    /// From GNOME's `days` list (0 = Monday … 6 = Sunday). `7` is also
    /// accepted as Sunday (written by early Helix builds).
    pub fn from_numbers(days: &[i32]) -> Self {
        let mut w = Weekdays(0);
        for &d in days {
            if (0..=7).contains(&d) {
                w.0 |= 1 << if d == 7 { 6 } else { d };
            }
        }
        w
    }
    pub fn to_numbers(self) -> Vec<i32> {
        (0..7).filter(|i| self.0 & (1 << i) != 0).collect()
    }

    /// "Every Day", "Weekdays", "Weekends", "Mon, Wed, Fri" or "" (one-shot).
    pub fn summary(self) -> String {
        match self.0 & 0x7f {
            0 => String::new(),
            0x7f => "Every Day".into(),
            0x1f => "Weekdays".into(),
            0x60 => "Weekends".into(),
            _ => Self::ALL
                .iter()
                .filter(|d| self.contains(**d))
                .map(|d| short_name(*d))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

pub fn short_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alarm {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub hour: u32,
    pub minute: u32,
    pub days: Weekdays,
    pub ring_minutes: u32,
    pub snooze_minutes: u32,
}

pub const DURATION_CHOICES: [u32; 9] = [1, 2, 3, 5, 10, 15, 20, 30, 60];

impl Default for Alarm {
    fn default() -> Self {
        Alarm {
            id: String::new(),
            name: "Alarm".into(),
            active: true,
            hour: 7,
            minute: 0,
            days: Weekdays::default(),
            ring_minutes: 5,
            snooze_minutes: 10,
        }
    }
}

/// Resolves a local wall time to an instant. Ambiguous times (DST fall
/// back) take the earlier instant; nonexistent times (DST gap) are moved
/// forward by the gap length, i.e. to the first valid instant after it.
pub fn resolve_local<Z: TimeZone>(tz: &Z, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(t) => Some(t.with_timezone(&Utc)),
        LocalResult::Ambiguous(a, b) => Some(a.with_timezone(&Utc).min(b.with_timezone(&Utc))),
        LocalResult::None => {
            // Interpret the wall time with the offset in effect before the gap.
            let before = tz
                .from_local_datetime(&(naive - Duration::hours(3)))
                .earliest()?;
            let off = before.offset().fix();
            let instant = (naive - Duration::seconds(off.local_minus_utc() as i64)).and_utc();
            Some(instant)
        }
    }
}

use chrono::Offset;

/// The next instant strictly after `after` at which `alarm` should ring,
/// ignoring whether it is active.
pub fn next_ring<Z: TimeZone>(
    alarm: &Alarm,
    after: DateTime<Utc>,
    tz: &Z,
) -> Option<DateTime<Utc>> {
    let start = after.with_timezone(tz).date_naive();
    for d in 0..=8 {
        let date = start + Duration::days(d);
        if !alarm.days.is_empty() && !alarm.days.contains(date.weekday()) {
            continue;
        }
        let naive = date.and_hms_opt(alarm.hour, alarm.minute, 0)?;
        if let Some(t) = resolve_local(tz, naive) {
            if t > after {
                return Some(t);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RingState {
    Idle,
    Ringing { since: DateTime<Utc> },
    Snoozed { until: DateTime<Utc> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Started ringing (first time or after a snooze).
    Ring,
    /// Stopped ringing because the ring duration elapsed.
    Timeout,
    /// An occurrence was missed by more than the ring duration.
    Missed,
}

/// An alarm plus its runtime state.
#[derive(Debug, Clone)]
pub struct AlarmEntry {
    pub alarm: Alarm,
    pub state: RingState,
    pub next: Option<DateTime<Utc>>,
}

impl AlarmEntry {
    pub fn new<Z: TimeZone>(alarm: Alarm, now: DateTime<Utc>, tz: &Z) -> Self {
        let mut e = AlarmEntry {
            alarm,
            state: RingState::Idle,
            next: None,
        };
        e.reschedule(now, tz);
        e
    }

    pub fn reschedule<Z: TimeZone>(&mut self, now: DateTime<Utc>, tz: &Z) {
        self.state = RingState::Idle;
        self.next = if self.alarm.active {
            next_ring(&self.alarm, now, tz)
        } else {
            None
        };
    }

    pub fn set_active<Z: TimeZone>(&mut self, active: bool, now: DateTime<Utc>, tz: &Z) {
        self.alarm.active = active;
        self.reschedule(now, tz);
    }

    pub fn is_ringing(&self) -> bool {
        matches!(self.state, RingState::Ringing { .. })
    }

    fn ring_len(&self) -> Duration {
        Duration::minutes(self.alarm.ring_minutes.max(1) as i64)
    }

    /// Advances the state machine to `now`.
    pub fn tick<Z: TimeZone>(&mut self, now: DateTime<Utc>, tz: &Z) -> Option<Event> {
        match self.state {
            RingState::Ringing { since } => {
                if now >= since + self.ring_len() {
                    self.finish(now, tz);
                    return Some(Event::Timeout);
                }
                None
            }
            RingState::Snoozed { until } => {
                if now >= until {
                    if now - until < self.ring_len() {
                        self.state = RingState::Ringing { since: until };
                        return Some(Event::Ring);
                    }
                    self.finish(now, tz);
                    return Some(Event::Missed);
                }
                None
            }
            RingState::Idle => {
                let due = self.next.filter(|t| now >= *t)?;
                if now - due < self.ring_len() {
                    self.state = RingState::Ringing { since: due };
                    Some(Event::Ring)
                } else {
                    self.finish(now, tz);
                    Some(Event::Missed)
                }
            }
        }
    }

    pub fn snooze(&mut self, now: DateTime<Utc>) {
        self.state = RingState::Snoozed {
            until: now + Duration::minutes(self.alarm.snooze_minutes.max(1) as i64),
        };
    }

    pub fn stop<Z: TimeZone>(&mut self, now: DateTime<Utc>, tz: &Z) {
        self.finish(now, tz);
    }

    /// Ends the current occurrence: one-shot alarms deactivate, repeating
    /// ones are scheduled for their next day.
    fn finish<Z: TimeZone>(&mut self, now: DateTime<Utc>, tz: &Z) {
        if self.alarm.days.is_empty() {
            self.alarm.active = false;
        }
        // The occurrence that just ended is <= now, so this is the next one.
        self.reschedule(now, tz);
    }
}

/// A random-ish identifier (hex) for new alarms.
pub fn new_id(now: DateTime<Utc>, salt: u64) -> String {
    let mut x = now.timestamp_nanos_opt().unwrap_or_default() as u64
        ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    format!("{x:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::{America::New_York, Europe::Berlin};

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }
    fn local<Z: TimeZone>(tz: &Z, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }
    fn alarm(h: u32, m: u32, days: &[i32]) -> Alarm {
        Alarm {
            hour: h,
            minute: m,
            days: Weekdays::from_numbers(days),
            ..Alarm::default()
        }
    }

    #[test]
    fn weekdays_roundtrip_and_summary() {
        let w = Weekdays::from_numbers(&[0, 2, 4, 9, -1]);
        assert_eq!(w.to_numbers(), vec![0, 2, 4]);
        assert!(w.contains(Weekday::Wed));
        assert!(!w.contains(Weekday::Sun));
        assert_eq!(w.summary(), "Mon, Wed, Fri");
        assert_eq!(
            Weekdays::from_numbers(&[0, 1, 2, 3, 4]).summary(),
            "Weekdays"
        );
        assert_eq!(Weekdays::from_numbers(&[5, 6]).summary(), "Weekends");
        assert_eq!(
            Weekdays::from_numbers(&[5, 7]).summary(),
            "Weekends",
            "7 is a legacy Sunday"
        );
        assert_eq!(
            Weekdays::from_numbers(&[0, 1, 2, 3, 4, 5, 6]).summary(),
            "Every Day"
        );
        assert_eq!(Weekdays::default().summary(), "");
        let mut w = Weekdays::default();
        w.set(Weekday::Sun, true);
        assert_eq!(w.to_numbers(), vec![6]);
        w.set(Weekday::Sun, false);
        assert!(w.is_empty());
    }

    #[test]
    fn one_shot_today_or_tomorrow() {
        // 2024-03-05 is a Tuesday. Berlin is UTC+1 in winter.
        let a = alarm(7, 30, &[]);
        let now = local(&Berlin, 2024, 3, 5, 6, 0);
        assert_eq!(
            next_ring(&a, now, &Berlin),
            Some(local(&Berlin, 2024, 3, 5, 7, 30))
        );
        let now = local(&Berlin, 2024, 3, 5, 7, 30);
        assert_eq!(
            next_ring(&a, now, &Berlin),
            Some(local(&Berlin, 2024, 3, 6, 7, 30)),
            "strictly after"
        );
        let now = local(&Berlin, 2024, 3, 5, 23, 59);
        assert_eq!(
            next_ring(&a, now, &Berlin),
            Some(local(&Berlin, 2024, 3, 6, 7, 30))
        );
    }

    #[test]
    fn repeating_skips_days() {
        // Weekdays alarm, asked on Friday evening → Monday.
        let a = alarm(6, 45, &[0, 1, 2, 3, 4]);
        let fri = local(&New_York, 2024, 3, 8, 20, 0);
        assert_eq!(
            next_ring(&a, fri, &New_York),
            Some(local(&New_York, 2024, 3, 11, 6, 45))
        );
        // Sunday-only alarm asked on Sunday after it → next Sunday.
        let a = alarm(9, 0, &[6]);
        let sun = local(&New_York, 2024, 3, 3, 10, 0);
        assert_eq!(
            next_ring(&a, sun, &New_York),
            Some(local(&New_York, 2024, 3, 10, 9, 0))
        );
    }

    #[test]
    fn dst_gap_and_overlap() {
        // US spring forward 2024-03-10 02:00 → 03:00. A 02:30 alarm rings
        // at 03:30 EDT (= 02:30 EST), i.e. 07:30 UTC.
        let a = alarm(2, 30, &[]);
        let now = local(&New_York, 2024, 3, 10, 0, 0);
        assert_eq!(next_ring(&a, now, &New_York), Some(at(2024, 3, 10, 7, 30)));
        // Fall back 2024-11-03 02:00 → 01:00. A 01:30 alarm rings at the
        // first 01:30 (EDT, 05:30 UTC).
        let a = alarm(1, 30, &[]);
        let now = local(&New_York, 2024, 11, 3, 0, 0);
        assert_eq!(next_ring(&a, now, &New_York), Some(at(2024, 11, 3, 5, 30)));
        // A daily 08:00 alarm keeps 08:00 local across the DST change.
        let a = alarm(8, 0, &[0, 1, 2, 3, 4, 5, 6]);
        let sat = local(&New_York, 2024, 3, 9, 9, 0);
        let sun = next_ring(&a, sat, &New_York).unwrap();
        assert_eq!(sun, at(2024, 3, 10, 12, 0));
        let before = next_ring(&a, local(&New_York, 2024, 3, 8, 9, 0), &New_York).unwrap();
        assert_eq!(before, at(2024, 3, 9, 13, 0));
    }

    #[test]
    fn ring_snooze_stop_cycle() {
        let tz = Berlin;
        let start = local(&tz, 2024, 3, 5, 6, 0);
        let mut e = AlarmEntry::new(alarm(7, 0, &[]), start, &tz);
        let due = local(&tz, 2024, 3, 5, 7, 0);
        assert_eq!(e.next, Some(due));
        assert_eq!(e.tick(due - Duration::seconds(1), &tz), None);
        assert_eq!(e.tick(due, &tz), Some(Event::Ring));
        assert!(e.is_ringing());
        assert_eq!(e.tick(due + Duration::seconds(30), &tz), None);
        e.snooze(due + Duration::minutes(1));
        assert_eq!(
            e.state,
            RingState::Snoozed {
                until: due + Duration::minutes(11)
            }
        );
        assert_eq!(e.tick(due + Duration::minutes(10), &tz), None);
        assert_eq!(e.tick(due + Duration::minutes(11), &tz), Some(Event::Ring));
        e.stop(due + Duration::minutes(12), &tz);
        assert_eq!(e.state, RingState::Idle);
        assert!(!e.alarm.active, "one-shot alarm disables itself");
        assert_eq!(e.next, None);
    }

    #[test]
    fn ring_timeout_reschedules_repeating() {
        let tz = Berlin;
        let mut a = alarm(7, 0, &[0, 1, 2, 3, 4, 5, 6]);
        a.ring_minutes = 2;
        let mut e = AlarmEntry::new(a, local(&tz, 2024, 3, 5, 6, 0), &tz);
        let due = local(&tz, 2024, 3, 5, 7, 0);
        assert_eq!(e.tick(due + Duration::seconds(5), &tz), Some(Event::Ring));
        assert_eq!(e.tick(due + Duration::seconds(119), &tz), None);
        assert_eq!(
            e.tick(due + Duration::seconds(120), &tz),
            Some(Event::Timeout)
        );
        assert!(e.alarm.active);
        assert_eq!(e.next, Some(local(&tz, 2024, 3, 6, 7, 0)));
    }

    #[test]
    fn missed_after_suspend() {
        let tz = Berlin;
        let mut e = AlarmEntry::new(alarm(7, 0, &[1]), local(&tz, 2024, 3, 5, 6, 0), &tz);
        // Woke up at 09:00, two hours after a 5-minute alarm.
        assert_eq!(
            e.tick(local(&tz, 2024, 3, 5, 9, 0), &tz),
            Some(Event::Missed)
        );
        assert_eq!(e.next, Some(local(&tz, 2024, 3, 12, 7, 0)));
        assert_eq!(e.state, RingState::Idle);
    }

    #[test]
    fn stop_during_same_minute_does_not_rering() {
        let tz = Berlin;
        let mut e = AlarmEntry::new(
            alarm(7, 0, &[0, 1, 2, 3, 4, 5, 6]),
            local(&tz, 2024, 3, 5, 6, 0),
            &tz,
        );
        let due = local(&tz, 2024, 3, 5, 7, 0);
        assert_eq!(e.tick(due, &tz), Some(Event::Ring));
        e.stop(due + Duration::seconds(10), &tz);
        assert_eq!(e.tick(due + Duration::seconds(20), &tz), None);
        assert_eq!(e.next, Some(local(&tz, 2024, 3, 6, 7, 0)));
    }

    #[test]
    fn inactive_never_rings_and_toggle() {
        let tz = Berlin;
        let mut a = alarm(7, 0, &[]);
        a.active = false;
        let now = local(&tz, 2024, 3, 5, 6, 0);
        let mut e = AlarmEntry::new(a, now, &tz);
        assert_eq!(e.next, None);
        assert_eq!(e.tick(local(&tz, 2024, 3, 5, 7, 0), &tz), None);
        e.set_active(true, now, &tz);
        assert_eq!(e.next, Some(local(&tz, 2024, 3, 5, 7, 0)));
    }

    #[test]
    fn ids_differ() {
        let now = at(2024, 1, 1, 0, 0);
        assert_ne!(new_id(now, 1), new_id(now, 2));
        assert_eq!(new_id(now, 1).len(), 16);
    }
}
