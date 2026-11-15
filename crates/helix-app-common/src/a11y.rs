//! Accessibility helpers shared by the Helix apps.
//!
//! GTK exposes a widget's tooltip as its accessible *description*, not its
//! *name*, so an icon-only button with only a tooltip is read as just
//! "button". The constructors here set both: the tooltip for sighted
//! pointer users and the accessible label for assistive technology.
//!
//! Names are short and do not contain keyboard shortcuts (screen readers
//! announce shortcuts separately); a tooltip may still mention one.
//!
//! Setting `HELIX_A11Y_CHECK=1` makes every app built on
//! [`crate::install_standard_actions`] list the visible buttons that have no
//! accessible name, and the visible entries, drop-downs and scales that are
//! not labelled (a placeholder is not a name), on standard error,
//! re-checking every two seconds, and log every announcement it makes.
//!
//! [`announce`] and friends tell screen readers about status changes that
//! happen without focus moving (see their docs).

use gtk::prelude::*;
use gtk::{accessible, glib};
use std::cell::RefCell;
use std::time::{Duration, Instant};

/// How an announcement treats what the screen reader is currently saying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    /// Waits for current speech to finish. Use for results and progress
    /// that the user asked for: search result counts, "Copy finished",
    /// "Now playing …", "Route found".
    Polite,
    /// Interrupts. Use only for failures and alarms that need attention
    /// now: a playback or loading error, a timer going off.
    Assertive,
}

/// Identical announcements closer together than this are spoken once.
const REPEAT_WINDOW: Duration = Duration::from_secs(2);

/// Drops an announcement that repeats the previous one within
/// [`REPEAT_WINDOW`], so a status set twice is not spoken twice.
#[derive(Default)]
struct Recent {
    last: Option<(String, Instant)>,
}

impl Recent {
    fn admit(&mut self, message: &str, now: Instant) -> bool {
        if let Some((text, at)) = &self.last {
            if text == message && now.duration_since(*at) < REPEAT_WINDOW {
                return false;
            }
        }
        self.last = Some((message.to_string(), now));
        true
    }
}

thread_local! {
    static RECENT: RefCell<Recent> = RefCell::default();
}

/// Asks assistive technology to speak `message` now, on behalf of
/// `widget` (the label or page that shows the status, or the window).
///
/// Announce outcomes, not every step: the start and end of long
/// operations, failures, result counts, alarms. Keep messages short and
/// self-contained. Empty messages and repeats within two seconds are
/// dropped. Toasts need no extra announcement (libadwaita speaks them).
pub fn announce(widget: &impl IsA<gtk::Widget>, message: &str, urgency: Urgency) {
    let message = message.trim();
    if message.is_empty() || !RECENT.with(|r| r.borrow_mut().admit(message, Instant::now())) {
        return;
    }
    let priority = match urgency {
        Urgency::Polite => gtk::AccessibleAnnouncementPriority::Medium,
        Urgency::Assertive => gtk::AccessibleAnnouncementPriority::High,
    };
    if std::env::var_os("HELIX_A11Y_CHECK").is_some() {
        eprintln!("helix-a11y: announce ({urgency:?}): {message}");
    }
    // Only AT-SPI contexts implement announcements; GTK 4.22 crashes
    // (calls a NULL function) when announcing through the fallback "test"
    // context used without an accessibility bus or on Broadway.
    if widget.as_ref().at_context().type_().name() == "GtkAtSpiContext" {
        widget.as_ref().announce(message, priority);
    }
}

const LAST_KEY: &str = "helix-a11y-last-announced";
const PENDING_KEY: &str = "helix-a11y-pending-announcement";

/// Like [`announce`], but only when `message` differs from the previous
/// `announce_change` for the same widget — for statuses refreshed often
/// with mostly the same content ("Now playing …").
pub fn announce_change(widget: &impl IsA<gtk::Widget>, message: &str, urgency: Urgency) {
    let w = widget.as_ref();
    // SAFETY: the key is only ever used with a `String` value.
    let same = unsafe {
        w.data::<String>(LAST_KEY)
            .is_some_and(|p| p.as_ref() == message)
    };
    if same {
        return;
    }
    // SAFETY: as above.
    unsafe { w.set_data(LAST_KEY, message.to_string()) };
    announce(w, message, urgency);
}

/// Politely announces `message` once it has stopped changing for 0.7 s —
/// for statuses that follow typing, such as "3 of 12 matches" in a search
/// bar, so only the settled result is spoken.
pub fn announce_settled(widget: &impl IsA<gtk::Widget>, message: &str) {
    let w = widget.as_ref();
    // SAFETY: the key is only ever used with a `u64` value.
    let generation = unsafe {
        let g = w.data::<u64>(PENDING_KEY).map_or(0, |p| *p.as_ref()) + 1;
        w.set_data(PENDING_KEY, g);
        g
    };
    let weak = w.downgrade();
    let message = message.to_string();
    glib::timeout_add_local_once(Duration::from_millis(700), move || {
        let Some(w) = weak.upgrade() else { return };
        // SAFETY: as above.
        let current = unsafe { w.data::<u64>(PENDING_KEY).map(|p| *p.as_ref()) };
        if current == Some(generation) {
            announce_change(&w, &message, Urgency::Polite);
        }
    });
}

/// Shows or clears an input error in a way that does not rely on colour.
///
/// With `Some(message)`, `input` is marked invalid, linked to the visible
/// `error_label` (accessible "error-message" relation) when there is one,
/// and `message` is announced politely — once, not again on every key
/// press while the same error stays. With `None` the invalid state and the
/// relation are cleared, so the next error is spoken again. The caller
/// still styles and fills the visible label.
pub fn set_input_error(
    input: &impl IsA<gtk::Widget>,
    error_label: Option<&gtk::Label>,
    message: Option<&str>,
) {
    let w = input.as_ref();
    match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(message) => {
            w.update_state(&[accessible::State::Invalid(
                gtk::AccessibleInvalidState::True,
            )]);
            if let Some(label) = error_label {
                w.update_relation(&[accessible::Relation::ErrorMessage(&[
                    label.upcast_ref::<gtk::Accessible>()
                ])]);
            }
            announce_change(w, message, Urgency::Polite);
        }
        None => {
            w.reset_state(gtk::AccessibleState::Invalid);
            w.reset_relation(gtk::AccessibleRelation::ErrorMessage);
            // SAFETY: the key is only ever used with a `String` value.
            unsafe { w.set_data(LAST_KEY, String::new()) };
        }
    }
}

/// Object data key marking a widget whose accessible name was set here.
const NAMED_KEY: &str = "helix-a11y-named";

fn mark_named(widget: &gtk::Widget) {
    // SAFETY: the key is only ever used with a `bool` value.
    unsafe { widget.set_data(NAMED_KEY, true) };
}

fn is_marked(widget: &gtk::Widget) -> bool {
    // SAFETY: the key is only ever used with a `bool` value.
    unsafe { widget.data::<bool>(NAMED_KEY).is_some() }
}

/// Sets `label` as the accessible name of `widget`.
///
/// For a [`gtk::MenuButton`], [`gtk::ColorDialogButton`] or
/// [`gtk::FontDialogButton`] the name is also put on its internal button,
/// which is what assistive technology focuses.
pub fn set_accessible_label(widget: &impl IsA<gtk::Widget>, label: &str) {
    let w = widget.as_ref();
    w.update_property(&[accessible::Property::Label(label)]);
    if w.is::<gtk::MenuButton>()
        || w.is::<gtk::ColorDialogButton>()
        || w.is::<gtk::FontDialogButton>()
    {
        if let Some(inner) = w.first_child().filter(|c| c.is::<gtk::Button>()) {
            inner.update_property(&[accessible::Property::Label(label)]);
            mark_named(&inner);
        }
    }
    mark_named(w);
}

/// Builder-style [`set_accessible_label`]:
/// `gtk::Button::builder().icon_name(…).build().labelled("Back")`.
pub trait Labelled: IsA<gtk::Widget> + Sized {
    fn labelled(self, label: &str) -> Self {
        set_accessible_label(&self, label);
        self
    }
}

impl<T: IsA<gtk::Widget>> Labelled for T {}

/// The accessible name for a button whose tooltip may end in a keyboard
/// shortcut: "Search (Ctrl+F)" → "Search". Other parenthesised text is
/// kept ("Next Match (newer output)").
pub fn name_from_tooltip(tooltip: &str) -> &str {
    let t = tooltip.trim_end();
    if let Some(body) = t.strip_suffix(')') {
        if let Some(open) = body.rfind('(') {
            let inner = &body[open + 1..];
            let is_key = |w: &str| {
                matches!(w, "Ctrl" | "Alt" | "Shift" | "Super" | "Esc" | "Escape")
                    || (w.len() >= 2
                        && w.starts_with('F')
                        && w[1..].chars().all(|c| c.is_ascii_digit()))
            };
            if inner.split(['+', ' ', ',']).any(is_key) {
                return body[..open].trim_end();
            }
        }
    }
    t
}

/// An icon-only button whose tooltip and accessible name are `label`.
pub fn icon_button(icon_name: &str, label: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon_name)
        .tooltip_text(label)
        .build()
        .labelled(label)
}

/// An icon-only toggle button whose tooltip and accessible name are
/// `label`. GTK exposes its on/off state as the "pressed" state.
pub fn icon_toggle_button(icon_name: &str, label: &str) -> gtk::ToggleButton {
    gtk::ToggleButton::builder()
        .icon_name(icon_name)
        .tooltip_text(label)
        .build()
        .labelled(label)
}

/// An icon-only menu button whose tooltip and accessible name are `label`.
pub fn icon_menu_button(icon_name: &str, label: &str) -> gtk::MenuButton {
    gtk::MenuButton::builder()
        .icon_name(icon_name)
        .tooltip_text(label)
        .build()
        .labelled(label)
}

/// Changes the icon, tooltip and accessible name of an icon-only button
/// together, for buttons whose meaning follows a state (Play / Pause,
/// Enter / Leave Fullscreen, …).
pub fn set_icon_and_label(button: &impl IsA<gtk::Widget>, icon_name: &str, label: &str) {
    let w = button.as_ref();
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        b.set_icon_name(icon_name);
    } else if let Some(b) = w.downcast_ref::<gtk::MenuButton>() {
        b.set_icon_name(icon_name);
    }
    w.set_tooltip_text(Some(label));
    set_accessible_label(w, label);
}

/// Names `widget` by a visible `label` widget (accessible "labelled-by"
/// relation), for entries, switches, scales and spin buttons placed
/// outside an `Adw*Row`.
pub fn label_widget(widget: &impl IsA<gtk::Widget>, label: &impl IsA<gtk::Widget>) {
    let w = widget.as_ref();
    w.update_relation(&[accessible::Relation::LabelledBy(&[label
        .as_ref()
        .upcast_ref::<gtk::Accessible>(
    )])]);
    mark_named(w);
}

/// Names the unnamed icon-only buttons inside a third-party composite
/// widget (for example a map's zoom buttons) after their tooltips. Only
/// for widgets the app does not build itself; its own buttons use the
/// constructors above.
pub fn name_buttons_from_tooltips(root: &impl IsA<gtk::Widget>) {
    fn walk(w: &gtk::Widget) {
        if (w.is::<gtk::Button>() || w.is::<gtk::MenuButton>()) && !has_name(w) {
            if let Some(tip) = w.tooltip_text() {
                set_accessible_label(w, name_from_tooltip(&tip));
            }
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            walk(&c);
            child = c.next_sibling();
        }
    }
    walk(root.as_ref());
}

/// Whether `w` (or something inside it) shows text that GTK uses as the
/// accessible name.
fn shows_text(w: &gtk::Widget) -> bool {
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        return l.is_visible() && !l.label().trim().is_empty();
    }
    if let Some(c) = w.downcast_ref::<adw::ButtonContent>() {
        if !c.label().trim().is_empty() {
            return true;
        }
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if shows_text(&c) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

fn has_name(w: &gtk::Widget) -> bool {
    if is_marked(w) || shows_text(w) {
        return true;
    }
    match w.downcast_ref::<gtk::MenuButton>() {
        Some(mb) => mb.label().is_some_and(|l| !l.is_empty()),
        None => w
            .downcast_ref::<gtk::Button>()
            .and_then(|b| b.label())
            .is_some_and(|l| !l.is_empty()),
    }
}

/// Composite widgets whose internal buttons are named by GTK or
/// libadwaita themselves (or are covered by the composite's own name).
fn is_internal_container(w: &gtk::Widget) -> bool {
    w.is::<gtk::WindowControls>()
        || w.is::<gtk::SpinButton>()
        || w.is::<gtk::PopoverMenu>()
        || w.is::<gtk::Calendar>()
        || w.is::<adw::SplitButton>()
        || w.is::<adw::TabBar>()
        || w.is::<adw::TabOverview>()
        || w.is::<adw::ViewSwitcher>()
        || w.is::<adw::ViewSwitcherBar>()
        || w.is::<adw::Banner>()
        || matches!(
            w.type_().name(),
            "AdwToastWidget" | "AdwEntryRow" | "AdwPasswordEntryRow" | "GtkPasswordEntry"
        )
}

/// Describes every mapped button under `root` that has no accessible name,
/// and counts the buttons checked.
pub fn unnamed_buttons(root: &gtk::Widget) -> (Vec<String>, usize) {
    fn walk(w: &gtk::Widget, out: &mut Vec<String>, n: &mut usize) {
        if !w.is_mapped() || is_internal_container(w) {
            return;
        }
        let is_button = w.is::<gtk::Button>() || w.is::<gtk::MenuButton>();
        // A menu button's inner toggle button is checked via the menu button.
        let inner = w.parent().is_some_and(|p| p.is::<gtk::MenuButton>());
        if is_button && !inner {
            *n += 1;
        }
        if is_button && !inner && !has_name(w) {
            let icon = w
                .downcast_ref::<gtk::Button>()
                .and_then(|b| b.icon_name())
                .or_else(|| {
                    w.downcast_ref::<gtk::MenuButton>()
                        .and_then(|b| b.icon_name())
                })
                .unwrap_or_default();
            let tip = w.tooltip_text().unwrap_or_default();
            out.push(format!(
                "{} icon={icon:?} tooltip={tip:?} in {}",
                w.type_().name(),
                w.parent()
                    .map(|p| p.type_().name().to_string())
                    .unwrap_or_default()
            ));
        }
        if is_button && !w.is::<gtk::MenuButton>() {
            return;
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            walk(&c, out, n);
            child = c.next_sibling();
        }
    }
    let mut out = Vec::new();
    let mut n = 0;
    walk(root, &mut out, &mut n);
    (out, n)
}

/// Input widgets whose children are part of the input itself.
fn is_input(w: &gtk::Widget) -> bool {
    w.is::<gtk::Entry>()
        || w.is::<gtk::SearchEntry>()
        || w.is::<gtk::PasswordEntry>()
        || w.is::<gtk::DropDown>()
        || w.is::<gtk::Scale>()
}

/// Describes every mapped entry, search entry, password entry, drop-down
/// and scale under `root` that is neither named here (see
/// [`set_accessible_label`], [`label_widget`]) nor the mnemonic widget of
/// a label. Inputs inside libadwaita rows that label them are skipped.
pub fn unlabelled_inputs(root: &gtk::Widget) -> Vec<String> {
    fn mnemonic_targets(w: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        if let Some(t) = w
            .downcast_ref::<gtk::Label>()
            .and_then(|l| l.mnemonic_widget())
        {
            out.push(t);
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            mnemonic_targets(&c, out);
            child = c.next_sibling();
        }
    }
    fn walk(w: &gtk::Widget, targets: &[gtk::Widget], out: &mut Vec<String>) {
        if !w.is_mapped()
            || matches!(
                w.type_().name(),
                "AdwEntryRow" | "AdwPasswordEntryRow" | "AdwComboRow" | "AdwSpinRow"
            )
            || w.is::<gtk::ScaleButton>()
        {
            return;
        }
        if is_input(w) {
            if !is_marked(w) && !targets.contains(w) {
                let hint = w
                    .downcast_ref::<gtk::Entry>()
                    .and_then(|e| e.placeholder_text())
                    .or_else(|| {
                        w.downcast_ref::<gtk::SearchEntry>()
                            .and_then(|e| e.placeholder_text())
                    })
                    .unwrap_or_default();
                out.push(format!(
                    "{} placeholder={hint:?} in {}",
                    w.type_().name(),
                    w.parent()
                        .map(|p| p.type_().name().to_string())
                        .unwrap_or_default()
                ));
            }
            return;
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            walk(&c, targets, out);
            child = c.next_sibling();
        }
    }
    let mut targets = Vec::new();
    mnemonic_targets(root, &mut targets);
    let mut out = Vec::new();
    walk(root, &targets, &mut out);
    out
}

/// With `HELIX_A11Y_CHECK` set, reports unnamed buttons and unlabelled
/// inputs of every window
/// `app` shows (see the module docs). The check repeats every two seconds,
/// so rows, pages and dialogs shown later are covered; each problem is
/// reported once.
pub fn install_check(app: &impl IsA<gtk::Application>) {
    if std::env::var_os("HELIX_A11Y_CHECK").is_none() {
        return;
    }
    app.connect_window_added(|_, window| {
        let window = window.downgrade();
        let mut seen = std::collections::HashSet::new();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            let Some(window) = window.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !window.is_mapped() {
                return glib::ControlFlow::Continue;
            }
            let title = window.title().unwrap_or_default();
            let (unnamed, total) = unnamed_buttons(window.upcast_ref());
            for line in unnamed {
                if seen.insert(line.clone()) {
                    eprintln!("helix-a11y: unnamed button in “{title}”: {line}");
                }
            }
            for line in unlabelled_inputs(window.upcast_ref()) {
                if seen.insert(line.clone()) {
                    eprintln!("helix-a11y: unlabelled input in “{title}”: {line}");
                }
            }
            if seen.insert(format!("total {total}")) {
                eprintln!("helix-a11y: checked “{title}”: {total} buttons");
            }
            glib::ControlFlow::Continue
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{name_from_tooltip, Recent, REPEAT_WINDOW};
    use std::time::{Duration, Instant};

    #[test]
    fn repeated_announcements_are_spoken_once() {
        let mut r = Recent::default();
        let t0 = Instant::now();
        assert!(r.admit("3 results", t0));
        assert!(!r.admit("3 results", t0 + Duration::from_millis(500)));
        assert!(r.admit("4 results", t0 + Duration::from_millis(600)));
        assert!(r.admit("3 results", t0 + Duration::from_millis(700)));
        let later = t0 + Duration::from_millis(700) + REPEAT_WINDOW;
        assert!(r.admit("3 results", later));
    }

    #[test]
    fn shortcut_is_not_part_of_the_name() {
        assert_eq!(name_from_tooltip("Search (Ctrl+F)"), "Search");
        assert_eq!(name_from_tooltip("Up (Alt+Up)"), "Up");
        assert_eq!(name_from_tooltip("Sidebar (F9)"), "Sidebar");
        assert_eq!(name_from_tooltip("Back"), "Back");
        assert_eq!(
            name_from_tooltip("Next Match (newer output)"),
            "Next Match (newer output)"
        );
        assert_eq!(name_from_tooltip("Zoom (%)"), "Zoom (%)");
    }
}
