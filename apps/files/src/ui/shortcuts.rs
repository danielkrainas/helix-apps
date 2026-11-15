//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

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
pub fn show(parent: &impl gtk::prelude::IsA<gtk::Window>) {
    helix_app_common::shortcuts::show(parent, GROUPS);
}

#[cfg(test)]
mod tests {
    use super::GROUPS;
    use helix_app_common::shortcuts::listed_keys;

    fn listed() -> Vec<&'static str> {
        listed_keys(GROUPS)
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
