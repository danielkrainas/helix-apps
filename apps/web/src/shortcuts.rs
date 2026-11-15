//! The Keyboard Shortcuts window (Ctrl+?, main menu).

/// A title and its keys in GTK accelerator syntax; alternatives are
/// separated by spaces, `A...B` is a range.
type Item = (&'static str, &'static str);

const GROUPS: &[(&str, &[Item])] = &[
    (
        "Tabs and Windows",
        &[
            ("New Tab", "<Control>t"),
            ("New Window", "<Control>n"),
            ("New Private Window", "<Control><Shift>n"),
            ("Close Tab", "<Control>w"),
            ("Reopen Closed Tab", "<Control><Shift>t"),
            ("Close Window", "<Control><Shift>w"),
            ("Show All Tabs", "<Control><Shift>o"),
            ("Next Tab", "<Control>Page_Down <Control>Tab"),
            ("Previous Tab", "<Control>Page_Up <Control><Shift>Tab"),
            ("Switch to Tab 1–9", "<Alt>1...<Alt>9"),
            ("Move Tab Right", "<Control><Shift>Page_Down"),
            ("Move Tab Left", "<Control><Shift>Page_Up"),
            ("Quit", "<Control>q"),
        ],
    ),
    (
        "Navigation",
        &[
            ("Go to Address Bar", "<Control>l F6 <Alt>d"),
            ("Open Suggestion in New Tab", "<Alt>Return <Control>Return"),
            ("Back", "<Alt>Left Back"),
            ("Forward", "<Alt>Right Forward"),
            ("Reload", "<Control>r F5 Reload"),
            ("Reload Without Cache", "<Control><Shift>r <Shift>F5"),
            ("Stop Loading", "Escape"),
            ("Home Page", "<Alt>Home HomePage"),
        ],
    ),
    (
        "Page",
        &[
            ("Find", "<Control>f"),
            ("Next Match", "<Control>g"),
            ("Previous Match", "<Control><Shift>g"),
            ("Print", "<Control>p"),
            ("Reader View", "<Control><Alt>r"),
            ("Bookmark Page", "<Control>d"),
            ("History", "<Control>h"),
        ],
    ),
    (
        "View",
        &[
            (
                "Zoom In",
                "<Control>plus <Control>equal <Control>KP_Add ZoomIn",
            ),
            ("Zoom Out", "<Control>minus <Control>KP_Subtract ZoomOut"),
            ("Reset Zoom", "<Control>0 <Control>KP_0"),
            ("Fullscreen", "F11"),
        ],
    ),
    (
        "General",
        &[
            ("Preferences", "<Control>comma"),
            ("Keyboard Shortcuts", "<Control>question"),
        ],
    ),
];

/// Builds the window shown by `win.show-help-overlay`.
pub fn window() -> gtk::ShortcutsWindow {
    let section = gtk::ShortcutsSection::builder()
        .section_name("shortcuts")
        .build();
    for (title, items) in GROUPS {
        let group = gtk::ShortcutsGroup::builder().title(*title).build();
        for (name, accel) in *items {
            group.add_shortcut(
                &gtk::ShortcutsShortcut::builder()
                    .title(*name)
                    .accelerator(*accel)
                    .build(),
            );
        }
        section.add_group(&group);
    }
    let win = gtk::ShortcutsWindow::builder().modal(true).build();
    win.add_section(&section);
    win
}

#[cfg(test)]
mod tests {
    use super::GROUPS;

    #[test]
    fn every_accelerator_is_listed() {
        let listed: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .flat_map(|(_, keys)| keys.split(' '))
            .collect();
        for (action, keys) in crate::app::ACCELS {
            for key in *keys {
                assert!(listed.contains(key), "{action} {key} is not listed");
            }
        }
    }
}
