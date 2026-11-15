//! Date & Time panel (timedated + clock settings).

use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::picker::{self, Picker};
use super::widgets::{self, switch, watch};
use super::{spawn_worker, Ctx, Panel};
use crate::{dbus, values};

const INTERFACE: &str = "org.gnome.desktop.interface";
const DATETIME: &str = "org.gnome.desktop.datetime";
const CALENDAR: &str = "org.gnome.desktop.calendar";

pub struct DateTimePanel;

struct State {
    ctx: std::rc::Weak<Ctx>,
    iface: helix_app_common::Prefs,
    ntp: adw::SwitchRow,
    tz_row: adw::ActionRow,
    time_row: adw::ActionRow,
    timezone: RefCell<String>,
    /// Suppresses SetNTP while the switch is updated from timedated.
    loading: Cell<bool>,
    /// "Unavailable" was already reported with a toast.
    reported_unavailable: Cell<bool>,
}

/// Formats `now` in `tz` per `clock-format` (`24h` / `12h`).
pub fn format_time(now: &glib::DateTime, twelve_hour: bool) -> String {
    let fmt = if twelve_hour {
        "%e %B %Y, %l:%M %p"
    } else {
        "%e %B %Y, %H:%M"
    };
    now.format(fmt)
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

impl State {
    fn tick(&self) {
        let tz_name = self.timezone.borrow().clone();
        #[allow(deprecated)] // from_identifier needs GLib 2.68 bindings
        let tz = if tz_name.is_empty() {
            glib::TimeZone::local()
        } else {
            glib::TimeZone::new(Some(&tz_name))
        };
        if let Ok(now) = glib::DateTime::now(&tz) {
            let twelve = self.iface.get_enum_nick("clock-format", "24h") == "12h";
            self.time_row.set_subtitle(&format_time(&now, twelve));
        }
    }

    fn load(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        spawn_worker(dbus::read_timedate, move |res| {
            let Some(this) = weak.upgrade() else { return };
            match res {
                Ok(td) => {
                    this.loading.set(true);
                    this.ntp.set_active(td.ntp);
                    this.ntp.set_sensitive(td.can_ntp);
                    let sync = if td.ntp_synchronized {
                        "Synchronized with network time servers"
                    } else {
                        "Requires internet access"
                    };
                    // Speak a change of sync state (not the first reading).
                    let before = this.ntp.subtitle();
                    this.ntp.set_subtitle(sync);
                    if before.is_some_and(|b| !b.is_empty() && b != sync) && td.ntp {
                        helix_app_common::announce(
                            &this.ntp,
                            sync,
                            helix_app_common::Urgency::Polite,
                        );
                    }
                    this.loading.set(false);
                    this.tz_row.set_subtitle(&td.timezone);
                    *this.timezone.borrow_mut() = td.timezone;
                    this.tz_row.set_sensitive(true);
                }
                Err(e) => {
                    this.ntp.set_sensitive(false);
                    this.tz_row.set_sensitive(false);
                    this.tz_row.set_subtitle("Unavailable");
                    eprintln!("helix-settings-app: timedate1: {e}");
                    // Report once, not on every reload.
                    if !this.reported_unavailable.replace(true) {
                        if let Some(ctx) = this.ctx.upgrade() {
                            ctx.toast("Date and time settings are unavailable");
                        }
                    }
                }
            }
            this.tick();
        });
    }

    fn report(&self, what: &str, res: Result<(), String>) {
        if let Err(e) = res {
            if let Some(ctx) = self.ctx.upgrade() {
                ctx.toast(&format!("Could not change {what}: {e}"));
            }
        }
    }
}

impl Panel for DateTimePanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let iface = ctx.prefs(INTERFACE);
        let datetime = ctx.prefs(DATETIME);
        let calendar = ctx.prefs(CALENDAR);
        let page = widgets::page();

        let group = widgets::group("");
        let ntp = adw::SwitchRow::builder()
            .title("Automatic Date &amp; Time")
            .sensitive(false)
            .build();
        group.add(&ntp);
        group.add(&switch(
            &datetime,
            "automatic-timezone",
            false,
            false,
            "Automatic Time Zone",
            "Requires location services",
        ));
        let time_row = adw::ActionRow::builder().title("Date &amp; Time").build();
        group.add(&time_row);
        let tz_row = adw::ActionRow::builder()
            .title("Time Zone")
            .subtitle("Loading…")
            .activatable(true)
            .sensitive(false)
            .build();
        tz_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        group.add(&tz_row);
        group.add(&widgets::nick_combo(
            &iface,
            "clock-format",
            "24h",
            &[("24h", "24-hour"), ("12h", "AM / PM")],
            "Time Format",
        ));
        page.add(&group);

        let clock = widgets::group("Clock & Calendar");
        clock.set_description(Some(
            "Control how the time and date are shown in the top bar",
        ));
        clock.add(&switch(
            &iface,
            "clock-show-weekday",
            false,
            false,
            "Week Day",
            "",
        ));
        clock.add(&switch(&iface, "clock-show-date", true, false, "Date", ""));
        clock.add(&switch(
            &iface,
            "clock-show-seconds",
            false,
            false,
            "Seconds",
            "",
        ));
        clock.add(&switch(
            &calendar,
            "show-weekdate",
            false,
            false,
            "Week Numbers",
            "Show week numbers in the calendar",
        ));
        page.add(&clock);

        let state = Rc::new(State {
            ctx: Rc::downgrade(ctx),
            iface: iface.clone(),
            ntp: ntp.clone(),
            tz_row: tz_row.clone(),
            time_row,
            timezone: RefCell::default(),
            loading: Cell::new(false),
            reported_unavailable: Cell::new(false),
        });
        state.load();
        state.tick();

        let weak = Rc::downgrade(&state);
        ntp.connect_active_notify(move |r| {
            let Some(st) = weak.upgrade() else { return };
            if st.loading.get() {
                return;
            }
            let enabled = r.is_active();
            let weak = Rc::downgrade(&st);
            // System change, only on user action.
            spawn_worker(
                move || dbus::set_ntp(enabled),
                move |res| {
                    if let Some(st) = weak.upgrade() {
                        st.report("automatic date & time", res);
                        st.load();
                    }
                },
            );
        });

        let weak = Rc::downgrade(&state);
        let window = ctx.window.clone();
        tz_row.connect_activated(move |_| {
            if let Some(st) = weak.upgrade() {
                timezone_dialog(&window, &st);
            }
        });

        let weak = Rc::downgrade(&state);
        watch(&iface, "clock-format", move || {
            if let Some(st) = weak.upgrade() {
                st.tick();
            }
        });
        let weak = Rc::downgrade(&state);
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(st) => {
                st.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        page.connect_destroy(move |_| {
            let _ = &state;
        });
        page.upcast()
    }
}

fn timezone_dialog(parent: &adw::ApplicationWindow, state: &Rc<State>) {
    let picker = Picker::new(
        "Select Time Zone",
        "Search for a city, country or region",
        "Search Time Zones",
        "Loading Time Zones…",
    );
    // (zone id, label), sorted by label.
    let zones: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
    let fill = {
        let (picker, zones) = (Rc::downgrade(&picker), zones.clone());
        move |q: &str| {
            let Some(picker) = picker.upgrade() else {
                return;
            };
            let zones = zones.borrow();
            let matches: Vec<&(String, String)> = zones
                .iter()
                .filter(|(z, label)| values::zone_matches(z, label, q))
                .collect();
            let shown: Vec<picker::Choice> = matches
                .iter()
                .take(picker::MAX_ROWS)
                .map(|(z, label)| (z.clone(), label.clone(), z.replace('_', " ")))
                .collect();
            picker.set_choices(&shown, matches.len());
        }
    };
    {
        let (fill, zones, search) = (fill.clone(), zones.clone(), picker.search.clone());
        spawn_worker(
            || {
                let countries = dbus::zone_countries();
                let mut list: Vec<(String, String)> = dbus::list_timezones()
                    .into_iter()
                    .map(|z| {
                        let label = values::zone_label(&z, countries.get(&z).map(String::as_str));
                        (z, label)
                    })
                    .collect();
                list.sort_by_key(|(_, label)| label.to_lowercase());
                list
            },
            move |list| {
                *zones.borrow_mut() = list;
                fill(&search.text());
            },
        );
    }
    picker
        .search
        .connect_search_changed(move |e| fill(&e.text()));
    let weak = Rc::downgrade(state);
    picker.connect_pick(move |tz| {
        let tz = tz.to_string();
        if let Some(st) = weak.upgrade() {
            if *st.timezone.borrow() != tz {
                let weak = Rc::downgrade(&st);
                // System change, only on user action.
                spawn_worker(
                    move || dbus::set_timezone(&tz),
                    move |res| {
                        if let Some(st) = weak.upgrade() {
                            st.report("the time zone", res);
                            st.load();
                        }
                    },
                );
            }
        }
    });
    picker.present(parent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats() {
        let tz = glib::TimeZone::utc();
        let dt = glib::DateTime::new(&tz, 2026, 3, 7, 15, 4, 0.0).unwrap();
        assert_eq!(format_time(&dt, false), "7 March 2026, 15:04");
        assert_eq!(format_time(&dt, true), "7 March 2026, 3:04 PM");
    }
}
