//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.shutter", &["space", "Return", "KP_Enter"]),
    ("win.cancel-countdown", &["Escape"]),
    ("win.mode::photo", &["<Control>1"]),
    ("win.mode::video", &["<Control>2"]),
    ("win.next-camera", &["<Control>Tab"]),
    ("win.open-gallery", &["<Control>g"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

pub const GROUPS: &[Group] = &[
    (
        n_("Capture"),
        &[
            Keys(
                n_("Take a Photo or Start or Stop Recording"),
                "space Return KP_Enter",
            ),
            Keys(n_("Cancel the Countdown"), "Escape"),
            Keys(n_("Photo Mode"), "<Control>1"),
            Keys(n_("Video Mode"), "<Control>2"),
            Keys(n_("Switch Camera"), "<Control>Tab"),
            Keys(n_("Open the Last Capture"), "<Control>g"),
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
