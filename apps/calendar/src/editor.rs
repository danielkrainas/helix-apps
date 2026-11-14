//! Event editor dialog.

use adw::prelude::*;
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};
use gtk::glib;
use helix_app_common::Labelled;
use helix_eds::ical::{Alarm, Event, Time};
use helix_eds::ModType;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::backend::Cmd;
use crate::dates;
use crate::model::CompKey;
use crate::recur::{self, Ends, Preset};
use crate::ui::Ui;

#[derive(Debug, Clone)]
pub enum Target {
    New {
        start: NaiveDateTime,
        end: NaiveDateTime,
        all_day: bool,
    },
    Existing {
        cal: String,
        component: CompKey,
        rid: String,
        start: NaiveDateTime,
        end: NaiveDateTime,
        all_day: bool,
    },
}

type DateCallback = Box<dyn Fn(NaiveDate)>;

/// A date button with a calendar popover.
struct DateButton {
    button: gtk::MenuButton,
    calendar: gtk::Calendar,
    date: Cell<NaiveDate>,
    changed: RefCell<Option<DateCallback>>,
}

impl DateButton {
    fn new(d: NaiveDate) -> Rc<DateButton> {
        let calendar = gtk::Calendar::new();
        let pop = gtk::Popover::builder().child(&calendar).build();
        let button = gtk::MenuButton::builder()
            .popover(&pop)
            .valign(gtk::Align::Center)
            .build();
        let b = Rc::new(DateButton {
            button,
            calendar,
            date: Cell::new(d),
            changed: RefCell::default(),
        });
        b.set(d);
        let weak = Rc::downgrade(&b);
        b.calendar.connect_day_selected(move |c| {
            let Some(b) = weak.upgrade() else { return };
            let g = c.date();
            if let Some(d) =
                NaiveDate::from_ymd_opt(g.year(), g.month() as u32, g.day_of_month() as u32)
            {
                if d != b.date.get() {
                    b.date.set(d);
                    b.button.set_label(&dates::short_date(d));
                    if let Some(f) = b.changed.borrow().as_ref() {
                        f(d);
                    }
                }
            }
            pop.popdown();
        });
        b
    }

    fn set(&self, d: NaiveDate) {
        self.date.set(d);
        self.button.set_label(&format!(
            "{} {}",
            dates::short_date(d),
            chrono::Datelike::year(&d)
        ));
        if let Ok(g) = glib::DateTime::from_local(
            chrono::Datelike::year(&d),
            chrono::Datelike::month(&d) as i32,
            chrono::Datelike::day(&d) as i32,
            0,
            0,
            0.0,
        ) {
            self.calendar.select_day(&g);
        }
    }
}

struct Form {
    dialog: adw::Dialog,
    title: adw::EntryRow,
    cal: adw::ComboRow,
    cal_uids: Vec<String>,
    location: adw::EntryRow,
    all_day: adw::SwitchRow,
    start_date: Rc<DateButton>,
    start_time: gtk::Entry,
    end_date: Rc<DateButton>,
    end_time: gtk::Entry,
    repeat: adw::ComboRow,
    presets: Vec<Preset>,
    ends: adw::ComboRow,
    count: adw::SpinRow,
    until: Rc<DateButton>,
    until_row: adw::ActionRow,
    alarms: RefCell<Vec<Alarm>>,
    alarm_group: adw::PreferencesGroup,
    alarm_rows: RefCell<Vec<adw::ActionRow>>,
    notes: gtk::TextView,
    twelve: bool,
}

struct Values {
    title: String,
    cal: String,
    location: String,
    all_day: bool,
    start: NaiveDateTime,
    /// Exclusive end (next day for all-day events).
    end: NaiveDateTime,
    preset: Preset,
    ends: Ends,
    notes: String,
}

fn time_entry(t: NaiveTime, twelve: bool) -> gtk::Entry {
    gtk::Entry::builder()
        .text(dates::format_time(t, twelve))
        .width_chars(8)
        .max_width_chars(8)
        .valign(gtk::Align::Center)
        .build()
}

impl Form {
    fn read_time(&self, e: &gtk::Entry) -> Result<NaiveTime, String> {
        dates::parse_time_input(&e.text())
            .ok_or_else(|| format!("“{}” is not a valid time", e.text()))
    }

    fn values(&self) -> Result<Values, String> {
        let all_day = self.all_day.is_active();
        let (sd, ed) = (self.start_date.date.get(), self.end_date.date.get());
        let (start, end) = if all_day {
            if ed < sd {
                return Err("The event ends before it starts".into());
            }
            (
                sd.and_time(NaiveTime::MIN),
                (ed + Duration::days(1)).and_time(NaiveTime::MIN),
            )
        } else {
            let s = sd.and_time(self.read_time(&self.start_time)?);
            let e = ed.and_time(self.read_time(&self.end_time)?);
            if e < s {
                return Err("The event ends before it starts".into());
            }
            (s, e)
        };
        let preset = self
            .presets
            .get(self.repeat.selected() as usize)
            .copied()
            .unwrap_or(Preset::None);
        let ends = match self.ends.selected() {
            1 => Ends::Count(self.count.value() as u32),
            2 => Ends::Until(self.until.date.get()),
            _ => Ends::Forever,
        };
        let buf = self.notes.buffer();
        Ok(Values {
            title: self.title.text().trim().to_string(),
            cal: self
                .cal_uids
                .get(self.cal.selected() as usize)
                .cloned()
                .unwrap_or_default(),
            location: self.location.text().trim().to_string(),
            all_day,
            start,
            end,
            preset,
            ends,
            notes: buf
                .text(&buf.start_iter(), &buf.end_iter(), false)
                .trim_end()
                .to_string(),
        })
    }

    fn update_visibility(&self) {
        let all_day = self.all_day.is_active();
        self.start_time.set_visible(!all_day);
        self.end_time.set_visible(!all_day);
        let repeating = self
            .presets
            .get(self.repeat.selected() as usize)
            .is_some_and(|p| *p != Preset::None);
        self.ends.set_visible(repeating);
        self.count
            .set_visible(repeating && self.ends.selected() == 1);
        self.until_row
            .set_visible(repeating && self.ends.selected() == 2);
    }

    fn add_alarm_row(self: &Rc<Self>, alarm: Alarm) {
        let row = adw::ActionRow::builder()
            .title(recur::describe_reminder(alarm.before_start))
            .build();
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove Reminder")
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .build()
            .labelled("Remove Reminder");
        row.add_suffix(&remove);
        self.alarm_group.add(&row);
        self.alarms.borrow_mut().push(alarm);
        self.alarm_rows.borrow_mut().push(row.clone());
        let weak = Rc::downgrade(self);
        remove.connect_clicked(move |_| {
            let Some(f) = weak.upgrade() else { return };
            let idx = f.alarm_rows.borrow().iter().position(|r| *r == row);
            if let Some(i) = idx {
                f.alarm_rows.borrow_mut().remove(i);
                f.alarms.borrow_mut().remove(i);
                f.alarm_group.remove(&row);
            }
        });
    }
}

fn make_time(t: NaiveDateTime, all_day: bool, zone: &str) -> Time {
    if all_day {
        Time::Date(t.date())
    } else if zone == "UTC" {
        Time::Utc(t)
    } else {
        Time::Zoned(t, zone.to_string())
    }
}

/// Parses a recurrence-id string in the frame of `like`.
fn rid_time(rid: &str, like: &Time) -> Option<Time> {
    if rid.len() == 8 {
        return NaiveDate::parse_from_str(rid, "%Y%m%d")
            .ok()
            .map(Time::Date);
    }
    let utc = rid.ends_with('Z');
    let n = NaiveDateTime::parse_from_str(rid.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok()?;
    Some(if utc {
        Time::Utc(n)
    } else {
        like.with_naive(n)
    })
}

pub fn open(ui: &Rc<Ui>, target: Target) {
    let (start, end, all_day) = match &target {
        Target::New {
            start,
            end,
            all_day,
        } => (*start, *end, *all_day),
        Target::Existing {
            start,
            end,
            all_day,
            ..
        } => (*start, *end, *all_day),
    };
    let existing: Option<(String, Event, Option<Event>)> = match &target {
        Target::Existing { cal, component, .. } => {
            let st = ui.state.borrow();
            let comp = st.store.get(cal, component).cloned();
            let master = st.store.master(cal, &component.0).cloned();
            comp.map(|c| (cal.clone(), c, master))
        }
        Target::New { .. } => None,
    };
    if matches!(target, Target::Existing { .. }) && existing.is_none() {
        ui.toast("The event is no longer available");
        return;
    }
    glib::g_debug!(
        "helix-calendar",
        "editor for {}",
        existing
            .as_ref()
            .map(|(c, e, _)| format!("{c}/{} “{}”", e.uid, e.summary))
            .unwrap_or_else(|| "new event".into())
    );
    let cals = ui.calendar_choices();
    let writable = match &existing {
        Some((cal, ..)) => ui
            .state
            .borrow()
            .writable
            .get(cal)
            .copied()
            .unwrap_or(false),
        None => true,
    };
    let selectable: Vec<(String, String)> = cals
        .iter()
        .filter(|(uid, _, w)| *w || existing.as_ref().is_some_and(|(c, ..)| c == uid))
        .map(|(u, n, _)| (u.clone(), n.clone()))
        .collect();
    if selectable.is_empty() {
        ui.toast("No writable calendar is available");
        return;
    }
    let event = existing.as_ref().map(|(_, e, _)| e.clone());
    let master = existing.as_ref().and_then(|(_, _, m)| m.clone());
    let series_rule = master
        .as_ref()
        .and_then(|m| m.rrule.clone())
        .or_else(|| event.as_ref().and_then(|e| e.rrule.clone()));
    let recurring = master.as_ref().is_some_and(|m| m.is_recurring())
        || event.as_ref().is_some_and(|e| e.recurrence_id.is_some());
    let twelve = ui.twelve();

    let dialog = adw::Dialog::builder()
        .title(if existing.is_some() {
            "Edit Event"
        } else {
            "New Event"
        })
        .content_width(460)
        .content_height(640)
        .build();
    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder()
        .label(if existing.is_some() { "Save" } else { "Add" })
        .css_classes(["suggested-action"])
        .build();
    save.set_sensitive(writable);
    header.pack_start(&cancel);
    header.pack_end(&save);
    let page = adw::PreferencesPage::new();

    let main = adw::PreferencesGroup::new();
    let title = adw::EntryRow::builder()
        .title("Title")
        .text(event.as_ref().map(|e| e.summary.as_str()).unwrap_or(""))
        .build();
    let names: Vec<&str> = selectable.iter().map(|(_, n)| n.as_str()).collect();
    let cal = adw::ComboRow::builder()
        .title("Calendar")
        .model(&gtk::StringList::new(&names))
        .build();
    let cur_cal = existing
        .as_ref()
        .map(|(c, ..)| c.clone())
        .unwrap_or_else(|| ui.default_calendar());
    cal.set_selected(
        selectable
            .iter()
            .position(|(u, _)| *u == cur_cal)
            .unwrap_or(0) as u32,
    );
    let location = adw::EntryRow::builder()
        .title("Location")
        .text(event.as_ref().map(|e| e.location.as_str()).unwrap_or(""))
        .build();
    main.add(&title);
    main.add(&cal);
    main.add(&location);
    page.add(&main);

    let sched = adw::PreferencesGroup::builder().title("Schedule").build();
    let all_day_row = adw::SwitchRow::builder()
        .title("All Day")
        .active(all_day)
        .build();
    let end_display = if all_day {
        (end - Duration::days(1)).date().max(start.date())
    } else {
        end.date()
    };
    let start_date = DateButton::new(start.date());
    let end_date = DateButton::new(end_display);
    let start_time = time_entry(start.time(), twelve);
    let end_time = time_entry(end.time(), twelve);
    let start_row = adw::ActionRow::builder().title("Starts").build();
    start_row.add_suffix(&start_date.button);
    start_row.add_suffix(&start_time);
    let end_row = adw::ActionRow::builder().title("Ends").build();
    end_row.add_suffix(&end_date.button);
    end_row.add_suffix(&end_time);
    sched.add(&all_day_row);
    sched.add(&start_row);
    sched.add(&end_row);

    let preset = recur::classify(series_rule.as_ref());
    let presets: Vec<Preset> = recur::PRESETS
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| *p != Preset::Custom || preset == Preset::Custom)
        .collect();
    let preset_names: Vec<&str> = recur::PRESETS
        .iter()
        .filter(|(p, _)| presets.contains(p))
        .map(|(_, n)| *n)
        .collect();
    let repeat = adw::ComboRow::builder()
        .title("Repeat")
        .model(&gtk::StringList::new(&preset_names))
        .build();
    repeat.set_selected(presets.iter().position(|p| *p == preset).unwrap_or(0) as u32);
    let ends = adw::ComboRow::builder()
        .title("Ends")
        .model(&gtk::StringList::new(&[
            "Never",
            "After a Number of Times",
            "On a Date",
        ]))
        .build();
    let cur_ends = recur::ends_of(series_rule.as_ref());
    let count = adw::SpinRow::with_range(1.0, 999.0, 1.0);
    count.set_title("Occurrences");
    count.set_value(10.0);
    let until = DateButton::new(start.date() + Duration::days(30));
    match cur_ends {
        Ends::Forever => ends.set_selected(0),
        Ends::Count(c) => {
            ends.set_selected(1);
            count.set_value(c as f64);
        }
        Ends::Until(d) => {
            ends.set_selected(2);
            until.set(d);
        }
    }
    let until_row = adw::ActionRow::builder().title("End Date").build();
    until_row.add_suffix(&until.button);
    sched.add(&repeat);
    sched.add(&ends);
    sched.add(&count);
    sched.add(&until_row);
    page.add(&sched);

    let alarm_group = adw::PreferencesGroup::builder().title("Reminders").build();
    let add_alarm = gtk::MenuButton::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Reminder")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build()
        .labelled("Add Reminder");
    alarm_group.set_header_suffix(Some(&add_alarm));
    page.add(&alarm_group);

    let notes_group = adw::PreferencesGroup::builder().title("Notes").build();
    let notes = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .height_request(96)
        .build();
    notes
        .buffer()
        .set_text(event.as_ref().map(|e| e.description.as_str()).unwrap_or(""));
    notes_group.add(
        &gtk::Frame::builder()
            .child(&notes)
            .css_classes(["view"])
            .build(),
    );
    page.add(&notes_group);

    let delete = gtk::Button::builder()
        .label("Delete Event")
        .css_classes(["destructive-action", "pill"])
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    if existing.is_some() && writable {
        let g = adw::PreferencesGroup::new();
        g.add(&delete);
        page.add(&g);
    }
    if !writable {
        for w in [
            title.upcast_ref::<gtk::Widget>(),
            cal.upcast_ref(),
            location.upcast_ref(),
            sched.upcast_ref(),
            add_alarm.upcast_ref(),
            notes.upcast_ref(),
        ] {
            w.set_sensitive(false);
        }
    }

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    dialog.set_child(Some(&view));

    let form = Rc::new(Form {
        dialog: dialog.clone(),
        title,
        cal,
        cal_uids: selectable.iter().map(|(u, _)| u.clone()).collect(),
        location,
        all_day: all_day_row,
        start_date,
        start_time,
        end_date,
        end_time,
        repeat,
        presets,
        ends,
        count,
        until,
        until_row,
        alarms: RefCell::default(),
        alarm_group,
        alarm_rows: RefCell::default(),
        notes,
        twelve,
    });
    let initial_alarms = match &event {
        Some(e) => e.alarms.clone(),
        None => Vec::new(),
    };
    for a in initial_alarms {
        form.add_alarm_row(a);
    }
    // Reminder menu.
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let pop = gtk::Popover::builder().child(&menu_box).build();
    add_alarm.set_popover(Some(&pop));
    for (mins, label) in recur::REMINDERS {
        let b = gtk::Button::builder()
            .label(label)
            .css_classes(["flat"])
            .build();
        let weak = Rc::downgrade(&form);
        let p = pop.clone();
        b.connect_clicked(move |_| {
            if let Some(f) = weak.upgrade() {
                f.add_alarm_row(Alarm::before(Duration::minutes(mins)));
            }
            p.popdown();
        });
        menu_box.append(&b);
    }

    // Keep the event's length when the start moves.
    let duration = end - start;
    let span = (end_display - start.date()).num_days().max(0);
    let weak = Rc::downgrade(&form);
    *form.start_date.changed.borrow_mut() = Some(Box::new(move |d| {
        if let Some(f) = weak.upgrade() {
            f.end_date.set(d + Duration::days(span));
        }
    }));
    let weak = Rc::downgrade(&form);
    form.start_time.connect_activate(move |e| {
        let Some(f) = weak.upgrade() else { return };
        if let Some(t) = dates::parse_time_input(&e.text()) {
            e.set_text(&dates::format_time(t, f.twelve));
            let s = f.start_date.date.get().and_time(t);
            if !f.all_day.is_active() {
                let new_end = s + duration.max(Duration::minutes(0));
                f.end_date.set(new_end.date());
                f.end_time
                    .set_text(&dates::format_time(new_end.time(), f.twelve));
            }
        }
    });
    let weak = Rc::downgrade(&form);
    form.end_time.connect_activate(move |e| {
        if let (Some(f), Some(t)) = (weak.upgrade(), dates::parse_time_input(&e.text())) {
            e.set_text(&dates::format_time(t, f.twelve));
        }
    });
    for row in [
        form.all_day.upcast_ref::<glib::Object>(),
        form.repeat.upcast_ref(),
        form.ends.upcast_ref(),
    ] {
        let weak = Rc::downgrade(&form);
        let prop = if row.is::<adw::SwitchRow>() {
            "active"
        } else {
            "selected"
        };
        row.connect_notify_local(Some(prop), move |_, _| {
            if let Some(f) = weak.upgrade() {
                f.update_visibility();
            }
        });
    }
    form.update_visibility();

    let d = dialog.clone();
    cancel.connect_clicked(move |_| {
        d.close();
    });

    let orig = (start, end, all_day);
    let weak_ui = Rc::downgrade(ui);
    let f = form.clone();
    let ev2 = event.clone();
    let master2 = master.clone();
    let cal_orig = existing.as_ref().map(|(c, ..)| c.clone());
    let rid = match &target {
        Target::Existing { rid, .. } => rid.clone(),
        _ => String::new(),
    };
    let rid2 = rid.clone();
    save.connect_clicked(move |_| {
        let Some(ui) = weak_ui.upgrade() else { return };
        let v = match f.values() {
            Ok(v) => v,
            Err(e) => {
                ui.toast(&e);
                return;
            }
        };
        let zone = ui.zone_name();
        let alarms = f.alarms.borrow().clone();
        let times_changed = (v.start, v.end, v.all_day) != orig;
        let apply_fields = |e: &mut Event| {
            e.summary = v.title.clone();
            e.location = v.location.clone();
            e.description = v.notes.clone();
            e.alarms = alarms.clone();
        };
        match (&ev2, &cal_orig) {
            (None, _) => {
                let mut e = Event::new(
                    make_time(v.start, v.all_day, &zone),
                    make_time(v.end, v.all_day, &zone),
                );
                apply_fields(&mut e);
                e.rrule = recur::build(v.preset, v.ends, None, v.all_day);
                ui.remember_calendar(&v.cal);
                ui.backend.send(Cmd::Create {
                    cal: v.cal.clone(),
                    ics: vec![e.to_ics(Utc::now())],
                });
                f.dialog.close();
            }
            (Some(comp), Some(cal)) => {
                let rule_changed =
                    recur::build(v.preset, v.ends, series_rule.as_ref(), v.all_day) != series_rule;
                if !recurring {
                    let mut e = comp.clone();
                    apply_fields(&mut e);
                    if times_changed {
                        e.start = make_time(v.start, v.all_day, &zone);
                        e.end = make_time(v.end, v.all_day, &zone);
                    }
                    e.rrule = recur::build(v.preset, v.ends, None, v.all_day);
                    e.sequence += 1;
                    let ics = vec![e.to_ics(Utc::now())];
                    if v.cal != *cal {
                        ui.backend.send(Cmd::Move {
                            from: cal.clone(),
                            to: v.cal.clone(),
                            ics,
                            uid: e.uid.clone(),
                        });
                    } else {
                        ui.backend.send(Cmd::Modify {
                            cal: cal.clone(),
                            ics,
                            mod_type: ModType::All,
                        });
                    }
                    f.dialog.close();
                    return;
                }
                let save_all = {
                    let master = master2.clone();
                    let ui = ui.clone();
                    let cal = cal.clone();
                    let v_cal = v.cal.clone();
                    let (vs, ve, vad, vp, vends) = (v.start, v.end, v.all_day, v.preset, v.ends);
                    let fields = (
                        v.title.clone(),
                        v.location.clone(),
                        v.notes.clone(),
                        alarms.clone(),
                    );
                    let zone = zone.clone();
                    let series_rule = series_rule.clone();
                    move || {
                        let Some(mut m) = master.clone() else {
                            ui.toast("The recurring event's series is not loaded");
                            return;
                        };
                        m.summary = fields.0.clone();
                        m.location = fields.1.clone();
                        m.description = fields.2.clone();
                        m.alarms = fields.3.clone();
                        if times_changed {
                            let zones = ui.zones.borrow();
                            let delta = vs - orig.0;
                            let ms = m.start.local(&zones) + delta;
                            m.start = make_time(ms, vad, &zone);
                            m.end = make_time(ms + (ve - vs), vad, &zone);
                        }
                        m.rrule = recur::build(vp, vends, series_rule.as_ref(), vad);
                        m.sequence += 1;
                        let ics = vec![m.to_ics(Utc::now())];
                        if v_cal != cal {
                            ui.backend.send(Cmd::Move {
                                from: cal.clone(),
                                to: v_cal.clone(),
                                ics,
                                uid: m.uid.clone(),
                            });
                        } else {
                            ui.backend.send(Cmd::Modify {
                                cal: cal.clone(),
                                ics,
                                mod_type: ModType::All,
                            });
                        }
                    }
                };
                if rule_changed || v.cal != *cal {
                    save_all();
                    f.dialog.close();
                    return;
                }
                let save_this = {
                    let comp = comp.clone();
                    let ui = ui.clone();
                    let cal = cal.clone();
                    let rid = rid2.clone();
                    let (vs, ve, vad) = (v.start, v.end, v.all_day);
                    let fields = (
                        v.title.clone(),
                        v.location.clone(),
                        v.notes.clone(),
                        alarms.clone(),
                    );
                    let zone = zone.clone();
                    move || {
                        let mut e = comp.clone();
                        if e.recurrence_id.is_none() {
                            let Some(r) = rid_time(&rid, &e.start) else {
                                ui.toast("Could not identify this occurrence");
                                return;
                            };
                            let occ_start = r.naive();
                            e.recurrence_id = Some(r);
                            e.rrule = None;
                            e.rdates.clear();
                            e.exdates.clear();
                            let dur = e.duration();
                            e.start = e.start.with_naive(occ_start);
                            e.end = e.start.with_naive(occ_start + dur);
                        }
                        e.summary = fields.0.clone();
                        e.location = fields.1.clone();
                        e.description = fields.2.clone();
                        e.alarms = fields.3.clone();
                        if times_changed {
                            e.start = make_time(vs, vad, &zone);
                            e.end = make_time(ve, vad, &zone);
                        }
                        e.sequence += 1;
                        ui.backend.send(Cmd::Modify {
                            cal: cal.clone(),
                            ics: vec![e.to_ics(Utc::now())],
                            mod_type: ModType::This,
                        });
                    }
                };
                let dialog = f.dialog.clone();
                ask_scope(
                    &f.dialog,
                    "Save Changes to Recurring Event?",
                    "Save",
                    move |this| {
                        if this {
                            save_this();
                        } else {
                            save_all();
                        }
                        dialog.close();
                    },
                );
            }
            _ => {}
        }
    });

    if let (Some(comp), Some(cal)) = (event.clone(), existing.as_ref().map(|(c, ..)| c.clone())) {
        let weak_ui = Rc::downgrade(ui);
        let dialog2 = dialog.clone();
        delete.connect_clicked(move |_| {
            let Some(ui) = weak_ui.upgrade() else { return };
            let uid = comp.uid.clone();
            if !recurring {
                ui.delete_with_undo(&cal, &uid, &comp.summary);
                dialog2.close();
                return;
            }
            let (ui2, cal2, rid3, d3) = (ui.clone(), cal.clone(), rid.clone(), dialog2.clone());
            let own_rid = comp.recurrence_id.as_ref().map(Time::value_string);
            ask_scope(&dialog2, "Delete Recurring Event?", "Delete", move |this| {
                if this {
                    let r = own_rid.clone().unwrap_or_else(|| rid3.clone());
                    ui2.backend.send(Cmd::Remove {
                        cal: cal2.clone(),
                        ids: vec![(uid.clone(), r)],
                        mod_type: ModType::This,
                    });
                } else {
                    ui2.backend.send(Cmd::Remove {
                        cal: cal2.clone(),
                        ids: vec![(uid.clone(), String::new())],
                        mod_type: ModType::All,
                    });
                }
                d3.close();
            });
        });
    }

    dialog.present(Some(&ui.window));
    if existing.is_none() {
        form.title.grab_focus();
    }
}

/// Asks whether a change applies to this occurrence or the whole series.
fn ask_scope(parent: &adw::Dialog, heading: &str, verb: &str, f: impl Fn(bool) + 'static) {
    let alert = adw::AlertDialog::new(Some(heading), Some("This event is part of a series."));
    alert.add_response("cancel", "_Cancel");
    alert.add_response("this", &format!("{verb} _Only This Event"));
    alert.add_response("all", &format!("{verb} _All Events"));
    alert.set_close_response("cancel");
    alert.set_default_response(Some("this"));
    alert.set_response_appearance("this", adw::ResponseAppearance::Suggested);
    alert.connect_response(None, move |_, r| match r {
        "this" => f(true),
        "all" => f(false),
        _ => {}
    });
    alert.present(Some(parent));
}

/// Default slot for a new timed event starting on `day`: the next whole
/// hour today, 09:00 on other days.
pub fn default_slot(day: NaiveDate, now: NaiveDateTime) -> (NaiveDateTime, NaiveDateTime) {
    let start = if day == now.date() {
        let h = (now.time().hour() + 1).min(23);
        day.and_hms_opt(h, 0, 0).unwrap()
    } else {
        day.and_hms_opt(9, 0, 0).unwrap()
    };
    (start, start + Duration::hours(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn rid_parsing() {
        let like = Time::Zoned(dt("2026-01-01 09:00"), "Europe/Berlin".into());
        assert_eq!(
            rid_time("20261113T090000", &like),
            Some(Time::Zoned(dt("2026-11-13 09:00"), "Europe/Berlin".into()))
        );
        assert_eq!(
            rid_time("20261113T090000Z", &like),
            Some(Time::Utc(dt("2026-11-13 09:00")))
        );
        assert_eq!(
            rid_time("20261113", &like),
            Some(Time::Date(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap()))
        );
        assert_eq!(rid_time("bogus", &like), None);
    }

    #[test]
    fn times() {
        assert_eq!(
            make_time(dt("2026-11-13 09:00"), true, "UTC"),
            Time::Date(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap())
        );
        assert_eq!(
            make_time(dt("2026-11-13 09:00"), false, "UTC"),
            Time::Utc(dt("2026-11-13 09:00"))
        );
        assert_eq!(
            make_time(dt("2026-11-13 09:00"), false, "Europe/Paris"),
            Time::Zoned(dt("2026-11-13 09:00"), "Europe/Paris".into())
        );
    }

    #[test]
    fn slots() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        assert_eq!(
            default_slot(today, dt("2026-11-13 14:20")).0,
            dt("2026-11-13 15:00")
        );
        assert_eq!(
            default_slot(today + Duration::days(1), dt("2026-11-13 14:20")).0,
            dt("2026-11-14 09:00")
        );
        assert_eq!(
            default_slot(today, dt("2026-11-13 23:30")).0,
            dt("2026-11-13 23:00")
        );
    }
}
