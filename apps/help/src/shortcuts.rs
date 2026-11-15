//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
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
        n_("Navigation"),
        &[
            Keys(n_("Back"), "<Alt>Left Back"),
            Keys(n_("Forward"), "<Alt>Right Forward"),
            Keys(n_("All Help Documents"), "<Alt>Home"),
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Show Sidebar"), "F9"),
            Keys(n_("Caret Browsing"), "F7"),
        ],
    ),
    (
        n_("Text Size"),
        &[
            Keys(
                n_("Larger Text"),
                "<Control>plus <Control>equal <Control>KP_Add",
            ),
            Keys(n_("Smaller Text"), "<Control>minus <Control>KP_Subtract"),
            Keys(n_("Normal Text Size"), "<Control>0 <Control>KP_0"),
        ],
    ),
    (
        n_("General"),
        &[
            Keys(n_("Print"), "<Control>p"),
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
