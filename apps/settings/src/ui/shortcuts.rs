//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

const GROUPS: &[Group] = &[
    (
        "Search",
        &[
            Keys("Search Settings", "<Control>f"),
            Keys("Open the First Result", "Return"),
            Keys("Clear and Close the Search", "Escape"),
        ],
    ),
    (
        "General",
        &[
            Keys("Keyboard Shortcuts", "<Control>question"),
            Keys("About Settings", "F1"),
            Keys("Close Window", "<Control>w"),
            Keys("Quit", "<Control>q"),
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
        // Registered by helix_app_common::install_standard_actions.
        for key in ["<Control>q", "<Control>w"] {
            assert!(listed.contains(&key), "{key} is not listed");
        }
    }
}
