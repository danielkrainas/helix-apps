//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.scan-folder", &["<Control>o"]),
    ("win.rescan", &["<Control>r", "F5"]),
    ("win.cancel-scan", &["Escape"]),
    ("win.chart-up", &["<Alt>Up"]),
    ("win.preferences", &["<Control>comma"]),
    // Item actions, for the selected row or the focused chart item.
    ("win.open-item", &["<Control>Return", "<Control>KP_Enter"]),
    ("win.copy-path", &["<Control><Shift>c"]),
];

pub const GROUPS: &[Group] = &[
    (
        "Scanning",
        &[
            Keys("Scan a Folder", "<Control>o"),
            Keys("Scan Again", "<Control>r F5"),
            Keys("Stop Scanning", "Escape"),
        ],
    ),
    (
        "Items",
        &[
            Keys("Open", "<Control>Return <Control>KP_Enter"),
            Keys("Copy Path", "<Control><Shift>c"),
            Keys("Move to Trash", "Delete"),
            Keys("Show the Item Menu", "Menu <Shift>F10"),
        ],
    ),
    (
        "Chart",
        &[
            Keys("Previous or Next Item", "Left Right"),
            Keys("Towards the Centre or Outwards", "Up Down"),
            Keys("First or Last Item", "Home End"),
            Keys("Open the Folder", "Return"),
            Keys("Parent Folder", "BackSpace <Alt>Up"),
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
        helix_app_common::shortcuts::assert_all_listed(super::GROUPS, super::ACCELS);
    }
}
