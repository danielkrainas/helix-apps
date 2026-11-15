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

use gtk::prelude::*;

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
    let section = gtk::ShortcutsSection::builder()
        .section_name("main")
        .build();
    for (title, items) in groups {
        let group = gtk::ShortcutsGroup::builder().title(*title).build();
        for item in *items {
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
}
