//! Worker thread owning the EDS connection, opened calendars and their
//! range views. The UI sends [`Cmd`]s and receives [`Msg`]s.

use chrono::{DateTime, Utc};
use helix_eds::client::split_calendar_id;
use helix_eds::ical::{parse_events, resolve_tzid, Event, Time};
use helix_eds::{sexp, Calendar, Eds, ModType, Source, SourceKind, View, ViewEvent, Watch};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

#[derive(Debug)]
pub enum Cmd {
    Resync,
    SetRange(DateTime<Utc>, DateTime<Utc>),
    Create { cal: String, ics: Vec<String> },
    Modify { cal: String, ics: Vec<String>, mod_type: ModType },
    Remove { cal: String, ids: Vec<(String, String)>, mod_type: ModType },
    /// Moves a whole series to another calendar.
    Move { from: String, to: String, ics: Vec<String>, uid: String },
    Search { query: String, tag: u64 },
    WriteSource(Box<Source>),
    CreateSource { uid: String, data: String },
    RemoveSource(Box<Source>),
    Import { cal: String, events: Vec<String>, timezones: Vec<String> },
    Find(String),
    /// Fetch the VTIMEZONE for a TZID the client could not resolve.
    ResolveTimezone(String),
}

#[derive(Debug)]
pub enum Msg {
    Unavailable(String),
    Sources(Vec<Source>),
    Opened { uid: String, writable: bool },
    Closed(String),
    /// A new view generation started for a calendar; drop its events.
    Reset { cal: String, gen: u64 },
    Upsert { cal: String, gen: u64, events: Vec<Event> },
    Removed { cal: String, gen: u64, ids: Vec<(String, String)> },
    Loaded { cal: String, gen: u64 },
    /// A TZID that needs resolving (send back [`Cmd::ResolveTimezone`]).
    TimezoneNeeded(String),
    /// VTIMEZONE text.
    TimezoneData(String),
    SearchResults { tag: u64, results: Vec<(String, Event)> },
    Found(Option<Box<(String, Event)>>),
    Imported(usize),
    Error(String),
}

#[derive(Clone)]
pub struct Backend {
    tx: mpsc::Sender<Cmd>,
}

impl Backend {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Open {
    // Dropped before the calendar closes.
    view: Option<View>,
    cal: Calendar,
}

pub fn start(out: async_channel::Sender<Msg>) -> Backend {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let me = tx.clone();
    std::thread::Builder::new()
        .name("calendar-eds".into())
        .spawn(move || Worker { out, eds: None, watch: None, open: HashMap::new(), range: None, gen: 0, tz_seen: HashSet::new() }.run(rx, me))
        .expect("spawn EDS worker");
    Backend { tx }
}

struct Worker {
    out: async_channel::Sender<Msg>,
    eds: Option<Eds>,
    watch: Option<Watch>,
    open: HashMap<String, Open>,
    range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    gen: u64,
    tz_seen: HashSet<String>,
}

impl Worker {
    fn send(&self, m: Msg) {
        let _ = self.out.send_blocking(m);
    }

    fn run(mut self, rx: mpsc::Receiver<Cmd>, me: mpsc::Sender<Cmd>) {
        let mut pending = Some(Cmd::Resync);
        loop {
            let cmd = match pending.take() {
                Some(c) => c,
                None => match rx.recv() {
                    Ok(c) => c,
                    Err(_) => return,
                },
            };
            if self.eds.is_none() {
                match Eds::session().and_then(|e| e.sources().map(|_| e)) {
                    Ok(e) => {
                        let t = me.clone();
                        self.watch = e
                            .watch_sources(move || {
                                let _ = t.send(Cmd::Resync);
                            })
                            .ok();
                        self.eds = Some(e);
                    }
                    Err(e) => {
                        self.send(Msg::Unavailable(e.to_string()));
                        if let Cmd::SetRange(a, b) = cmd {
                            self.range = Some((a, b));
                        }
                        continue;
                    }
                }
            }
            // Coalesce bursts of the same request.
            let cmd = match cmd {
                Cmd::Resync | Cmd::SetRange(..) => {
                    let mut cur = cmd;
                    let mut resync = matches!(cur, Cmd::Resync);
                    while let Ok(next) = rx.try_recv() {
                        match next {
                            Cmd::Resync => resync = true,
                            Cmd::SetRange(..) => cur = next,
                            other => {
                                pending = Some(other);
                                break;
                            }
                        }
                    }
                    if let Cmd::SetRange(a, b) = cur {
                        self.range = Some((a, b));
                        if !resync {
                            self.restart_views();
                            continue;
                        }
                    }
                    Cmd::Resync
                }
                c => c,
            };
            self.handle(cmd);
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        let Some(eds) = self.eds.clone() else { return };
        match cmd {
            Cmd::Resync => self.sync(&eds),
            Cmd::SetRange(..) => {}
            Cmd::Create { cal, ics } => {
                if let Some(o) = self.open.get(&cal) {
                    if let Err(e) = o.cal.create(&ics) {
                        self.send(Msg::Error(format!("Could not create event: {e}")));
                    }
                } else {
                    self.with_temp(&eds, &cal, |c| c.create(&ics).map(|_| ()), "Could not create event");
                }
            }
            Cmd::Modify { cal, ics, mod_type } => {
                if let Some(o) = self.open.get(&cal) {
                    if let Err(e) = o.cal.modify(&ics, mod_type) {
                        self.send(Msg::Error(format!("Could not save event: {e}")));
                    }
                }
            }
            Cmd::Remove { cal, ids, mod_type } => {
                if let Some(o) = self.open.get(&cal) {
                    if let Err(e) = o.cal.remove(&ids, mod_type) {
                        self.send(Msg::Error(format!("Could not delete event: {e}")));
                    }
                }
            }
            Cmd::Move { from, to, ics, uid } => {
                let created = match self.open.get(&to) {
                    Some(o) => o.cal.create(&ics).map(|_| ()),
                    None => eds.open_calendar(&to).and_then(|c| c.create(&ics).map(|_| ())),
                };
                match created {
                    Ok(()) => {
                        if let Some(o) = self.open.get(&from) {
                            if let Err(e) = o.cal.remove(&[(uid, String::new())], ModType::All) {
                                self.send(Msg::Error(format!("Event copied but not removed from the old calendar: {e}")));
                            }
                        }
                    }
                    Err(e) => self.send(Msg::Error(format!("Could not move event: {e}"))),
                }
            }
            Cmd::Search { query, tag } => {
                let q = sexp::events_containing(&query);
                let mut results = Vec::new();
                for (uid, o) in &self.open {
                    if let Ok(evs) = o.cal.events(&q) {
                        results.extend(evs.into_iter().map(|e| (uid.clone(), e)));
                    }
                }
                results.sort_by_key(|(_, e)| e.start.naive());
                self.send(Msg::SearchResults { tag, results });
            }
            Cmd::ResolveTimezone(tzid) => {
                if self.tz_seen.insert(tzid.clone()) {
                    if let Some(text) = self.timezone(&tzid) {
                        self.send(Msg::TimezoneData(text));
                    }
                }
            }
            Cmd::Find(uid) => {
                let mut found = None;
                for (cal, o) in &self.open {
                    if let Ok(text) = o.cal.object(&uid, "") {
                        if let Some(e) = parse_events(&text).iter().find_map(Event::from_component) {
                            found = Some(Box::new((cal.clone(), e)));
                            break;
                        }
                    }
                }
                self.send(Msg::Found(found));
            }
            Cmd::WriteSource(s) => {
                if let Err(e) = eds.write_source(&s) {
                    self.send(Msg::Error(format!("Could not update calendar: {e}")));
                }
            }
            Cmd::CreateSource { uid, data } => {
                if let Err(e) = eds.create_source(&uid, &data) {
                    self.send(Msg::Error(format!("Could not create calendar: {e}")));
                }
            }
            Cmd::RemoveSource(s) => {
                self.open.remove(&s.uid);
                if let Err(e) = eds.remove_source(&s) {
                    self.send(Msg::Error(format!("Could not remove calendar: {e}")));
                }
            }
            Cmd::Import { cal, events, timezones } => {
                let n = events.len();
                let mut ok = 0;
                let mut result = |c: &Calendar| {
                    for tz in &timezones {
                        let _ = c.add_timezone(tz);
                    }
                    for ics in &events {
                        let attempt = c.create(std::slice::from_ref(ics));
                        let attempt = match attempt {
                            Ok(_) => Ok(()),
                            Err(_) => {
                                // Probably an existing UID: import as a copy.
                                let mut ev = Event::parse(ics).ok_or_else(|| helix_eds::Error::Invalid("bad event".into()))?;
                                ev.uid = helix_eds::new_id();
                                c.create(&[ev.to_ics(Utc::now())]).map(|_| ())
                            }
                        };
                        if attempt.is_ok() {
                            ok += 1;
                        }
                    }
                    Ok::<(), helix_eds::Error>(())
                };
                let r = match self.open.get(&cal) {
                    Some(o) => result(&o.cal),
                    None => eds.open_calendar(&cal).and_then(|c| result(&c)),
                };
                match r {
                    Ok(()) if ok == n => self.send(Msg::Imported(ok)),
                    Ok(()) => self.send(Msg::Error(format!("Imported {ok} of {n} events"))),
                    Err(e) => self.send(Msg::Error(format!("Import failed: {e}"))),
                }
            }
        }
    }

    fn with_temp(&self, eds: &Eds, cal: &str, f: impl FnOnce(&Calendar) -> helix_eds::Result<()>, what: &str) {
        if let Err(e) = eds.open_calendar(cal).and_then(|c| f(&c)) {
            self.send(Msg::Error(format!("{what}: {e}")));
        }
    }

    fn sync(&mut self, eds: &Eds) {
        let sources = match eds.sources() {
            Ok(s) => s,
            Err(e) => {
                self.send(Msg::Error(e.to_string()));
                return;
            }
        };
        let parent_enabled =
            |s: &Source| s.parent().and_then(|p| sources.iter().find(|x| x.uid == p)).map(|p| p.enabled()).unwrap_or(true);
        let wanted: Vec<Source> = sources
            .iter()
            .filter(|s| s.is(SourceKind::Calendar) && s.enabled() && s.selected() && parent_enabled(s))
            .cloned()
            .collect();
        self.send(Msg::Sources(sources.clone()));
        let stale: Vec<String> = self.open.keys().filter(|k| !wanted.iter().any(|s| &s.uid == *k)).cloned().collect();
        for uid in stale {
            self.open.remove(&uid);
            self.send(Msg::Closed(uid));
        }
        for s in wanted {
            if self.open.contains_key(&s.uid) {
                continue;
            }
            match eds.open_calendar(&s.uid) {
                Ok(cal) => {
                    self.send(Msg::Opened { uid: s.uid.clone(), writable: cal.writable });
                    self.open.insert(s.uid.clone(), Open { view: None, cal });
                    self.start_view(&s.uid);
                }
                Err(e) => self.send(Msg::Error(format!("Could not open “{}”: {e}", s.display_name()))),
            }
        }
    }

    fn restart_views(&mut self) {
        let uids: Vec<String> = self.open.keys().cloned().collect();
        for u in uids {
            self.start_view(&u);
        }
    }

    fn start_view(&mut self, uid: &str) {
        let Some((from, to)) = self.range else { return };
        self.gen += 1;
        let gen = self.gen;
        let Some(open) = self.open.get_mut(uid) else { return };
        open.view = None;
        let _ = self.out.send_blocking(Msg::Reset { cal: uid.to_string(), gen });
        let tx = self.out.clone();
        let cal = uid.to_string();
        let mut seen_tz: HashSet<String> = self.tz_seen.clone();
        let view = open.cal.view(&sexp::occur_in_range(from, to), move |ev| {
            let parse = |v: Vec<String>| -> Vec<Event> {
                v.iter().flat_map(|o| parse_events(o)).filter_map(|c| Event::from_component(&c)).collect()
            };
            let m = match ev {
                ViewEvent::Added(v) | ViewEvent::Modified(v) => {
                    let events = parse(v);
                    for e in &events {
                        for t in [&e.start, &e.end] {
                            if let Time::Zoned(_, tz) = t {
                                if resolve_tzid(tz).is_none() && seen_tz.insert(tz.clone()) {
                                    let _ = tx.send_blocking(Msg::TimezoneNeeded(tz.clone()));
                                }
                            }
                        }
                    }
                    Msg::Upsert { cal: cal.clone(), gen, events }
                }
                ViewEvent::Removed(ids) => {
                    Msg::Removed { cal: cal.clone(), gen, ids: ids.iter().map(|i| split_calendar_id(i)).collect() }
                }
                ViewEvent::Complete(_) => Msg::Loaded { cal: cal.clone(), gen },
                ViewEvent::Progress(..) => return,
            };
            let _ = tx.send_blocking(m);
        });
        match view {
            Ok(v) => open.view = Some(v),
            Err(e) => {
                let _ = self.out.send_blocking(Msg::Error(format!("Could not read calendar: {e}")));
            }
        }
    }

    /// VTIMEZONE text for a TZID from any open calendar.
    fn timezone(&self, tzid: &str) -> Option<String> {
        self.open.values().find_map(|o| o.cal.timezone(tzid).ok())
    }
}
