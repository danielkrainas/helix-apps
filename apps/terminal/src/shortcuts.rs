//! Keyboard shortcuts window.

use gtk::prelude::*;

/// One row of the shortcuts window.
enum Item {
    /// A title and the keys (GTK accelerator syntax; alternatives are
    /// separated by spaces, `A...B` is a range).
    Keys(&'static str, &'static str),
    /// A pointer gesture: title, how to do it, icon.
    Pointer(&'static str, &'static str, &'static str),
}

use Item::{Keys, Pointer};

type Group = (&'static str, &'static [Item]);

const GROUPS: &[Group] = &[
    (
        "Tabs and Windows",
        &[
            Keys("New Tab", "<Control><Shift>t"),
            Keys("New Window", "<Control><Shift>n"),
            Keys("Close Tab", "<Control><Shift>w"),
            Keys("Close Window", "<Control><Shift>q"),
            Keys("Show All Tabs", "<Control><Shift>o"),
            Keys("Next Tab", "<Control>Page_Down"),
            Keys("Previous Tab", "<Control>Page_Up"),
            Keys("Switch to Tab 1–9", "<Alt>1...<Alt>9"),
            Keys("Move Tab Right", "<Control><Shift>Page_Down"),
            Keys("Move Tab Left", "<Control><Shift>Page_Up"),
            Keys("Fullscreen", "F11"),
        ],
    ),
    (
        "Terminal",
        &[
            Keys("Copy", "<Control><Shift>c"),
            Keys("Paste", "<Control><Shift>v"),
            Keys("Select All", "<Control><Shift>a"),
            Keys("Context Menu", "Menu <Shift>F10"),
            Pointer("Open Link", "Ctrl + click", "input-mouse-symbolic"),
        ],
    ),
    (
        "Find",
        &[
            Keys("Find", "<Control><Shift>f"),
            Keys("Find Next", "<Control><Shift>g"),
            Keys("Find Previous", "<Control><Shift>h"),
            Keys("Find Next from the Find Bar", "Return"),
            Keys("Find Previous from the Find Bar", "<Shift>Return"),
            Keys("Close Find Bar", "Escape"),
        ],
    ),
    (
        "View",
        &[
            Keys("Zoom In", "<Control>plus <Control>equal <Control>KP_Add"),
            Keys("Zoom Out", "<Control>minus <Control>KP_Subtract"),
            Keys("Reset Zoom", "<Control>0 <Control>KP_0"),
            Pointer("Zoom", "Ctrl + scroll", "input-mouse-symbolic"),
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

    #[test]
    fn every_accelerator_is_listed() {
        let listed: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .filter_map(|i| match i {
                Item::Keys(_, keys) => Some(*keys),
                Item::Pointer(..) => None,
            })
            .flat_map(|keys| keys.split(' '))
            .collect();
        for (action, keys) in crate::app::ACCELS {
            for key in *keys {
                assert!(listed.contains(key), "{action} {key} is not listed");
            }
        }
    }
}
