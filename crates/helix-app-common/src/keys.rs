//! Single-key shortcuts that do not take keys away from focused widgets.
//!
//! GTK runs application accelerators (`set_accels_for_action`) in the
//! capture phase, before the focused widget sees the key. For shortcuts
//! without a modifier that is wrong: Space should press a focused button,
//! the arrow keys should move a focused slider, Escape should close an open
//! popover. [`add_bubble_shortcuts`] installs such shortcuts on the window
//! in the bubble phase instead, so they run only when the focused widget
//! did not use the key.

use gtk::prelude::*;
use gtk::{gio, glib};

/// Adds `(trigger, action)` shortcuts to `window` that run after the
/// focused widget had the chance to handle the key. Triggers use GTK's
/// syntax (`"space"`, `"Left"`, `"<Shift>Right"`, `"m"`); actions are
/// detailed action names (`"win.mute"`, `"win.rate::1.25"`).
pub fn add_bubble_shortcuts(window: &impl IsA<gtk::Widget>, shortcuts: &[(&str, &str)]) {
    let controller = gtk::ShortcutController::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Bubble);
    for (trigger, action) in shortcuts {
        let Some(t) = gtk::ShortcutTrigger::parse_string(trigger) else {
            glib::g_warning!("helix", "invalid shortcut trigger {trigger:?}");
            continue;
        };
        let Ok((name, target)) = gio::Action::parse_detailed_name(action) else {
            glib::g_warning!("helix", "invalid action name {action:?}");
            continue;
        };
        let shortcut = gtk::Shortcut::new(Some(t), Some(gtk::NamedAction::new(&name)));
        shortcut.set_arguments(target.as_ref());
        controller.add_shortcut(shortcut);
    }
    window.add_controller(controller);
}
