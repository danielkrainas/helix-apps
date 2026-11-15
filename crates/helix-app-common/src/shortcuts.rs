//! Table-driven Keyboard Shortcuts windows (Ctrl+?).
//!
//! An app describes its shortcuts as a `&[Group]` table and shows it with
//! [`show`]. Keeping the table next to the app's accelerator table lets a
//! unit test check, with [`listed_keys`], that every registered key is
//! listed:
//!
//! ```ignore
//! for (action, keys) in ACCELS {
//!     for key in *keys {
//!         assert!(listed_keys(GROUPS).contains(key), "{action} {key}");
//!     }
//! }
//! ```
//!
//! Apps keep their accelerators in an `ACCELS` table and register it with
//! [`install`], which also adds `app.shortcuts` (Ctrl+?) to open the window, and check the table in a unit test with
//! [`assert_all_listed`]. The primary-menu entry is [`MENU_LABEL`], placed
//! after Preferences and before About.

use gtk::prelude::*;

/// The primary-menu label for the shortcuts window.
pub const MENU_LABEL: &str = "_Keyboard Shortcuts";

/// The action [`install`] adds.
pub const ACTION: &str = "app.shortcuts";

/// Accelerators every app gets from [`crate::install_standard_actions`]
/// and [`install`].
pub const STANDARD_ACCELS: &[(&str, &[&str])] = &[
    ("app.quit", &["<Control>q"]),
    ("window.close", &["<Control>w"]),
    (ACTION, &["<Control>question"]),
];

/// The "General" group most apps end with: shortcuts window, close, quit.
pub const GENERAL: Group = (
    "General",
    &[
        Item::Keys("Keyboard Shortcuts", "<Control>question"),
        Item::Keys("Close Window", "<Control>w"),
        Item::Keys("Quit", "<Control>q"),
    ],
);

/// Registers each `(action, keys)` of `accels` on `app`.
pub fn set_accels(app: &impl IsA<gtk::Application>, accels: &[(&str, &[&str])]) {
    for (action, keys) in accels {
        app.as_ref().set_accels_for_action(action, keys);
    }
}

/// Registers `accels` on `app` and adds `app.shortcuts` (Ctrl+?), which
/// shows the shortcuts window for `groups` over the active window.
pub fn install(
    app: &impl IsA<gtk::Application>,
    accels: &[(&str, &[&str])],
    groups: &'static [Group],
) {
    set_accels(app, accels);
    let app = app.as_ref();
    let action = gtk::gio::SimpleAction::new("shortcuts", None);
    let weak = app.downgrade();
    action.connect_activate(move |_, _| {
        if let Some(win) = weak.upgrade().and_then(|a| a.active_window()) {
            show(&win, groups);
        }
    });
    app.add_action(&action);
    app.set_accels_for_action(ACTION, &["<Control>question"]);
}

/// Panics unless every key of `accels` and of [`STANDARD_ACCELS`] is
/// listed in `groups`. For the apps' unit tests.
pub fn assert_all_listed(groups: &[Group], accels: &[(&str, &[&str])]) {
    let listed = listed_keys(groups);
    for (action, keys) in accels.iter().chain(STANDARD_ACCELS) {
        for key in *keys {
            assert!(
                listed.contains(key),
                "{action} {key} is not in the shortcuts window"
            );
        }
    }
}

/// One row of a shortcuts window.
pub enum Item {
    /// A title and its keys in GTK accelerator syntax; alternatives are
    /// separated by spaces.
    Keys(&'static str, &'static str),
    /// A pointer or touch gesture: title, how to do it, icon name.
    Pointer(&'static str, &'static str, &'static str),
}

/// A titled group of rows.
pub type Group = (&'static str, &'static [Item]);

/// Every key listed in `groups`, one entry per alternative.
pub fn listed_keys(groups: &[Group]) -> Vec<&'static str> {
    groups
        .iter()
        .flat_map(|(_, items)| items.iter())
        .filter_map(|i| match i {
            Item::Keys(_, keys) => Some(*keys),
            Item::Pointer(..) => None,
        })
        .flat_map(|keys| keys.split(' '))
        .collect()
}

/// Builds the shortcuts window for `groups`.
pub fn window(groups: &[Group]) -> gtk::ShortcutsWindow {
    window_from(groups.iter().map(|(title, items)| (*title, *items)))
}

/// Builds the shortcuts window for groups made at run time, for apps that
/// generate them from a richer table of their own.
pub fn window_from<'a>(
    groups: impl IntoIterator<Item = (&'a str, &'a [Item])>,
) -> gtk::ShortcutsWindow {
    let section = gtk::ShortcutsSection::builder()
        .section_name("main")
        .build();
    for (title, items) in groups {
        let group = gtk::ShortcutsGroup::builder().title(title).build();
        for item in items {
            let row = match item {
                Item::Keys(name, accel) => gtk::ShortcutsShortcut::builder()
                    .title(*name)
                    .accelerator(*accel)
                    .build(),
                Item::Pointer(name, how, icon) => gtk::ShortcutsShortcut::builder()
                    .shortcut_type(gtk::ShortcutType::Gesture)
                    .title(*name)
                    .subtitle(*how)
                    .icon(&gtk::gio::ThemedIcon::new(icon))
                    .build(),
            };
            group.add_shortcut(&row);
        }
        section.add_group(&group);
    }
    let win = gtk::ShortcutsWindow::builder().modal(true).build();
    win.add_section(&section);
    win
}

/// Shows the shortcuts window for `groups` over `parent`.
pub fn show(parent: &impl IsA<gtk::Window>, groups: &[Group]) {
    let win = window(groups);
    win.set_transient_for(Some(parent));
    win.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternatives_are_split() {
        const G: &[Group] = &[(
            "A",
            &[
                Item::Keys("Zoom", "<Control>plus <Control>equal"),
                Item::Pointer("Pan", "Drag", "input-mouse-symbolic"),
            ],
        )];
        assert_eq!(listed_keys(G), vec!["<Control>plus", "<Control>equal"]);
    }

    #[test]
    fn general_group_covers_standard_accels() {
        assert_all_listed(&[GENERAL], &[]);
    }

    #[test]
    #[should_panic(expected = "win.x <Control>x")]
    fn missing_key_is_reported() {
        assert_all_listed(&[GENERAL], &[("win.x", &["<Control>x"])]);
    }
}
