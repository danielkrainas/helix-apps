//! Input-method composition (preedit) in text fields.
//!
//! While someone composes Chinese, Japanese or Korean text, keys such as
//! Return, Escape, Tab, Space and the arrows belong to the input method
//! (they pick and confirm candidates). A key controller in the capture
//! phase on a text field sees them first, so it must leave them alone
//! while [`is_composing`]. Call [`track_composition`] once on the field.

use gtk::prelude::*;

const KEY: &str = "helix-ime-composing";

fn set(widget: &gtk::Widget, composing: bool) {
    // SAFETY: the key is only ever used with a `bool` value.
    unsafe { widget.set_data(KEY, composing) };
}

/// Follows the preedit of `field`: a `GtkEntry`, `GtkSearchEntry`,
/// `GtkPasswordEntry`, `GtkText` or `GtkTextView` (others are ignored).
pub fn track_composition(field: &impl IsA<gtk::Widget>) {
    let owner = field.as_ref().clone();
    let text: Option<gtk::Text> = if let Some(t) = owner.downcast_ref::<gtk::Text>() {
        Some(t.clone())
    } else if let Some(e) = owner.dynamic_cast_ref::<gtk::Editable>() {
        e.delegate().and_then(|d| d.downcast::<gtk::Text>().ok())
    } else {
        None
    };
    let weak = owner.downgrade();
    let update = move |preedit: &str| {
        if let Some(w) = weak.upgrade() {
            set(&w, !preedit.is_empty());
        }
    };
    if let Some(t) = text {
        t.connect_preedit_changed(move |_, p| update(p));
    } else if let Some(v) = owner.downcast_ref::<gtk::TextView>() {
        v.connect_preedit_changed(move |_, p| update(p));
    } else {
        return;
    }
    set(&owner, false);
}

/// Whether an input method is composing text in `field` (tracked with
/// [`track_composition`]).
pub fn is_composing(field: &impl IsA<gtk::Widget>) -> bool {
    // SAFETY: as in `set`.
    unsafe {
        field
            .as_ref()
            .data::<bool>(KEY)
            .is_some_and(|p| *p.as_ref())
    }
}
