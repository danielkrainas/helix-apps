//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys};

pub const GROUPS: &[Group] = &[
    (
        n_("Playback"),
        &[
            Keys(n_("Play or Pause"), "space <Control>space"),
            Keys(n_("Previous Song"), "<Control>b"),
            Keys(n_("Next Song"), "<Control>n"),
            Keys(n_("Cycle Repeat Mode"), "<Control>r"),
            Keys(n_("Shuffle"), "<Control>s"),
        ],
    ),
    (
        n_("Navigation"),
        &[
            Keys(n_("Albums"), "<Alt>1"),
            Keys(n_("Artists"), "<Alt>2"),
            Keys(n_("Songs"), "<Alt>3"),
            Keys(n_("Playlists"), "<Alt>4"),
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Go Back"), "Escape <Alt>Left"),
        ],
    ),
    (
        n_("General"),
        &[
            Keys(n_("Rescan Library"), "F5"),
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
