//! The Keyboard Shortcuts window (Ctrl+?).

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
        "General",
        &[
            Keys("Processes", "<Control>1"),
            Keys("Resources", "<Control>2"),
            Keys("File Systems", "<Control>3"),
            Keys("Refresh", "<Control>r F5"),
            Keys("Search", "<Control>f"),
            Keys("Preferences", "<Control>comma"),
            Keys("Keyboard Shortcuts", "<Control>question"),
            Keys("Close Window", "<Control>w"),
            Keys("Quit", "<Control>q"),
        ],
    ),
    (
        "Process",
        &[
            Keys("Properties", "<Alt>Return"),
            Keys("Context Menu", "Menu <Shift>F10"),
            Keys("Change Priority", "<Control>n"),
        ],
    ),
    (
        "Signals",
        &[
            Keys("Stop", "<Control>s"),
            Keys("Continue", "<Control>c"),
            Keys("End", "<Control>e"),
            Keys("Kill", "<Control>k"),
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
