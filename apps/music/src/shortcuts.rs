//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

pub const GROUPS: &[Group] = &[
    (
        "Playback",
        &[
            Keys("Play or Pause", "space <Control>space"),
            Keys("Previous Song", "<Control>b"),
            Keys("Next Song", "<Control>n"),
            Keys("Cycle Repeat Mode", "<Control>r"),
            Keys("Shuffle", "<Control>s"),
        ],
    ),
    (
        "Navigation",
        &[
            Keys("Albums", "<Alt>1"),
            Keys("Artists", "<Alt>2"),
            Keys("Songs", "<Alt>3"),
            Keys("Playlists", "<Alt>4"),
            Keys("Search", "<Control>f"),
            Keys("Go Back", "Escape <Alt>Left"),
        ],
    ),
    (
        "General",
        &[
            Keys("Rescan Library", "F5"),
            Keys("Preferences", "<Control>comma"),
            Keys("Keyboard Shortcuts", "<Control>question"),
            Keys("Close Window", "<Control>w"),
            Keys("Quit", "<Control>q"),
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
