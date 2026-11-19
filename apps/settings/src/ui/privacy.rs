//! Privacy & Security panel.

use adw::prelude::*;
use std::rc::Rc;

use super::agent_access;
use super::widgets::{self, days_label, preset_combo, seconds_label, switch, watch, Num};
use super::{Ctx, Panel};
use crate::values;
use helix_app_common::i18n::tr;

pub const SESSION: &str = "org.gnome.desktop.session";
const SCREENSAVER: &str = "org.gnome.desktop.screensaver";
const NOTIFICATIONS: &str = "org.gnome.desktop.notifications";
const PRIVACY: &str = "org.gnome.desktop.privacy";
const LOCATION: &str = "org.gnome.system.location";

pub struct PrivacyPanel;

/// The panel's root page, under its sub-pages (Agent Access).
const ROOT_TAG: &str = "privacy";

impl Panel for PrivacyPanel {
    fn own_header(&self) -> bool {
        true
    }

    fn show_page(&self, ctx: &Rc<Ctx>, content: &gtk::Widget, page: &str) {
        let Some(nav) = content.downcast_ref::<adw::NavigationView>() else {
            return;
        };
        if page != agent_access::TAG || !agent_access::available() {
            return;
        }
        if nav.visible_page().and_then(|p| p.tag()).as_deref() == Some(agent_access::TAG) {
            return;
        }
        nav.pop_to_tag(ROOT_TAG);
        nav.push(&agent_access::page(ctx, nav));
    }

    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let nav = adw::NavigationView::new();
        let session = ctx.prefs(SESSION);
        let saver = ctx.prefs(SCREENSAVER);
        let notif = ctx.prefs(NOTIFICATIONS);
        let privacy = ctx.prefs(PRIVACY);
        let location = ctx.prefs(LOCATION);
        let page = widgets::page();

        let lock = widgets::group(tr("Screen Lock"));
        lock.set_description(Some(tr(
            "Automatically lock the screen when you are not using the device",
        )));
        let blank = preset_combo(
            &session,
            "idle-delay",
            Num::U32,
            300,
            values::blank_delays,
            seconds_label,
            tr("Blank Screen Delay"),
        );
        blank.set_subtitle(tr(
            "Period of inactivity after which the screen will go blank",
        ));
        lock.add(&blank);
        let auto = switch(
            &saver,
            "lock-enabled",
            true,
            false,
            tr("Automatic Screen Lock"),
            tr("Locks the screen after it goes blank"),
        );
        lock.add(&auto);
        let delay = preset_combo(
            &saver,
            "lock-delay",
            Num::U32,
            0,
            values::lock_delays,
            seconds_label,
            tr("Automatic Screen Lock Delay"),
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
            tr("Lock Screen Notifications"),
            "",
        ));
        page.add(&lock);

        let files = widgets::group(tr("File History & Trash"));
        let history = switch(
            &privacy,
            "remember-recent-files",
            true,
            false,
            tr("File History"),
            tr("Remember recently used files"),
        );
        files.add(&history);
        let age = preset_combo(
            &privacy,
            "recent-files-max-age",
            Num::I32,
            -1,
            values::recent_ages,
            days_label,
            tr("File History Duration"),
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
            tr("Automatically Delete Trash Content"),
            "",
        ));
        files.add(&switch(
            &privacy,
            "remove-old-temp-files",
            false,
            false,
            tr("Automatically Delete Temporary Files"),
            "",
        ));
        files.add(&preset_combo(
            &privacy,
            "old-files-age",
            Num::U32,
            30,
            values::old_files_ages,
            days_label,
            tr("Automatically Delete Period"),
        ));
        page.add(&files);

        let loc = widgets::group(tr("Location"));
        loc.add(&switch(
            &location,
            "enabled",
            false,
            false,
            tr("Location Services"),
            tr("Let apps determine your geographical location"),
        ));
        page.add(&loc);

        let devices = widgets::group(tr("Devices"));
        devices.add(&switch(
            &privacy,
            "disable-camera",
            false,
            true,
            tr("Camera"),
            tr("Allow apps to use the camera"),
        ));
        devices.add(&switch(
            &privacy,
            "disable-microphone",
            false,
            true,
            tr("Microphone"),
            tr("Allow apps to use the microphone"),
        ));
        page.add(&devices);

        // Helix's Agent Access, a permission like Location and Camera;
        // not offered when the shell's settings are not installed.
        if agent_access::available() {
            let agents = widgets::group(tr("Agents"));
            let row = widgets::subpage_row(tr("Agent Access"), "", "system-run-symbolic");
            let prefs = ctx.prefs(crate::agents::SCHEMA);
            let sync = {
                let (row, prefs) = (row.downgrade(), prefs.clone());
                move || {
                    if let Some(row) = row.upgrade() {
                        row.set_subtitle(if prefs.get::<bool>("enabled", false) {
                            tr("On")
                        } else {
                            tr("Off")
                        });
                    }
                }
            };
            sync();
            watch(&prefs, "enabled", sync);
            let (weak_nav, weak_ctx) = (nav.downgrade(), Rc::downgrade(ctx));
            row.connect_activated(move |_| {
                if let (Some(nav), Some(ctx)) = (weak_nav.upgrade(), weak_ctx.upgrade()) {
                    nav.push(&agent_access::page(&ctx, &nav));
                }
            });
            agents.add(&row);
            page.add(&agents);
        }

        let diag = widgets::group(tr("Diagnostics"));
        diag.add(&switch(
            &privacy,
            "report-technical-problems",
            false,
            false,
            tr("Automatic Problem Reporting"),
            tr("Send technical reports when problems occur"),
        ));
        page.add(&diag);
        nav.add(&widgets::nav_page(
            tr("Privacy & Security"),
            &page,
            Some(ROOT_TAG),
        ));
        nav.upcast()
    }
}
