//! One open document: buffer, editor widgets, find bar and file state.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::{Rc, Weak};

use crate::app::App;
use crate::encoding::{self, TextFormat};
use crate::search::{self, Matcher, SearchOptions};
use crate::settings::Settings;
use crate::textops::{self, Indent};
use crate::view::{EditorView, LineGutter};

/// Maximum number of matches that get highlighted.
const MAX_HIGHLIGHTS: usize = 10_000;

pub struct FindBar {
    pub revealer: gtk::Revealer,
    pub entry: gtk::SearchEntry,
    pub replace_revealer: gtk::Revealer,
    pub replace_entry: gtk::Entry,
    pub replace_toggle: gtk::ToggleButton,
    pub count: gtk::Label,
    pub case_btn: gtk::CheckButton,
    pub word_btn: gtk::CheckButton,
    pub regex_btn: gtk::CheckButton,
}

pub struct Doc {
    pub app: Weak<App>,
    pub id: String,
    pub buffer: gtk::TextBuffer,
    pub view: EditorView,
    pub gutter: LineGutter,
    /// The tab's content; a breakpoint bin so the find bar can adapt to
    /// the tab's width.
    pub root: adw::BreakpointBin,
    pub file: RefCell<Option<gio::File>>,
    pub format: Cell<TextFormat>,
    pub untitled: Cell<u32>,
    pub loading: Cell<bool>,
    etag: RefCell<Option<String>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    saving: Cell<bool>,
    generation: Cell<u64>,
    banner: gtk::Revealer,
    banner_label: gtk::Label,
    banner_reload: gtk::Button,
    pub find: FindBar,
    match_tag: gtk::TextTag,
    /// The current match, marked apart from the selection (which looks
    /// different, or not at all, while the find bar has focus).
    current_tag: gtk::TextTag,
    matches: RefCell<Vec<Range<usize>>>,
    search_pending: Cell<bool>,
    indent: Cell<Indent>,
    auto_indent: Cell<bool>,
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    helix_app_common::set_accessible_label(&b, helix_app_common::a11y::name_from_tooltip(tooltip));
    b.add_css_class("flat");
    b
}

fn option_check(label: &str) -> gtk::CheckButton {
    gtk::CheckButton::builder()
        .label(label)
        .use_underline(true)
        .build()
}

/// Applies the style's match colours to the two search tags, now and
/// whenever the accent colour or the dark or high-contrast style changes.
fn style_match_tags(match_tag: &gtk::TextTag, current_tag: &gtk::TextTag) {
    fn apply(tag: &gtk::TextTag, current: bool) {
        let s = crate::view::match_style(
            current,
            helix_app_common::style::is_dark(),
            helix_app_common::style::is_high_contrast(),
            &helix_app_common::style::accent_rgba(),
        );
        tag.set_background_rgba(Some(&s.background));
        tag.set_foreground_rgba(Some(&s.foreground));
        tag.set_underline(s.underline);
    }
    apply(match_tag, false);
    apply(current_tag, true);
    let style = adw::StyleManager::default();
    for prop in ["dark", "high-contrast", "accent-color"] {
        if style.find_property(prop).is_none() {
            continue;
        }
        let (m, c) = (match_tag.downgrade(), current_tag.downgrade());
        let id: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
        let id2 = id.clone();
        let handler = style.connect_notify_local(Some(prop), move |style, _| {
            match (m.upgrade(), c.upgrade()) {
                (Some(m), Some(c)) => {
                    apply(&m, false);
                    apply(&c, true);
                }
                // The document is gone.
                _ => {
                    if let Some(id) = id2.borrow_mut().take() {
                        style.disconnect(id);
                    }
                }
            }
        });
        *id.borrow_mut() = Some(handler);
    }
}

impl Doc {
    pub fn new(app: &Rc<App>, id: Option<String>) -> Rc<Doc> {
        let buffer = gtk::TextBuffer::new(None);
        let match_tag = buffer.create_tag(Some("search-match"), &[]).expect("tag");
        // Above the match tag, so it wins where both apply.
        let current_tag = buffer.create_tag(Some("search-current"), &[]).expect("tag");
        style_match_tags(&match_tag, &current_tag);

        let view = EditorView::new(&buffer);
        view.add_css_class("helix-editor");
        view.set_monospace(true);
        view.set_left_margin(8);
        view.set_right_margin(8);
        view.set_top_margin(6);
        view.set_bottom_margin(6);
        view.set_vexpand(true);
        let gutter = LineGutter::new(&view);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .build();

        // Reload banner.
        let banner_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .build();
        let banner_reload = gtk::Button::with_label("_Reload");
        banner_reload.set_use_underline(true);
        banner_reload.add_css_class("suggested-action");
        let banner_ignore = gtk::Button::with_label("_Ignore");
        banner_ignore.set_use_underline(true);
        let banner_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        banner_box.add_css_class("change-banner");
        banner_box.append(&banner_label);
        banner_box.append(&banner_ignore);
        banner_box.append(&banner_reload);
        let banner = gtk::Revealer::builder().child(&banner_box).build();

        // Find bar.
        // Small minimum widths: the entries grow into the room there is.
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Find")
            .hexpand(true)
            .width_chars(6)
            .build();
        helix_app_common::set_accessible_label(&entry, "Find");
        let count = gtk::Label::new(None);
        count.add_css_class("dim-label");
        count.add_css_class("numeric");
        let prev = icon_button("go-up-symbolic", "Previous Match (Ctrl+Shift+G)");
        let next = icon_button("go-down-symbolic", "Next Match (Ctrl+G)");
        // Search options live in a menu so the bar fits narrow windows.
        let case_btn = option_check("_Case Sensitive");
        let word_btn = option_check("Match _Whole Words Only");
        let regex_btn = option_check("_Regular Expression");
        let options_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        options_box.append(&case_btn);
        options_box.append(&word_btn);
        options_box.append(&regex_btn);
        let options = helix_app_common::icon_menu_button("view-more-symbolic", "Search Options");
        options.set_popover(Some(&gtk::Popover::builder().child(&options_box).build()));
        options.add_css_class("flat");
        let replace_toggle = gtk::ToggleButton::new();
        replace_toggle.set_icon_name("edit-find-replace-symbolic");
        replace_toggle.set_tooltip_text(Some("Replace (Ctrl+H)"));
        helix_app_common::set_accessible_label(&replace_toggle, "Replace");
        replace_toggle.add_css_class("flat");
        let close = icon_button("window-close-symbolic", "Close (Escape)");
        let find_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for w in [
            entry.upcast_ref::<gtk::Widget>(),
            count.upcast_ref(),
            prev.upcast_ref(),
            next.upcast_ref(),
            options.upcast_ref(),
            replace_toggle.upcast_ref(),
            close.upcast_ref(),
        ] {
            find_row.append(w);
        }
        let replace_entry = gtk::Entry::builder()
            .placeholder_text("Replace")
            .hexpand(true)
            .width_chars(6)
            .build();
        helix_app_common::set_accessible_label(&replace_entry, "Replace With");
        let replace_btn = gtk::Button::with_label("_Replace");
        replace_btn.set_use_underline(true);
        let replace_all_btn = gtk::Button::with_label("Replace _All");
        replace_all_btn.set_use_underline(true);
        let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        replace_row.append(&replace_entry);
        replace_row.append(&replace_btn);
        replace_row.append(&replace_all_btn);
        let replace_revealer = gtk::Revealer::builder().child(&replace_row).build();
        let find_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        find_box.add_css_class("find-bar");
        find_box.append(&find_row);
        find_box.append(&replace_revealer);
        let find_revealer = gtk::Revealer::builder().child(&find_box).build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&banner);
        content.append(&find_revealer);
        content.append(&scroller);
        let root = adw::BreakpointBin::builder()
            .child(&content)
            .width_request(300)
            .height_request(120)
            .build();
        // Narrow tabs: drop the match count from the bar (it is still
        // announced) so the entry keeps room.
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            450.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&count, "visible", Some(&false.to_value()));
        root.add_breakpoint(narrow);
        view.set_gutter(gtk::TextWindowType::Left, Some(&gutter));

        let settings = app.settings.borrow().clone();
        let doc = Rc::new(Doc {
            app: Rc::downgrade(app),
            id: id.unwrap_or_else(crate::session::new_id),
            buffer,
            view,
            gutter,
            root,
            file: RefCell::default(),
            format: Cell::default(),
            untitled: Cell::new(0),
            loading: Cell::new(false),
            etag: RefCell::default(),
            monitor: RefCell::default(),
            saving: Cell::new(false),
            generation: Cell::new(0),
            banner,
            banner_label,
            banner_reload: banner_reload.clone(),
            find: FindBar {
                revealer: find_revealer,
                entry,
                replace_revealer,
                replace_entry,
                replace_toggle,
                count,
                case_btn,
                word_btn,
                regex_btn,
            },
            match_tag,
            current_tag,
            matches: RefCell::default(),
            search_pending: Cell::new(false),
            indent: Cell::new(settings.indent()),
            auto_indent: Cell::new(settings.auto_indent),
        });
        doc.apply_settings(&settings);
        doc.connect_signals(
            &find_box,
            &scroller,
            &banner_ignore,
            &prev,
            &next,
            &close,
            &replace_btn,
            &replace_all_btn,
        );
        doc
    }

    fn weak(self: &Rc<Self>) -> Weak<Self> {
        Rc::downgrade(self)
    }

    #[allow(clippy::too_many_arguments)]
    fn connect_signals(
        self: &Rc<Self>,
        find_box: &gtk::Box,
        scroller: &gtk::ScrolledWindow,
        banner_ignore: &gtk::Button,
        prev: &gtk::Button,
        next: &gtk::Button,
        close: &gtk::Button,
        replace_btn: &gtk::Button,
        replace_all_btn: &gtk::Button,
    ) {
        macro_rules! with_doc {
            ($w:expr, |$d:ident| $body:expr) => {{
                let w = $w.clone();
                move || {
                    if let Some($d) = w.upgrade() {
                        $body;
                    }
                }
            }};
        }
        let w = self.weak();

        self.buffer.connect_changed({
            let f = with_doc!(w, |d| d.on_changed());
            move |_| f()
        });
        self.buffer.connect_modified_changed({
            let f = with_doc!(w, |d| d.notify_state());
            move |_| f()
        });
        self.buffer.connect_notify_local(Some("cursor-position"), {
            let f = with_doc!(w, |d| d.on_cursor_moved());
            move |_, _| f()
        });
        scroller.vadjustment().connect_value_changed({
            let f = with_doc!(w, |d| d.gutter.queue_draw());
            move |_| f()
        });
        self.view.connect_notify_local(Some("wrap-mode"), {
            let f = with_doc!(w, |d| d.gutter.queue_draw());
            move |_, _| f()
        });

        // Banner.
        self.banner_reload.connect_clicked({
            let f = with_doc!(w, |d| d.reload());
            move |_| f()
        });
        banner_ignore.connect_clicked({
            let f = with_doc!(w, |d| d.banner.set_reveal_child(false));
            move |_| f()
        });

        // Keys: auto-indent, tab handling.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let w = w.clone();
            move |_, key, _, state| match w.upgrade() {
                Some(d) => d.on_key(key, state),
                None => glib::Propagation::Proceed,
            }
        });
        self.view.add_controller(keys);

        // Ctrl+scroll zooms.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll(|c, _, dy| {
            let ctrl = c
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            if !ctrl || dy == 0.0 {
                return glib::Propagation::Proceed;
            }
            let action = if dy < 0.0 {
                "app.zoom-in"
            } else {
                "app.zoom-out"
            };
            if let Some(w) = c.widget() {
                w.activate_action(action, None).ok();
            }
            glib::Propagation::Stop
        });
        self.view.add_controller(scroll);

        // Find bar.
        let find = &self.find;
        find.entry.connect_search_changed({
            let f = with_doc!(w, |d| d.refresh_search(true));
            move |_| f()
        });
        for b in [&find.case_btn, &find.word_btn, &find.regex_btn] {
            b.connect_toggled({
                let f = with_doc!(w, |d| d.refresh_search(false));
                move |_| f()
            });
        }
        find.entry.connect_activate({
            let f = with_doc!(w, |d| d.find_next(false));
            move |_| f()
        });
        find.entry.connect_next_match({
            let f = with_doc!(w, |d| d.find_next(false));
            move |_| f()
        });
        find.entry.connect_previous_match({
            let f = with_doc!(w, |d| d.find_next(true));
            move |_| f()
        });
        find.entry.connect_stop_search({
            let f = with_doc!(w, |d| d.hide_find());
            move |_| f()
        });
        let shift_enter = gtk::EventControllerKey::new();
        shift_enter.connect_key_pressed({
            let w = w.clone();
            move |_, key, _, state| {
                let Some(d) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                    && state.contains(gdk::ModifierType::SHIFT_MASK)
                {
                    d.find_next(true);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        find.entry.add_controller(shift_enter);
        // Escape closes the bar from any control in it (the find entry
        // handles it itself through stop-search).
        let esc = gtk::EventControllerKey::new();
        esc.connect_key_pressed({
            let w = w.clone();
            move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    if let Some(d) = w.upgrade() {
                        d.hide_find();
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            }
        });
        find_box.add_controller(esc);
        prev.connect_clicked({
            let f = with_doc!(w, |d| d.find_next(true));
            move |_| f()
        });
        next.connect_clicked({
            let f = with_doc!(w, |d| d.find_next(false));
            move |_| f()
        });
        close.connect_clicked({
            let f = with_doc!(w, |d| d.hide_find());
            move |_| f()
        });
        find.replace_toggle
            .bind_property("active", &find.replace_revealer, "reveal-child")
            .sync_create()
            .build();
        find.replace_entry.connect_activate({
            let f = with_doc!(w, |d| d.replace_one());
            move |_| f()
        });
        replace_btn.connect_clicked({
            let f = with_doc!(w, |d| d.replace_one());
            move |_| f()
        });
        replace_all_btn.connect_clicked({
            let f = with_doc!(w, |d| d.replace_all());
            move |_| f()
        });
    }

    // ---------------------------------------------------------------- state

    pub fn title(&self) -> String {
        if let Some(f) = self.file.borrow().as_ref() {
            return f
                .basename()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.uri().to_string());
        }
        match self.untitled.get() {
            0 | 1 => "New Document".into(),
            n => format!("New Document {n}"),
        }
    }

    pub fn subtitle(&self) -> String {
        let file = self.file.borrow();
        let Some(f) = file.as_ref() else {
            return "Draft".into();
        };
        let Some(parent) = f.parent() else {
            return String::new();
        };
        match parent.path() {
            Some(p) => {
                let home = glib::home_dir();
                match p.strip_prefix(&home) {
                    Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
                    Ok(rest) => format!("~/{}", rest.display()),
                    Err(_) => p.display().to_string(),
                }
            }
            None => parent.uri().to_string(),
        }
    }

    pub fn is_modified(&self) -> bool {
        self.buffer.is_modified()
    }

    /// An untouched, empty new document (can be replaced when opening).
    pub fn is_blank(&self) -> bool {
        self.file.borrow().is_none()
            && !self.is_modified()
            && self.buffer.char_count() == 0
            && !self.loading.get()
    }

    pub fn text(&self) -> String {
        let (s, e) = self.buffer.bounds();
        self.buffer.text(&s, &e, true).to_string()
    }

    pub fn uri(&self) -> Option<String> {
        self.file.borrow().as_ref().map(|f| f.uri().to_string())
    }

    /// (line, column) of the cursor, 0-based, column in characters.
    pub fn cursor(&self) -> (u32, u32) {
        let it = self.buffer.iter_at_mark(&self.buffer.get_insert());
        (it.line().max(0) as u32, it.line_offset().max(0) as u32)
    }

    /// (line, visual column), 1-based, for the status bar.
    pub fn cursor_display(&self) -> (u32, u32) {
        let it = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let mut start = it;
        start.set_line_offset(0);
        let prefix = self.buffer.text(&start, &it, true);
        (
            it.line() as u32 + 1,
            textops::visual_column(&prefix, self.indent.get().tab_width) + 1,
        )
    }

    fn notify_state(self: &Rc<Self>) {
        if let Some(app) = self.app.upgrade() {
            app.doc_state_changed(self);
        }
    }

    fn on_changed(self: &Rc<Self>) {
        self.generation.set(self.generation.get() + 1);
        self.gutter.update_width();
        self.gutter.queue_draw();
        if self.find.revealer.reveals_child() {
            self.queue_search_refresh();
        }
        if let Some(app) = self.app.upgrade() {
            app.schedule_autosave();
        }
    }

    fn on_cursor_moved(self: &Rc<Self>) {
        self.gutter.queue_draw();
        self.view.queue_draw();
        if let Some(app) = self.app.upgrade() {
            app.doc_cursor_moved(self);
        }
        if self.find.revealer.reveals_child() {
            self.update_count();
        }
    }

    pub fn apply_settings(&self, s: &Settings) {
        self.indent.set(s.indent());
        self.auto_indent.set(s.auto_indent);
        self.view.set_wrap_mode(if s.wrap_text {
            gtk::WrapMode::WordChar
        } else {
            gtk::WrapMode::None
        });
        self.gutter.set_visible(s.show_line_numbers);
        self.view
            .set_highlight_current_line(s.highlight_current_line);
        self.view
            .set_right_margin_column(s.show_right_margin.then_some(s.right_margin_position));
        self.gutter.queue_resize();
        self.update_tabs();
        // Font changes apply on the next style update; recompute tab stops then.
        let view = self.view.downgrade();
        let gutter = self.gutter.downgrade();
        let tab_width = s.tab_width;
        glib::idle_add_local_once(move || {
            if let (Some(view), Some(gutter)) = (view.upgrade(), gutter.upgrade()) {
                set_tab_stops(&view, tab_width);
                gutter.queue_resize();
            }
        });
    }

    pub fn update_tabs(&self) {
        set_tab_stops(&self.view, self.indent.get().tab_width);
    }

    /// Replaces the buffer contents without an undo step.
    pub fn set_text(&self, text: &str) {
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(text);
        self.buffer.end_irreversible_action();
        self.buffer.place_cursor(&self.buffer.start_iter());
    }

    pub fn goto(&self, line: u32, col: Option<u32>) {
        let last = self.buffer.line_count().max(1) - 1;
        let line = (line.saturating_sub(1) as i32).min(last);
        let Some(mut it) = self.buffer.iter_at_line(line) else {
            return;
        };
        if let Some(c) = col {
            let mut end = it;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            it.set_line_offset((c.saturating_sub(1) as i32).min(end.line_offset()));
        }
        self.buffer.place_cursor(&it);
        self.view
            .scroll_to_mark(&self.buffer.get_insert(), 0.1, true, 0.0, 0.3);
    }

    pub fn grab_focus(&self) {
        self.view.grab_focus();
    }

    // ---------------------------------------------------------------- keys

    fn on_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let mods = state
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK);
        if !mods.is_empty() || !self.view.is_editable() {
            return glib::Propagation::Proceed;
        }
        let buf = &self.buffer;
        let indent = self.indent.get();
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter if self.auto_indent.get() => {
                if state.contains(gdk::ModifierType::SHIFT_MASK) {
                    return glib::Propagation::Proceed;
                }
                buf.begin_user_action();
                buf.delete_selection(true, true);
                let it = buf.iter_at_mark(&buf.get_insert());
                let mut start = it;
                start.set_line_offset(0);
                let before = buf.text(&start, &it, true);
                let ws = textops::newline_indent(&before, &indent);
                buf.insert_at_cursor(&format!("\n{ws}"));
                buf.end_user_action();
                self.view.scroll_mark_onscreen(&buf.get_insert());
                glib::Propagation::Stop
            }
            gdk::Key::Tab | gdk::Key::KP_Tab | gdk::Key::ISO_Left_Tab => {
                let shift =
                    key == gdk::Key::ISO_Left_Tab || state.contains(gdk::ModifierType::SHIFT_MASK);
                let multi = buf
                    .selection_bounds()
                    .is_some_and(|(s, e)| s.line() != e.line());
                if shift || multi {
                    self.indent_selection(!shift);
                    return glib::Propagation::Stop;
                }
                if indent.use_spaces {
                    buf.begin_user_action();
                    buf.delete_selection(true, true);
                    let it = buf.iter_at_mark(&buf.get_insert());
                    let mut start = it;
                    start.set_line_offset(0);
                    let col =
                        textops::visual_column(&buf.text(&start, &it, true), indent.tab_width);
                    buf.insert_at_cursor(&indent.unit_at(col));
                    buf.end_user_action();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// Indents (or unindents) every line touched by the selection/cursor.
    fn indent_selection(&self, increase: bool) {
        let buf = &self.buffer;
        let indent = self.indent.get();
        let (s, e) = buf.selection_bounds().unwrap_or_else(|| {
            let it = buf.iter_at_mark(&buf.get_insert());
            (it, it)
        });
        let first = s.line();
        let mut last = e.line();
        if e.line_offset() == 0 && e.line() > s.line() {
            last -= 1;
        }
        buf.begin_user_action();
        for line in first..=last {
            let Some(mut ls) = buf.iter_at_line(line) else {
                continue;
            };
            let mut le = ls;
            if !le.ends_line() {
                le.forward_to_line_end();
            }
            let old = buf.text(&ls, &le, true).to_string();
            let new = if increase {
                textops::indent_line(&old, &indent)
            } else {
                textops::unindent_line(&old, &indent)
            };
            if new != old {
                // Only rewrite the leading whitespace to keep marks stable.
                let old_ws = textops::leading_whitespace(&old).chars().count() as i32;
                let new_ws = textops::leading_whitespace(&new).to_string();
                let mut we = ls;
                we.forward_chars(old_ws);
                buf.delete(&mut ls, &mut we);
                buf.insert(&mut ls, &new_ws);
            }
        }
        // Re-select the full lines.
        if let (Some(a), Some(mut b)) = (buf.iter_at_line(first), buf.iter_at_line(last)) {
            if !b.ends_line() {
                b.forward_to_line_end();
            }
            if first != last {
                buf.select_range(&a, &b);
            }
        }
        buf.end_user_action();
    }

    // ---------------------------------------------------------------- find

    pub fn show_find(self: &Rc<Self>, replace: bool) {
        if let Some((s, e)) = self.buffer.selection_bounds() {
            if s.line() == e.line() {
                self.find.entry.set_text(&self.buffer.text(&s, &e, true));
            }
        }
        self.find.revealer.set_reveal_child(true);
        if replace {
            self.find.replace_toggle.set_active(true);
        }
        self.find.entry.grab_focus();
        self.find.entry.select_region(0, -1);
        self.refresh_search(false);
    }

    pub fn hide_find(&self) {
        self.find.revealer.set_reveal_child(false);
        self.find.replace_toggle.set_active(false);
        let (s, e) = self.buffer.bounds();
        self.buffer.remove_tag(&self.match_tag, &s, &e);
        self.buffer.remove_tag(&self.current_tag, &s, &e);
        self.matches.borrow_mut().clear();
        self.view.grab_focus();
    }

    fn options(&self) -> SearchOptions {
        SearchOptions {
            case_sensitive: self.find.case_btn.is_active(),
            whole_word: self.find.word_btn.is_active(),
            regex: self.find.regex_btn.is_active(),
        }
    }

    fn matcher(&self) -> Option<Result<Matcher, String>> {
        let q = self.find.entry.text();
        if q.is_empty() {
            return None;
        }
        Some(Matcher::new(&q, self.options()))
    }

    fn queue_search_refresh(self: &Rc<Self>) {
        if self.search_pending.replace(true) {
            return;
        }
        let w = self.weak();
        glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            if let Some(d) = w.upgrade() {
                d.search_pending.set(false);
                d.refresh_search(false);
            }
        });
    }

    /// Recomputes and highlights all matches. With `jump`, selects the first
    /// match at or after the cursor (incremental search).
    pub fn refresh_search(self: &Rc<Self>, jump: bool) {
        let (s, e) = self.buffer.bounds();
        self.buffer.remove_tag(&self.match_tag, &s, &e);
        self.buffer.remove_tag(&self.current_tag, &s, &e);
        self.find.entry.remove_css_class("error");
        let mut error = None;
        let matches = match self.matcher() {
            None => Vec::new(),
            Some(Err(_)) => {
                self.find.entry.add_css_class("error");
                error = Some("Invalid regular expression");
                Vec::new()
            }
            Some(Ok(m)) => {
                let text = self.text();
                search::to_char_ranges(&text, &m.find_all(&text))
            }
        };
        for r in matches.iter().take(MAX_HIGHLIGHTS) {
            let a = self.buffer.iter_at_offset(r.start as i32);
            let b = self.buffer.iter_at_offset(r.end as i32);
            self.buffer.apply_tag(&self.match_tag, &a, &b);
        }
        *self.matches.borrow_mut() = matches;
        helix_app_common::a11y::set_input_error(&self.find.entry, None, error);
        if jump {
            let start = self
                .buffer
                .selection_bounds()
                .map(|(s, _)| s)
                .unwrap_or_else(|| self.buffer.iter_at_mark(&self.buffer.get_insert()));
            self.select_match(search::pick(
                &self.matches.borrow(),
                start.offset() as usize,
                false,
            ));
        }
        self.update_count();
    }

    fn current_match(&self) -> Option<usize> {
        let (s, e) = self.buffer.selection_bounds()?;
        let r = s.offset() as usize..e.offset() as usize;
        self.matches.borrow().iter().position(|m| *m == r)
    }

    fn update_count(&self) {
        let n = self.matches.borrow().len();
        let invalid = self.find.entry.has_css_class("error");
        let text = match (n, self.current_match()) {
            (0, _) if self.find.entry.text().is_empty() => String::new(),
            // Spoken right away by refresh_search.
            (0, _) if invalid => "Invalid expression".into(),
            (0, _) => "No results".into(),
            (n, Some(i)) => format!("{} of {}", i + 1, n),
            (n, None) => format!("{n} results"),
        };
        if !invalid && !text.is_empty() && text != self.find.count.text() {
            helix_app_common::announce_settled(&self.find.count, &text);
        }
        self.find.count.set_text(&text);
    }

    fn select_match(&self, idx: Option<usize>) {
        let Some(i) = idx else { return };
        let Some(r) = self.matches.borrow().get(i).cloned() else {
            return;
        };
        let a = self.buffer.iter_at_offset(r.start as i32);
        let b = self.buffer.iter_at_offset(r.end as i32);
        let (s, e) = self.buffer.bounds();
        self.buffer.remove_tag(&self.current_tag, &s, &e);
        self.buffer.apply_tag(&self.current_tag, &a, &b);
        self.buffer.select_range(&a, &b);
        self.view
            .scroll_to_mark(&self.buffer.get_insert(), 0.1, false, 0.0, 0.0);
        self.update_count();
    }

    pub fn find_next(self: &Rc<Self>, backward: bool) {
        if !self.find.revealer.reveals_child() {
            if self.find.entry.text().is_empty() {
                return;
            }
            self.refresh_search(false);
        }
        let (s, e) = self.buffer.selection_bounds().unwrap_or_else(|| {
            let it = self.buffer.iter_at_mark(&self.buffer.get_insert());
            (it, it)
        });
        let offset = if backward { s.offset() } else { e.offset() } as usize;
        let idx = search::pick(&self.matches.borrow(), offset, backward);
        self.select_match(idx);
    }

    fn replace_one(self: &Rc<Self>) {
        let Some(Ok(m)) = self.matcher() else { return };
        let Some(i) = self.current_match() else {
            self.find_next(false);
            return;
        };
        let r = self.matches.borrow()[i].clone();
        let text = self.text();
        let byte = |c: usize| {
            text.char_indices()
                .nth(c)
                .map(|(b, _)| b)
                .unwrap_or(text.len())
        };
        let replacement = m.replacement_for(
            &text,
            byte(r.start)..byte(r.end),
            &self.find.replace_entry.text(),
        );
        let buf = &self.buffer;
        buf.begin_user_action();
        let mut a = buf.iter_at_offset(r.start as i32);
        let mut b = buf.iter_at_offset(r.end as i32);
        buf.delete(&mut a, &mut b);
        buf.insert(&mut a, &replacement);
        buf.place_cursor(&a);
        buf.end_user_action();
        self.refresh_search(false);
        self.find_next(false);
    }

    fn replace_all(self: &Rc<Self>) {
        let Some(Ok(m)) = self.matcher() else { return };
        let text = self.text();
        let bytes = m.find_all(&text);
        let chars = search::to_char_ranges(&text, &bytes);
        let replacement = self.find.replace_entry.text();
        let buf = &self.buffer;
        buf.begin_user_action();
        for (br, cr) in bytes.iter().zip(chars.iter()).rev() {
            let rep = m.replacement_for(&text, br.clone(), &replacement);
            let mut a = buf.iter_at_offset(cr.start as i32);
            let mut b = buf.iter_at_offset(cr.end as i32);
            buf.delete(&mut a, &mut b);
            buf.insert(&mut a, &rep);
        }
        buf.end_user_action();
        self.refresh_search(false);
        if let Some(app) = self.app.upgrade() {
            let n = bytes.len();
            app.toast_for(
                self,
                &format!("Replaced {n} occurrence{}", if n == 1 { "" } else { "s" }),
            );
        }
    }

    // ---------------------------------------------------------------- files

    /// Loads `file` into this document (replacing its contents).
    pub async fn load(self: &Rc<Self>, file: gio::File) -> Result<(), glib::Error> {
        self.loading.set(true);
        self.view.set_editable(false);
        let res = file.load_contents_future().await;
        let result = match res {
            Ok((bytes, etag)) => {
                let bytes = bytes.to_vec();
                let (text, fmt) = gio::spawn_blocking(move || encoding::decode(&bytes))
                    .await
                    .unwrap_or_default();
                self.set_text(&text);
                self.format.set(fmt);
                *self.file.borrow_mut() = Some(file);
                *self.etag.borrow_mut() = etag.map(|e| e.to_string());
                self.buffer.set_modified(false);
                self.banner.set_reveal_child(false);
                self.watch();
                Ok(())
            }
            Err(e) => Err(e),
        };
        self.loading.set(false);
        self.view.set_editable(true);
        self.notify_state();
        result
    }

    /// Restores a draft: unsaved text, optionally tied to `file`.
    pub fn restore_draft(self: &Rc<Self>, text: &str, file: Option<gio::File>, fmt: TextFormat) {
        self.set_text(text);
        self.format.set(fmt);
        let has_file = file.is_some();
        *self.file.borrow_mut() = file;
        self.buffer.set_modified(true);
        if has_file {
            // Remember the current on-disk version so later edits are detected.
            let w = self.weak();
            glib::spawn_future_local(async move {
                let Some(d) = w.upgrade() else { return };
                let Some(f) = d.file.borrow().clone() else {
                    return;
                };
                if let Ok(info) = f
                    .query_info_future(
                        "etag::value",
                        gio::FileQueryInfoFlags::NONE,
                        glib::Priority::DEFAULT,
                    )
                    .await
                {
                    *d.etag.borrow_mut() = info.etag().map(|e| e.to_string());
                }
                d.watch();
            });
        }
        self.notify_state();
    }

    /// Writes the document to `file`. Returns `Ok(true)` when the original
    /// encoding could not represent the text and UTF-8 was used instead.
    pub async fn save_to(self: &Rc<Self>, file: gio::File) -> Result<bool, glib::Error> {
        let text = self.text();
        let generation = self.generation.get();
        let mut fmt = self.format.get();
        let mut fell_back = false;
        let bytes = match encoding::encode(&text, &fmt) {
            Ok(b) => b,
            Err(_) => {
                fell_back = true;
                fmt = TextFormat {
                    encoding: encoding_rs::UTF_8,
                    bom: false,
                    ..fmt
                };
                encoding::encode(&text, &fmt).unwrap_or_else(|_| text.into_bytes())
            }
        };
        self.saving.set(true);
        let res = file
            .replace_contents_future(bytes, None, false, gio::FileCreateFlags::NONE)
            .await;
        self.saving.set(false);
        let (_, etag) = res.map_err(|(_, e)| e)?;
        let changed_file = self.file.borrow().as_ref().is_none_or(|f| !f.equal(&file));
        *self.file.borrow_mut() = Some(file);
        *self.etag.borrow_mut() = Some(etag.to_string());
        self.format.set(fmt);
        if self.generation.get() == generation {
            self.buffer.set_modified(false);
        }
        self.banner.set_reveal_child(false);
        if changed_file || self.monitor.borrow().is_none() {
            self.watch();
        }
        self.notify_state();
        Ok(fell_back)
    }

    fn watch(self: &Rc<Self>) {
        if let Some(m) = self.monitor.take() {
            m.cancel();
        }
        let Some(file) = self.file.borrow().clone() else {
            return;
        };
        let Ok(monitor) =
            file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let w = self.weak();
        monitor.connect_changed(move |_, _, _, event| {
            use gio::FileMonitorEvent as E;
            if matches!(
                event,
                E::ChangesDoneHint
                    | E::Created
                    | E::Deleted
                    | E::Renamed
                    | E::MovedOut
                    | E::MovedIn
            ) {
                if let Some(d) = w.upgrade() {
                    d.check_disk();
                }
            }
        });
        *self.monitor.borrow_mut() = Some(monitor);
    }

    /// Compares the on-disk etag with the last loaded/saved one.
    fn check_disk(self: &Rc<Self>) {
        if self.saving.get() {
            return;
        }
        let w = self.weak();
        glib::spawn_future_local(async move {
            let Some(d) = w.upgrade() else { return };
            let Some(f) = d.file.borrow().clone() else {
                return;
            };
            let res = f
                .query_info_future(
                    "etag::value",
                    gio::FileQueryInfoFlags::NONE,
                    glib::Priority::DEFAULT,
                )
                .await;
            if d.saving.get() {
                return;
            }
            match res {
                Ok(info) => {
                    let disk = info.etag().map(|e| e.to_string());
                    if disk != *d.etag.borrow() {
                        d.show_banner(false);
                    }
                }
                Err(e) if e.matches(gio::IOErrorEnum::NotFound) => d.show_banner(true),
                Err(_) => {}
            }
        });
    }

    fn show_banner(&self, deleted: bool) {
        self.banner_label.set_text(if deleted {
            "The file has been deleted from disk. Saving will recreate it."
        } else {
            "The file has changed on disk."
        });
        self.banner_reload.set_visible(!deleted);
        if !self.banner.reveals_child() {
            helix_app_common::announce(
                &self.banner_label,
                &self.banner_label.text(),
                helix_app_common::a11y::Urgency::Polite,
            );
        }
        self.banner.set_reveal_child(true);
    }

    pub fn reload(self: &Rc<Self>) {
        let Some(file) = self.file.borrow().clone() else {
            return;
        };
        let (line, col) = self.cursor();
        let w = self.weak();
        glib::spawn_future_local(async move {
            let Some(d) = w.upgrade() else { return };
            match d.load(file).await {
                Ok(()) => d.goto(line + 1, Some(col + 1)),
                Err(e) => {
                    if let Some(app) = d.app.upgrade() {
                        app.toast_for(&d, &format!("Could not reload: {}", e.message()));
                    }
                }
            }
        });
    }

    /// Stops monitoring (document closed).
    pub fn dispose(&self) {
        if let Some(m) = self.monitor.take() {
            m.cancel();
        }
    }
}

/// Sets tab stops to `tab_width` characters of the view's current font.
fn set_tab_stops(view: &EditorView, tab_width: u32) {
    let layout = view.create_pango_layout(Some(&" ".repeat(tab_width.max(1) as usize)));
    let width = layout.pixel_size().0.max(1);
    let mut tabs = gtk::pango::TabArray::new(1, true);
    tabs.set_tab(0, gtk::pango::TabAlign::Left, width);
    view.set_tabs(&tabs);
}
