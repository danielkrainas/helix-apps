//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[("win.search", &["<Control>f"])];

pub const GROUPS: &[Group] = &[
    (
        "Characters",
        &[
            Keys("Search", "<Control>f"),
            Keys("Show the Character's Details", "Return"),
            Keys("Copy the Character (in its details)", "<Control>c"),
        ],
    ),
    GENERAL,
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_accelerator_is_listed() {
        helix_app_common::shortcuts::assert_all_listed(super::GROUPS, super::ACCELS);
    }
}
