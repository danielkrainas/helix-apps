//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

const GROUPS: &[Group] = &[
    (
        n_("Windows and Tabs"),
        &[
            Keys(n_("New Window"), "<Control>n"),
            Keys(n_("New Tab"), "<Control>t"),
            Keys(n_("Close Tab"), "<Control>w"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
    (
        n_("Navigation"),
        &[
            Keys(n_("Back"), "<Alt>Left Back"),
            Keys(n_("Forward"), "<Alt>Right Forward"),
            Keys(n_("Parent Folder"), "<Alt>Up"),
            Keys(n_("Home Folder"), "<Alt>Home"),
            Keys(n_("Enter a Location"), "<Control>l"),
            Keys(n_("Hide the Location Entry"), "Escape"),
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Reload"), "F5 <Control>r"),
            Keys(n_("Bookmark This Location"), "<Control>d"),
            Pointer(
                n_("Back or Forward"),
                n_("Extra mouse buttons"),
                "input-mouse-symbolic",
            ),
        ],
    ),
    (
        n_("View"),
        &[
            Keys(n_("Grid View"), "<Control>1"),
            Keys(n_("List View"), "<Control>2"),
            Keys(
                n_("Zoom In"),
                "<Control>plus <Control>equal <Control>KP_Add",
            ),
            Keys(n_("Zoom Out"), "<Control>minus <Control>KP_Subtract"),
            Keys(n_("Reset Size"), "<Control>0 <Control>KP_0"),
            Keys(n_("Show Hidden Files"), "<Control>h"),
            Keys(n_("Show Sidebar"), "F9"),
        ],
    ),
    (
        n_("Files and Folders"),
        &[
            Keys(n_("Open"), "<Alt>Down"),
            Keys(n_("Open in New Tab"), "<Control>Return"),
            Keys(n_("New Folder"), "<Shift><Control>n"),
            Keys(n_("Rename"), "F2"),
            Keys(n_("Move to Trash"), "Delete KP_Delete"),
            Keys(n_("Delete Permanently"), "<Shift>Delete <Shift>KP_Delete"),
            Keys(n_("Properties"), "<Alt>Return <Control>i"),
            Keys(n_("Show Item Menu"), "Menu <Shift>F10"),
        ],
    ),
    (
        n_("Editing"),
        &[
            Keys(n_("Select All"), "<Control>a"),
            Keys(n_("Invert Selection"), "<Shift><Control>i"),
            Keys(n_("Cut"), "<Control>x"),
            Keys(n_("Copy"), "<Control>c"),
            Keys(n_("Paste"), "<Control>v"),
            Keys(n_("Undo"), "<Control>z"),
        ],
    ),
    (
        n_("General"),
        &[
            Keys(n_("Preferences"), "<Control>comma"),
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
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
