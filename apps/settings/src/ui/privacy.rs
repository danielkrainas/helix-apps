//! Privacy & Security panel.

use adw::prelude::*;
use std::rc::Rc;

use super::widgets::{self, days_label, preset_combo, seconds_label, switch, Num};
use super::{Ctx, Panel};
use crate::values;

pub const SESSION: &str = "org.gnome.desktop.session";
const SCREENSAVER: &str = "org.gnome.desktop.screensaver";
const NOTIFICATIONS: &str = "org.gnome.desktop.notifications";
const PRIVACY: &str = "org.gnome.desktop.privacy";
const LOCATION: &str = "org.gnome.system.location";

pub struct PrivacyPanel;

impl Panel for PrivacyPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let session = ctx.prefs(SESSION);
        let saver = ctx.prefs(SCREENSAVER);
        let notif = ctx.prefs(NOTIFICATIONS);
        let privacy = ctx.prefs(PRIVACY);
        let location = ctx.prefs(LOCATION);
        let page = widgets::page();

        let lock = widgets::group("Screen Lock");
        lock.set_description(Some(
            "Automatically lock the screen when you are not using the device",
        ));
        let blank = preset_combo(
            &session,
            "idle-delay",
            Num::U32,
            300,
            values::blank_delays,
            seconds_label,
            "Blank Screen Delay",
        );
        blank.set_subtitle("Period of inactivity after which the screen will go blank");
        lock.add(&blank);
        let auto = switch(
            &saver,
            "lock-enabled",
            true,
            false,
            "Automatic Screen Lock",
            "Locks the screen after it goes blank",
        );
        lock.add(&auto);
        let delay = preset_combo(
            &saver,
            "lock-delay",
            Num::U32,
            0,
            values::lock_delays,
            seconds_label,
            "Automatic Screen Lock Delay",
        );
        auto.bind_property("active", &delay, "sensitive")
            .sync_create()
            .build();
        lock.add(&delay);
        lock.add(&switch(
            &notif,
            "show-in-lock-screen",
            true,
            false,
            "Lock Screen Notifications",
            "",
        ));
        page.add(&lock);

        let files = widgets::group("File History & Trash");
        let history = switch(
            &privacy,
            "remember-recent-files",
            true,
            false,
            "File History",
            "Remember recently used files",
        );
        files.add(&history);
        let age = preset_combo(
            &privacy,
            "recent-files-max-age",
            Num::I32,
            -1,
            values::recent_ages,
            days_label,
            "File History Duration",
        );
        history
            .bind_property("active", &age, "sensitive")
            .sync_create()
            .build();
        files.add(&age);
        files.add(&switch(
            &privacy,
            "remove-old-trash-files",
            false,
            false,
            "Automatically Delete Trash Content",
            "",
        ));
        files.add(&switch(
            &privacy,
            "remove-old-temp-files",
            false,
            false,
            "Automatically Delete Temporary Files",
            "",
        ));
        files.add(&preset_combo(
            &privacy,
            "old-files-age",
            Num::U32,
            30,
            values::old_files_ages,
            days_label,
            "Automatically Delete Period",
        ));
        page.add(&files);

        let loc = widgets::group("Location");
        loc.add(&switch(
            &location,
            "enabled",
            false,
            false,
            "Location Services",
            "Let apps determine your geographical location",
        ));
        page.add(&loc);

        let devices = widgets::group("Devices");
        devices.add(&switch(
            &privacy,
            "disable-camera",
            false,
            true,
            "Camera",
            "Allow apps to use the camera",
        ));
        devices.add(&switch(
            &privacy,
            "disable-microphone",
            false,
            true,
            "Microphone",
            "Allow apps to use the microphone",
        ));
        page.add(&devices);

        let diag = widgets::group("Diagnostics");
        diag.add(&switch(
            &privacy,
            "report-technical-problems",
            false,
            false,
            "Automatic Problem Reporting",
            "Send technical reports when problems occur",
        ));
        page.add(&diag);
        page.upcast()
    }
}
