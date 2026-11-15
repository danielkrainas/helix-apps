//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.export", &["<Control>s"]),
    ("win.refresh", &["F5", "<Control>r"]),
    ("win.search", &["<Control>f"]),
];

pub const GROUPS: &[Group] = &[
    (
        n_("Logs"),
        &[
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Refresh"), "F5 <Control>r"),
            Keys(n_("Export"), "<Control>s"),
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
