//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

const GROUPS: &[Group] = &[
    (
        n_("Documents"),
        &[
            Keys(n_("New Tab"), "<Control>t"),
            Keys(n_("New Window"), "<Control>n"),
            Keys(n_("Open"), "<Control>o"),
            Keys(n_("Save"), "<Control>s"),
            Keys(n_("Save As"), "<Control><Shift>s"),
            Keys(n_("Close Tab"), "<Control>w"),
            Keys(n_("Close Window"), "<Control><Shift>w"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
    (
        n_("Tabs"),
        &[
            Keys(n_("Next Tab"), "<Control>Page_Down"),
            Keys(n_("Previous Tab"), "<Control>Page_Up"),
            Keys(n_("Switch to Tab 1–9"), "<Alt>1...<Alt>9"),
            Keys(n_("Move Tab Right"), "<Control><Shift>Page_Down"),
            Keys(n_("Move Tab Left"), "<Control><Shift>Page_Up"),
        ],
    ),
    (
        n_("Editing"),
        &[
            Keys(n_("Undo"), "<Control>z"),
            Keys(n_("Redo"), "<Control><Shift>z"),
            Keys(n_("Indent"), "Tab"),
            Keys(n_("Unindent"), "<Shift>Tab"),
            Keys(n_("Go to Line"), "<Control>i"),
        ],
    ),
    (
        n_("Find and Replace"),
        &[
            Keys(n_("Find"), "<Control>f"),
            Keys(n_("Find and Replace"), "<Control>h"),
            Keys(n_("Next Match"), "<Control>g"),
            Keys(n_("Previous Match"), "<Control><Shift>g"),
            Keys(n_("Next Match from the Find Bar"), "Return"),
            Keys(n_("Previous Match from the Find Bar"), "<Shift>Return"),
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

/// Builds the window shown by `win.show-help-overlay`.
pub fn window() -> gtk::ShortcutsWindow {
    helix_app_common::shortcuts::window(GROUPS)
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
