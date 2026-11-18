//! Editor window: header bar, tabs, status bar and window actions.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::shortcut_tooltip;
use helix_app_common::Labelled;
use std::cell::Cell;
use std::rc::{Rc, Weak};

use crate::app::App;
use crate::doc::Doc;
use crate::encoding::LineEnding;
use crate::textops;
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;

pub struct Window {
    app: Weak<App>,
    this: Weak<Window>,
    pub win: adw::ApplicationWindow,
    pub tab_view: adw::TabView,
    toasts: adw::ToastOverlay,
    title: adw::WindowTitle,
    zoom_level: gtk::Button,
    pos_btn: gtk::MenuButton,
    goto_entry: gtk::Entry,
    indent_label: gtk::Label,
    enc_btn: gtk::MenuButton,
    line_ending: gio::SimpleAction,
    recent_list: gtk::ListBox,
    recent_search: gtk::SearchEntry,
    recent_popover: gtk::Popover,
    force: Cell<bool>,
    closed: Cell<bool>,
    closing_tab: Cell<bool>,
}

/// How documents with unsaved changes are described.
const UNSAVED: &str = n_("Unsaved Changes");

/// Shown under the Go to Line entry for input it cannot use.
const GOTO_ERROR: &str = n_("Enter a line number, or line:column");

fn primary_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some(tr("New _Tab")), Some("win.new-tab"));
    s1.append(Some(tr("New _Window")), Some("app.new-window"));
    s1.append(Some(tr("Save _As…")), Some("win.save-as"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some(tr("_Find…")), Some("win.find"));
    s2.append(Some(tr("Find and _Replace…")), Some("win.replace"));
    s2.append(Some(tr("_Go to Line…")), Some("win.goto-line"));
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    // A row of Zoom Out / level (resets) / Zoom In, added by `Window::new`.
    let zoom = gio::MenuItem::new(None, None);
    zoom.set_attribute_value("custom", Some(&"zoom".to_variant()));
    s3.append_item(&zoom);
    menu.append_section(None, &s3);
    let s4 = gio::Menu::new();
    s4.append(Some(tr("_Preferences")), Some("app.preferences"));
    s4.append(
        Some(tr("_Keyboard Shortcuts")),
        Some("win.show-help-overlay"),
    );
    s4.append(Some(tr("_About Text Editor")), Some("app.about"));
    menu.append_section(None, &s4);
    menu
}

fn line_ending_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let section = gio::Menu::new();
    for le in LineEnding::ALL {
        let item = gio::MenuItem::new(Some(le.label()), None);
        item.set_action_and_target_value(Some("win.line-ending"), Some(&le.nick().to_variant()));
        section.append_item(&item);
    }
    menu.append_section(Some(tr("Line Ending")), &section);
    menu
}

impl Window {
    pub fn new(app: &Rc<App>) -> Rc<Window> {
        let win = adw::ApplicationWindow::builder()
            .application(&app.gtk)
            .title(tr("Text Editor"))
            .default_width(900)
            .default_height(700)
            .width_request(360)
            .height_request(294)
            .build();
        helix_app_common::window_state::persist(
            &win,
            helix_app_common::window_state::Store::file("helix-text-editor"),
        );
        win.set_help_overlay(Some(&crate::shortcuts::window()));

        // Recent documents popover.
        let recent_search = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search documents"))
            .build()
            .labelled(tr("Search Recent Documents"));
        let recent_list = gtk::ListBox::new();
        recent_list.add_css_class("navigation-sidebar");
        recent_list.set_placeholder(Some(
            &gtk::Label::builder()
                .label(tr("No Recent Documents"))
                .margin_top(12)
                .margin_bottom(12)
                .css_classes(["dim-label"])
                .build(),
        ));
        let recent_scroll = gtk::ScrolledWindow::builder()
            .child(&recent_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(360)
            .min_content_width(320)
            .build();
        let browse = gtk::Button::with_mnemonic(tr("_Browse Files…"));
        browse.set_action_name(Some("win.open"));
        let recent_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        recent_box.append(&recent_search);
        recent_box.append(&recent_scroll);
        recent_box.append(&browse);
        let recent_popover = gtk::Popover::builder().child(&recent_box).build();
        let open_btn = adw::SplitButton::builder()
            .label(tr("_Open"))
            .use_underline(true)
            .action_name("win.open")
            .popover(&recent_popover)
            .tooltip_text(shortcut_tooltip(tr("Open a File"), "<Control>o"))
            .dropdown_tooltip(tr("Recent Documents"))
            .build();

        let new_tab_btn = gtk::Button::from_icon_name("tab-new-symbolic");
        new_tab_btn.set_action_name(Some("win.new-tab"));
        helix_app_common::set_label_and_shortcut(&new_tab_btn, tr("New Tab"), "<Control>t");
        let save_btn = gtk::Button::with_mnemonic(tr("_Save"));
        save_btn.set_action_name(Some("win.save"));
        save_btn.set_tooltip_text(Some(&shortcut_tooltip(tr("Save"), "<Control>s")));
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&primary_menu())
            .primary(true)
            .tooltip_text(tr("Main Menu"))
            .build()
            .labelled(tr("Main Menu"));
        let zoom_out = helix_app_common::icon_button("zoom-out-symbolic", tr("Zoom Out"));
        zoom_out.set_action_name(Some("app.zoom-out"));
        let zoom_level = gtk::Button::builder()
            .label("100%")
            .action_name("app.zoom-reset")
            .tooltip_text(tr("Reset Zoom"))
            .hexpand(true)
            .build();
        zoom_level.add_css_class("flat");
        let zoom_in = helix_app_common::icon_button("zoom-in-symbolic", tr("Zoom In"));
        zoom_in.set_action_name(Some("app.zoom-in"));
        for b in [&zoom_out, &zoom_in] {
            b.add_css_class("flat");
            b.add_css_class("circular");
        }
        let zoom_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        zoom_row.append(&zoom_out);
        zoom_row.append(&zoom_level);
        zoom_row.append(&zoom_in);
        if let Some(pm) = menu_btn.popover().and_downcast::<gtk::PopoverMenu>() {
            pm.add_child(&zoom_row, "zoom");
        }
        let title = adw::WindowTitle::new(tr("Text Editor"), "");
        let header = adw::HeaderBar::new();
        header.pack_start(&open_btn);
        header.pack_start(&new_tab_btn);
        header.set_title_widget(Some(&title));
        header.pack_end(&menu_btn);
        header.pack_end(&save_btn);
        let save_icon_btn = helix_app_common::icon_button("document-save-symbolic", tr("Save"));
        save_icon_btn.set_action_name(Some("win.save"));
        helix_app_common::set_label_and_shortcut(&save_icon_btn, tr("Save"), "<Control>s");
        save_icon_btn.set_visible(false);
        header.pack_end(&save_icon_btn);

        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder()
            .view(&tab_view)
            .autohide(true)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&tab_view));

        // Status bar.
        let goto_entry = gtk::Entry::builder()
            .placeholder_text(tr("Line or Line:Column"))
            .input_purpose(gtk::InputPurpose::Digits)
            .build();
        let goto_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let goto_heading = gtk::Label::builder()
            .label(tr("Go to Line"))
            .css_classes(["heading"])
            .xalign(0.0)
            .build();
        let goto_error = gtk::Label::builder()
            .label(tr(GOTO_ERROR))
            .css_classes(["error", "caption"])
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(24)
            .visible(false)
            .build();
        goto_box.append(&goto_heading);
        goto_box.append(&goto_entry);
        goto_box.append(&goto_error);
        helix_app_common::label_widget(&goto_entry, &goto_heading);
        let goto_popover = gtk::Popover::builder().child(&goto_box).build();
        let pos_btn = gtk::MenuButton::builder()
            .label(tr("Ln 1, Col 1"))
            .popover(&goto_popover)
            .tooltip_text(shortcut_tooltip(tr("Go to Line"), "<Control>i"))
            .css_classes(["flat"])
            .build();
        let indent_label = gtk::Label::new(None);
        indent_label.add_css_class("dim-label");
        let enc_btn = gtk::MenuButton::builder()
            .menu_model(&line_ending_menu())
            .tooltip_text(tr("Encoding and Line Ending"))
            .css_classes(["flat"])
            .always_show_arrow(false)
            .build();
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        status.add_css_class("statusbar");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        status.append(&spacer);
        status.append(&indent_label);
        status.append(&enc_btn);
        status.append(&pos_btn);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        toolbar.add_bottom_bar(&status);
        toolbar.set_content(Some(&toasts));
        win.set_content(Some(&toolbar));

        // Narrow windows: New Tab stays in the main menu (and Ctrl+T), Save
        // becomes an icon and the indentation readout is left to
        // Preferences, so the header and status bar fit 360px.
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            450.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&new_tab_btn, "visible", Some(&false.to_value()));
        narrow.add_setter(&save_btn, "visible", Some(&false.to_value()));
        narrow.add_setter(&save_icon_btn, "visible", Some(&true.to_value()));
        narrow.add_setter(&indent_label, "visible", Some(&false.to_value()));
        win.add_breakpoint(narrow);

        let line_ending = gio::SimpleAction::new_stateful(
            "line-ending",
            Some(glib::VariantTy::STRING),
            &"lf".to_variant(),
        );

        let w = Rc::new_cyclic(|this| Window {
            app: Rc::downgrade(app),
            this: this.clone(),
            win,
            tab_view,
            toasts,
            title,
            zoom_level,
            pos_btn,
            goto_entry,
            indent_label,
            enc_btn,
            line_ending,
            recent_list,
            recent_search,
            recent_popover,
            force: Cell::new(false),
            closed: Cell::new(false),
            closing_tab: Cell::new(false),
        });
        w.setup_actions();
        w.setup_signals(&goto_popover, &goto_error);
        w.setup_file_drops(&tab_bar);
        w.update_status();
        w
    }

    /// Files dropped on the editor area or the tab bar open as tabs. The
    /// editor-area target runs in the capture phase so it sees file drags
    /// before the text view, which would otherwise insert their addresses
    /// as text; drags that carry only text are left to the text view.
    fn setup_file_drops(self: &Rc<Self>, tab_bar: &adw::TabBar) {
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.set_propagation_phase(gtk::PropagationPhase::Capture);
        let wk = self.this.clone();
        drop.connect_drop(move |_, value, _, _| {
            match (wk.upgrade(), value.get::<gdk::FileList>()) {
                (Some(w), Ok(list)) => w.open_dropped(list.files()),
                _ => false,
            }
        });
        self.tab_view.add_controller(drop);

        tab_bar.setup_extra_drop_target(gdk::DragAction::COPY, &[gdk::FileList::static_type()]);
        let wk = self.this.clone();
        tab_bar.connect_extra_drag_drop(move |_, _, value| {
            match (wk.upgrade(), value.get::<gdk::FileList>()) {
                (Some(w), Ok(list)) => w.open_dropped(list.files()),
                _ => false,
            }
        });
    }

    fn open_dropped(&self, files: Vec<gio::File>) -> bool {
        if files.is_empty() {
            return false;
        }
        for f in files {
            self.open_file(f);
        }
        self.win.present();
        true
    }

    fn app(&self) -> Rc<App> {
        self.app.upgrade().expect("app alive")
    }

    fn rc(&self) -> Rc<Window> {
        self.this.upgrade().expect("window alive")
    }

    fn setup_actions(self: &Rc<Self>) {
        let add = |name: &str, f: fn(&Rc<Window>)| {
            let a = gio::SimpleAction::new(name, None);
            let w = self.this.clone();
            a.connect_activate(move |_, _| {
                if let Some(w) = w.upgrade() {
                    f(&w);
                }
            });
            self.win.add_action(&a);
        };
        add("new-tab", |w| {
            w.new_tab();
        });
        add("open", |w| w.open_dialog());
        add("save", |w| {
            if let Some(d) = w.current_doc() {
                let w = w.clone();
                glib::spawn_future_local(async move {
                    w.save_doc(&d, false).await;
                });
            }
        });
        add("save-as", |w| {
            if let Some(d) = w.current_doc() {
                let w = w.clone();
                glib::spawn_future_local(async move {
                    w.save_doc(&d, true).await;
                });
            }
        });
        add("close-tab", |w| {
            if let Some(p) = w.tab_view.selected_page() {
                w.tab_view.close_page(&p);
            }
        });
        add("find", |w| {
            if let Some(d) = w.current_doc() {
                d.show_find(false);
            }
        });
        add("replace", |w| {
            if let Some(d) = w.current_doc() {
                d.show_find(true);
            }
        });
        add("find-next", |w| {
            if let Some(d) = w.current_doc() {
                d.find_next(false);
            }
        });
        add("find-previous", |w| {
            if let Some(d) = w.current_doc() {
                d.find_next(true);
            }
        });
        add("goto-line", |w| {
            if w.current_doc().is_some() && w.win.visible_dialog().is_none() {
                w.pos_btn.popup();
            }
        });

        let wk = self.this.clone();
        self.line_ending.connect_change_state(move |a, value| {
            let (Some(w), Some(v)) = (wk.upgrade(), value) else {
                return;
            };
            let Some(nick) = v.str() else { return };
            a.set_state(v);
            if let Some(d) = w.current_doc() {
                let mut fmt = d.format.get();
                let le = LineEnding::from_nick(nick);
                if fmt.line_ending != le {
                    fmt.line_ending = le;
                    d.format.set(fmt);
                    d.buffer.set_modified(true);
                    w.update_status();
                }
            }
        });
        self.win.add_action(&self.line_ending);
    }

    fn setup_signals(self: &Rc<Self>, goto_popover: &gtk::Popover, goto_error: &gtk::Label) {
        let wk = self.this.clone();
        self.tab_view.connect_close_page(move |tv, page| {
            let Some(w) = wk.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let app = w.app();
            let Some(doc) = app.doc_for_child(&page.child()) else {
                tv.close_page_finish(page, true);
                return glib::Propagation::Stop;
            };
            if !doc.is_modified() {
                w.finish_close(page, &doc);
                return glib::Propagation::Stop;
            }
            let page = page.clone();
            glib::spawn_future_local(async move {
                let ok = match confirm_close(&w.win, std::slice::from_ref(&doc))
                    .await
                    .as_str()
                {
                    "save" => w.save_doc(&doc, false).await,
                    "discard" => true,
                    _ => false,
                };
                if ok {
                    w.finish_close(&page, &doc);
                } else {
                    w.tab_view.close_page_finish(&page, false);
                }
            });
            glib::Propagation::Stop
        });

        let wk = self.this.clone();
        self.tab_view.connect_page_attached(move |_, page, _| {
            let Some(w) = wk.upgrade() else { return };
            if let Some(d) = w.app().doc_for_child(&page.child()) {
                w.refresh_doc(&d);
            }
        });
        let wk = self.this.clone();
        self.tab_view.connect_page_detached(move |tv, _, _| {
            let Some(w) = wk.upgrade() else { return };
            if w.closed.get() || tv.n_pages() > 0 {
                return;
            }
            let closing_tab = w.closing_tab.get();
            glib::idle_add_local_once(move || {
                if w.closed.get() || w.tab_view.n_pages() > 0 {
                    return;
                }
                if closing_tab {
                    w.new_tab();
                } else {
                    // The last tab was dragged to another window.
                    w.force_close();
                }
            });
        });
        let wk = self.this.clone();
        self.tab_view.connect_create_window(move |_| {
            let w = wk.upgrade()?;
            let nw = w.app().new_window(false);
            nw.win.present();
            Some(nw.tab_view.clone())
        });
        let wk = self.this.clone();
        self.tab_view.connect_selected_page_notify(move |_| {
            let Some(w) = wk.upgrade() else { return };
            w.update_title();
            w.update_status();
            if let Some(d) = w.current_doc() {
                glib::idle_add_local_once(move || {
                    if !d.find.revealer.reveals_child() {
                        d.grab_focus();
                    }
                });
            }
        });

        let wk = self.this.clone();
        self.win.connect_close_request(move |_| match wk.upgrade() {
            Some(w) => w.on_close_request(),
            None => glib::Propagation::Proceed,
        });

        // Go to line.
        let wk = self.this.clone();
        goto_popover.connect_show(move |_| {
            if let Some(w) = wk.upgrade() {
                if let Some(d) = w.current_doc() {
                    let (l, _) = d.cursor_display();
                    w.goto_entry.set_text(&l.to_string());
                    w.goto_entry.grab_focus();
                    w.goto_entry.select_region(0, -1);
                }
            }
        });
        let wk = self.this.clone();
        let pop = goto_popover.clone();
        let goto_error = goto_error.clone();
        self.goto_entry.connect_changed({
            let goto_error = goto_error.clone();
            move |e| {
                goto_error.set_visible(false);
                helix_app_common::a11y::set_input_error(e, None, None);
            }
        });
        self.goto_entry.connect_activate(move |e| {
            let Some(w) = wk.upgrade() else { return };
            match textops::parse_goto(&e.text()) {
                Some((line, col)) => {
                    pop.popdown();
                    if let Some(d) = w.current_doc() {
                        d.goto(line, col);
                        d.grab_focus();
                    }
                }
                None => {
                    goto_error.set_visible(true);
                    helix_app_common::a11y::set_input_error(
                        e,
                        Some(&goto_error),
                        Some(tr(GOTO_ERROR)),
                    );
                }
            }
        });

        // Recent documents.
        let wk = self.this.clone();
        self.recent_popover.connect_show(move |_| {
            if let Some(w) = wk.upgrade() {
                w.rebuild_recent();
                w.recent_search.set_text("");
                w.recent_search.grab_focus();
            }
        });
        let search = self.recent_search.clone();
        self.recent_list.set_filter_func(move |row| {
            let q = search.text().to_lowercase();
            if q.is_empty() {
                return true;
            }
            row.downcast_ref::<adw::ActionRow>()
                .map(|r| {
                    r.title().to_lowercase().contains(&q)
                        || r.subtitle().is_some_and(|s| s.to_lowercase().contains(&q))
                })
                .unwrap_or(true)
        });
        let list = self.recent_list.clone();
        self.recent_search
            .connect_search_changed(move |_| list.invalidate_filter());
        let wk = self.this.clone();
        self.recent_search.connect_activate(move |_| {
            let Some(w) = wk.upgrade() else { return };
            let mut i = 0;
            while let Some(row) = w.recent_list.row_at_index(i) {
                if row.is_child_visible() {
                    row.activate();
                    break;
                }
                i += 1;
            }
        });
        let wk = self.this.clone();
        self.recent_list.connect_row_activated(move |_, row| {
            let Some(w) = wk.upgrade() else { return };
            let uri = row.widget_name();
            w.recent_popover.popdown();
            w.open_file(gio::File::for_uri(&uri));
        });
    }

    fn rebuild_recent(&self) {
        self.recent_list.remove_all();
        for uri in self.app().recent.borrow().iter() {
            let file = gio::File::for_uri(uri);
            let name = file
                .basename()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| uri.clone());
            let dir = file
                .parent()
                .map(|p| {
                    p.path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| p.uri().to_string())
                })
                .unwrap_or_default();
            let home = glib::home_dir().display().to_string();
            let dir = match dir.strip_prefix(&home) {
                Some(rest) => format!("~{rest}"),
                None => dir,
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&name))
                .subtitle(glib::markup_escape_text(&dir))
                .activatable(true)
                .build();
            row.set_widget_name(uri);
            self.recent_list.append(&row);
        }
    }

    // ------------------------------------------------------------ documents

    pub fn current_doc(&self) -> Option<Rc<Doc>> {
        let page = self.tab_view.selected_page()?;
        self.app().doc_for_child(&page.child())
    }

    pub fn docs(&self) -> Vec<Rc<Doc>> {
        let app = self.app();
        (0..self.tab_view.n_pages())
            .filter_map(|i| app.doc_for_child(&self.tab_view.nth_page(i).child()))
            .collect()
    }

    pub fn add_doc(&self, id: Option<String>, select: bool) -> Rc<Doc> {
        let app = self.app();
        let doc = Doc::new(&app, id);
        doc.untitled.set(app.next_untitled());
        app.register_doc(&doc);
        let page = self.tab_view.append(&doc.root);
        self.refresh_doc(&doc);
        if select {
            self.tab_view.set_selected_page(&page);
        }
        doc
    }

    pub fn new_tab(&self) -> Rc<Doc> {
        let doc = self.add_doc(None, true);
        doc.grab_focus();
        doc
    }

    fn finish_close(&self, page: &adw::TabPage, doc: &Rc<Doc>) {
        self.closing_tab.set(true);
        self.tab_view.close_page_finish(page, true);
        self.closing_tab.set(false);
        self.app().unregister_doc(doc);
    }

    pub fn open_file(&self, file: gio::File) {
        let app = self.app();
        if let Some(doc) = app.find_doc_by_file(&file) {
            if let Some(w) = app.window_of(&doc) {
                w.tab_view.set_selected_page(&w.tab_view.page(&doc.root));
                w.win.present();
            }
            return;
        }
        let (doc, fresh) = match self.current_doc() {
            Some(d) if d.is_blank() => (d, false),
            _ => (self.add_doc(None, true), true),
        };
        doc.loading.set(true);
        let w = self.rc();
        glib::spawn_future_local(async move {
            w.tab_view.page(&doc.root).set_loading(true);
            let res = doc.load(file.clone()).await;
            w.tab_view.page(&doc.root).set_loading(false);
            match res {
                Ok(()) => {
                    w.app().add_recent(&file.uri());
                    doc.grab_focus();
                }
                Err(e) => {
                    let name = file.parse_name();
                    w.toast(&trf!(
                        "Could not open “{name}”: {error}",
                        name = name,
                        error = e.message()
                    ));
                    if fresh {
                        w.tab_view.close_page(&w.tab_view.page(&doc.root));
                    }
                }
            }
        });
    }

    /// Restores a clean file tab from the session.
    pub fn restore_file(&self, id: &str, file: gio::File, line: u32, column: u32, select: bool) {
        let doc = self.add_doc(Some(id.to_string()), select);
        doc.loading.set(true);
        let w = self.rc();
        glib::spawn_future_local(async move {
            match doc.load(file).await {
                Ok(()) => doc.goto(line + 1, Some(column + 1)),
                Err(_) => {
                    w.tab_view.close_page(&w.tab_view.page(&doc.root));
                }
            }
        });
    }

    fn open_dialog(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Open Files"))
            .modal(true)
            .build();
        if let Some(dir) = self
            .current_doc()
            .and_then(|d| d.file.borrow().as_ref().and_then(|f| f.parent()))
        {
            dialog.set_initial_folder(Some(&dir));
        }
        let w = self.clone();
        glib::spawn_future_local(async move {
            let Ok(files) = dialog.open_multiple_future(Some(&w.win)).await else {
                return;
            };
            for i in 0..files.n_items() {
                if let Some(f) = files.item(i).and_downcast::<gio::File>() {
                    w.open_file(f);
                }
            }
        });
    }

    async fn choose_save_file(&self, doc: &Doc) -> Option<gio::File> {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Save As"))
            .modal(true)
            .initial_name(doc.title())
            .build();
        let current = doc.file.borrow().clone();
        match current {
            Some(f) => dialog.set_initial_file(Some(&f)),
            None => {
                if let Some(dir) = glib::user_special_dir(glib::UserDirectory::Documents) {
                    dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
                }
            }
        }
        dialog.save_future(Some(&self.win)).await.ok()
    }

    /// Saves `doc` (asking for a location when needed). Returns success.
    pub async fn save_doc(&self, doc: &Rc<Doc>, save_as: bool) -> bool {
        let existing = doc.file.borrow().clone();
        let file = match existing {
            Some(f) if !save_as => f,
            _ => match self.choose_save_file(doc).await {
                Some(f) => f,
                None => return false,
            },
        };
        match doc.save_to(file.clone()).await {
            Ok(fell_back) => {
                if fell_back {
                    self.toast(tr("The document could not be saved in its original encoding and was saved as UTF-8"));
                }
                self.app().add_recent(&file.uri());
                self.app().schedule_autosave();
                true
            }
            Err(e) => {
                self.toast(&trf!("Could not save “{}”: {}", doc.title(), e.message()));
                false
            }
        }
    }

    // ------------------------------------------------------------ display

    pub fn toast(&self, msg: &str) {
        let t = adw::Toast::new(&glib::markup_escape_text(msg));
        t.set_timeout(4);
        self.toasts.add_toast(t);
    }

    pub fn refresh_doc(&self, doc: &Rc<Doc>) {
        if doc.root.parent().is_none() {
            return;
        }
        let page = self.tab_view.page(&doc.root);
        page.set_title(&doc.title());
        // Unsaved changes: an icon on the tab, and in words in its tooltip
        // (which is also the tab's accessible description).
        let modified = doc.is_modified();
        let icon = modified.then(|| gio::ThemedIcon::new("document-edit-symbolic"));
        page.set_indicator_icon(icon.as_ref());
        page.set_indicator_tooltip(if modified { tr(UNSAVED) } else { "" });
        let tip = if modified {
            format!("{}\n{}", tr(UNSAVED), doc.subtitle())
        } else {
            doc.subtitle()
        };
        page.set_tooltip(&glib::markup_escape_text(&tip));
        if self.current_doc().is_some_and(|d| Rc::ptr_eq(&d, doc)) {
            self.update_title();
            self.update_status();
        }
    }

    fn update_title(&self) {
        match self.current_doc() {
            Some(d) => {
                self.title.set_title(&d.title());
                if d.is_modified() {
                    self.title
                        .set_subtitle(&format!("{} · {}", tr(UNSAVED), d.subtitle()));
                    self.win.set_title(Some(&trf!(
                        "{} ({}) — Text Editor",
                        d.title(),
                        tr(UNSAVED).to_lowercase()
                    )));
                } else {
                    self.title.set_subtitle(&d.subtitle());
                    self.win
                        .set_title(Some(&trf!("{} — Text Editor", d.title())));
                }
            }
            None => {
                self.title.set_title(tr("Text Editor"));
                self.title.set_subtitle("");
                self.win.set_title(Some(tr("Text Editor")));
            }
        }
    }

    pub fn update_status(&self) {
        let app = self.app();
        let s = app.settings.borrow();
        self.indent_label.set_text(&if s.indent_spaces {
            trf!("Spaces: {}", s.tab_width)
        } else {
            trf!("Tab Width: {}", s.tab_width)
        });
        drop(s);
        let zoom = format!("{}%", app.zoom_percent());
        self.zoom_level.set_label(&zoom);
        helix_app_common::set_accessible_label(
            &self.zoom_level,
            &trf!("Reset Zoom, now {zoom}", zoom = zoom),
        );
        let Some(d) = self.current_doc() else { return };
        let (l, c) = d.cursor_display();
        self.pos_btn
            .set_label(&trf!("Ln {l}, Col {c}", l = l, c = c));
        let fmt = d.format.get();
        let bom = if fmt.bom { tr(" (BOM)") } else { "" };
        self.enc_btn.set_label(&format!(
            "{}{bom} · {}",
            fmt.encoding_name(),
            fmt.line_ending.short_label()
        ));
        self.line_ending
            .set_state(&fmt.line_ending.nick().to_variant());
    }

    // ------------------------------------------------------------ closing

    pub fn force_close(&self) {
        self.force.set(true);
        self.win.close();
    }

    fn cleanup(&self) {
        self.closed.set(true);
        let app = self.app();
        for d in self.docs() {
            app.unregister_doc(&d);
        }
        app.window_removed(self);
    }

    fn on_close_request(&self) -> glib::Propagation {
        let app = self.app();
        if self.force.get() || app.quitting.get() {
            self.cleanup();
            return glib::Propagation::Proceed;
        }
        let last = app.windows.borrow().len() <= 1;
        let keep = last && !app.standalone && app.settings.borrow().restore_session;
        if keep {
            app.save_session_now();
            app.quitting.set(true);
            self.cleanup();
            return glib::Propagation::Proceed;
        }
        let unsaved: Vec<Rc<Doc>> = self
            .docs()
            .into_iter()
            .filter(|d| d.is_modified())
            .collect();
        if unsaved.is_empty() {
            if last {
                app.clear_session();
            }
            self.cleanup();
            return glib::Propagation::Proceed;
        }
        let w = self.rc();
        glib::spawn_future_local(async move {
            let ok = match confirm_close(&w.win, &unsaved).await.as_str() {
                "save" => {
                    let mut all = true;
                    for d in &unsaved {
                        w.tab_view.set_selected_page(&w.tab_view.page(&d.root));
                        if !w.save_doc(d, false).await {
                            all = false;
                            break;
                        }
                    }
                    all
                }
                "discard" => true,
                _ => false,
            };
            if ok {
                let app = w.app();
                if app.windows.borrow().len() <= 1 {
                    app.clear_session();
                }
                w.force_close();
            }
        });
        glib::Propagation::Stop
    }
}

/// Asks whether to save `docs` before closing. Returns `save`, `discard` or
/// `cancel`.
async fn confirm_close(win: &adw::ApplicationWindow, docs: &[Rc<Doc>]) -> String {
    let (heading, body) = if docs.len() == 1 {
        (
            trf!("Save Changes to “{}”?", docs[0].title()),
            tr("Unsaved changes will be permanently lost.").to_string(),
        )
    } else {
        let names: Vec<String> = docs.iter().map(|d| format!("• {}", d.title())).collect();
        (
            tr("Save Changes?").to_string(),
            trf!(
                "These documents have unsaved changes:\n{documents}\nUnsaved changes will be permanently lost.",
                documents = names.join("\n")
            ),
        )
    };
    let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
    let multi = docs.len() > 1;
    dialog.add_responses(&[
        ("cancel", tr("_Cancel")),
        (
            "discard",
            if multi {
                tr("_Discard All")
            } else {
                tr("_Discard")
            },
        ),
        ("save", if multi { tr("_Save All") } else { tr("_Save") }),
    ]);
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    dialog.choose_future(win).await.to_string()
}
