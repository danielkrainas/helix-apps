//! Keyboard shortcuts window.

use helix_app_common::shortcuts::{
    Group,
    Item::{Keys, Pointer},
};

const GROUPS: &[Group] = &[
    (
        "Tabs and Windows",
        &[
            Keys("New Tab", "<Control><Shift>t"),
            Keys("New Window", "<Control><Shift>n"),
            Keys("Close Tab", "<Control><Shift>w"),
            Keys("Close Window", "<Control><Shift>q"),
            Keys("Show All Tabs", "<Control><Shift>o"),
            Keys("Next Tab", "<Control>Page_Down"),
            Keys("Previous Tab", "<Control>Page_Up"),
            Keys("Switch to Tab 1–9", "<Alt>1...<Alt>9"),
            Keys("Move Tab Right", "<Control><Shift>Page_Down"),
            Keys("Move Tab Left", "<Control><Shift>Page_Up"),
            Keys("Fullscreen", "F11"),
        ],
    ),
    (
        "Terminal",
        &[
            Keys("Copy", "<Control><Shift>c"),
            Keys("Paste", "<Control><Shift>v"),
            Keys("Select All", "<Control><Shift>a"),
            Keys("Context Menu", "Menu <Shift>F10"),
            Pointer("Open Link", "Ctrl + click", "input-mouse-symbolic"),
        ],
    ),
    (
        "Find",
        &[
            Keys("Find", "<Control><Shift>f"),
            Keys("Find Next", "<Control><Shift>g"),
            Keys("Find Previous", "<Control><Shift>h"),
            Keys("Find Next from the Find Bar", "Return"),
            Keys("Find Previous from the Find Bar", "<Shift>Return"),
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
