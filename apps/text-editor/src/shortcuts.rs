//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

const GROUPS: &[Group] = &[
    (
        "Documents",
        &[
            Keys("New Tab", "<Control>t"),
            Keys("New Window", "<Control>n"),
            Keys("Open", "<Control>o"),
            Keys("Save", "<Control>s"),
            Keys("Save As", "<Control><Shift>s"),
            Keys("Close Tab", "<Control>w"),
            Keys("Close Window", "<Control><Shift>w"),
            Keys("Quit", "<Control>q"),
        ],
    ),
    (
        "Tabs",
        &[
            Keys("Next Tab", "<Control>Page_Down"),
            Keys("Previous Tab", "<Control>Page_Up"),
            Keys("Switch to Tab 1–9", "<Alt>1...<Alt>9"),
            Keys("Move Tab Right", "<Control><Shift>Page_Down"),
            Keys("Move Tab Left", "<Control><Shift>Page_Up"),
        ],
    ),
    (
        "Editing",
        &[
            Keys("Undo", "<Control>z"),
            Keys("Redo", "<Control><Shift>z"),
            Keys("Indent", "Tab"),
            Keys("Unindent", "<Shift>Tab"),
            Keys("Go to Line", "<Control>i"),
        ],
    ),
    (
        "Find and Replace",
        &[
            Keys("Find", "<Control>f"),
            Keys("Find and Replace", "<Control>h"),
            Keys("Next Match", "<Control>g"),
            Keys("Previous Match", "<Control><Shift>g"),
            Keys("Next Match from the Find Bar", "Return"),
            Keys("Previous Match from the Find Bar", "<Shift>Return"),
            Keys("Close Find Bar", "Escape"),
        ],
    ),
    (
        "View",
        &[
            Keys("Zoom In", "<Control>plus <Control>equal <Control>KP_Add"),
            Keys("Zoom Out", "<Control>minus <Control>KP_Subtract"),
            Keys("Reset Zoom", "<Control>0 <Control>KP_0"),
            Pointer("Zoom", "Ctrl + scroll", "input-mouse-symbolic"),
        ],
    ),
    (
        "General",
        &[
            Keys("Preferences", "<Control>comma"),
            Keys("Keyboard Shortcuts", "<Control>question"),
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
