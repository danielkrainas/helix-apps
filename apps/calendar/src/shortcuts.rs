//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

const GROUPS: &[Group] = &[
    (
        "Events",
        &[
            Keys("New Event on the Selected Day", "<Control>n"),
            Keys("Import", "<Control>i"),
            Keys("Search", "<Control>f"),
        ],
    ),
    (
        "Navigation",
        &[
            Keys("Go to Today", "<Control>t"),
            Keys("Previous Week or Month", "<Alt>Left"),
            Keys("Next Week or Month", "<Alt>Right"),
            Keys("Week View", "<Control>1"),
            Keys("Month View", "<Control>2"),
            Keys("Show Sidebar", "F9"),
        ],
    ),
    (
        "Month View and Day Headers",
        &[
            Keys("Move Between Days", "Left Right Up Down"),
            Keys("First or Last Day of the Week", "Home End"),
            Keys(
                "Previous or Next Month (Week in Week View)",
                "Page_Up Page_Down",
            ),
            Keys("Select the Focused Day", "Return"),
        ],
    ),
    (
        "Week Grid",
        &[
            Keys("Move the Time Cursor", "Up Down Left Right"),
            Keys("Start or End of the Day", "Home End"),
            Keys("Previous or Next Week", "Page_Up Page_Down"),
            Keys("New Event at the Time Cursor", "Return"),
            Keys("Zoom In", "<Control>plus <Control>equal"),
            Keys("Zoom Out", "<Control>minus"),
            Keys("Reset Zoom", "<Control>0"),
        ],
    ),
    (
        "General",
        &[
            Keys("Keyboard Shortcuts", "<Control>question"),
            Keys("Quit", "<Control>q"),
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
