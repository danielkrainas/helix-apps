//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
use helix_app_common::shortcuts::{Group, Item::Keys};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.show-page::processes", &["<Control>1"]),
    ("win.show-page::resources", &["<Control>2"]),
    ("win.show-page::disks", &["<Control>3"]),
    ("win.refresh", &["<Control>r", "F5"]),
    ("win.search", &["<Control>f"]),
    ("win.preferences", &["<Control>comma"]),
    ("win.proc-properties", &["<Alt>Return"]),
    ("win.proc-priority", &["<Control>n"]),
    ("win.proc-stop", &["<Control>s"]),
    ("win.proc-continue", &["<Control>c"]),
    ("win.proc-end", &["<Control>e"]),
    ("win.proc-kill", &["<Control>k"]),
    ("win.proc-menu", &["Menu", "<Shift>F10"]),
];

pub const GROUPS: &[Group] = &[
    (
        n_("General"),
        &[
            Keys(n_("Processes"), "<Control>1"),
            Keys(n_("Resources"), "<Control>2"),
            Keys(n_("File Systems"), "<Control>3"),
            Keys(n_("Refresh"), "<Control>r F5"),
            Keys(n_("Search"), "<Control>f"),
            Keys(n_("Preferences"), "<Control>comma"),
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
            Keys(n_("Close Window"), "<Control>w"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
    (
        n_("Process"),
        &[
            Keys(n_("Properties"), "<Alt>Return"),
            Keys(n_("Context Menu"), "Menu <Shift>F10"),
            Keys(n_("Change Priority"), "<Control>n"),
        ],
    ),
    (
        n_("Signals"),
        &[
            Keys(n_("Stop"), "<Control>s"),
            Keys(n_("Continue"), "<Control>c"),
            Keys(n_("End"), "<Control>e"),
            Keys(n_("Kill"), "<Control>k"),
        ],
    ),
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_accelerator_is_listed() {
        helix_app_common::shortcuts::assert_all_listed(super::GROUPS, super::ACCELS);
    }

    #[test]
    fn page_accelerators_match_the_pages() {
        for (i, (name, _, _)) in crate::ui::PAGES.iter().enumerate() {
            let action = format!("win.show-page::{name}");
            let key = format!("<Control>{}", i + 1);
            assert!(
                super::ACCELS
                    .iter()
                    .any(|(a, k)| *a == action && k.contains(&key.as_str())),
                "{action}"
            );
        }
    }
}
