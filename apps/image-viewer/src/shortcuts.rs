//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys, GENERAL};

pub const GROUPS: &[Group] = &[
    (
        "Images",
        &[
            Keys("New Window", "<Control>n"),
            Keys("Open", "<Control>o"),
            Keys("Open With", "<Control><Shift>o"),
            Keys("Copy", "<Control>c"),
            Keys("Move to Trash", "Delete KP_Delete"),
            Keys("Save Rotation", "<Control>s"),
            Keys("Properties", "F9 <Alt>Return"),
        ],
    ),
    (
        "Navigation",
        &[
            Keys("Previous Image", "Page_Up KP_Page_Up Left"),
            Keys("Next Image", "Page_Down KP_Page_Down Right"),
            Keys("First Image", "Home"),
            Keys("Last Image", "End"),
        ],
    ),
    (
        "View",
        &[
            Keys("Zoom In", "<Control>plus <Control>equal <Control>KP_Add"),
            Keys("Zoom Out", "<Control>minus <Control>KP_Subtract"),
            Keys("Best Fit", "<Control>0 <Control>KP_0"),
            Keys("Original Size", "<Control>1 <Control>KP_1"),
            Keys("Double Size", "<Control>2 <Control>KP_2"),
            Keys("Pan a Zoomed Image", "Left Right Up Down"),
            Keys("Rotate Right", "<Control>r"),
            Keys("Rotate Left", "<Control><Shift>r"),
            Keys("Fullscreen", "F11"),
            Keys("Leave Fullscreen", "Escape"),
            Keys("Slideshow", "F5"),
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
