//! The Keyboard Shortcuts window (Ctrl+?, main menu).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys};

const GROUPS: &[Group] = &[
    (
        n_("Tabs and Windows"),
        &[
            Keys(n_("New Tab"), "<Control>t"),
            Keys(n_("New Window"), "<Control>n"),
            Keys(n_("New Private Window"), "<Control><Shift>n"),
            Keys(n_("Close Tab"), "<Control>w"),
            Keys(n_("Reopen Closed Tab"), "<Control><Shift>t"),
            Keys(n_("Mute or Unmute Tab"), "<Control>m"),
            Keys(n_("Tab Menu"), "<Shift>F10 Menu"),
            Keys(n_("Close Window"), "<Control><Shift>w"),
            Keys(n_("Show All Tabs"), "<Control><Shift>o"),
            Keys(n_("Next Tab"), "<Control>Page_Down <Control>Tab"),
            Keys(n_("Previous Tab"), "<Control>Page_Up <Control><Shift>Tab"),
            Keys(n_("Switch to Tab 1–9"), "<Alt>1...<Alt>9"),
            Keys(n_("Move Tab Right"), "<Control><Shift>Page_Down"),
            Keys(n_("Move Tab Left"), "<Control><Shift>Page_Up"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
    (
        n_("Navigation"),
        &[
            Keys(n_("Go to Address Bar"), "<Control>l <Alt>d"),
            Keys(n_("Next Pane (Address Bar, Prompts, Page)"), "F6"),
            Keys(n_("Previous Pane"), "<Shift>F6"),
            Keys(
                n_("Open Suggestion in New Tab"),
                "<Alt>Return <Control>Return",
            ),
            Keys(n_("Back"), "<Alt>Left Back"),
            Keys(n_("Forward"), "<Alt>Right Forward"),
            Keys(n_("Reload"), "<Control>r F5 Reload"),
            Keys(n_("Reload Without Cache"), "<Control><Shift>r <Shift>F5"),
            Keys(n_("Stop Loading"), "Escape"),
            Keys(n_("Home Page"), "<Alt>Home HomePage"),
        ],
    ),
    (
        n_("Page"),
        &[
            Keys(n_("Find"), "<Control>f"),
            Keys(n_("Next Match"), "<Control>g"),
            Keys(n_("Previous Match"), "<Control><Shift>g"),
            Keys(n_("Print"), "<Control>p"),
            Keys(n_("Reader View"), "<Control><Alt>r"),
            Keys(n_("Bookmark Page"), "<Control>d"),
            Keys(n_("Site Information"), "<Control>i"),
            Keys(n_("History"), "<Control>h"),
        ],
    ),
    (
        n_("View"),
        &[
            Keys(
                n_("Zoom In"),
                "<Control>plus <Control>equal <Control>KP_Add ZoomIn",
            ),
            Keys(
                n_("Zoom Out"),
                "<Control>minus <Control>KP_Subtract ZoomOut",
            ),
            Keys(n_("Reset Zoom"), "<Control>0 <Control>KP_0"),
            Keys(n_("Fullscreen"), "F11"),
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
