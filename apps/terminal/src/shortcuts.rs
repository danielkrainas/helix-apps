//! Keyboard shortcuts window.

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

const GROUPS: &[Group] = &[
    (
        n_("Tabs and Windows"),
        &[
            Keys(n_("New Tab"), "<Control><Shift>t"),
            Keys(n_("New Window"), "<Control><Shift>n"),
            Keys(n_("Close Tab"), "<Control><Shift>w"),
            Keys(n_("Close Window"), "<Control><Shift>q"),
            Keys(n_("Show All Tabs"), "<Control><Shift>o"),
            Keys(n_("Next Tab"), "<Control>Page_Down"),
            Keys(n_("Previous Tab"), "<Control>Page_Up"),
            Keys(n_("Switch to Tab 1–9"), "<Alt>1...<Alt>9"),
            Keys(n_("Move Tab Right"), "<Control><Shift>Page_Down"),
            Keys(n_("Move Tab Left"), "<Control><Shift>Page_Up"),
            Keys(n_("Fullscreen"), "F11"),
        ],
    ),
    (
        n_("Terminal"),
        &[
            Keys(n_("Copy"), "<Control><Shift>c"),
            Keys(n_("Paste"), "<Control><Shift>v"),
            Keys(n_("Select All"), "<Control><Shift>a"),
            Keys(n_("Context Menu"), "Menu <Shift>F10"),
            Pointer(n_("Open Link"), n_("Ctrl + click"), "input-mouse-symbolic"),
        ],
    ),
    (
        n_("Find"),
        &[
            Keys(n_("Find"), "<Control><Shift>f"),
            Keys(n_("Find Next"), "<Control><Shift>g"),
            Keys(n_("Find Previous"), "<Control><Shift>h"),
            Keys(n_("Find Next from the Find Bar"), "Return"),
            Keys(n_("Find Previous from the Find Bar"), "<Shift>Return"),
            Keys(n_("Close Find Bar"), "Escape"),
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
            Keys(n_("Reset Zoom"), "<Control>0 <Control>KP_0"),
            Pointer(n_("Zoom"), n_("Ctrl + scroll"), "input-mouse-symbolic"),
        ],
    ),
    (
        n_("General"),
        &[
            Keys(n_("Preferences"), "<Control>comma"),
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
        ],
    ),
];

pub fn show(parent: &impl gtk::prelude::IsA<gtk::Window>) {
    helix_app_common::shortcuts::show(parent, GROUPS);
}

#[cfg(test)]
mod tests {
    use super::GROUPS;
    use helix_app_common::shortcuts::listed_keys;

    #[test]
    fn every_accelerator_is_listed() {
        let listed = listed_keys(GROUPS);
        for (action, keys) in crate::app::ACCELS {
            for key in *keys {
                assert!(listed.contains(key), "{action} {key} is not listed");
            }
        }
    }
}
