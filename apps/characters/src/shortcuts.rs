//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[("win.search", &["<Control>f"])];

pub const GROUPS: &[Group] = &[
    (
        n_("Characters"),
        &[
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Show the Character's Details"), "Return"),
            Keys(n_("Copy the Character (in its details)"), "<Control>c"),
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
