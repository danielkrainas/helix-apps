//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

const GROUPS: &[Group] = &[
    (
        "Contacts",
        &[
            Keys("New Contact", "<Control>n"),
            Keys("Edit Contact", "<Control>e"),
            Keys("Delete Contact", "Delete"),
            Keys("Import", "<Control>i"),
        ],
    ),
    (
        "Search and Selection",
        &[
            Keys("Search", "<Control>f"),
            Keys("Cancel Editing or Leave Selection Mode", "Escape"),
        ],
    ),
    (
        "General",
        &[
            Keys("Keyboard Shortcuts", "<Control>question"),
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
        // Standard actions and the window's own key handler.
        for key in ["<Control>q", "<Control>w", "Delete", "Escape"] {
            assert!(listed.contains(&key), "{key} is not listed");
        }
    }
}
