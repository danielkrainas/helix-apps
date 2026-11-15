//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

const GROUPS: &[Group] = &[
    (
        "General",
        &[
            Keys("Preferences", "<Control>comma"),
            Keys("Keyboard Shortcuts", "<Control>question"),
            Keys("Close Window", "<Control>w"),
            Keys("Quit", "<Control>q"),
        ],
    ),
    (
        "Map",
        &[
            Keys("Go to Current Location", "<Control>l"),
            Keys(
                "Zoom In",
                "<Control>plus <Control>equal <Control>KP_Add plus equal KP_Add",
            ),
            Keys(
                "Zoom Out",
                "<Control>minus <Control>KP_Subtract minus KP_Subtract",
            ),
            Keys("Pan the Map", "Left Right Up Down"),
            Keys(
                "Pan in Larger Steps",
                "<Shift>Left <Shift>Right <Shift>Up <Shift>Down",
            ),
            Keys("Actions for the Map Centre", "Menu <Shift>F10"),
            Pointer("Pan the Map", "Drag", "input-mouse-symbolic"),
            Pointer(
                "Actions for a Point",
                "Secondary click or long press",
                "input-mouse-symbolic",
            ),
        ],
    ),
    (
        "Search and Directions",
        &[
            Keys("Search", "<Control>f"),
            Keys("Move to the Results", "Down"),
            Keys("Directions", "<Control>d"),
            Keys("Close the Sidebar", "Escape"),
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
