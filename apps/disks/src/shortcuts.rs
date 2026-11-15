//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.attach-image", &["<Control>i"]),
    ("win.smart", &["<Control><Shift>s"]),
];

pub const GROUPS: &[Group] = &[
    (
        n_("Disks"),
        &[
            Keys(n_("Attach Disk Image"), "<Control>i"),
            Keys(n_("SMART Data and Self-Tests"), "<Control><Shift>s"),
        ],
    ),
    (
        n_("Volumes"),
        &[Keys(n_("Select the Previous or Next Volume"), "Left Right")],
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
