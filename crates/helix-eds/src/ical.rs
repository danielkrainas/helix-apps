//! iCalendar (RFC 5545) components, a typed [`Event`] view, time-zone
//! handling and recurrence expansion into displayable [`Instance`]s.

use crate::contentline::{self, fold, Property};
use crate::rrule::{RRule, Until};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub props: Vec<Property>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Component {
        Component { name: name.to_ascii_uppercase(), ..Default::default() }
    }

    pub fn get(&self, name: &str) -> Option<&Property> {
        self.props.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn get_text(&self, name: &str) -> Option<String> {
        self.get(name).map(|p| p.text_value())
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.props.iter().filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn remove_all(&mut self, name: &str) {
        self.props.retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    pub fn set(&mut self, prop: Property) {
        self.remove_all(&prop.name.clone());
        self.props.push(prop);
    }

    pub fn set_text(&mut self, name: &str, value: &str) {
        if value.is_empty() {
            self.remove_all(name);
        } else {
            self.set(Property::text(name, value));
        }
    }

    /// Serializes with CRLF line endings and folding.
    pub fn to_ics(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        out.push_str("BEGIN:");
        out.push_str(&self.name);
        out.push_str("\r\n");
        for p in &self.props {
            out.push_str(&fold(&p.to_line()));
            out.push_str("\r\n");
        }
        for c in &self.children {
            c.write(out);
        }
        out.push_str("END:");
        out.push_str(&self.name);
        out.push_str("\r\n");
    }

    /// All descendants (and self) named `name`.
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Component>) {
        if self.name.eq_ignore_ascii_case(name) {
            out.push(self);
        }
        for c in &self.children {
            c.find_all(name, out);
        }
    }
}

/// Parses all top-level components in `text`.
pub fn parse(text: &str) -> Vec<Component> {
    let mut stack: Vec<Component> = Vec::new();
    let mut out = Vec::new();
    for line in contentline::unfold(text) {
        let Some(p) = contentline::parse_line(&line) else { continue };
        if p.name == "BEGIN" {
            stack.push(Component::new(p.value.trim()));
        } else if p.name == "END" {
            if let Some(c) = stack.pop() {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(c),
                    None => out.push(c),
                }
            }
        } else if let Some(cur) = stack.last_mut() {
            cur.props.push(p);
        }
    }
    out
}

/// All VEVENT components in `text`, whether bare or inside a VCALENDAR.
pub fn parse_events(text: &str) -> Vec<Component> {
    let mut found = Vec::new();
    let comps = parse(text);
    for c in &comps {
        c.find_all("VEVENT", &mut found);
    }
    found.into_iter().cloned().collect()
}

/// A DATE or DATE-TIME value with its zone semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Time {
    Date(NaiveDate),
    Floating(NaiveDateTime),
    Utc(NaiveDateTime),
    Zoned(NaiveDateTime, String),
}

impl Time {
    pub fn from_property(p: &Property) -> Option<Time> {
        let v = p.value.trim();
        let v = v.split(',').next()?;
        if p.param("VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE")) || v.len() == 8 {
            return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(Time::Date);
        }
        let utc = v.ends_with('Z') || v.ends_with('z');
        let ndt = NaiveDateTime::parse_from_str(v.trim_end_matches(['Z', 'z']), "%Y%m%dT%H%M%S").ok()?;
        Some(if utc {
            Time::Utc(ndt)
        } else if let Some(tz) = p.param("TZID") {
            Time::Zoned(ndt, tz.to_string())
        } else {
            Time::Floating(ndt)
        })
    }

    /// Every value of a (possibly multi-valued) EXDATE/RDATE property.
    pub fn list_from_property(p: &Property) -> Vec<Time> {
        p.value
            .split(',')
            .filter_map(|v| {
                let mut single = p.clone();
                single.value = v.to_string();
                Time::from_property(&single)
            })
            .collect()
    }

    pub fn to_property(&self, name: &str) -> Property {
        match self {
            Time::Date(d) => Property::new(name, d.format("%Y%m%d").to_string()).with_param("VALUE", "DATE"),
            Time::Floating(t) => Property::new(name, t.format("%Y%m%dT%H%M%S").to_string()),
            Time::Utc(t) => Property::new(name, t.format("%Y%m%dT%H%M%SZ").to_string()),
            Time::Zoned(t, tz) => Property::new(name, t.format("%Y%m%dT%H%M%S").to_string()).with_param("TZID", tz),
        }
    }

    /// The value as written (what EDS calls the recurrence-id string).
    pub fn value_string(&self) -> String {
        self.to_property("X").value
    }

    pub fn is_date(&self) -> bool {
        matches!(self, Time::Date(_))
    }

    /// Wall-clock value in its own frame.
    pub fn naive(&self) -> NaiveDateTime {
        match self {
            Time::Date(d) => d.and_time(NaiveTime::MIN),
            Time::Floating(t) | Time::Utc(t) | Time::Zoned(t, _) => *t,
        }
    }

    /// Same kind of value with a different wall-clock time.
    pub fn with_naive(&self, t: NaiveDateTime) -> Time {
        match self {
            Time::Date(_) => Time::Date(t.date()),
            Time::Floating(_) => Time::Floating(t),
            Time::Utc(_) => Time::Utc(t),
            Time::Zoned(_, z) => Time::Zoned(t, z.clone()),
        }
    }

    /// Absolute instant. Dates and floating times are taken in `local`.
    pub fn instant(&self, zones: &Zones) -> DateTime<Utc> {
        match self {
            Time::Utc(t) => Utc.from_utc_datetime(t),
            Time::Date(_) | Time::Floating(_) => to_utc(zones.local, self.naive()),
            Time::Zoned(t, tz) => to_utc(zones.resolve(tz), *t),
        }
    }

    /// Wall-clock time in the display zone.
    pub fn local(&self, zones: &Zones) -> NaiveDateTime {
        match self {
            Time::Date(_) | Time::Floating(_) => self.naive(),
            _ => self.instant(zones).with_timezone(&zones.local).naive_local(),
        }
    }
}

/// Converts a wall-clock time in `tz` to UTC; ambiguous times take the
/// earlier instant, times in a DST gap are shifted forward by an hour.
pub fn to_utc(tz: Tz, t: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&t) {
        chrono::LocalResult::Single(d) => d.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(a, _) => a.with_timezone(&Utc),
        chrono::LocalResult::None => tz
            .from_local_datetime(&(t + Duration::hours(1)))
            .earliest()
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&t)),
    }
}

/// Time-zone resolution: the display (local) zone plus a cache from TZID
/// strings to IANA zones.
#[derive(Debug, Clone)]
pub struct Zones {
    pub local: Tz,
    cache: std::cell::RefCell<HashMap<String, Tz>>,
    /// Extra TZID → IANA name hints (e.g. from VTIMEZONE X-LIC-LOCATION).
    hints: HashMap<String, String>,
}

impl Zones {
    pub fn new(local: Tz) -> Zones {
        Zones { local, cache: Default::default(), hints: HashMap::new() }
    }

    /// Uses the system zone (`TZ`, then `/etc/localtime`), falling back to UTC.
    pub fn system() -> Zones {
        Zones::new(system_zone())
    }

    /// Learns TZID mappings from VTIMEZONE components.
    pub fn add_vtimezones(&mut self, comps: &[Component]) {
        let mut found = Vec::new();
        for c in comps {
            c.find_all("VTIMEZONE", &mut found);
        }
        for tz in found {
            if let (Some(id), Some(loc)) = (tz.get_text("TZID"), tz.get_text("X-LIC-LOCATION")) {
                self.hints.insert(id, loc);
            }
        }
    }

    pub fn resolve(&self, tzid: &str) -> Tz {
        if let Some(tz) = self.cache.borrow().get(tzid) {
            return *tz;
        }
        let tz = self
            .hints
            .get(tzid)
            .and_then(|h| resolve_tzid(h))
            .or_else(|| resolve_tzid(tzid))
            .unwrap_or(self.local);
        self.cache.borrow_mut().insert(tzid.to_string(), tz);
        tz
    }

    /// IANA name of the display zone (used as TZID for new events).
    pub fn local_name(&self) -> &'static str {
        self.local.name()
    }
}

pub fn system_zone() -> Tz {
    if let Ok(tz) = std::env::var("TZ") {
        if let Some(z) = resolve_tzid(tz.trim_start_matches(':')) {
            return z;
        }
    }
    iana_time_zone::get_timezone().ok().and_then(|n| Tz::from_str(&n).ok()).unwrap_or(Tz::UTC)
}

/// Maps TZID strings to IANA zones. Handles plain names and libical-style
/// prefixed ids (`/freeassociation.sourceforge.net/Tzfile/Europe/Berlin`).
pub fn resolve_tzid(tzid: &str) -> Option<Tz> {
    let t = tzid.trim().trim_matches('"');
    if let Ok(z) = Tz::from_str(t) {
        return Some(z);
    }
    let parts: Vec<&str> = t.split('/').filter(|p| !p.is_empty()).collect();
    for i in 1..parts.len() {
        if let Ok(z) = Tz::from_str(&parts[i..].join("/")) {
            return Some(z);
        }
    }
    match t {
        "GMT" | "UTC" | "Z" => Some(Tz::UTC),
        _ => None,
    }
}

/// Parses an iCalendar DURATION (`-PT15M`, `P1D`, `P1W`, `PT1H30M`).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    for c in s.chars() {
        match c {
            '0'..='9' => num.push(c),
            'T' => in_time = true,
            'W' | 'D' | 'H' | 'M' | 'S' => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (c, in_time) {
                    ('W', _) => Duration::weeks(n),
                    ('D', _) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => return None,
                };
            }
            _ => return None,
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(if neg { -total } else { total })
}

pub fn format_duration(d: Duration) -> String {
    let neg = d < Duration::zero();
    let mut secs = d.num_seconds().abs();
    let mut out = String::from(if neg { "-P" } else { "P" });
    if secs == 0 {
        return "PT0S".into();
    }
    if secs % (7 * 86400) == 0 {
        out.push_str(&format!("{}W", secs / (7 * 86400)));
        return out;
    }
    let days = secs / 86400;
    secs %= 86400;
    if days > 0 {
        out.push_str(&format!("{days}D"));
    }
    if secs > 0 {
        out.push('T');
        let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
        if h > 0 {
            out.push_str(&format!("{h}H"));
        }
        if m > 0 {
            out.push_str(&format!("{m}M"));
        }
        if s > 0 {
            out.push_str(&format!("{s}S"));
        }
    }
    out
}

/// A reminder. `before_start` is how long before the event start it fires
/// (negative = after); `None` for absolute or end-relative triggers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alarm {
    pub before_start: Option<Duration>,
    raw: Option<Component>,
}

impl Alarm {
    pub fn before(d: Duration) -> Alarm {
        Alarm { before_start: Some(d), raw: None }
    }

    fn from_component(c: &Component) -> Alarm {
        let before_start = c.get("TRIGGER").and_then(|t| {
            let related_end = t.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END"));
            let is_dt = t.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"));
            if related_end || is_dt {
                None
            } else {
                parse_duration(&t.value).map(|d| -d)
            }
        });
        Alarm { before_start, raw: Some(c.clone()) }
    }

    fn to_component(&self, summary: &str) -> Component {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut c = Component::new("VALARM");
        c.props.push(Property::new("ACTION", "DISPLAY"));
        let d = self.before_start.unwrap_or_else(Duration::zero);
        c.props.push(Property::new("TRIGGER", format_duration(-d)).with_param("RELATED", "START"));
        c.props.push(Property::text("DESCRIPTION", if summary.is_empty() { "Reminder" } else { summary }));
        c
    }
}

/// A VEVENT. Unmanaged properties and sub-components are preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub uid: String,
    pub recurrence_id: Option<Time>,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub url: String,
    pub start: Time,
    /// Exclusive end. Derived from DTEND or DURATION when absent.
    pub end: Time,
    pub rrule: Option<RRule>,
    pub exdates: Vec<Time>,
    pub rdates: Vec<Time>,
    pub alarms: Vec<Alarm>,
    pub status: String,
    pub sequence: i64,
    pub color: String,
    pub organizer: String,
    pub attendees: Vec<String>,
    pub raw: Component,
}

const MANAGED: &[&str] = &[
    "UID", "RECURRENCE-ID", "SUMMARY", "DESCRIPTION", "LOCATION", "URL", "DTSTART", "DTEND", "DURATION", "RRULE",
    "EXDATE", "RDATE", "STATUS", "SEQUENCE", "COLOR", "DTSTAMP", "LAST-MODIFIED",
];

impl Event {
    /// A new event with a fresh UID.
    pub fn new(start: Time, end: Time) -> Event {
        Event {
            uid: crate::new_id(),
            recurrence_id: None,
            summary: String::new(),
            description: String::new(),
            location: String::new(),
            url: String::new(),
            start,
            end,
            rrule: None,
            exdates: Vec::new(),
            rdates: Vec::new(),
            alarms: Vec::new(),
            status: String::new(),
            sequence: 0,
            color: String::new(),
            organizer: String::new(),
            attendees: Vec::new(),
            raw: Component::new("VEVENT"),
        }
    }

    pub fn from_component(c: &Component) -> Option<Event> {
        let start = Time::from_property(c.get("DTSTART")?)?;
        let end = c
            .get("DTEND")
            .and_then(Time::from_property)
            .or_else(|| {
                let d = parse_duration(&c.get("DURATION")?.value)?;
                Some(start.with_naive(start.naive() + d))
            })
            .unwrap_or_else(|| match &start {
                Time::Date(d) => Time::Date(*d + Duration::days(1)),
                other => other.clone(),
            });
        Some(Event {
            uid: c.get_text("UID").unwrap_or_default(),
            recurrence_id: c.get("RECURRENCE-ID").and_then(Time::from_property),
            summary: c.get_text("SUMMARY").unwrap_or_default(),
            description: c.get_text("DESCRIPTION").unwrap_or_default(),
            location: c.get_text("LOCATION").unwrap_or_default(),
            url: c.get("URL").map(|p| p.value.clone()).unwrap_or_default(),
            start,
            end,
            rrule: c.get("RRULE").and_then(|p| RRule::parse(&p.value)),
            exdates: c.all("EXDATE").flat_map(Time::list_from_property).collect(),
            rdates: c.all("RDATE").flat_map(Time::list_from_property).collect(),
            alarms: c.children.iter().filter(|a| a.name == "VALARM").map(Alarm::from_component).collect(),
            status: c.get("STATUS").map(|p| p.value.to_ascii_uppercase()).unwrap_or_default(),
            sequence: c.get("SEQUENCE").and_then(|p| p.value.trim().parse().ok()).unwrap_or(0),
            color: c.get("COLOR").map(|p| p.value.clone()).unwrap_or_default(),
            organizer: c.get("ORGANIZER").map(|p| p.param("CN").unwrap_or(&p.value).to_string()).unwrap_or_default(),
            attendees: c.all("ATTENDEE").map(|p| p.param("CN").unwrap_or(&p.value).to_string()).collect(),
            raw: c.clone(),
        })
    }

    pub fn parse(text: &str) -> Option<Event> {
        parse_events(text).first().and_then(Event::from_component)
    }

    pub fn all_day(&self) -> bool {
        self.start.is_date()
    }

    /// Builds the VEVENT, stamping DTSTAMP / LAST-MODIFIED with `now`.
    pub fn to_component(&self, now: DateTime<Utc>) -> Component {
        let mut c = Component::new("VEVENT");
        let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        c.props.push(Property::text("UID", &self.uid));
        c.props.push(Property::new("DTSTAMP", stamp.clone()));
        if let Some(r) = &self.recurrence_id {
            c.props.push(r.to_property("RECURRENCE-ID"));
        }
        c.props.push(self.start.to_property("DTSTART"));
        c.props.push(self.end.to_property("DTEND"));
        for (name, v) in [("SUMMARY", &self.summary), ("DESCRIPTION", &self.description), ("LOCATION", &self.location)] {
            if !v.is_empty() {
                c.props.push(Property::text(name, v));
            }
        }
        if !self.url.is_empty() {
            c.props.push(Property::new("URL", self.url.clone()));
        }
        if let Some(r) = &self.rrule {
            c.props.push(Property::new("RRULE", r.to_string()));
        }
        for x in &self.exdates {
            c.props.push(x.to_property("EXDATE"));
        }
        for x in &self.rdates {
            c.props.push(x.to_property("RDATE"));
        }
        if !self.status.is_empty() {
            c.props.push(Property::new("STATUS", self.status.clone()));
        }
        if self.sequence > 0 {
            c.props.push(Property::new("SEQUENCE", self.sequence.to_string()));
        }
        if !self.color.is_empty() {
            c.props.push(Property::new("COLOR", self.color.clone()));
        }
        c.props.push(Property::new("LAST-MODIFIED", stamp));
        c.props.extend(self.raw.props.iter().filter(|p| !MANAGED.contains(&p.name.as_str())).cloned());
        c.children.extend(self.raw.children.iter().filter(|x| x.name != "VALARM").cloned());
        c.children.extend(self.alarms.iter().map(|a| a.to_component(&self.summary)));
        c
    }

    pub fn to_ics(&self, now: DateTime<Utc>) -> String {
        self.to_component(now).to_ics()
    }

    pub fn duration(&self) -> Duration {
        let d = self.end.naive() - self.start.naive();
        if d < Duration::zero() {
            Duration::zero()
        } else {
            d
        }
    }

    pub fn is_recurring(&self) -> bool {
        self.rrule.is_some() || !self.rdates.is_empty()
    }

    /// Unique key for a component: UID plus recurrence-id value.
    pub fn key(&self) -> (String, String) {
        (self.uid.clone(), self.recurrence_id.as_ref().map(Time::value_string).unwrap_or_default())
    }
}

/// One displayed occurrence of an event, in display-zone wall-clock time.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub uid: String,
    /// Recurrence-id string for this occurrence (empty for non-recurring).
    pub rid: String,
    pub start: NaiveDateTime,
    /// Exclusive end.
    pub end: NaiveDateTime,
    pub all_day: bool,
    /// Index into the event slice passed to [`expand`].
    pub event: usize,
}

impl Instance {
    /// Whether the instance touches the given day.
    pub fn on_day(&self, day: NaiveDate) -> bool {
        let ds = day.and_time(NaiveTime::MIN);
        let de = ds + Duration::days(1);
        if self.start == self.end {
            return self.start >= ds && self.start < de;
        }
        self.start < de && self.end > ds
    }

    /// Multi-day or all-day instances are drawn in the all-day area.
    pub fn spans_days(&self) -> bool {
        self.all_day || self.end - self.start >= Duration::days(1)
    }
}

/// Expands events (masters plus detached recurrence overrides) into
/// instances overlapping `[from, to)` (display-zone wall clock).
pub fn expand(events: &[Event], from: NaiveDateTime, to: NaiveDateTime, zones: &Zones) -> Vec<Instance> {
    let mut out = Vec::new();
    // Overrides keyed by (uid, instant of recurrence id).
    let mut overrides: HashMap<(String, i64), usize> = HashMap::new();
    for (i, e) in events.iter().enumerate() {
        if let Some(r) = &e.recurrence_id {
            overrides.insert((e.uid.clone(), r.instant(zones).timestamp()), i);
        }
    }
    let masters: std::collections::HashSet<&str> =
        events.iter().filter(|e| e.recurrence_id.is_none()).map(|e| e.uid.as_str()).collect();
    for (i, e) in events.iter().enumerate() {
        if e.recurrence_id.is_some() {
            // Overrides are placed via their master; orphans stand alone.
            if !masters.contains(e.uid.as_str()) {
                push_instance(&mut out, e, i, &e.start, e.recurrence_id.as_ref(), from, to, zones);
            }
            continue;
        }
        if !e.is_recurring() {
            push_instance(&mut out, e, i, &e.start, None, from, to, zones);
            continue;
        }
        let dur = e.duration();
        // Generate in the event's frame with a 2-day margin for zone offsets.
        let horizon = to + Duration::days(2);
        let until = e.rrule.as_ref().and_then(|r| r.until).map(|u| match u {
            Until::Date(d) => d.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap()),
            Until::DateTime { value, utc: true } => match &e.start {
                Time::Zoned(_, tz) => zones.resolve(tz).from_utc_datetime(&value).naive_local(),
                Time::Utc(_) => value,
                _ => zones.local.from_utc_datetime(&value).naive_local(),
            },
            Until::DateTime { value, utc: false } => value,
        });
        let mut starts = match &e.rrule {
            Some(r) => r.occurrences(e.start.naive(), until, horizon, 20_000),
            None => vec![e.start.naive()],
        };
        for rd in &e.rdates {
            let local = e.start.with_naive(rd.naive());
            starts.push(match rd {
                Time::Utc(_) | Time::Zoned(..) => match &e.start {
                    Time::Zoned(_, tz) => {
                        rd.instant(zones).with_timezone(&zones.resolve(tz)).naive_local()
                    }
                    _ => local.naive(),
                },
                _ => rd.naive(),
            });
        }
        starts.sort();
        starts.dedup();
        let excluded: std::collections::HashSet<i64> = e
            .exdates
            .iter()
            .map(|x| match (x, &e.start) {
                // Date exclusions apply to the whole day of an occurrence.
                (Time::Date(d), _) => e.start.with_naive(d.and_time(e.start.naive().time())).instant(zones).timestamp(),
                _ => x.instant(zones).timestamp(),
            })
            .collect();
        for s in starts {
            let occ = e.start.with_naive(s);
            let key = occ.instant(zones).timestamp();
            if excluded.contains(&key) {
                continue;
            }
            if let Some(&oi) = overrides.get(&(e.uid.clone(), key)) {
                let o = &events[oi];
                push_instance(&mut out, o, oi, &o.start, Some(&occ), from, to, zones);
                continue;
            }
            let end = occ.with_naive(s + dur);
            let (ls, le) = (occ.local(zones), end.local(zones));
            if overlaps(ls, le, from, to) {
                out.push(Instance {
                    uid: e.uid.clone(),
                    rid: occ.value_string(),
                    start: ls,
                    end: le,
                    all_day: occ.is_date(),
                    event: i,
                });
            }
        }
    }
    out.sort_by_key(|a| (a.start, std::cmp::Reverse(a.end)));
    out
}

#[allow(clippy::too_many_arguments)]
fn push_instance(
    out: &mut Vec<Instance>,
    e: &Event,
    idx: usize,
    start: &Time,
    rid: Option<&Time>,
    from: NaiveDateTime,
    to: NaiveDateTime,
    zones: &Zones,
) {
    let (ls, le) = (start.local(zones), e.end.local(zones));
    let le = if le < ls { ls } else { le };
    if overlaps(ls, le, from, to) {
        out.push(Instance {
            uid: e.uid.clone(),
            rid: rid.map(Time::value_string).unwrap_or_default(),
            start: ls,
            end: le,
            all_day: start.is_date(),
            event: idx,
        });
    }
}

fn overlaps(s: NaiveDateTime, e: NaiveDateTime, from: NaiveDateTime, to: NaiveDateTime) -> bool {
    if s == e {
        s >= from && s < to
    } else {
        s < to && e > from
    }
}

/// Wraps components in a VCALENDAR (for `.ics` export).
pub fn wrap_calendar(components: &[Component]) -> String {
    let mut cal = Component::new("VCALENDAR");
    cal.props.push(Property::new("VERSION", "2.0"));
    cal.props.push(Property::new("PRODID", "-//Helix//Calendar//EN"));
    cal.children.extend(components.iter().cloned());
    cal.to_ics()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zones() -> Zones {
        Zones::new(chrono_tz::Europe::Berlin)
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    const EV: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:/freeassociation.sourceforge.net/Tzfile/America/New_York\r\nX-LIC-LOCATION:America/New_York\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:abc\r\nDTSTART;TZID=America/New_York:20261102T090000\r\nDTEND;TZID=America/New_York:20261102T100000\r\nSUMMARY:Stand-up\\, daily\r\nLOCATION:Room 1\r\nRRULE:FREQ=DAILY;COUNT=5\r\nEXDATE;TZID=America/New_York:20261104T090000\r\nX-FOO:bar\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;RELATED=START:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parse_event_fields() {
        let e = Event::parse(EV).unwrap();
        assert_eq!(e.uid, "abc");
        assert_eq!(e.summary, "Stand-up, daily");
        assert_eq!(e.location, "Room 1");
        assert_eq!(e.start, Time::Zoned(dt("2026-11-02 09:00"), "America/New_York".into()));
        assert_eq!(e.duration(), Duration::hours(1));
        assert_eq!(e.alarms[0].before_start, Some(Duration::minutes(15)));
        assert_eq!(e.exdates.len(), 1);
        assert!(e.is_recurring());
    }

    #[test]
    fn round_trip_keeps_unknown() {
        let e = Event::parse(EV).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 13, 12, 0, 0).unwrap();
        let text = e.to_ics(now);
        assert!(text.contains("X-FOO:bar\r\n"));
        assert!(text.contains("DTSTAMP:20261113T120000Z"));
        let again = Event::parse(&text).unwrap();
        assert_eq!(again.summary, e.summary);
        assert_eq!(again.start, e.start);
        assert_eq!(again.end, e.end);
        assert_eq!(again.rrule, e.rrule);
        assert_eq!(again.exdates, e.exdates);
        assert_eq!(again.alarms.len(), 1);
    }

    #[test]
    fn expand_with_exdate_and_zone_conversion() {
        let e = Event::parse(EV).unwrap();
        let inst = expand(&[e], dt("2026-11-01 00:00"), dt("2026-11-30 00:00"), &zones());
        // 5 occurrences minus the excluded one.
        assert_eq!(inst.len(), 4);
        // 09:00 New York = 15:00 Berlin (both on standard time on Nov 2).
        assert_eq!(inst[0].start, dt("2026-11-02 15:00"));
        assert_eq!(inst[0].end, dt("2026-11-02 16:00"));
        assert_eq!(inst[0].rid, "20261102T090000");
        assert_eq!(inst[1].start.date(), NaiveDate::from_ymd_opt(2026, 11, 3).unwrap());
        assert_eq!(inst[2].start.date(), NaiveDate::from_ymd_opt(2026, 11, 5).unwrap());
    }

    #[test]
    fn overrides_replace_instances() {
        let master = Event::parse(
            "BEGIN:VEVENT\nUID:w\nDTSTART:20261102T080000Z\nDTEND:20261102T090000Z\nRRULE:FREQ=WEEKLY;COUNT=3\nSUMMARY:Weekly\nEND:VEVENT\n",
        )
        .unwrap();
        let moved = Event::parse(
            "BEGIN:VEVENT\nUID:w\nRECURRENCE-ID:20261109T080000Z\nDTSTART:20261110T120000Z\nDTEND:20261110T130000Z\nSUMMARY:Moved\nEND:VEVENT\n",
        )
        .unwrap();
        let z = Zones::new(Tz::UTC);
        let inst = expand(&[master, moved], dt("2026-11-01 00:00"), dt("2026-12-01 00:00"), &z);
        assert_eq!(inst.len(), 3);
        assert_eq!(inst[1].start, dt("2026-11-10 12:00"));
        assert_eq!(inst[1].event, 1);
        assert_eq!(inst[1].rid, "20261109T080000Z");
    }

    #[test]
    fn all_day_events() {
        let e = Event::parse("BEGIN:VEVENT\nUID:b\nDTSTART;VALUE=DATE:20261224\nDTEND;VALUE=DATE:20261227\nEND:VEVENT\n").unwrap();
        assert!(e.all_day());
        let inst = expand(&[e], dt("2026-12-01 00:00"), dt("2027-01-01 00:00"), &zones());
        assert_eq!(inst.len(), 1);
        assert!(inst[0].spans_days());
        assert!(inst[0].on_day(NaiveDate::from_ymd_opt(2026, 12, 26).unwrap()));
        assert!(!inst[0].on_day(NaiveDate::from_ymd_opt(2026, 12, 27).unwrap()));
        // Missing DTEND on a date event lasts one day.
        let e = Event::parse("BEGIN:VEVENT\nUID:c\nDTSTART;VALUE=DATE:20261224\nEND:VEVENT\n").unwrap();
        assert_eq!(e.end, Time::Date(NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()));
    }

    #[test]
    fn yearly_birthday_outside_window_is_skipped() {
        let e = Event::parse("BEGIN:VEVENT\nUID:y\nDTSTART;VALUE=DATE:19900517\nDTEND;VALUE=DATE:19900518\nRRULE:FREQ=YEARLY\nEND:VEVENT\n").unwrap();
        let inst = expand(&[e], dt("2026-05-01 00:00"), dt("2026-06-01 00:00"), &zones());
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].start, dt("2026-05-17 00:00"));
        assert_eq!(inst[0].rid, "19900517".replace("1990", "2026"));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(parse_duration("P1DT2H"), Some(Duration::hours(26)));
        assert_eq!(parse_duration("P2W"), Some(Duration::weeks(2)));
        assert_eq!(parse_duration("PT"), Some(Duration::zero()));
        assert_eq!(parse_duration("15M"), None);
        assert_eq!(format_duration(Duration::minutes(-15)), "-PT15M");
        assert_eq!(format_duration(Duration::hours(26)), "P1DT2H");
        assert_eq!(format_duration(Duration::weeks(1)), "P1W");
        let ev = Event::parse("BEGIN:VEVENT\nUID:d\nDTSTART:20260101T100000Z\nDURATION:PT90M\nEND:VEVENT\n").unwrap();
        assert_eq!(ev.end, Time::Utc(dt("2026-01-01 11:30")));
    }

    #[test]
    fn tzid_resolution() {
        assert_eq!(resolve_tzid("/freeassociation.sourceforge.net/Tzfile/Europe/Paris"), Some(chrono_tz::Europe::Paris));
        assert_eq!(resolve_tzid("/softwarestudio.org/Olson_20011030_5/America/Chicago"), Some(chrono_tz::America::Chicago));
        assert_eq!(resolve_tzid("Nowhere/Special"), None);
        let mut z = zones();
        z.add_vtimezones(&parse(EV));
        assert_eq!(z.resolve("/freeassociation.sourceforge.net/Tzfile/America/New_York"), chrono_tz::America::New_York);
        assert_eq!(z.resolve("Weird Zone"), chrono_tz::Europe::Berlin);
    }

    #[test]
    fn new_alarm_serializes() {
        let mut e = Event::new(Time::Utc(dt("2026-01-01 10:00")), Time::Utc(dt("2026-01-01 11:00")));
        e.summary = "Dentist".into();
        e.alarms.push(Alarm::before(Duration::minutes(30)));
        let text = e.to_ics(Utc::now());
        assert!(text.contains("TRIGGER;RELATED=START:-PT30M"));
        let back = Event::parse(&text).unwrap();
        assert_eq!(back.alarms[0].before_start, Some(Duration::minutes(30)));
        assert_eq!(back.uid.len(), 40);
    }

    #[test]
    fn wrap_has_calendar() {
        let e = Event::parse(EV).unwrap();
        let s = wrap_calendar(&[e.to_component(Utc::now())]);
        assert!(s.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert_eq!(parse_events(&s).len(), 1);
    }
}
