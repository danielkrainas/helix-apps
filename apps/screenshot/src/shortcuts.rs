//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.new", &["Escape", "<Alt>Left"]),
    ("win.copy", &["<Control>c"]),
    ("win.save", &["<Control>s"]),
];

pub const GROUPS: &[Group] = &[
    (
        "Screenshot",
        &[
            Keys("Copy to Clipboard", "<Control>c"),
            Keys("Save", "<Control>s"),
            Keys("New Screenshot", "Escape <Alt>Left"),
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
