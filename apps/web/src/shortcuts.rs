//! The Keyboard Shortcuts window (Ctrl+?, main menu).

use helix_app_common::shortcuts::{Group, Item::Keys};

const GROUPS: &[Group] = &[
    (
        "Tabs and Windows",
        &[
            Keys("New Tab", "<Control>t"),
            Keys("New Window", "<Control>n"),
            Keys("New Private Window", "<Control><Shift>n"),
            Keys("Close Tab", "<Control>w"),
            Keys("Reopen Closed Tab", "<Control><Shift>t"),
            Keys("Close Window", "<Control><Shift>w"),
            Keys("Show All Tabs", "<Control><Shift>o"),
            Keys("Next Tab", "<Control>Page_Down <Control>Tab"),
            Keys("Previous Tab", "<Control>Page_Up <Control><Shift>Tab"),
            Keys("Switch to Tab 1–9", "<Alt>1...<Alt>9"),
            Keys("Move Tab Right", "<Control><Shift>Page_Down"),
            Keys("Move Tab Left", "<Control><Shift>Page_Up"),
            Keys("Quit", "<Control>q"),
        ],
    ),
    (
        "Navigation",
        &[
            Keys("Go to Address Bar", "<Control>l F6 <Alt>d"),
            Keys("Open Suggestion in New Tab", "<Alt>Return <Control>Return"),
            Keys("Back", "<Alt>Left Back"),
            Keys("Forward", "<Alt>Right Forward"),
            Keys("Reload", "<Control>r F5 Reload"),
            Keys("Reload Without Cache", "<Control><Shift>r <Shift>F5"),
            Keys("Stop Loading", "Escape"),
            Keys("Home Page", "<Alt>Home HomePage"),
        ],
    ),
    (
        "Page",
        &[
            Keys("Find", "<Control>f"),
            Keys("Next Match", "<Control>g"),
            Keys("Previous Match", "<Control><Shift>g"),
            Keys("Print", "<Control>p"),
            Keys("Reader View", "<Control><Alt>r"),
            Keys("Bookmark Page", "<Control>d"),
            Keys("History", "<Control>h"),
        ],
    ),
    (
        "View",
        &[
            Keys(
                "Zoom In",
                "<Control>plus <Control>equal <Control>KP_Add ZoomIn",
            ),
            Keys("Zoom Out", "<Control>minus <Control>KP_Subtract ZoomOut"),
            Keys("Reset Zoom", "<Control>0 <Control>KP_0"),
            Keys("Fullscreen", "F11"),
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
