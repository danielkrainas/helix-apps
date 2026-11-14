//! Notifications panel.

use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;

use super::widgets::{self, switch};
use super::{Ctx, Panel};

const NOTIFICATIONS: &str = "org.gnome.desktop.notifications";
const APP_SCHEMA: &str = "org.gnome.desktop.notifications.application";

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
            "Do Not Disturb",
            "Notification banners are hidden",
        ));
        general.add(&switch(
            &prefs,
            "show-in-lock-screen",
            true,
            false,
            "Lock Screen Notifications",
            "Show notifications on the lock screen",
        ));
        page.add(&general);

        let schema = helix_app_common::lookup_schema(APP_SCHEMA);
        let children: Vec<String> = prefs.get::<Vec<String>>("application-children", Vec::new());
        if let Some(schema) = schema.filter(|_| !children.is_empty()) {
            let apps = widgets::group("Apps");
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
                let image = gtk::Image::builder().pixel_size(32).build();
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
    #[test]
    fn path() {
        assert_eq!(
            super::app_path("org-gnome-Calculator"),
            "/org/gnome/desktop/notifications/application/org-gnome-Calculator/"
        );
    }
}
