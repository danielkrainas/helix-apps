//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys};

const GROUPS: &[Group] = &[
    (
        n_("Contacts"),
        &[
            Keys(n_("New Contact"), "<Control>n"),
            Keys(n_("Edit Contact"), "<Control>e"),
            Keys(n_("Delete Contact"), "Delete"),
            Keys(n_("Import"), "<Control>i"),
        ],
    ),
    (
        n_("Search and Selection"),
        &[
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Cancel Editing or Leave Selection Mode"), "Escape"),
        ],
    ),
    (
        n_("General"),
        &[
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
            Keys(n_("Close Window"), "<Control>w"),
            Keys(n_("Quit"), "<Control>q"),
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
