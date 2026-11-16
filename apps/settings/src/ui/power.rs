//! Power panel.

use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

use super::widgets::{self, preset_combo, seconds_label, switch, Num};
use super::{spawn_worker, Ctx, Panel};
use crate::dbus;
use crate::values::{self, POWER_BUTTON_ACTIONS};
use helix_app_common::i18n::tr;

const POWER: &str = "org.gnome.settings-daemon.plugins.power";
const INTERFACE: &str = "org.gnome.desktop.interface";
const BATTERY_REFRESH_SECS: u32 = 30;

pub struct PowerPanel;

struct BatteryUi {
    group: adw::PreferencesGroup,
    level: gtk::LevelBar,
    row: adw::ActionRow,
    on_battery_rows: Vec<gtk::Widget>,
}

impl BatteryUi {
    fn update(&self, battery: Option<dbus::Battery>) {
        let present = battery.is_some();
        self.group.set_visible(present);
        for w in &self.on_battery_rows {
            w.set_visible(present);
        }
        if let Some(b) = battery {
            let percent = format!("{:.0} %", b.percentage);
            self.level.set_value(b.percentage / 100.0);
            // Read as "Battery Level, 57 %".
            self.level
                .update_property(&[gtk::accessible::Property::ValueText(&percent)]);
            self.row.set_title(tr("Battery"));
            let mut subtitle = format!("{percent} · {}", values::battery_state_label(b.state));
            if let Some(t) = values::battery_time_label(b.state, b.time_to_empty, b.time_to_full) {
                subtitle = format!("{subtitle} — {t}");
            }
            self.row.set_subtitle(&subtitle);
        }
    }
}

fn refresh(ui: &Rc<BatteryUi>) {
    let weak = Rc::downgrade(ui);
    spawn_worker(
        || dbus::read_battery().ok().flatten(),
        move |b| {
            if let Some(ui) = weak.upgrade() {
                ui.update(b);
            }
        },
    );
}

impl Panel for PowerPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let power = ctx.prefs(POWER);
        let session = ctx.prefs(super::privacy::SESSION);
        let iface = ctx.prefs(INTERFACE);
        let page = widgets::page();

        // Battery (hidden until UPower reports one).
        let battery = widgets::group(tr("Battery"));
        battery.set_visible(false);
        let level = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(1.0)
            .valign(gtk::Align::Center)
            .width_request(160)
            .build();
        helix_app_common::set_accessible_label(&level, tr("Battery Level"));
        let row = adw::ActionRow::builder().title(tr("Battery")).build();
        row.add_suffix(&level);
        battery.add(&row);
        page.add(&battery);

        let saving = widgets::group(tr("Power Saving"));
        saving.add(&switch(
            &power,
            "idle-dim",
            true,
            false,
            tr("Dim Screen"),
            tr("Reduces the screen brightness when the computer is inactive"),
        ));
        let blank = preset_combo(
            &session,
            "idle-delay",
            Num::U32,
            300,
            values::blank_delays,
            seconds_label,
            tr("Screen Blank"),
        );
        blank.set_subtitle(tr("Turns the screen off after a period of inactivity"));
        saving.add(&blank);
        let mut battery_rows = Vec::new();
        for (source, title, delay_name) in [
            (
                "ac",
                tr("Automatic Suspend When Plugged In"),
                tr("Suspend Delay When Plugged In"),
            ),
            (
                "battery",
                tr("Automatic Suspend On Battery Power"),
                tr("Suspend Delay On Battery Power"),
            ),
        ] {
            let type_key: &'static str = if source == "ac" {
                "sleep-inactive-ac-type"
            } else {
                "sleep-inactive-battery-type"
            };
            let timeout_key: &'static str = if source == "ac" {
                "sleep-inactive-ac-timeout"
            } else {
                "sleep-inactive-battery-timeout"
            };
            let sw =
                widgets::nick_switch(&power, type_key, ("suspend", "nothing"), "suspend", title);
            sw.set_subtitle(tr("Pauses the computer after a period of inactivity"));
            let delay = preset_combo(
                &power,
                timeout_key,
                Num::I32,
                900,
                values::suspend_delays,
                seconds_label,
                tr("Delay"),
            );
            // Both rows read "Delay"; say which suspend each belongs to.
            helix_app_common::set_accessible_label(&delay, delay_name);
            sw.bind_property("active", &delay, "sensitive")
                .sync_create()
                .build();
            saving.add(&sw);
            saving.add(&delay);
            if source == "battery" {
                sw.set_visible(false);
                delay.set_visible(false);
                battery_rows.push(sw.upcast());
                battery_rows.push(delay.upcast());
            }
        }
        page.add(&saving);

        let general = widgets::group(tr("General"));
        general.add(&power_button_row(&power));
        general.add(&switch(
            &iface,
            "show-battery-percentage",
            false,
            false,
            tr("Show Battery Percentage"),
            tr("Show the exact charge level in the top bar"),
        ));
        page.add(&general);

        let ui = Rc::new(BatteryUi {
            group: battery,
            level,
            row,
            on_battery_rows: battery_rows,
        });
        refresh(&ui);
        let weak = Rc::downgrade(&ui);
        glib::timeout_add_seconds_local(BATTERY_REFRESH_SECS, move || match weak.upgrade() {
            Some(ui) => {
                refresh(&ui);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        // Keep the UI state alive as long as the page.
        page.connect_destroy(move |_| {
            let _ = &ui;
        });
        page.upcast()
    }
}

fn power_button_row(power: &helix_app_common::Prefs) -> adw::ComboRow {
    widgets::nick_combo(
        power,
        "power-button-action",
        "suspend",
        POWER_BUTTON_ACTIONS,
        tr("Power Button Behavior"),
    )
}
