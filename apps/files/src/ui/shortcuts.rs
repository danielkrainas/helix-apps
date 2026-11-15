//! The Keyboard Shortcuts window (Ctrl+?).

use gtk::prelude::*;

/// One row of the shortcuts window.
enum Item {
    /// A title and its keys (GTK accelerator syntax; alternatives are
    /// separated by spaces).
    Keys(&'static str, &'static str),
    /// A pointer gesture: title, how to do it, icon.
    Pointer(&'static str, &'static str, &'static str),
}

use Item::{Keys, Pointer};

type Group = (&'static str, &'static [Item]);

const GROUPS: &[Group] = &[
    (
        "Windows and Tabs",
        &[
            Keys("New Window", "<Control>n"),
            Keys("New Tab", "<Control>t"),
            Keys("Close Tab", "<Control>w"),
            Keys("Quit", "<Control>q"),
        ],
    ),
    (
        "Navigation",
        &[
            Keys("Back", "<Alt>Left Back"),
            Keys("Forward", "<Alt>Right Forward"),
            Keys("Parent Folder", "<Alt>Up"),
            Keys("Home Folder", "<Alt>Home"),
            Keys("Enter a Location", "<Control>l"),
            Keys("Hide the Location Entry", "Escape"),
            Keys("Search", "<Control>f"),
            Keys("Reload", "F5 <Control>r"),
            Keys("Bookmark This Location", "<Control>d"),
            Pointer(
                "Back or Forward",
                "Extra mouse buttons",
                "input-mouse-symbolic",
            ),
        ],
    ),
    (
        "View",
        &[
            Keys("Grid View", "<Control>1"),
            Keys("List View", "<Control>2"),
            Keys("Zoom In", "<Control>plus <Control>equal <Control>KP_Add"),
            Keys("Zoom Out", "<Control>minus <Control>KP_Subtract"),
            Keys("Reset Size", "<Control>0 <Control>KP_0"),
            Keys("Show Hidden Files", "<Control>h"),
            Keys("Show Sidebar", "F9"),
        ],
    ),
    (
        "Files and Folders",
        &[
            Keys("Open", "<Alt>Down"),
            Keys("Open in New Tab", "<Control>Return"),
            Keys("New Folder", "<Shift><Control>n"),
            Keys("Rename", "F2"),
            Keys("Move to Trash", "Delete KP_Delete"),
            Keys("Delete Permanently", "<Shift>Delete <Shift>KP_Delete"),
            Keys("Properties", "<Alt>Return <Control>i"),
            Keys("Show Item Menu", "Menu <Shift>F10"),
        ],
    ),
    (
        "Editing",
        &[
            Keys("Select All", "<Control>a"),
            Keys("Invert Selection", "<Shift><Control>i"),
            Keys("Cut", "<Control>x"),
            Keys("Copy", "<Control>c"),
            Keys("Paste", "<Control>v"),
            Keys("Undo", "<Control>z"),
        ],
    ),
    (
        "General",
        &[
            Keys("Preferences", "<Control>comma"),
            Keys("Keyboard Shortcuts", "<Control>question"),
        ],
    ),
];

/// Shows the Keyboard Shortcuts window for `parent`.
pub fn show(parent: &impl IsA<gtk::Window>) {
    let section = gtk::ShortcutsSection::builder()
        .section_name("main")
        .build();
    for (title, items) in GROUPS {
        let group = gtk::ShortcutsGroup::builder().title(*title).build();
        for item in *items {
            let row = match item {
                Keys(name, accel) => gtk::ShortcutsShortcut::builder()
                    .title(*name)
                    .accelerator(*accel)
                    .build(),
                Pointer(name, how, icon) => gtk::ShortcutsShortcut::builder()
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
    let win = gtk::ShortcutsWindow::builder()
        .transient_for(parent)
        .modal(true)
        .build();
    win.add_section(&section);
    win.present();
}

#[cfg(test)]
mod tests {
    use super::{Item, GROUPS};

    fn listed() -> Vec<&'static str> {
        GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .filter_map(|i| match i {
                Item::Keys(_, keys) => Some(*keys),
                Item::Pointer(..) => None,
            })
            .flat_map(|keys| keys.split(' '))
            .collect()
    }

    #[test]
    fn every_accelerator_is_listed() {
        let listed = listed();
        for (action, keys) in crate::ui::ACCELS {
            for key in *keys {
                assert!(listed.contains(key), "{action} {key} is not listed");
            }
        }
        for (key, action) in crate::ui::window::VIEW_SHORTCUTS {
            assert!(listed.contains(key), "{action} {key} is not listed");
        }
        assert!(listed.contains(&"<Control>q"), "Quit is not listed");
    }

    #[test]
    fn no_key_is_listed_twice() {
        let listed = listed();
        for (i, key) in listed.iter().enumerate() {
            assert!(!listed[..i].contains(key), "{key} is listed twice");
        }
    }
}
