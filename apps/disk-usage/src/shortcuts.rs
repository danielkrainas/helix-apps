//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
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
        n_("Scanning"),
        &[
            Keys(n_("Scan a Folder"), "<Control>o"),
            Keys(n_("Scan Again"), "<Control>r F5"),
            Keys(n_("Stop Scanning"), "Escape"),
        ],
    ),
    (
        n_("Items"),
        &[
            Keys(n_("Open"), "<Control>Return <Control>KP_Enter"),
            Keys(n_("Copy Path"), "<Control><Shift>c"),
            Keys(n_("Move to Trash"), "Delete"),
            Keys(n_("Show the Item Menu"), "Menu <Shift>F10"),
        ],
    ),
    (
        n_("Chart"),
        &[
            Keys(n_("Previous or Next Item"), "Left Right"),
            Keys(n_("Towards the Centre or Outwards"), "Up Down"),
            Keys(n_("First or Last Item"), "Home End"),
            Keys(n_("Open the Folder"), "Return"),
            Keys(n_("Parent Folder"), "BackSpace <Alt>Up"),
        ],
    ),
    (
        n_("General"),
        &[
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
        helix_app_common::shortcuts::assert_all_listed(super::GROUPS, super::ACCELS);
    }
}
