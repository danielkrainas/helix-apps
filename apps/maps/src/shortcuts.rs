//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

const GROUPS: &[Group] = &[
    (
        n_("General"),
        &[
            Keys(n_("Preferences"), "<Control>comma"),
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
            Keys(n_("Close Window"), "<Control>w"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
    (
        n_("Map"),
        &[
            Keys(n_("Go to Current Location"), "<Control>l"),
            Keys(
                n_("Zoom In"),
                "<Control>plus <Control>equal <Control>KP_Add plus equal KP_Add",
            ),
            Keys(
                n_("Zoom Out"),
                "<Control>minus <Control>KP_Subtract minus KP_Subtract",
            ),
            Keys(n_("Pan the Map"), "Left Right Up Down"),
            Keys(
                n_("Pan in Larger Steps"),
                "<Shift>Left <Shift>Right <Shift>Up <Shift>Down",
            ),
            Keys(n_("Actions for the Map Centre"), "Menu <Shift>F10"),
            Pointer(n_("Pan the Map"), n_("Drag"), "input-mouse-symbolic"),
            Pointer(
                n_("Actions for a Point"),
                n_("Secondary click or long press"),
                "input-mouse-symbolic",
            ),
        ],
    ),
    (
        n_("Search and Directions"),
        &[
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Move to the Results"), "Down"),
            Keys(n_("Directions"), "<Control>d"),
            Keys(n_("Close the Sidebar"), "Escape"),
        ],
    ),
];

/// Shows the shortcuts window over `parent`.
pub fn show(parent: &impl gtk::prelude::IsA<gtk::Window>) {
    helix_app_common::shortcuts::show(parent, GROUPS);
}

#[cfg(test)]
mod tests {
    use super::GROUPS;
    use helix_app_common::shortcuts::listed_keys;

    #[test]
    fn every_accelerator_is_listed() {
        let listed = listed_keys(GROUPS);
        for (action, keys) in crate::ui::ACCELS {
            for key in *keys {
                assert!(listed.contains(key), "{action} {key} is not listed");
            }
        }
        for key in crate::ui::MAP_KEYS {
            assert!(listed.contains(key), "map key {key} is not listed");
        }
        // Standard actions and the window's own key handler.
        for key in ["<Control>q", "<Control>w", "Escape"] {
            assert!(listed.contains(&key), "{key} is not listed");
        }
    }
}
