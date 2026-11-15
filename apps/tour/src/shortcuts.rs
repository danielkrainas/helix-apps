//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application (the tour's own keys are
/// handled by the window, see `ui::build`).
pub const ACCELS: &[(&str, &[&str])] = &[];

pub const GROUPS: &[Group] = &[
    (
        "Tour",
        &[
            Keys("Previous or Next Page", "Left Right"),
            Keys("Skip the Tour", "Escape"),
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
