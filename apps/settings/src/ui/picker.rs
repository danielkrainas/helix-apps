//! A searchable "pick one" dialog (time zones, input sources) with
//! loading, "No Results Found" and capped-results states.

use adw::prelude::*;
use gtk::glib;
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_app_common::trnf;
use helix_app_common::Labelled;
use std::cell::RefCell;
use std::rc::Rc;

/// One choice: (id, title, subtitle).
pub type Choice = (String, String, String);

/// At most this many rows are listed; more matches are summarised.
pub const MAX_ROWS: usize = 400;

/// The note under a capped list ("Showing 400 of 1,234 …"), or `None`.
pub fn cap_note(shown: usize, total: usize) -> Option<String> {
    (total > shown).then(|| {
        trf!(
            "Showing {shown} of {total} matches. Type to narrow the search.",
            shown = shown,
            total = total
        )
    })
}

/// What is announced after the list changed for a search.
pub fn count_message(total: usize) -> String {
    match total {
        0 => tr("No results found").to_string(),
        n => trnf!("{n} result", "{n} results", n),
    }
}

type PickFn = Box<dyn Fn(&str)>;

pub struct Picker {
    pub dialog: adw::Dialog,
    pub search: gtk::SearchEntry,
    list: gtk::ListBox,
    stack: gtk::Stack,
    note: gtk::Label,
    on_pick: RefCell<Option<PickFn>>,
    /// Keeps the picker alive while its dialog is open (callbacks only
    /// hold weak references).
    this: RefCell<Option<Rc<Picker>>>,
}

impl Picker {
    /// `loading` is the loading page's text ("Loading Time Zones…"); the
    /// dialog starts on it until [`Picker::set_choices`] is first called.
    pub fn new(title: &str, placeholder: &str, search_name: &str, loading: &str) -> Rc<Picker> {
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(420)
            .content_height(560)
            .build();
        let search = gtk::SearchEntry::builder()
            .placeholder_text(placeholder)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build()
            .labelled(search_name);
        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        helix_app_common::set_accessible_label(&list, title);
        let note = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .margin_start(18)
            .margin_end(18)
            .margin_top(6)
            .margin_bottom(6)
            .visible(false)
            .css_classes(["dim-label", "caption"])
            .build();
        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // Above the list, so it is seen before scrolling through 400 rows.
        list_box.append(&note);
        list_box.append(&list);
        let scrolled = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list_box)
            .build();

        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .build();
        helix_app_common::set_accessible_label(&spinner, loading.trim_end_matches('…'));
        let loading_page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        loading_page.append(&spinner);
        loading_page.append(
            &gtk::Label::builder()
                .label(loading)
                .css_classes(["dim-label"])
                .build(),
        );
        let empty = adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title(tr("No Results Found"))
            .description(tr("Try a different search"))
            .build();
        empty.add_css_class("compact");
        let stack = gtk::Stack::new();
        stack.add_named(&loading_page, Some("loading"));
        stack.add_named(&scrolled, Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.set_visible_child_name("loading");

        let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bx.append(&search);
        bx.append(&stack);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&bx));
        dialog.set_child(Some(&view));

        let picker = Rc::new(Picker {
            dialog,
            search,
            list,
            stack,
            note,
            on_pick: RefCell::default(),
            this: RefCell::default(),
        });
        *picker.this.borrow_mut() = Some(picker.clone());
        let weak = Rc::downgrade(&picker);
        picker.dialog.connect_closed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.this.borrow_mut().take();
            }
        });
        let weak = Rc::downgrade(&picker);
        picker.list.connect_row_activated(move |_, row| {
            if let Some(p) = weak.upgrade() {
                p.pick(&row.widget_name());
            }
        });
        // Enter picks the first result.
        let weak = Rc::downgrade(&picker);
        picker.search.connect_activate(move |_| {
            let Some(p) = weak.upgrade() else { return };
            if let Some(row) = p.list.row_at_index(0) {
                p.pick(&row.widget_name());
            }
        });
        // Down moves from the search into the list.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&picker);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Down {
                if let Some(row) = weak.upgrade().and_then(|p| p.list.row_at_index(0)) {
                    row.grab_focus();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        picker.search.add_controller(keys);
        picker.search.set_key_capture_widget(Some(&picker.dialog));
        // Announce that loading takes a moment, if it does.
        let stack = picker.stack.downgrade();
        let loading = loading.trim_end_matches('…').to_string();
        glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
            if let Some(stack) = stack.upgrade() {
                if stack.visible_child_name().as_deref() == Some("loading") {
                    helix_app_common::announce(&stack, &loading, helix_app_common::Urgency::Polite);
                }
            }
        });
        picker
    }

    /// Runs `f` with the chosen id; the dialog then closes.
    pub fn connect_pick(&self, f: impl Fn(&str) + 'static) {
        *self.on_pick.borrow_mut() = Some(Box::new(f));
    }

    fn pick(&self, id: &str) {
        if let Some(f) = self.on_pick.borrow().as_ref() {
            f(id);
        }
        self.dialog.close();
    }

    /// Shows `choices` (already capped) out of `total` matches for the
    /// current search.
    pub fn set_choices(&self, choices: &[Choice], total: usize) {
        self.list.remove_all();
        for (id, title, subtitle) in choices {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(title.as_str())
                .subtitle(subtitle.as_str())
                .activatable(true)
                .build();
            row.set_widget_name(id);
            self.list.append(&row);
        }
        let note = cap_note(choices.len(), total);
        self.note.set_label(note.as_deref().unwrap_or(""));
        self.note.set_visible(note.is_some());
        self.stack
            .set_visible_child_name(if choices.is_empty() { "empty" } else { "list" });
        if !self.search.text().trim().is_empty() {
            let mut message = count_message(total);
            if note.is_some() {
                message = trf!(
                    "{message}, showing the first {choices}",
                    message = message,
                    choices = choices.len()
                );
            }
            helix_app_common::announce_settled(&self.search, &message);
        }
    }

    pub fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
        self.search.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capping_is_explained() {
        assert_eq!(cap_note(10, 10), None);
        assert_eq!(
            cap_note(400, 1234).unwrap(),
            "Showing 400 of 1234 matches. Type to narrow the search."
        );
        assert_eq!(count_message(0), "No results found");
        assert_eq!(count_message(1), "1 result");
    }
}
