//! The Keyboard Shortcuts window (Ctrl+?).

use helix_app_common::i18n::n_;
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
        n_("General"),
        &[
            Keys(n_("World"), "<Alt>1"),
            Keys(n_("Alarms"), "<Alt>2"),
            Keys(n_("Stopwatch"), "<Alt>3"),
            Keys(n_("Timer"), "<Alt>4"),
            Keys(n_("New Location, Alarm or Timer"), "<Control>n"),
            Keys(n_("Keyboard Shortcuts"), "<Control>question"),
            Keys(n_("Close Window"), "<Control>w"),
            Keys(n_("Quit"), "<Control>q"),
        ],
    ),
    (
        n_("Stopwatch"),
        &[
            Keys(n_("Start or Pause"), "space"),
            Keys(n_("Lap"), "l"),
            Keys(n_("Reset"), "Delete BackSpace"),
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
