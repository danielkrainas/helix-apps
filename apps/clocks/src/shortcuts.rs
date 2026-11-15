//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::shortcuts::{Group, Item::Keys};

/// Accelerators registered on the application.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.page::world", &["<Alt>1"]),
    ("win.page::alarm", &["<Alt>2"]),
    ("win.page::stopwatch", &["<Alt>3"]),
    ("win.page::timer", &["<Alt>4"]),
    ("win.new", &["<Control>n"]),
    ("win.show-help-overlay", &["<Control>question"]),
];

pub const GROUPS: &[Group] = &[
    (
        "General",
        &[
            Keys("World", "<Alt>1"),
            Keys("Alarms", "<Alt>2"),
            Keys("Stopwatch", "<Alt>3"),
            Keys("Timer", "<Alt>4"),
            Keys("New Location, Alarm or Timer", "<Control>n"),
            Keys("Keyboard Shortcuts", "<Control>question"),
            Keys("Close Window", "<Control>w"),
            Keys("Quit", "<Control>q"),
        ],
    ),
    (
        "Stopwatch",
        &[
            Keys("Start or Pause", "space"),
            Keys("Lap", "l"),
            Keys("Reset", "Delete BackSpace"),
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
            let action = format!("win.page::{name}");
            let key = format!("<Alt>{}", i + 1);
            assert!(
                super::ACCELS
                    .iter()
                    .any(|(a, k)| *a == action && k.contains(&key.as_str())),
                "{action}"
            );
        }
    }
}
