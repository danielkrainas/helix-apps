//! Notifications panel.

use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;

use super::widgets::{self, preset_combo, seconds_label, switch, Num};
use super::{Ctx, Panel};
use crate::values;
use helix_app_common::i18n::tr;
use helix_app_common::Prefs;

const NOTIFICATIONS: &str = "org.gnome.desktop.notifications";
const APP_SCHEMA: &str = "org.gnome.desktop.notifications.application";
/// Helix's shell settings (helixdesktop/spec
/// `contracts/gsettings/org.helixdesktop.shell.md`).
const SHELL: &str = "org.helixdesktop.shell";
const BANNER_TIMEOUT: &str = "banner-ignore-timeout";
const CRITICAL_BANNER_TIMEOUT: &str = "critical-banner-ignore-timeout";

/// The banner ignore-timeout keys the installed `org.helixdesktop.shell` has
/// (an older helix-desktop has neither), with their defaults.
fn banner_timeout_keys(shell: &Prefs) -> Vec<(&'static str, i64)> {
    [(BANNER_TIMEOUT, 10), (CRITICAL_BANNER_TIMEOUT, 30)]
        .into_iter()
        .filter(|(key, _)| widgets::has_key(shell, key))
        .collect()
}

/// "Hide ignored notifications after" rows, or `None` when the installed
/// shell schema lacks both keys.
fn banner_timeouts(shell: &Prefs) -> Option<adw::PreferencesGroup> {
    let keys = banner_timeout_keys(shell);
    if keys.is_empty() {
        return None;
    }
    let group = widgets::group(tr("Banners"));
    group.set_description(Some(tr(
        "Time counts only while you are using the computer. \
         Hidden notifications stay in the notification list.",
    )));
    for (key, default) in keys {
        let title = if key == BANNER_TIMEOUT {
            tr("Hide Ignored Notifications After")
        } else {
            tr("Hide Ignored Urgent Notifications After")
        };
        let row = preset_combo(
            shell,
            key,
            Num::U32,
            default,
            values::banner_ignore_timeouts,
            seconds_label,
            title,
        );
        // Long translated titles wrap instead of widening the window.
        row.set_title_lines(0);
        group.add(&row);
    }
    Some(group)
}

pub struct NotificationsPanel;

/// Path of an app's relocatable notification settings.
pub fn app_path(app_id: &str) -> String {
    format!("/org/gnome/desktop/notifications/application/{app_id}/")
}

impl Panel for NotificationsPanel {
    fn build(&self, ctx: &Rc<Ctx>) -> gtk::Widget {
        let prefs = ctx.prefs(NOTIFICATIONS);
        let page = widgets::page();
        let general = widgets::group("");
        general.add(&switch(
            &prefs,
            "show-banners",
            true,
            true,
            tr("Do Not Disturb"),
            tr("Notification banners are hidden"),
        ));
        general.add(&switch(
            &prefs,
            "show-in-lock-screen",
            true,
            false,
            tr("Lock Screen Notifications"),
            tr("Show notifications on the lock screen"),
        ));
        page.add(&general);
        if let Some(group) = banner_timeouts(&ctx.prefs(SHELL)) {
            page.add(&group);
        }

        let schema = helix_app_common::lookup_schema(APP_SCHEMA);
        let children: Vec<String> = prefs.get::<Vec<String>>("application-children", Vec::new());
        if let Some(schema) = schema.filter(|_| !children.is_empty()) {
            let apps = widgets::group(tr("Apps"));
            let mut entries: Vec<(String, String, Option<gio::Icon>)> = children
                .iter()
                .map(|id| {
                    let info = gio::DesktopAppInfo::new(&format!("{id}.desktop"));
                    let name = info
                        .as_ref()
                        .map(|i| i.display_name().to_string())
                        .unwrap_or_else(|| id.clone());
                    (id.clone(), name, info.and_then(|i| i.icon()))
                })
                .collect();
            entries.sort_by_key(|e| e.1.to_lowercase());
            for (id, name, icon) in entries {
                let settings = gio::Settings::new_full(
                    &schema,
                    None::<&gio::SettingsBackend>,
                    Some(&app_path(&id)),
                );
                let row = adw::SwitchRow::builder()
                    .title(name.as_str())
                    .use_markup(false)
                    .build();
                // Decorative: the row's title names the app.
                let image = gtk::Image::builder()
                    .pixel_size(32)
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .build();
                match icon {
                    Some(icon) => image.set_from_gicon(&icon),
                    None => image.set_icon_name(Some("application-x-executable")),
                }
                row.add_prefix(&image);
                settings.bind("enable", &row, "active").build();
                apps.add(&row);
            }
            page.add(&apps);
        }
        page.upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::{get_num, set_num};

    /// helix-desktop's `org.helixdesktop.shell` with the timeout keys, and an
    /// older one without them.
    const NEW_SHELL: &str = r#"<schemalist><schema id="org.helixdesktop.shell" path="/org/helixdesktop/shell/">
        <key name="panel-left" type="as"><default>[]</default></key>
        <key name="banner-ignore-timeout" type="u"><range min="0" max="600"/><default>10</default></key>
        <key name="critical-banner-ignore-timeout" type="u"><range min="0" max="600"/><default>30</default></key>
        </schema></schemalist>"#;
    const OLD_SHELL: &str = r#"<schemalist><schema id="org.helixdesktop.shell" path="/org/helixdesktop/shell/">
        <key name="panel-left" type="as"><default>[]</default></key>
        </schema></schemalist>"#;

    /// `org.helixdesktop.shell` compiled from `xml` in a scratch directory, on a
    /// memory backend; `None` when glib-compile-schemas is unavailable.
    fn shell_prefs(name: &str, xml: &str) -> Option<Prefs> {
        let dir = std::env::temp_dir().join(format!(
            "helix-settings-shell-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("org.helixdesktop.shell.gschema.xml"), xml).unwrap();
        let compiled = std::process::Command::new("glib-compile-schemas")
            .arg("--strict")
            .arg(&dir)
            .status();
        let source = matches!(compiled, Ok(s) if s.success())
            .then(|| gio::SettingsSchemaSource::from_directory(&dir, None, false).ok())
            .flatten();
        std::fs::remove_dir_all(&dir).ok();
        let Some(schema) = source.and_then(|s| s.lookup(SHELL, false)) else {
            eprintln!("glib-compile-schemas unavailable; skipping");
            return None;
        };
        let backend = gio::memory_settings_backend_new();
        let settings = gio::Settings::new_full(&schema, Some(&backend), None);
        Some(Prefs::from_settings(settings))
    }

    #[test]
    fn banner_timeout_rows_need_the_keys() {
        let Some(new) = shell_prefs("new", NEW_SHELL) else {
            return;
        };
        assert_eq!(
            banner_timeout_keys(&new),
            [(BANNER_TIMEOUT, 10), (CRITICAL_BANNER_TIMEOUT, 30)]
        );
        let Some(old) = shell_prefs("old", OLD_SHELL) else {
            return;
        };
        assert!(banner_timeout_keys(&old).is_empty());
        // No org.helixdesktop.shell at all.
        assert!(banner_timeout_keys(&Prefs::new("org.helixdesktop.NoSuchShell")).is_empty());
    }

    #[test]
    fn banner_timeouts_read_and_write() {
        let Some(prefs) = shell_prefs("rw", NEW_SHELL) else {
            return;
        };
        let s = prefs.gsettings().unwrap().clone();
        assert_eq!(get_num(&prefs, BANNER_TIMEOUT, Num::U32, 0.0), 10.0);
        assert_eq!(
            get_num(&prefs, CRITICAL_BANNER_TIMEOUT, Num::U32, 0.0),
            30.0
        );
        set_num(&prefs, BANNER_TIMEOUT, Num::U32, 0.0);
        set_num(&prefs, CRITICAL_BANNER_TIMEOUT, Num::U32, 300.0);
        assert_eq!(s.uint(BANNER_TIMEOUT), 0);
        assert_eq!(s.uint(CRITICAL_BANNER_TIMEOUT), 300);
        // A value set elsewhere (gsettings, the shell) reads back.
        s.set_uint(BANNER_TIMEOUT, 45).unwrap();
        assert_eq!(get_num(&prefs, BANNER_TIMEOUT, Num::U32, 0.0), 45.0);
    }

    #[test]
    fn path() {
        assert_eq!(
            super::app_path("org-gnome-Calculator"),
            "/org/gnome/desktop/notifications/application/org-gnome-Calculator/"
        );
    }
}
