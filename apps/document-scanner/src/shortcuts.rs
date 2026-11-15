//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys};

pub const GROUPS: &[Group] = &[
    (
        n_("Scanning"),
        &[
            Keys(n_("Scan a Single Page"), "<Control>1"),
            Keys(n_("Scan All Pages from the Feeder"), "<Control>f"),
            Keys(n_("Scan Multiple Pages from the Flatbed"), "<Control>m"),
            Keys(n_("Stop Scanning"), "Escape"),
            Keys(n_("Refresh Scanners"), "F5"),
        ],
    ),
    (
        n_("Document"),
        &[
            Keys(n_("New Document"), "<Control>n"),
            Keys(n_("Save"), "<Control>s"),
            Keys(n_("Copy Page"), "<Control>c"),
            Keys(n_("Print"), "<Control>p"),
            Keys(n_("Email"), "<Control>e"),
        ],
    ),
    (
        n_("Pages"),
        &[
            Keys(n_("Rotate Page Left"), "bracketleft"),
            Keys(n_("Rotate Page Right"), "bracketright"),
            Keys(n_("Move Page Left"), "<Control>Page_Up"),
            Keys(n_("Move Page Right"), "<Control>Page_Down"),
            Keys(n_("Delete Page"), "Delete"),
            Keys(n_("Page Menu"), "<Shift>F10"),
            Keys(n_("Move Crop Area"), "Left Up Right Down"),
            Keys(
                n_("Resize Crop Area"),
                "<Shift>Left <Shift>Up <Shift>Right <Shift>Down",
            ),
            Keys(
                n_("Larger Crop Steps"),
                "<Control>Left <Control><Shift>Left",
            ),
        ],
    ),
    (
        n_("General"),
        &[
            Keys(n_("Preferences"), "<Control>comma"),
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
            Keys(n_("Close Window"), "<Control>w"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_accelerator_is_listed() {
        helix_app_common::shortcuts::assert_all_listed(super::GROUPS, crate::ui::ACCELS);
    }
}
