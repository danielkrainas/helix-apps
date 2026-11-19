//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
    GENERAL,
};

pub const GROUPS: &[Group] = &[
    (
        n_("Images"),
        &[
            Keys(n_("New Window"), "<Control>n"),
            Keys(n_("Open"), "<Control>o"),
            Keys(n_("Open With"), "<Control><Shift>o"),
            Keys(n_("Copy"), "<Control>c"),
            Keys(n_("Move to Trash"), "Delete KP_Delete"),
            Keys(n_("Save Rotation"), "<Control>s"),
            Keys(n_("Properties"), "F9 <Alt>Return"),
            Keys(n_("Image Menu"), "Menu <Shift>F10"),
            Pointer(
                n_("Image Menu"),
                n_("Secondary click or long press"),
                "input-mouse-symbolic",
            ),
        ],
    ),
    (
        n_("Navigation"),
        &[
            Keys(n_("Previous Image"), "Page_Up KP_Page_Up Left"),
            Keys(n_("Next Image"), "Page_Down KP_Page_Down Right"),
            Keys(n_("First Image"), "Home"),
            Keys(n_("Last Image"), "End"),
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
            Keys(n_("Best Fit"), "<Control>0 <Control>KP_0"),
            Keys(n_("Original Size"), "<Control>1 <Control>KP_1"),
            Keys(n_("Double Size"), "<Control>2 <Control>KP_2"),
            Keys(n_("Pan a Zoomed Image"), "Left Right Up Down"),
            Keys(n_("Rotate Right"), "<Control>r"),
            Keys(n_("Rotate Left"), "<Control><Shift>r"),
            Keys(n_("Fullscreen"), "F11"),
            Keys(n_("Leave Fullscreen"), "Escape"),
            Keys(n_("Slideshow"), "F5"),
            Keys(n_("Show Thumbnails"), "<Control>F9"),
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
