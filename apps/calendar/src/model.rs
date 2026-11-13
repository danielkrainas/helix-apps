//! In-memory copy of the components delivered by the calendar views, and
//! their expansion into displayable items.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use helix_eds::ical::{expand, Event, Zones};
use std::collections::HashMap;

/// (UID, recurrence-id string) of a stored component.
pub type CompKey = (String, String);

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub calendar: String,
    /// Component the instance came from (master or detached override).
    pub component: CompKey,
    pub uid: String,
    /// Recurrence-id of this occurrence (empty if not recurring).
    pub rid: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
    pub summary: String,
    pub location: String,
    pub recurring: bool,
}

impl Item {
    pub fn on_day(&self, day: NaiveDate) -> bool {
        let ds = day.and_time(NaiveTime::MIN);
        let de = ds + chrono::Duration::days(1);
        if self.start == self.end {
            return self.start >= ds && self.start < de;
        }
        self.start < de && self.end > ds
    }

    /// Drawn in the all-day area / as a bar in month cells.
    pub fn spans_days(&self) -> bool {
        self.all_day || self.end - self.start >= chrono::Duration::days(1)
    }

    pub fn title(&self) -> &str {
        if self.summary.trim().is_empty() {
            "(No title)"
        } else {
            &self.summary
        }
    }
}

#[derive(Debug, Default)]
pub struct Store {
    /// calendar uid → components.
    pub calendars: HashMap<String, HashMap<CompKey, Event>>,
}

impl Store {
    pub fn clear(&mut self, cal: &str) {
        self.calendars.remove(cal);
    }

    pub fn upsert(&mut self, cal: &str, events: Vec<Event>) {
        let map = self.calendars.entry(cal.to_string()).or_default();
        for e in events {
            map.insert(e.key(), e);
        }
    }

    /// Removes by view id: `uid` alone removes all components of that UID.
    pub fn remove(&mut self, cal: &str, ids: &[(String, String)]) {
        if let Some(map) = self.calendars.get_mut(cal) {
            for (uid, rid) in ids {
                if rid.is_empty() {
                    map.retain(|(u, _), _| u != uid);
                } else {
                    // EDS may report rids in a different textual form; match on
                    // the stored key or on any component with that rid digits.
                    let norm = |s: &str| s.trim_end_matches('Z').to_string();
                    map.retain(|(u, r), _| !(u == uid && norm(r) == norm(rid)));
                }
            }
        }
    }

    pub fn get(&self, cal: &str, key: &CompKey) -> Option<&Event> {
        self.calendars.get(cal)?.get(key)
    }

    /// The master component of a series (or the event itself).
    pub fn master(&self, cal: &str, uid: &str) -> Option<&Event> {
        self.calendars.get(cal)?.get(&(uid.to_string(), String::new()))
    }

    /// Items overlapping `[from, to)` in calendars accepted by `visible`.
    pub fn items(&self, from: NaiveDateTime, to: NaiveDateTime, zones: &Zones, visible: impl Fn(&str) -> bool) -> Vec<Item> {
        let mut out = Vec::new();
        for (cal, comps) in &self.calendars {
            if !visible(cal) {
                continue;
            }
            let events: Vec<Event> = comps.values().cloned().collect();
            let masters: HashMap<&str, bool> = events
                .iter()
                .filter(|e| e.recurrence_id.is_none())
                .map(|e| (e.uid.as_str(), e.is_recurring()))
                .collect();
            for inst in expand(&events, from, to, zones) {
                let e = &events[inst.event];
                out.push(Item {
                    calendar: cal.clone(),
                    component: e.key(),
                    uid: inst.uid.clone(),
                    rid: inst.rid.clone(),
                    start: inst.start,
                    end: inst.end,
                    all_day: inst.all_day,
                    summary: e.summary.clone(),
                    location: e.location.clone(),
                    recurring: masters.get(e.uid.as_str()).copied().unwrap_or(false) || e.recurrence_id.is_some(),
                });
            }
        }
        sort_items(&mut out);
        out
    }
}

/// All-day / multi-day first, then by start, longer first.
pub fn sort_items(items: &mut [Item]) {
    items.sort_by(|a, b| {
        b.spans_days()
            .cmp(&a.spans_days())
            .then(a.start.cmp(&b.start))
            .then(b.end.cmp(&a.end))
            .then(a.summary.cmp(&b.summary))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn ev(text: &str) -> Event {
        Event::parse(text).unwrap()
    }

    #[test]
    fn store_items_and_removal() {
        let mut s = Store::default();
        s.upsert(
            "c1",
            vec![
                ev("BEGIN:VEVENT\nUID:a\nDTSTART:20261113T090000Z\nDTEND:20261113T100000Z\nSUMMARY:A\nEND:VEVENT\n"),
                ev("BEGIN:VEVENT\nUID:r\nDTSTART:20261110T080000Z\nDTEND:20261110T083000Z\nRRULE:FREQ=DAILY;COUNT=3\nSUMMARY:R\nEND:VEVENT\n"),
                ev("BEGIN:VEVENT\nUID:d\nDTSTART;VALUE=DATE:20261113\nSUMMARY:Holiday\nEND:VEVENT\n"),
            ],
        );
        s.upsert("c2", vec![ev("BEGIN:VEVENT\nUID:b\nDTSTART:20261113T070000Z\nDTEND:20261113T080000Z\nSUMMARY:B\nEND:VEVENT\n")]);
        let z = Zones::new(Tz::UTC);
        let items = s.items(dt("2026-11-01 00:00"), dt("2026-12-01 00:00"), &z, |_| true);
        assert_eq!(items.len(), 6);
        assert_eq!(items[0].summary, "Holiday", "all-day first");
        assert!(items.iter().filter(|i| i.uid == "r").all(|i| i.recurring));
        let only_c1 = s.items(dt("2026-11-01 00:00"), dt("2026-12-01 00:00"), &z, |c| c == "c1");
        assert_eq!(only_c1.len(), 5);
        let day: Vec<&Item> = items.iter().filter(|i| i.on_day(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap())).collect();
        assert_eq!(day.len(), 3);

        s.remove("c1", &[("r".into(), String::new())]);
        assert_eq!(s.items(dt("2026-11-01 00:00"), dt("2026-12-01 00:00"), &z, |_| true).len(), 3);
        s.clear("c2");
        assert!(s.get("c2", &("b".into(), String::new())).is_none());
        assert!(s.master("c1", "a").is_some());
    }

    #[test]
    fn titles() {
        let item = Item {
            calendar: "c".into(),
            component: ("a".into(), String::new()),
            uid: "a".into(),
            rid: String::new(),
            start: dt("2026-11-13 09:00"),
            end: dt("2026-11-13 09:00"),
            all_day: false,
            summary: " ".into(),
            location: String::new(),
            recurring: false,
        };
        assert_eq!(item.title(), "(No title)");
        assert!(item.on_day(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap()));
    }

    #[test]
    fn detached_removal_by_rid() {
        let mut s = Store::default();
        s.upsert(
            "c",
            vec![
                ev("BEGIN:VEVENT\nUID:w\nDTSTART:20261102T080000Z\nDTEND:20261102T090000Z\nRRULE:FREQ=WEEKLY;COUNT=3\nEND:VEVENT\n"),
                ev("BEGIN:VEVENT\nUID:w\nRECURRENCE-ID:20261109T080000Z\nDTSTART:20261110T120000Z\nDTEND:20261110T130000Z\nEND:VEVENT\n"),
            ],
        );
        s.remove("c", &[("w".into(), "20261109T080000Z".into())]);
        assert_eq!(s.calendars["c"].len(), 1);
    }
}
