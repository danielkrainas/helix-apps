//! Keeping keyboard focus (and scroll position) across list rebuilds.
//!
//! Many lists are rebuilt from scratch after a change. Rebuilding destroys
//! the focused widget, so focus falls back to the window and screen
//! readers lose their place. To keep it:
//!
//! 1. tag each rebuilt row with a stable key ([`set_key`]) and, inside a
//!    row, the controls that can hold focus with a part name
//!    ([`set_part`], e.g. `"switch"`, `"delete"`);
//! 2. wrap the rebuild in [`rebuild_keeping_focus`] (or call
//!    [`Memo::capture`] before and [`Memo::restore`] after).
//!
//! Focus returns to the same part of the same row. If that row is gone
//! (it was deleted) focus moves to the row that followed it, or else the
//! one before it, or else the given fallback widget (an "Add" button).

use gtk::prelude::*;

const KEY: &str = "helix-focus-key";
const PART: &str = "helix-focus-part";

/// Tags `row` with a stable `key` (an item id) for focus restoration.
pub fn set_key(row: &impl IsA<gtk::Widget>, key: &str) {
    // SAFETY: the key is only ever used with a `String` value.
    unsafe { row.as_ref().set_data(KEY, key.to_string()) };
}

/// Tags a focusable control inside a keyed row with a part name.
pub fn set_part(widget: &impl IsA<gtk::Widget>, part: &str) {
    // SAFETY: the key is only ever used with a `String` value.
    unsafe { widget.as_ref().set_data(PART, part.to_string()) };
}

fn data(w: &gtk::Widget, key: &str) -> Option<String> {
    // SAFETY: both keys are only ever used with `String` values.
    unsafe { w.data::<String>(key).map(|p| p.as_ref().clone()) }
}

/// Keyed rows under `root`, in widget-tree order.
fn keyed_rows(root: &gtk::Widget) -> Vec<(String, gtk::Widget)> {
    fn walk(w: &gtk::Widget, out: &mut Vec<(String, gtk::Widget)>) {
        if let Some(k) = data(w, KEY) {
            out.push((k, w.clone()));
            return;
        }
        let mut c = w.first_child();
        while let Some(ch) = c {
            walk(&ch, out);
            c = ch.next_sibling();
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn find_part(w: &gtk::Widget, part: &str) -> Option<gtk::Widget> {
    if data(w, PART).as_deref() == Some(part) {
        return Some(w.clone());
    }
    let mut c = w.first_child();
    while let Some(ch) = c {
        if let Some(found) = find_part(&ch, part) {
            return Some(found);
        }
        c = ch.next_sibling();
    }
    None
}

/// Where focus should go after a rebuild: the index in `new_keys` of the
/// focused row, or of its nearest surviving neighbour in the old order
/// (following rows first, then preceding ones).
pub fn pick_focus(old_keys: &[String], focused: &str, new_keys: &[String]) -> Option<usize> {
    let pos = |k: &String| new_keys.iter().position(|n| n == k);
    if let Some(i) = new_keys.iter().position(|n| n == focused) {
        return Some(i);
    }
    let at = old_keys.iter().position(|k| k == focused)?;
    old_keys[at + 1..]
        .iter()
        .find_map(pos)
        .or_else(|| old_keys[..at].iter().rev().find_map(pos))
        .or(if new_keys.is_empty() { None } else { Some(0) })
}

/// What had focus inside a container before it was rebuilt.
#[derive(Debug, Default)]
pub struct Memo {
    key: Option<String>,
    part: Option<String>,
    old_keys: Vec<String>,
}

impl Memo {
    /// Records the focused row and part under `root` (nothing if focus is
    /// elsewhere).
    pub fn capture(root: &impl IsA<gtk::Widget>) -> Memo {
        let root = root.as_ref();
        let Some(focus) = root.root().and_then(|r| r.focus()) else {
            return Memo::default();
        };
        if !focus.is_ancestor(root) && &focus != root {
            return Memo::default();
        }
        let mut memo = Memo::default();
        let mut w = Some(focus);
        while let Some(cur) = w {
            if memo.part.is_none() {
                memo.part = data(&cur, PART);
            }
            if let Some(k) = data(&cur, KEY) {
                memo.key = Some(k);
                break;
            }
            if &cur == root {
                break;
            }
            w = cur.parent();
        }
        if memo.key.is_some() {
            memo.old_keys = keyed_rows(root).into_iter().map(|(k, _)| k).collect();
        }
        memo
    }

    /// Whether focus was on a keyed row when captured.
    pub fn had_focus(&self) -> bool {
        self.key.is_some()
    }

    /// Puts focus back (see the module docs). Returns whether focus moved.
    pub fn restore(&self, root: &impl IsA<gtk::Widget>, fallback: Option<&gtk::Widget>) -> bool {
        let Some(key) = &self.key else { return false };
        let rows = keyed_rows(root.as_ref());
        let new_keys: Vec<String> = rows.iter().map(|(k, _)| k.clone()).collect();
        if let Some(i) = pick_focus(&self.old_keys, key, &new_keys) {
            let row = &rows[i].1;
            // The same control of the row (or of the neighbour that took a
            // removed row's place, so repeated presses keep working).
            if let Some(part) = self.part.as_deref().and_then(|p| find_part(row, p)) {
                if part.is_sensitive() && part.grab_focus() {
                    return true;
                }
            }
            // A control was focused but is now unavailable (Move Up on the
            // first row): another control in the row, else the row.
            if self.part.is_some() && row.child_focus(gtk::DirectionType::TabForward) {
                return true;
            }
            if row.grab_focus() || row.child_focus(gtk::DirectionType::TabForward) {
                return true;
            }
        }
        fallback.is_some_and(|f| f.grab_focus())
    }
}

/// Focuses the row tagged `key` under `root`; returns whether it took focus.
pub fn focus_key(root: &impl IsA<gtk::Widget>, key: &str) -> bool {
    keyed_rows(root.as_ref())
        .into_iter()
        .find(|(k, _)| k == key)
        .is_some_and(|(_, w)| w.grab_focus())
}

/// Removes `row` from `list`; if focus was inside it, moves focus to the
/// same `part` (see [`set_part`]) of the row that took its place, else of
/// the row before it, else to the row itself, else to `fallback`.
pub fn remove_row_keeping_focus(
    list: &gtk::ListBox,
    row: &gtk::ListBoxRow,
    part: Option<&str>,
    fallback: Option<&gtk::Widget>,
) {
    let had_focus = row
        .root()
        .and_then(|r| r.focus())
        .is_some_and(|f| &f == row.upcast_ref::<gtk::Widget>() || f.is_ancestor(row));
    let index = row.index();
    list.remove(row);
    if !had_focus {
        return;
    }
    let target = list
        .row_at_index(index)
        .or_else(|| (index > 0).then(|| list.row_at_index(index - 1)).flatten());
    if let Some(t) = target {
        if let Some(p) = part.and_then(|p| find_part(t.upcast_ref(), p)) {
            if p.grab_focus() {
                return;
            }
        }
        if t.grab_focus() {
            return;
        }
    }
    if let Some(f) = fallback {
        f.grab_focus();
    }
}

/// Runs `rebuild` and then restores focus and the vertical scroll position
/// of the nearest scrolled window around `root`.
pub fn rebuild_keeping_focus(
    root: &impl IsA<gtk::Widget>,
    fallback: Option<&gtk::Widget>,
    rebuild: impl FnOnce(),
) {
    let root = root.as_ref().clone();
    let memo = Memo::capture(&root);
    let adj = root
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|s| s.downcast::<gtk::ScrolledWindow>().ok())
        .map(|s| s.vadjustment());
    let value = adj.as_ref().map(|a| a.value());
    // Removing the focused widget can turn the focus ring off; keep it as
    // it was, so keyboard users still see where they are.
    let window = root.root().and_then(|r| r.downcast::<gtk::Window>().ok());
    let visible = window.as_ref().map(|w| w.gets_focus_visible());
    rebuild();
    if memo.restore(&root, fallback) {
        if let (Some(w), Some(v)) = (window, visible) {
            w.set_focus_visible(v);
        }
    }
    if let (Some(adj), Some(value)) = (adj, value) {
        restore_scroll(&adj, value);
    }
}

/// Puts `adj` back at `value` now and, as the rebuilt content gets its
/// size over the next layouts (which can come after an idle callback),
/// again whenever its range changes, for half a second.
pub fn restore_scroll(adj: &gtk::Adjustment, value: f64) {
    use std::cell::RefCell;
    use std::rc::Rc;
    adj.set_value(value);
    let id: Rc<RefCell<Option<gtk::glib::SignalHandlerId>>> = Rc::default();
    *id.borrow_mut() = Some(adj.connect_changed(move |a| a.set_value(value)));
    let weak = adj.downgrade();
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
        if let (Some(adj), Some(id)) = (weak.upgrade(), id.borrow_mut().take()) {
            adj.disconnect(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::pick_focus;

    fn k(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn focus_stays_on_the_same_item() {
        assert_eq!(
            pick_focus(&k(&["a", "b", "c"]), "b", &k(&["a", "b", "c"])),
            Some(1)
        );
        // Moved up: focus follows the item.
        assert_eq!(
            pick_focus(&k(&["a", "b", "c"]), "b", &k(&["b", "a", "c"])),
            Some(0)
        );
    }

    #[test]
    fn deleting_moves_to_a_neighbour() {
        assert_eq!(
            pick_focus(&k(&["a", "b", "c"]), "b", &k(&["a", "c"])),
            Some(1)
        );
        // Last row deleted: the previous one.
        assert_eq!(
            pick_focus(&k(&["a", "b", "c"]), "c", &k(&["a", "b"])),
            Some(1)
        );
        assert_eq!(pick_focus(&k(&["a"]), "a", &k(&[])), None);
    }
}
