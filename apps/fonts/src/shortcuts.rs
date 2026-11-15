//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.open", &["<Control>o"]),
    ("win.search", &["<Control>f"]),
];

pub const GROUPS: &[Group] = &[
    (
        "Fonts",
        &[
            Keys("Open a Font File", "<Control>o"),
            Keys("Search", "<Control>f"),
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
