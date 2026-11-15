//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.back", &["<Alt>Left", "Back"]),
    ("win.forward", &["<Alt>Right", "Forward"]),
    ("win.home", &["<Alt>Home"]),
    ("win.search", &["<Control>f"]),
    ("win.print", &["<Control>p"]),
    (
        "win.zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
    ),
    ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
    ("win.zoom-reset", &["<Control>0", "<Control>KP_0"]),
    ("win.close", &["<Control>w"]),
    ("win.sidebar", &["F9"]),
    ("win.caret", &["F7"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

pub const GROUPS: &[Group] = &[
    (
        "Navigation",
        &[
            Keys("Back", "<Alt>Left Back"),
            Keys("Forward", "<Alt>Right Forward"),
            Keys("All Help Documents", "<Alt>Home"),
            Keys("Search", "<Control>f"),
            Keys("Show Sidebar", "F9"),
            Keys("Caret Browsing", "F7"),
        ],
    ),
    (
        "Text Size",
        &[
            Keys(
                "Larger Text",
                "<Control>plus <Control>equal <Control>KP_Add",
            ),
            Keys("Smaller Text", "<Control>minus <Control>KP_Subtract"),
            Keys("Normal Text Size", "<Control>0 <Control>KP_0"),
        ],
    ),
    (
        "General",
        &[
            Keys("Print", "<Control>p"),
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
