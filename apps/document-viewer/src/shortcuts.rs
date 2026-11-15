//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

pub const GROUPS: &[Group] = &[
    (
        n_("Documents"),
        &[
            Keys(n_("New Window"), "<Control>n"),
            Keys(n_("Open"), "<Control>o"),
            Keys(n_("Print"), "<Control>p"),
            Keys(n_("Properties"), "<Alt>Return"),
            Keys(n_("Show Sidebar"), "F9"),
        ],
    ),
    (
        n_("Find and Select"),
        &[
            Keys(n_("Find"), "<Control>f"),
            Keys(n_("Next Match"), "<Control>g"),
            Keys(n_("Previous Match"), "<Control><Shift>g"),
            Keys(n_("Copy the Selected Text"), "<Control>c"),
            Keys(n_("Select All"), "<Control>a"),
            Keys(n_("Caret Navigation"), "F7"),
            Keys(n_("Context Menu"), "Menu <Shift>F10"),
        ],
    ),
    (
        n_("Navigation"),
        &[
            Keys(n_("Go to Page"), "<Control>l"),
            Keys(n_("Back"), "<Alt>Left"),
            Keys(n_("Forward"), "<Alt>Right"),
        ],
    ),
    (
        n_("View"),
        &[
            Keys(
                n_("Zoom In"),
                "<Control>plus <Control>equal <Control>KP_Add",
            ),
            Keys(n_("Zoom Out"), "<Control>minus <Control>KP_Subtract"),
            Keys(n_("Automatic Zoom"), "<Control>0 <Control>KP_0"),
            Keys(n_("Rotate Left"), "<Control>Left"),
            Keys(n_("Rotate Right"), "<Control>Right"),
            Keys(n_("Night Mode"), "<Control>i"),
            Keys(n_("Presentation"), "F5"),
            Keys(n_("Fullscreen"), "F11"),
        ],
    ),
    GENERAL,
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_accelerator_is_listed() {
        helix_app_common::shortcuts::assert_all_listed(super::GROUPS, crate::ui::ACCELS);
    }
}
