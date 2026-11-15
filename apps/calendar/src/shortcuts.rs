//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys};

const GROUPS: &[Group] = &[
    (
        n_("Events"),
        &[
            Keys(n_("New Event on the Selected Day"), "<Control>n"),
            Keys(n_("Import"), "<Control>i"),
            Keys(n_("Search"), "<Control>f"),
        ],
    ),
    (
        n_("Navigation"),
        &[
            Keys(n_("Go to Today"), "<Control>t"),
            Keys(n_("Previous Week or Month"), "<Alt>Left"),
            Keys(n_("Next Week or Month"), "<Alt>Right"),
            Keys(n_("Week View"), "<Control>1"),
            Keys(n_("Month View"), "<Control>2"),
            Keys(n_("Show Sidebar"), "F9"),
        ],
    ),
    (
        n_("Month View and Day Headers"),
        &[
            Keys(n_("Move Between Days"), "Left Right Up Down"),
            Keys(n_("First or Last Day of the Week"), "Home End"),
            Keys(
                n_("Previous or Next Month (Week in Week View)"),
                "Page_Up Page_Down",
            ),
            Keys(n_("Select the Focused Day"), "Return"),
        ],
    ),
    (
        n_("Week Grid"),
        &[
            Keys(n_("Move the Time Cursor"), "Up Down Left Right"),
            Keys(n_("Start or End of the Day"), "Home End"),
            Keys(n_("Previous or Next Week"), "Page_Up Page_Down"),
            Keys(n_("New Event at the Time Cursor"), "Return"),
            Keys(n_("Zoom In"), "<Control>plus <Control>equal"),
            Keys(n_("Zoom Out"), "<Control>minus"),
            Keys(n_("Reset Zoom"), "<Control>0"),
        ],
    ),
    (
        n_("General"),
        &[
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
];

/// Shows the shortcuts window over `parent`.
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
        for (action, keys) in crate::ui::ACCELS {
            for key in *keys {
                assert!(listed.contains(key), "{action} {key} is not listed");
            }
        }
        assert!(listed.contains(&"<Control>q"), "Quit is not listed");
    }
}
