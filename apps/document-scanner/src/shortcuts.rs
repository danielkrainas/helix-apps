//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

pub const GROUPS: &[Group] = &[
    (
        "Scanning",
        &[
            Keys("Scan a Single Page", "<Control>1"),
            Keys("Scan All Pages from the Feeder", "<Control>f"),
            Keys("Scan Multiple Pages from the Flatbed", "<Control>m"),
            Keys("Stop Scanning", "Escape"),
            Keys("Refresh Scanners", "F5"),
        ],
    ),
    (
        "Document",
        &[
            Keys("New Document", "<Control>n"),
            Keys("Save", "<Control>s"),
            Keys("Copy Page", "<Control>c"),
            Keys("Print", "<Control>p"),
            Keys("Email", "<Control>e"),
        ],
    ),
    (
        "Pages",
        &[
            Keys("Rotate Page Left", "bracketleft"),
            Keys("Rotate Page Right", "bracketright"),
            Keys("Move Page Left", "<Control>Page_Up"),
            Keys("Move Page Right", "<Control>Page_Down"),
            Keys("Delete Page", "Delete"),
        ],
    ),
    (
        "General",
        &[
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
