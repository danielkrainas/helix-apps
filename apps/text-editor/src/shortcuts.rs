//! The Keyboard Shortcuts window (Ctrl+?).

/// One row of the shortcuts window.
enum Item {
    /// A title and its keys (GTK accelerator syntax; alternatives are
    /// separated by spaces, `A...B` is a range).
    Keys(&'static str, &'static str),
    /// A pointer gesture: title, how to do it, icon.
    Pointer(&'static str, &'static str, &'static str),
}

use Item::{Keys, Pointer};

const GROUPS: &[(&str, &[Item])] = &[
    (
        "Documents",
        &[
            Keys("New Tab", "<Control>t"),
            Keys("New Window", "<Control>n"),
            Keys("Open", "<Control>o"),
            Keys("Save", "<Control>s"),
            Keys("Save As", "<Control><Shift>s"),
            Keys("Close Tab", "<Control>w"),
            Keys("Close Window", "<Control><Shift>w"),
            Keys("Quit", "<Control>q"),
        ],
    ),
    (
        "Tabs",
        &[
            Keys("Next Tab", "<Control>Page_Down"),
            Keys("Previous Tab", "<Control>Page_Up"),
            Keys("Switch to Tab 1–9", "<Alt>1...<Alt>9"),
            Keys("Move Tab Right", "<Control><Shift>Page_Down"),
            Keys("Move Tab Left", "<Control><Shift>Page_Up"),
        ],
    ),
    (
        "Editing",
        &[
            Keys("Undo", "<Control>z"),
            Keys("Redo", "<Control><Shift>z"),
            Keys("Indent", "Tab"),
            Keys("Unindent", "<Shift>Tab"),
            Keys("Go to Line", "<Control>i"),
        ],
    ),
    (
        "Find and Replace",
        &[
            Keys("Find", "<Control>f"),
            Keys("Find and Replace", "<Control>h"),
            Keys("Next Match", "<Control>g"),
            Keys("Previous Match", "<Control><Shift>g"),
            Keys("Next Match from the Find Bar", "Return"),
            Keys("Previous Match from the Find Bar", "<Shift>Return"),
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

/// Builds the window shown by `win.show-help-overlay`.
pub fn window() -> gtk::ShortcutsWindow {
    let section = gtk::ShortcutsSection::builder()
        .section_name("shortcuts")
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
    let win = gtk::ShortcutsWindow::builder().modal(true).build();
    win.add_section(&section);
    win
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
