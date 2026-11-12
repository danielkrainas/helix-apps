//! Keyboard shortcuts window.

use gtk::prelude::*;

type Group = (&'static str, &'static [(&'static str, &'static str)]);

const GROUPS: &[Group] = &[
    (
        "Tabs and Windows",
        &[
            ("New Tab", "<Control><Shift>t"),
            ("New Window", "<Control><Shift>n"),
            ("Close Tab", "<Control><Shift>w"),
            ("Close Window", "<Control><Shift>q"),
            ("Show All Tabs", "<Control><Shift>o"),
            ("Next Tab", "<Control>Page_Down"),
            ("Previous Tab", "<Control>Page_Up"),
            ("Fullscreen", "F11"),
        ],
    ),
    (
        "Terminal",
        &[
            ("Copy", "<Control><Shift>c"),
            ("Paste", "<Control><Shift>v"),
            ("Select All", "<Control><Shift>a"),
            ("Find", "<Control><Shift>f"),
            ("Find Next", "<Control><Shift>g"),
        ],
    ),
    (
        "View",
        &[("Zoom In", "<Control>plus"), ("Zoom Out", "<Control>minus"), ("Reset Zoom", "<Control>0")],
    ),
    (
        "General",
        &[("Preferences", "<Control>comma"), ("Keyboard Shortcuts", "<Control>question")],
    ),
];

pub fn show(parent: &impl IsA<gtk::Window>) {
    let section = gtk::ShortcutsSection::builder().section_name("main").build();
    for (title, items) in GROUPS {
        let group = gtk::ShortcutsGroup::builder().title(*title).build();
        for (name, accel) in *items {
            group.add_shortcut(
                &gtk::ShortcutsShortcut::builder().title(*name).accelerator(*accel).build(),
            );
        }
        section.add_group(&group);
    }
    let win = gtk::ShortcutsWindow::builder().transient_for(parent).modal(true).build();
    win.add_section(&section);
    win.present();
}
