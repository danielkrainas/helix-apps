//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.new", &["Escape", "<Alt>Left"]),
    ("win.copy", &["<Control>c"]),
    ("win.save", &["<Control>s"]),
];

pub const GROUPS: &[Group] = &[
    (
        n_("Screenshot"),
        &[
            Keys(n_("Copy to Clipboard"), "<Control>c"),
            Keys(n_("Save"), "<Control>s"),
            Keys(n_("New Screenshot"), "Escape <Alt>Left"),
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
