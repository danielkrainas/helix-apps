//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

pub const GROUPS: &[Group] = &[
    (
        "Documents",
        &[
            Keys("New Window", "<Control>n"),
            Keys("Open", "<Control>o"),
            Keys("Print", "<Control>p"),
            Keys("Properties", "<Alt>Return"),
            Keys("Show Sidebar", "F9"),
        ],
    ),
    (
        "Find and Select",
        &[
            Keys("Find", "<Control>f"),
            Keys("Next Match", "<Control>g"),
            Keys("Previous Match", "<Control><Shift>g"),
            Keys("Copy the Selected Text", "<Control>c"),
            Keys("Select All", "<Control>a"),
            Keys("Caret Navigation", "F7"),
            Keys("Context Menu", "Menu <Shift>F10"),
        ],
    ),
    (
        "Navigation",
        &[
            Keys("Go to Page", "<Control>l"),
            Keys("Back", "<Alt>Left"),
            Keys("Forward", "<Alt>Right"),
        ],
    ),
    (
        "View",
        &[
            Keys("Zoom In", "<Control>plus <Control>equal <Control>KP_Add"),
            Keys("Zoom Out", "<Control>minus <Control>KP_Subtract"),
            Keys("Automatic Zoom", "<Control>0 <Control>KP_0"),
            Keys("Rotate Left", "<Control>Left"),
            Keys("Rotate Right", "<Control>Right"),
            Keys("Night Mode", "<Control>i"),
            Keys("Presentation", "F5"),
            Keys("Fullscreen", "F11"),
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
