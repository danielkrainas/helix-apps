//! GTK user interface.

use adw::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use helix_eds::vcard::{self, phone_label, Contact};
use helix_eds::Source;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::backend::{self, Backend, Cmd, Msg};
use crate::editor::{set_avatar_bytes, Editor};
use crate::folks;
use crate::individual::{self, Individual};
use crate::links::{Key, Links};
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use helix_app_common::trnf;

pub const APP_ID: &str = "org.helix.Contacts";
const SCHEMA: &str = "org.gnome.Contacts";
const DEFAULTS_SCHEMA: &str = "org.gnome.Evolution.DefaultSources";
const EDS_BOOK_SCHEMA: &str = "org.gnome.evolution-data-server.addressbook";

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub individual: Option<String>,
    pub email: Option<String>,
    pub search: Option<String>,
    pub files: Vec<std::path::PathBuf>,
}

/// Window accelerators: (action, keys). The Keyboard Shortcuts window
/// lists every one of them (checked by a test in `shortcuts`).
pub const ACCELS: &[(&str, &[&str])] = &[
    ("win.new-contact", &["<Control>n"]),
    ("win.edit", &["<Control>e"]),
    ("win.search", &["<Control>f"]),
    ("win.import", &["<Control>i"]),
    ("win.shortcuts", &["<Control>question"]),
];

pub fn run(opts: Options) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    helix_app_common::install_standard_actions(&app, tr("Contacts"), tr("Manage your contacts"));
    let ui: Rc<RefCell<Option<Rc<Ui>>>> = Rc::default();
    let opts = RefCell::new(Some(opts));
    app.connect_activate(move |app| {
        let existing = ui.borrow().clone();
        match existing {
            Some(u) => u.window.present(),
            None => {
                let u = Ui::build(app, opts.borrow_mut().take().unwrap_or_default());
                u.window.present();
                *ui.borrow_mut() = Some(u);
            }
        }
    });
    app.run_with_args::<&str>(&[])
}

type ActionFn = Box<dyn Fn(&Rc<Ui>)>;

#[derive(Default)]
struct State {
    books: HashMap<String, HashMap<String, Contact>>,
    writable: HashMap<String, bool>,
    loaded: HashSet<String>,
    sources: Vec<Source>,
    individuals: Vec<Individual>,
    /// Rows currently shown, in list order.
    shown: Vec<Individual>,
    /// Primary key of the selected individual.
    selected: Option<Key>,
    pending_delete: HashSet<Key>,
    selection_mode: bool,
    checked: HashSet<String>,
    unavailable: Option<String>,
    next_tag: u64,
    /// Tag of a pending create whose result should be selected.
    select_created: Option<u64>,
    startup: Option<Options>,
    setup_checked: bool,
}

struct Ui {
    window: adw::ApplicationWindow,
    prefs: Prefs,
    defaults: Prefs,
    backend: Backend,
    links: RefCell<Links>,
    links_path: std::path::PathBuf,
    /// Links made by GNOME Contacts (folks), recomputed on every rebuild.
    external_links: RefCell<Vec<Vec<Key>>>,
    folks_personas: Vec<folks::KeyFilePersona>,
    self_uid: String,
    state: RefCell<State>,
    editor: RefCell<Option<(Rc<Editor>, Option<Key>)>>,
    rebuild_pending: Cell<bool>,

    toasts: adw::ToastOverlay,
    split: adw::NavigationSplitView,
    search: gtk::SearchEntry,
    list: gtk::ListBox,
    list_stack: gtk::Stack,
    select_button: gtk::ToggleButton,
    /// Selection-mode check box of each shown row, by individual id.
    checks: RefCell<HashMap<String, (gtk::ListBoxRow, gtk::CheckButton)>>,
    side_page: adw::NavigationPage,
    action_bar: gtk::ActionBar,
    link_button: gtk::Button,
    delete_button: gtk::Button,
    export_button: gtk::Button,
    content_page: adw::NavigationPage,
    content_header: adw::HeaderBar,
    content_body: adw::Bin,
    edit_button: gtk::Button,
    done_button: gtk::Button,
    cancel_button: gtk::Button,
    contact_menu: gtk::MenuButton,
    main_stack: gtk::Stack,
    unavailable_page: adw::StatusPage,
    book_menu: gio::Menu,
}

fn tristate(on: bool) -> gtk::AccessibleTristate {
    if on {
        gtk::AccessibleTristate::True
    } else {
        gtk::AccessibleTristate::False
    }
}

/// Sidebar title: "Contacts", or in selection mode "Select Contacts" /
/// "1 Selected" / "N Selected".
fn selection_title(selection_mode: bool, n: usize) -> String {
    match (selection_mode, n) {
        (false, _) => tr("Contacts").into(),
        (true, 0) => tr("Select Contacts").into(),
        (true, n) => trf!("{n} Selected", n = n),
    }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    for c in classes {
        l.add_css_class(c);
    }
    l
}

impl Ui {
    fn build(app: &adw::Application, opts: Options) -> Rc<Ui> {
        let prefs = Prefs::new(SCHEMA);
        let defaults = Prefs::new(DEFAULTS_SCHEMA);
        let self_uid = Prefs::new(EDS_BOOK_SCHEMA).get("self-contact-uid", String::new());
        let links_path = helix_app_common::data_dir("helix-contacts").join("links.json");
        let links = Links::load(&links_path);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr("Contacts"))
            .default_width(800)
            .default_height(600)
            .width_request(360)
            .height_request(294)
            .build();
        {
            use helix_app_common::window_state::{persist, SizeKeys, Store};
            persist(
                &window,
                Store::settings(
                    &prefs,
                    SizeKeys::Split("window-width", "window-height"),
                    "window-maximized",
                    "helix-contacts",
                ),
            );
        }

        // Sidebar.
        let new_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(tr("New Contact"))
            .build()
            .labelled(tr("New Contact"));
        new_button.set_action_name(Some("win.new-contact"));
        let select_button = gtk::ToggleButton::builder()
            .icon_name("selection-mode-symbolic")
            .tooltip_text(tr("Select Contacts"))
            .build()
            .labelled(tr("Select Contacts"));
        let menu = gio::Menu::new();
        let sort = gio::Menu::new();
        sort.append(Some(tr("Sort by _First Name")), Some("win.sort::first"));
        sort.append(Some(tr("Sort by _Surname")), Some("win.sort::surname"));
        menu.append_section(None, &sort);
        let book_menu = gio::Menu::new();
        let io = gio::Menu::new();
        io.append(Some(tr("_Import…")), Some("win.import"));
        io.append_submenu(Some(tr("Default _Address Book")), &book_menu);
        menu.append_section(None, &io);
        let about = gio::Menu::new();
        about.append(Some(tr("_Keyboard Shortcuts")), Some("win.shortcuts"));
        about.append(Some(tr("_About Contacts")), Some("app.about"));
        menu.append_section(None, &about);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text(tr("Main Menu"))
            .build()
            .labelled(tr("Main Menu"));
        menu_button.set_primary(true);
        let side_header = adw::HeaderBar::new();
        side_header.pack_start(&new_button);
        side_header.pack_end(&menu_button);
        side_header.pack_end(&select_button);

        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search contacts"))
            .hexpand(true)
            .build()
            .labelled(tr("Search Contacts"));
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        search_box.set_margin_start(6);
        search_box.set_margin_end(6);
        search_box.set_margin_bottom(6);
        search_box.append(&search);

        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        let list_scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let list_stack = gtk::Stack::new();
        list_stack.add_named(&list_scroll, Some("list"));
        list_stack.add_named(
            &adw::StatusPage::builder()
                .icon_name("system-search-symbolic")
                .title(tr("No Results"))
                .build(),
            Some("no-results"),
        );
        let empty = adw::StatusPage::builder()
            .icon_name("avatar-default-symbolic")
            .title(tr("No Contacts"))
            .description(tr("Create a contact or import a vCard file"))
            .build();
        let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        empty_box.set_halign(gtk::Align::Center);
        let create = gtk::Button::builder()
            .label(tr("Create Contact"))
            .css_classes(["pill", "suggested-action"])
            .build();
        create.set_action_name(Some("win.new-contact"));
        let import = gtk::Button::builder()
            .label(tr("Import…"))
            .css_classes(["pill"])
            .build();
        import.set_action_name(Some("win.import"));
        empty_box.append(&create);
        empty_box.append(&import);
        empty.set_child(Some(&empty_box));
        list_stack.add_named(&empty, Some("empty"));
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .build();
        helix_app_common::set_accessible_label(&spinner, tr("Loading contacts"));
        let loading = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        loading.append(&spinner);
        loading.append(
            &gtk::Label::builder()
                .label(tr("Loading Contacts…"))
                .css_classes(["dim-label"])
                .build(),
        );
        list_stack.add_named(&loading, Some("loading"));
        list_stack.set_visible_child_name("loading");

        let action_bar = gtk::ActionBar::new();
        let link_button = gtk::Button::builder()
            .label(tr("Link"))
            .sensitive(false)
            .build();
        let export_button = gtk::Button::builder()
            .label(tr("Export…"))
            .sensitive(false)
            .build();
        let delete_button = gtk::Button::builder()
            .label(tr("Delete"))
            .css_classes(["destructive-action"])
            .sensitive(false)
            .build();
        action_bar.pack_start(&link_button);
        action_bar.pack_start(&export_button);
        action_bar.pack_end(&delete_button);
        action_bar.set_revealed(false);

        let side_view = adw::ToolbarView::new();
        // Typing while the contact list (or anything else in the sidebar)
        // has focus starts a search.
        search.set_key_capture_widget(Some(&side_view));
        side_view.add_top_bar(&side_header);
        side_view.add_top_bar(&search_box);
        side_view.set_content(Some(&list_stack));
        side_view.add_bottom_bar(&action_bar);
        let side_page = adw::NavigationPage::builder()
            .title(tr("Contacts"))
            .child(&side_view)
            .tag("sidebar")
            .build();

        // Content.
        let content_header = adw::HeaderBar::new();
        let edit_button = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text(tr("Edit Contact"))
            .visible(false)
            .build()
            .labelled(tr("Edit Contact"));
        edit_button.set_action_name(Some("win.edit"));
        let done_button = gtk::Button::builder()
            .label(tr("Done"))
            .css_classes(["suggested-action"])
            .visible(false)
            .build();
        let cancel_button = gtk::Button::builder()
            .label(tr("Cancel"))
            .visible(false)
            .build();
        let contact_menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text(tr("Contact Menu"))
            .visible(false)
            .build()
            .labelled(tr("Contact Menu"));
        content_header.pack_start(&cancel_button);
        content_header.pack_end(&contact_menu);
        content_header.pack_end(&edit_button);
        content_header.pack_end(&done_button);
        let content_body = adw::Bin::new();
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&content_header);
        content_view.set_content(Some(&content_body));
        let content_page = adw::NavigationPage::builder()
            .title(tr("Contact"))
            .child(&content_view)
            .tag("content")
            .build();

        let split = adw::NavigationSplitView::builder()
            .sidebar(&side_page)
            .content(&content_page)
            .build();
        split.set_min_sidebar_width(260.0);
        split.set_max_sidebar_width(360.0);

        let unavailable_page = adw::StatusPage::builder()
            .icon_name("dialog-warning-symbolic")
            .title(tr("Contacts Service Unavailable"))
            .build();
        let retry = gtk::Button::builder()
            .label(tr("Retry"))
            .css_classes(["pill"])
            .halign(gtk::Align::Center)
            .build();
        unavailable_page.set_child(Some(&retry));
        let unavailable_view = adw::ToolbarView::new();
        unavailable_view.add_top_bar(&adw::HeaderBar::new());
        unavailable_view.set_content(Some(&unavailable_page));

        let main_stack = gtk::Stack::new();
        main_stack.add_named(&split, Some("main"));
        main_stack.add_named(&unavailable_view, Some("unavailable"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&main_stack));
        window.set_content(Some(&toasts));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 500sp").unwrap());
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let (tx, rx) = async_channel::unbounded();
        let backend = backend::start(tx);

        let ui = Rc::new(Ui {
            window,
            prefs,
            defaults,
            backend,
            links: RefCell::new(links),
            links_path,
            external_links: RefCell::new(Vec::new()),
            folks_personas: folks::load_relationships(&folks::relationships_path()),
            self_uid,
            state: RefCell::new(State {
                startup: Some(opts.clone()),
                ..Default::default()
            }),
            editor: RefCell::default(),
            rebuild_pending: Cell::new(false),
            toasts,
            split,
            search,
            list,
            list_stack,
            select_button,
            checks: RefCell::default(),
            side_page: side_page.clone(),
            action_bar,
            link_button,
            delete_button,
            export_button,
            content_page,
            content_header,
            content_body,
            edit_button,
            done_button,
            cancel_button,
            contact_menu,
            main_stack,
            unavailable_page,
            book_menu,
        });
        if let Some(q) = &opts.search {
            ui.search.set_text(q);
        }
        ui.show_placeholder();
        // Say that contacts are loading if that takes noticeably long (the
        // finished state is announced from `refresh_list_rows`).
        let stack = ui.list_stack.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
            if let Some(stack) = stack.upgrade() {
                if stack.visible_child_name().as_deref() == Some("loading") {
                    helix_app_common::announce(
                        &stack,
                        tr("Loading contacts"),
                        helix_app_common::Urgency::Polite,
                    );
                }
            }
        });
        ui.connect(retry);
        let weak = Rc::downgrade(&ui);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                match weak.upgrade() {
                    Some(ui) => ui.handle(msg),
                    None => break,
                }
            }
        });
        ui
    }

    fn connect(self: &Rc<Self>, retry: gtk::Button) {
        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.backend.send(Cmd::Resync);
            }
        });

        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.refresh_list();
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_stop_search(move |s| {
            s.set_text("");
            if let Some(ui) = weak.upgrade() {
                ui.list.grab_focus();
            }
        });

        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(ui) = weak.upgrade() else { return };
            let idx = row.index() as usize;
            let ind = ui.state.borrow().shown.get(idx).cloned();
            let Some(ind) = ind else { return };
            if ui.state.borrow().selection_mode {
                // Toggle in place: the row keeps focus and the list its
                // scroll position.
                let check = ui.checks.borrow().get(&ind.id()).map(|(_, c)| c.clone());
                if let Some(check) = check {
                    check.set_active(!check.is_active());
                }
                return;
            }
            ui.leave_edit(move |ui| {
                ui.state.borrow_mut().selected = Some(ind.members[0].clone());
                ui.show_individual(&ind);
                ui.split.set_show_content(true);
            });
        });

        let weak = Rc::downgrade(self);
        self.select_button.connect_toggled(move |b| {
            if let Some(ui) = weak.upgrade() {
                {
                    let mut st = ui.state.borrow_mut();
                    st.selection_mode = b.is_active();
                    st.checked.clear();
                }
                ui.action_bar.set_revealed(b.is_active());
                ui.refresh_list();
                ui.update_selection_ui();
            }
        });
        let weak = Rc::downgrade(self);
        self.link_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.link_checked();
            }
        });
        let weak = Rc::downgrade(self);
        self.delete_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                let keys = ui.checked_keys();
                ui.select_button.set_active(false);
                ui.delete_keys(keys);
            }
        });
        let weak = Rc::downgrade(self);
        self.export_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                let contacts = ui
                    .checked_individuals()
                    .into_iter()
                    .map(|i| i.contact)
                    .collect();
                ui.export(contacts);
            }
        });
        let weak = Rc::downgrade(self);
        self.done_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.finish_edit(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.cancel_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.finish_edit(false);
            }
        });

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.flush_deletes();
            }
            glib::Propagation::Proceed
        });

        // Keys inside the window.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape {
                if ui.editor.borrow().is_some() {
                    ui.leave_edit(|_| {});
                    return glib::Propagation::Stop;
                }
                if ui.select_button.is_active() {
                    ui.select_button.set_active(false);
                    return glib::Propagation::Stop;
                }
            }
            if key == gtk::gdk::Key::Delete && mods.is_empty() && ui.editor.borrow().is_none() {
                let focus_in_entry = gtk::prelude::GtkWindowExt::focus(&ui.window)
                    .is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>());
                if !focus_in_entry {
                    if let Some(ind) = ui.selected_individual() {
                        ui.delete_keys(ind.members.clone());
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);

        self.install_actions();
    }

    fn install_actions(self: &Rc<Self>) {
        let w = &self.window;
        let app = w.application().expect("app");
        let add = |name: &str, f: ActionFn| {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(ui) = weak.upgrade() {
                    f(&ui);
                }
            });
            w.add_action(&a);
            let detailed = format!("win.{name}");
            if let Some((_, keys)) = ACCELS.iter().find(|(a, _)| *a == detailed) {
                app.set_accels_for_action(&detailed, keys);
            }
        };
        add("new-contact", Box::new(|ui| ui.new_contact(None)));
        add("edit", Box::new(|ui| ui.start_edit()));
        add(
            "search",
            Box::new(|ui| {
                ui.split.set_show_content(false);
                ui.search.grab_focus();
            }),
        );
        add("import", Box::new(|ui| ui.choose_import()));
        add(
            "shortcuts",
            Box::new(|ui| crate::shortcuts::show(&ui.window)),
        );
        add(
            "delete-contact",
            Box::new(|ui| {
                if let Some(ind) = ui.selected_individual() {
                    ui.delete_keys(ind.members.clone());
                }
            }),
        );
        add(
            "unlink",
            Box::new(|ui| {
                if let Some(ind) = ui.selected_individual() {
                    let external = ui.external_links.borrow().clone();
                    ui.links
                        .borrow_mut()
                        .unlink_members(&ind.members, &external);
                    ui.save_links();
                    ui.rebuild();
                    ui.toast(tr("Contacts unlinked"));
                }
            }),
        );
        add(
            "export-contact",
            Box::new(|ui| {
                if let Some(ind) = ui.selected_individual() {
                    ui.export(vec![ind.contact.clone()]);
                }
            }),
        );

        let surname = self.prefs.get("sort-on-surname", false);
        let sort = gio::SimpleAction::new_stateful(
            "sort",
            Some(glib::VariantTy::STRING),
            &(if surname { "surname" } else { "first" }).to_variant(),
        );
        let weak = Rc::downgrade(self);
        sort.connect_activate(move |a, v| {
            let (Some(ui), Some(v)) = (weak.upgrade(), v) else {
                return;
            };
            a.set_state(v);
            ui.prefs.set("sort-on-surname", v.str() == Some("surname"));
            ui.refresh_list();
        });
        w.add_action(&sort);

        let default_book = self.default_book();
        let book = gio::SimpleAction::new_stateful(
            "default-book",
            Some(glib::VariantTy::STRING),
            &default_book.to_variant(),
        );
        let weak = Rc::downgrade(self);
        book.connect_activate(move |a, v| {
            let (Some(ui), Some(v)) = (weak.upgrade(), v) else {
                return;
            };
            a.set_state(v);
            if let Some(s) = v.str() {
                ui.defaults.set("default-address-book", s.to_string());
            }
        });
        w.add_action(&book);
    }

    fn default_book(&self) -> String {
        let configured = self
            .defaults
            .get("default-address-book", "system-address-book".to_string());
        let st = self.state.borrow();
        if st.writable.get(&configured).copied().unwrap_or(false) || st.writable.is_empty() {
            return configured;
        }
        if st
            .writable
            .get("system-address-book")
            .copied()
            .unwrap_or(false)
        {
            return "system-address-book".into();
        }
        let mut w: Vec<&String> = st
            .writable
            .iter()
            .filter(|(_, w)| **w)
            .map(|(k, _)| k)
            .collect();
        w.sort();
        w.first().map(|s| s.to_string()).unwrap_or(configured)
    }

    fn book_name(&self, uid: &str) -> String {
        self.state
            .borrow()
            .sources
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| s.display_name())
            .unwrap_or_else(|| uid.to_string())
    }

    /// Writable books as (uid, "Name (Account)") pairs.
    fn writable_books(&self) -> Vec<(String, String)> {
        let st = self.state.borrow();
        let mut out: Vec<(String, String)> = st
            .writable
            .iter()
            .filter(|(_, w)| **w)
            .map(|(uid, _)| {
                let name = st
                    .sources
                    .iter()
                    .find(|s| &s.uid == uid)
                    .map(|s| {
                        let account = helix_eds::source::account_label(s, &st.sources);
                        if account == s.display_name() {
                            account
                        } else {
                            format!("{} ({account})", s.display_name())
                        }
                    })
                    .unwrap_or_else(|| uid.clone());
                (uid.clone(), name)
            })
            .collect();
        out.sort_by_key(|a| a.1.to_lowercase());
        out
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Unavailable(e) => {
                self.unavailable_page.set_description(Some(&format!(
                    "Could not reach Evolution Data Server.\n{}",
                    glib::markup_escape_text(&e)
                )));
                self.main_stack.set_visible_child_name("unavailable");
                self.state.borrow_mut().unavailable = Some(e);
            }
            Msg::Sources(sources) => {
                self.main_stack.set_visible_child_name("main");
                self.state.borrow_mut().unavailable = None;
                self.state.borrow_mut().sources = sources;
                self.update_book_menu();
                self.schedule_rebuild();
            }
            Msg::BookOpened { uid, writable } => {
                let mut st = self.state.borrow_mut();
                st.writable.insert(uid.clone(), writable);
                st.books.entry(uid).or_default();
                drop(st);
                self.update_book_menu();
            }
            Msg::BookClosed(uid) => {
                let mut st = self.state.borrow_mut();
                st.writable.remove(&uid);
                st.books.remove(&uid);
                st.loaded.remove(&uid);
                drop(st);
                self.update_book_menu();
                self.schedule_rebuild();
            }
            Msg::Added { book, contacts } | Msg::Modified { book, contacts } => {
                let mut st = self.state.borrow_mut();
                let map = st.books.entry(book).or_default();
                for c in contacts {
                    map.insert(c.uid.clone(), c);
                }
                drop(st);
                self.schedule_rebuild();
            }
            Msg::Removed { book, uids } => {
                {
                    let mut st = self.state.borrow_mut();
                    if let Some(map) = st.books.get_mut(&book) {
                        for u in &uids {
                            map.remove(u);
                        }
                    }
                    for u in &uids {
                        st.pending_delete.remove(&(book.clone(), u.clone()));
                    }
                }
                let mut links = self.links.borrow_mut();
                for u in uids {
                    links.forget(&(book.clone(), u));
                }
                drop(links);
                self.save_links();
                self.schedule_rebuild();
            }
            Msg::Loaded(book) => {
                let n = self.state.borrow().books.get(&book).map_or(0, |m| m.len());
                glib::g_debug!(
                    "helix-contacts",
                    "address book {book} loaded with {n} contacts"
                );
                self.state.borrow_mut().loaded.insert(book);
                self.schedule_rebuild();
                self.maybe_initial_setup();
                self.apply_startup();
            }
            Msg::Created { book, uids, tag } => {
                let select = self.state.borrow().select_created == Some(tag);
                if select {
                    if let Some(uid) = uids.first() {
                        self.state.borrow_mut().selected = Some((book, uid.clone()));
                        self.schedule_rebuild();
                    }
                }
            }
            Msg::Imported(n) => {
                self.toast(&trnf!("Imported {n} contact", "Imported {n} contacts", n))
            }
            Msg::Error(e) => self.toast(&e),
        }
    }

    fn update_book_menu(&self) {
        self.book_menu.remove_all();
        for (uid, name) in self.writable_books() {
            let item = gio::MenuItem::new(Some(&name), None);
            item.set_action_and_target_value(Some("win.default-book"), Some(&uid.to_variant()));
            self.book_menu.append_item(&item);
        }
        if let Some(a) = self
            .window
            .lookup_action("default-book")
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_state(&self.default_book().to_variant());
        }
    }

    fn schedule_rebuild(self: &Rc<Self>) {
        if self.rebuild_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
            if let Some(ui) = weak.upgrade() {
                ui.rebuild_pending.set(false);
                ui.rebuild();
            }
        });
    }

    fn rebuild(self: &Rc<Self>) {
        let surname = self.prefs.get("sort-on-surname", false);
        {
            let st = self.state.borrow();
            let mut books = st.books.clone();
            for (b, u) in &st.pending_delete {
                if let Some(m) = books.get_mut(b) {
                    m.remove(u);
                }
            }
            drop(st);
            let all_loaded = {
                let st = self.state.borrow();
                st.books.keys().all(|b| st.loaded.contains(b))
            };
            if all_loaded {
                let exists = |k: &Key| books.get(&k.0).is_some_and(|m| m.contains_key(&k.1));
                let changed = self.links.borrow_mut().prune(exists);
                if changed {
                    self.save_links();
                }
            }
            let mut external = folks::groups_from_contacts(&books);
            external.extend(folks::groups_from_keyfile(&self.folks_personas, &books));
            let effective = self.links.borrow().effective(&external);
            *self.external_links.borrow_mut() = external;
            let mut list = individual::build(&books, &effective);
            individual::sort(&mut list, surname);
            // The user's own contact goes first.
            if !self.self_uid.is_empty() {
                if let Some(pos) = list.iter().position(|i| i.contains_uid(&self.self_uid)) {
                    let me = list.remove(pos);
                    list.insert(0, me);
                }
            }
            glib::g_debug!("helix-contacts", "showing {} individuals", list.len());
            self.state.borrow_mut().individuals = list;
        }
        self.refresh_list();
    }

    /// Rebuilds the contact list, keeping keyboard focus on the same
    /// contact (e.g. while toggling rows in selection mode) and the scroll
    /// position.
    fn refresh_list(self: &Rc<Self>) {
        let list = self.list.clone();
        helix_app_common::focus::rebuild_keeping_focus(&list, None, || self.refresh_list_rows());
    }

    fn refresh_list_rows(self: &Rc<Self>) {
        let surname = self.prefs.get("sort-on-surname", false);
        let query = self.search.text().to_string();
        let (shown, selection_mode, checked, selected, any, loaded) = {
            let st = self.state.borrow();
            let shown: Vec<Individual> = individual::filter(&st.individuals, &query)
                .into_iter()
                .cloned()
                .collect();
            let loaded = !st.books.is_empty() && st.books.keys().all(|b| st.loaded.contains(b));
            (
                shown,
                st.selection_mode,
                st.checked.clone(),
                st.selected.clone(),
                !st.individuals.is_empty(),
                loaded,
            )
        };
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        self.checks.borrow_mut().clear();
        let mut select_row = None;
        for ind in &shown {
            let row = gtk::ListBoxRow::new();
            helix_app_common::focus::set_key(&row, &ind.id());
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);
            let name = ind.contact.display_name();
            if selection_mode {
                // The row is what has focus and is activated (Space,
                // Enter, click); the check box shows and names its state.
                let on = checked.contains(&ind.id());
                let check = gtk::CheckButton::builder()
                    .active(on)
                    .can_focus(false)
                    .can_target(false)
                    .build()
                    .labelled(&trf!("Select {name}", name = name));
                row.update_state(&[gtk::accessible::State::Checked(tristate(on))]);
                let weak = Rc::downgrade(self);
                let (id, r) = (ind.id(), row.clone());
                check.connect_toggled(move |c| {
                    let Some(ui) = weak.upgrade() else { return };
                    let on = c.is_active();
                    {
                        let mut st = ui.state.borrow_mut();
                        if on {
                            st.checked.insert(id.clone());
                        } else {
                            st.checked.remove(&id);
                        }
                    }
                    r.update_state(&[gtk::accessible::State::Checked(tristate(on))]);
                    ui.update_selection_ui();
                });
                hbox.append(&check);
                self.checks
                    .borrow_mut()
                    .insert(ind.id(), (row.clone(), check));
            }
            let avatar = adw::Avatar::new(32, Some(&name), true);
            if let Some(bytes) = ind.contact.photo_bytes() {
                set_avatar_bytes(&avatar, &bytes);
            }
            hbox.append(&avatar);
            let l = label(&name, &[]);
            l.set_xalign(0.0);
            l.set_hexpand(true);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            hbox.append(&l);
            if !self.self_uid.is_empty() && ind.contains_uid(&self.self_uid) {
                hbox.append(&label(tr("Me"), &["dim-label", "caption"]));
            }
            if ind.is_linked() {
                // The row says it is linked; the icon only shows it.
                let icon = gtk::Image::builder()
                    .icon_name("insert-link-symbolic")
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .build();
                icon.set_tooltip_text(Some(tr("Linked contact")));
                icon.add_css_class("dim-label");
                hbox.append(&icon);
                row.update_property(&[gtk::accessible::Property::Description(tr(
                    "Linked contact",
                ))]);
            }
            row.set_child(Some(&hbox));
            self.list.append(&row);
            if selected.as_ref().is_some_and(|k| ind.members.contains(k)) {
                select_row = Some((row, ind.clone()));
            }
        }
        // Letter headers.
        let keys: Vec<String> = shown
            .iter()
            .enumerate()
            .map(|(i, ind)| {
                if i == 0 && !self.self_uid.is_empty() && ind.contains_uid(&self.self_uid) {
                    String::new()
                } else {
                    individual::group_letter(&ind.contact.sort_key(surname))
                }
            })
            .collect();
        let keys = Rc::new(keys);
        self.list.set_header_func(move |row, before| {
            let i = row.index() as usize;
            let cur = keys.get(i).cloned().unwrap_or_default();
            let prev = before.map(|b| keys.get(b.index() as usize).cloned().unwrap_or_default());
            if !cur.is_empty() && prev.as_deref() != Some(cur.as_str()) {
                let l = gtk::Label::builder()
                    .label(&cur)
                    .xalign(0.0)
                    .css_classes(["heading", "dim-label"])
                    .build();
                l.set_margin_start(12);
                l.set_margin_top(8);
                l.set_margin_bottom(4);
                row.set_header(Some(&l));
            } else {
                row.set_header(None::<&gtk::Widget>);
            }
        });
        self.update_selection_ui();
        let page = if !shown.is_empty() {
            "list"
        } else if any {
            "no-results"
        } else if loaded
            || self.state.borrow().books.is_empty() && !self.state.borrow().sources.is_empty()
        {
            "empty"
        } else {
            "loading"
        };
        let was = self.list_stack.visible_child_name();
        self.list_stack.set_visible_child_name(page);
        if was.as_deref() != Some(page) {
            match (was.as_deref(), page) {
                (_, "loading") => helix_app_common::announce(
                    &self.list_stack,
                    tr("Loading contacts"),
                    helix_app_common::Urgency::Polite,
                ),
                (Some("loading"), "empty") => helix_app_common::announce(
                    &self.list_stack,
                    tr("No contacts"),
                    helix_app_common::Urgency::Polite,
                ),
                (Some("loading"), "list") => {
                    let n = self.state.borrow().individuals.len();
                    helix_app_common::announce(
                        &self.list_stack,
                        &trnf!("{n} contact loaded", "{n} contacts loaded", n),
                        helix_app_common::Urgency::Polite,
                    );
                }
                _ => {}
            }
        }
        if !query.trim().is_empty() {
            let text = match shown.len() {
                0 => tr("No results").to_string(),
                n => trnf!("{n} contact found", "{n} contacts found", n),
            };
            helix_app_common::announce_settled(&self.search, &text);
        }
        self.state.borrow_mut().shown = shown;
        if self.editor.borrow().is_some() {
            if let Some((row, _)) = &select_row {
                self.list.select_row(Some(row));
            }
            return;
        }
        match select_row {
            Some((row, ind)) => {
                if !selection_mode {
                    self.list.select_row(Some(&row));
                }
                self.show_individual(&ind);
            }
            None => {
                self.list.unselect_all();
                self.show_placeholder();
            }
        }
    }

    /// Action bar buttons and the sidebar title ("3 Selected") for the
    /// current selection.
    fn update_selection_ui(&self) {
        let (mode, n) = {
            let st = self.state.borrow();
            (st.selection_mode, st.checked.len())
        };
        self.link_button.set_sensitive(n >= 2);
        self.delete_button.set_sensitive(n >= 1);
        self.export_button.set_sensitive(n >= 1);
        self.side_page.set_title(&selection_title(mode, n));
    }

    fn selected_individual(&self) -> Option<Individual> {
        let st = self.state.borrow();
        let key = st.selected.as_ref()?;
        st.individuals
            .iter()
            .find(|i| i.members.contains(key))
            .cloned()
    }

    fn checked_individuals(&self) -> Vec<Individual> {
        let st = self.state.borrow();
        st.individuals
            .iter()
            .filter(|i| st.checked.contains(&i.id()))
            .cloned()
            .collect()
    }

    fn checked_keys(&self) -> Vec<Key> {
        self.checked_individuals()
            .into_iter()
            .flat_map(|i| i.members)
            .collect()
    }

    fn set_header_mode(&self, editing: bool, ind: Option<&Individual>) {
        self.done_button.set_visible(editing);
        self.cancel_button.set_visible(editing);
        let writable = ind.is_some_and(|i| self.primary_writable(i).is_some());
        self.edit_button.set_visible(!editing && writable);
        self.contact_menu.set_visible(!editing && ind.is_some());
        self.content_header.set_show_back_button(!editing);
    }

    fn primary_writable(&self, ind: &Individual) -> Option<Key> {
        let st = self.state.borrow();
        ind.members
            .iter()
            .find(|(b, _)| st.writable.get(b).copied().unwrap_or(false))
            .cloned()
    }

    fn show_placeholder(&self) {
        // With no contacts (or none loaded yet) the sidebar says so; an
        // empty content pane avoids the contradictory "Select a Contact".
        let has_contacts = !matches!(
            self.list_stack.visible_child_name().as_deref(),
            Some("empty" | "loading")
        );
        if has_contacts {
            let page = adw::StatusPage::builder()
                .icon_name("avatar-default-symbolic")
                .title(tr("Select a Contact"))
                .build();
            self.content_body.set_child(Some(&page));
        } else {
            self.content_body.set_child(None::<&gtk::Widget>);
        }
        self.content_page.set_title(tr("Contact"));
        self.set_header_mode(false, None);
    }

    fn show_individual(self: &Rc<Self>, ind: &Individual) {
        let c = &ind.contact;
        let name = c.display_name();
        self.content_page.set_title(&name);
        let menu = gio::Menu::new();
        if ind.is_linked() {
            menu.append(Some(tr("_Unlink")), Some("win.unlink"));
        }
        menu.append(Some(tr("_Export…")), Some("win.export-contact"));
        let writable_all = {
            let st = self.state.borrow();
            ind.members
                .iter()
                .all(|(b, _)| st.writable.get(b).copied().unwrap_or(false))
        };
        if writable_all {
            menu.append(Some(tr("_Delete Contact")), Some("win.delete-contact"));
        }
        self.contact_menu.set_menu_model(Some(&menu));
        self.set_header_mode(false, Some(ind));

        let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
        page.set_margin_top(24);
        page.set_margin_bottom(24);
        page.set_margin_start(12);
        page.set_margin_end(12);
        let avatar = adw::Avatar::new(96, Some(&name), true);
        if let Some(bytes) = c.photo_bytes() {
            set_avatar_bytes(&avatar, &bytes);
        }
        page.append(&avatar);
        let title = label(&name, &["title-1"]);
        title.set_wrap(true);
        title.set_justify(gtk::Justification::Center);
        title.set_selectable(true);
        page.append(&title);
        let mut sub = Vec::new();
        if !c.nickname.is_empty() && c.nickname != name {
            sub.push(format!("“{}”", c.nickname));
        }
        let job: Vec<&str> = [c.title.as_str(), c.org.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if !job.is_empty() {
            sub.push(job.join(", "));
        }
        if !sub.is_empty() {
            let s = label(&sub.join(" · "), &["dim-label"]);
            s.set_wrap(true);
            s.set_justify(gtk::Justification::Center);
            page.append(&s);
        }

        let group = |title: &str| adw::PreferencesGroup::builder().title(title).build();
        if !c.emails.is_empty() {
            let g = group(tr("Email"));
            for e in &c.emails {
                g.add(&self.value_row(
                    &e.value,
                    vcard::generic_label(&e.types),
                    Some((RowAction::Email, format!("mailto:{}", e.value))),
                ));
            }
            page.append(&g);
        }
        if !c.phones.is_empty() {
            let g = group(tr("Phone"));
            for p in &c.phones {
                let tel: String = p
                    .value
                    .chars()
                    .filter(|ch| ch.is_ascii_digit() || *ch == '+')
                    .collect();
                g.add(&self.value_row(
                    &p.value,
                    phone_label(p),
                    Some((RowAction::Call, format!("tel:{tel}"))),
                ));
            }
            page.append(&g);
        }
        if !c.addresses.is_empty() {
            let g = group(tr("Address"));
            for a in &c.addresses {
                let q = glib::Uri::escape_string(&a.one_line(), None, false);
                g.add(&self.value_row(
                    &a.formatted(),
                    vcard::generic_label(&a.types),
                    Some((RowAction::Map, format!("geo:0,0?q={q}"))),
                ));
            }
            page.append(&g);
        }
        if !c.urls.is_empty() {
            let g = group(tr("Website"));
            for u in &c.urls {
                let uri = if u.value.contains("://") {
                    u.value.clone()
                } else {
                    format!("https://{}", u.value)
                };
                g.add(&self.value_row(
                    &u.value,
                    vcard::generic_label(&u.types),
                    Some((RowAction::Website, uri)),
                ));
            }
            page.append(&g);
        }
        let mut personal = Vec::new();
        if let Some(d) = format_vcard_date(&c.birthday) {
            personal.push((tr("Birthday"), d));
        }
        if let Some(d) = format_vcard_date(&c.anniversary) {
            personal.push((tr("Anniversary"), d));
        }
        if !c.org_unit.is_empty() {
            personal.push((tr("Department"), c.org_unit.clone()));
        }
        if !c.categories.is_empty() {
            personal.push((tr("Groups"), c.categories.join(", ")));
        }
        if !personal.is_empty() {
            let g = group(tr("Personal"));
            for (k, v) in personal {
                g.add(&self.value_row(&v, k, None));
            }
            page.append(&g);
        }
        if !c.note.is_empty() {
            let g = group(tr("Notes"));
            let l = label(&c.note, &[]);
            l.set_wrap(true);
            l.set_xalign(0.0);
            l.set_selectable(true);
            l.set_margin_top(12);
            l.set_margin_bottom(12);
            l.set_margin_start(12);
            l.set_margin_end(12);
            let frame = gtk::Frame::builder()
                .child(&l)
                .css_classes(["view"])
                .build();
            g.add(&frame);
            page.append(&g);
        }
        let g = group(tr("Address Books"));
        for (b, _) in &ind.members {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&self.book_name(b)).as_str())
                .build();
            let st = self.state.borrow();
            if let Some(src) = st.sources.iter().find(|s| &s.uid == b) {
                let account = helix_eds::source::account_label(src, &st.sources);
                row.set_subtitle(&glib::markup_escape_text(&account));
            }
            if !st.writable.get(b).copied().unwrap_or(false) {
                row.add_suffix(&label(tr("Read-only"), &["dim-label", "caption"]));
            }
            g.add(&row);
        }
        page.append(&g);

        let clamp = adw::Clamp::builder().maximum_size(600).child(&page).build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&clamp)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        self.content_body.set_child(Some(&scroll));
    }

    /// A detail row: the value, its type, a Copy button and, for values
    /// that can be opened, a labelled button for that (activating the row
    /// does the same).
    fn value_row(
        &self,
        value: &str,
        kind: &str,
        action: Option<(RowAction, String)>,
    ) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(value).as_str())
            .subtitle(kind)
            .css_classes(["property"])
            .build();
        row.set_title_selectable(true);
        if let Some((action, uri)) = action {
            let open = gtk::Button::builder()
                .icon_name(action.icon())
                .tooltip_text(action.label())
                .css_classes(["flat"])
                .valign(gtk::Align::Center)
                .build()
                .labelled(action.label());
            // Read with the row, e.g. "jane@example.org, Work, Send Email".
            row.update_property(&[gtk::accessible::Property::Description(action.label())]);
            let launch: Rc<dyn Fn()> = {
                let w = self.window.clone();
                let toasts = self.toasts.clone();
                Rc::new(move || {
                    let toasts = toasts.clone();
                    gtk::UriLauncher::new(&uri).launch(
                        Some(&w),
                        None::<&gio::Cancellable>,
                        move |res| {
                            if let Err(e) = res {
                                if !e.matches(gtk::DialogError::Dismissed)
                                    && !e.matches(gtk::DialogError::Cancelled)
                                {
                                    toasts.add_toast(adw::Toast::new(action.failure()));
                                }
                            }
                        },
                    );
                })
            };
            let l = launch.clone();
            open.connect_clicked(move |_| l());
            row.add_suffix(&open);
            row.set_activatable(true);
            row.connect_activated(move |_| launch());
        }
        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text(tr("Copy"))
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build()
            .labelled(&copy_name(value));
        let v = value.to_string();
        let w = self.window.clone();
        let toasts = self.toasts.clone();
        copy.connect_clicked(move |_| {
            w.clipboard().set_text(&v);
            toasts.add_toast(adw::Toast::new(tr("Copied to clipboard")));
        });
        row.add_suffix(&copy);
        row
    }

    fn new_contact(self: &Rc<Self>, prefill_email: Option<&str>) {
        if self.state.borrow().unavailable.is_some() {
            return;
        }
        let books = self.writable_books();
        if books.is_empty() {
            self.toast(tr("No writable address book is available"));
            return;
        }
        if self.editor.borrow().is_some() {
            let email = prefill_email.map(str::to_string);
            self.leave_edit(move |ui| ui.new_contact(email.as_deref()));
            return;
        }
        let mut c = Contact::default();
        if let Some(e) = prefill_email {
            c.emails.push(vcard::Typed::new(&["HOME"], e));
        }
        let ed = Editor::new(&c, &books, &self.default_book());
        self.open_editor(ed, None, tr("New Contact"));
    }

    fn start_edit(self: &Rc<Self>) {
        if self.editor.borrow().is_some() {
            return;
        }
        let Some(ind) = self.selected_individual() else {
            return;
        };
        let Some(key) = self.primary_writable(&ind) else {
            return;
        };
        let base = self
            .state
            .borrow()
            .books
            .get(&key.0)
            .and_then(|m| m.get(&key.1))
            .cloned();
        let Some(base) = base else { return };
        let ed = Editor::new(&base, &[], "");
        self.open_editor(ed, Some(key), &base.display_name());
    }

    fn open_editor(self: &Rc<Self>, ed: Rc<Editor>, key: Option<Key>, title: &str) {
        self.content_body.set_child(Some(&ed.widget));
        self.content_page.set_title(title);
        self.set_header_mode(true, None);
        self.split.set_show_content(true);
        ed.focus_first();
        *self.editor.borrow_mut() = Some((ed, key));
    }

    /// Leaves the editor without saving, then runs `then`. Unsaved changes
    /// are only discarded after the user confirms; if they keep editing,
    /// `then` does not run. Without an editor `then` runs at once.
    fn leave_edit(self: &Rc<Self>, then: impl FnOnce(&Rc<Self>) + 'static) {
        let dirty = self.editor.borrow().as_ref().map(|(ed, _)| ed.is_dirty());
        match dirty {
            None => then(self),
            Some(false) => {
                self.finish_edit(false);
                then(self);
            }
            Some(true) => {
                let dialog = adw::AlertDialog::builder()
                    .heading(tr("Discard Changes?"))
                    .body(tr("Changes to this contact have not been saved."))
                    .default_response("cancel")
                    .close_response("cancel")
                    .build();
                dialog
                    .add_responses(&[("cancel", tr("_Keep Editing")), ("discard", tr("_Discard"))]);
                dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
                let weak = Rc::downgrade(self);
                let then = RefCell::new(Some(then));
                dialog.connect_response(Some("discard"), move |_, _| {
                    let (Some(ui), Some(then)) = (weak.upgrade(), then.take()) else {
                        return;
                    };
                    ui.finish_edit(false);
                    then(&ui);
                });
                dialog.present(Some(&self.window));
            }
        }
    }

    fn finish_edit(self: &Rc<Self>, save: bool) {
        let Some((ed, key)) = self.editor.borrow().clone() else {
            return;
        };
        if save {
            let contact = match ed.collect() {
                Ok(c) => c,
                Err(e) => {
                    // An invalid date is marked inline; take the user there.
                    ed.focus_invalid();
                    self.toast(&e);
                    return;
                }
            };
            match key {
                Some((book, _)) => self.backend.send(Cmd::Modify {
                    book,
                    contact: Box::new(contact),
                }),
                None => {
                    let book = ed.chosen_book().unwrap_or_else(|| self.default_book());
                    let tag = {
                        let mut st = self.state.borrow_mut();
                        st.next_tag += 1;
                        st.select_created = Some(st.next_tag);
                        st.next_tag
                    };
                    self.backend.send(Cmd::Create {
                        book,
                        contact: Box::new(contact),
                        tag,
                    });
                }
            }
        }
        *self.editor.borrow_mut() = None;
        match self.selected_individual() {
            Some(ind) => self.show_individual(&ind),
            None => {
                self.show_placeholder();
                if !save {
                    self.split.set_show_content(false);
                }
            }
        }
    }

    fn save_links(&self) {
        if let Err(e) = self.links.borrow().save(&self.links_path) {
            self.toast(&trf!("Could not save links: {e}", e = e));
        }
    }

    fn link_checked(self: &Rc<Self>) {
        let keys = self.checked_keys();
        if keys.len() < 2 {
            return;
        }
        let before = self.links.borrow().clone();
        self.links.borrow_mut().link(&keys);
        self.save_links();
        self.state.borrow_mut().selected = keys.first().cloned();
        self.select_button.set_active(false);
        self.rebuild();
        let weak = Rc::downgrade(self);
        let toast = adw::Toast::builder()
            .title(tr("Contacts linked"))
            .button_label(tr("Undo"))
            .build();
        toast.connect_button_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                *ui.links.borrow_mut() = before.clone();
                ui.save_links();
                ui.rebuild();
            }
        });
        self.toasts.add_toast(toast);
    }

    fn delete_keys(self: &Rc<Self>, keys: Vec<Key>) {
        let writable: Vec<Key> = {
            let st = self.state.borrow();
            keys.into_iter()
                .filter(|(b, _)| st.writable.get(b).copied().unwrap_or(false))
                .collect()
        };
        if writable.is_empty() {
            self.toast(tr("These contacts are read-only"));
            return;
        }
        // Count individuals, not raw members.
        let n = {
            let st = self.state.borrow();
            st.individuals
                .iter()
                .filter(|i| i.members.iter().any(|m| writable.contains(m)))
                .count()
        };
        self.state
            .borrow_mut()
            .pending_delete
            .extend(writable.iter().cloned());
        self.rebuild();
        let toast = adw::Toast::builder()
            .title(if n == 1 {
                tr("Contact deleted").to_string()
            } else {
                trf!("{n} contacts deleted", n = n)
            })
            .button_label(tr("Undo"))
            .timeout(5)
            .build();
        let undone = Rc::new(Cell::new(false));
        let weak = Rc::downgrade(self);
        let (k, u) = (writable.clone(), undone.clone());
        toast.connect_button_clicked(move |_| {
            u.set(true);
            if let Some(ui) = weak.upgrade() {
                let mut st = ui.state.borrow_mut();
                for key in &k {
                    st.pending_delete.remove(key);
                }
                drop(st);
                ui.rebuild();
            }
        });
        let weak = Rc::downgrade(self);
        toast.connect_dismissed(move |_| {
            if undone.get() {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                ui.commit_deletes(&writable);
            }
        });
        self.toasts.add_toast(toast);
    }

    fn commit_deletes(&self, keys: &[Key]) {
        let mut by_book: HashMap<String, Vec<String>> = HashMap::new();
        {
            let st = self.state.borrow();
            for k in keys.iter().filter(|k| st.pending_delete.contains(*k)) {
                by_book.entry(k.0.clone()).or_default().push(k.1.clone());
            }
        }
        for (book, uids) in by_book {
            self.backend.send(Cmd::Remove { book, uids });
        }
    }

    fn flush_deletes(&self) {
        let keys: Vec<Key> = self.state.borrow().pending_delete.iter().cloned().collect();
        if keys.is_empty() {
            return;
        }
        self.commit_deletes(&keys);
        self.state.borrow_mut().pending_delete.clear();
        // Give the worker a moment to deliver the removals before exit.
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    fn choose_import(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("vCard files"));
        filter.add_mime_type("text/vcard");
        filter.add_mime_type("text/x-vcard");
        filter.add_suffix("vcf");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Import Contacts"))
            .filters(&filters)
            .build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if let (Some(ui), Ok(file)) = (weak.upgrade(), res) {
                if let Some(p) = file.path() {
                    ui.import_file(p);
                }
            }
        });
    }

    fn import_file(&self, path: std::path::PathBuf) {
        let text = match std::fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                self.toast(&trf!(
                    "Could not read {path}: {e}",
                    path = path.display(),
                    e = e
                ));
                return;
            }
        };
        let contacts: Vec<Contact> = vcard::parse_many(&text)
            .iter()
            .map(Contact::from_vcard)
            .collect();
        if contacts.is_empty() {
            self.toast(tr("No contacts found in the file"));
            return;
        }
        self.backend.send(Cmd::Import {
            book: self.default_book(),
            contacts,
        });
    }

    fn export(&self, contacts: Vec<Contact>) {
        if contacts.is_empty() {
            return;
        }
        let name = if contacts.len() == 1 {
            format!("{}.vcf", contacts[0].display_name().replace('/', "-"))
        } else {
            "contacts.vcf".to_string()
        };
        let dialog = gtk::FileDialog::builder()
            .title(tr("Export Contacts"))
            .initial_name(name)
            .build();
        let toasts = self.toasts.clone();
        dialog.save(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            let Ok(file) = res else { return };
            let Some(path) = file.path() else { return };
            let text: String = contacts
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    if let Some(bytes) = c.photo_bytes() {
                        // Embed linked photos so the file stands alone.
                        c.photo = Some(vcard::Photo::Inline {
                            mime: None,
                            data: bytes,
                        });
                    }
                    c.to_vcard_string()
                })
                .collect();
            let msg = match std::fs::write(&path, text) {
                Ok(()) => trnf!(
                    "Exported {n} contact",
                    "Exported {n} contacts",
                    contacts.len()
                ),
                Err(e) => trf!("Export failed: {e}", e = e),
            };
            toasts.add_toast(adw::Toast::new(&msg));
        });
    }

    fn maybe_initial_setup(self: &Rc<Self>) {
        if self.state.borrow().setup_checked || self.prefs.get("did-initial-setup", false) {
            return;
        }
        let books = self.writable_books();
        if books.is_empty() {
            return;
        }
        self.state.borrow_mut().setup_checked = true;
        if books.len() == 1 {
            self.prefs.set("did-initial-setup", true);
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some(tr("Choose an Address Book")),
            Some(tr("New contacts will be saved in this address book. You can change it later from the main menu.")),
        );
        let names: Vec<&str> = books.iter().map(|(_, n)| n.as_str()).collect();
        let dd = gtk::DropDown::from_strings(&names);
        helix_app_common::set_accessible_label(&dd, tr("Address Book for New Contacts"));
        let current = self.default_book();
        dd.set_selected(books.iter().position(|(u, _)| *u == current).unwrap_or(0) as u32);
        dialog.set_extra_child(Some(&dd));
        dialog.add_response("done", tr("_Done"));
        dialog.set_default_response(Some("done"));
        dialog.set_response_appearance("done", adw::ResponseAppearance::Suggested);
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, _| {
            let Some(ui) = weak.upgrade() else { return };
            if let Some((uid, _)) = books.get(dd.selected() as usize) {
                ui.defaults.set("default-address-book", uid.clone());
                ui.update_book_menu();
            }
            ui.prefs.set("did-initial-setup", true);
        });
        dialog.present(Some(&self.window));
    }

    /// Applies `--individual` / `--email` and imports files given on the
    /// command line once the books are loaded.
    fn apply_startup(self: &Rc<Self>) {
        let all_loaded = {
            let st = self.state.borrow();
            !st.books.is_empty() && st.books.keys().all(|b| st.loaded.contains(b))
        };
        if !all_loaded {
            return;
        }
        let Some(opts) = self.state.borrow_mut().startup.take() else {
            return;
        };
        self.rebuild();
        if let Some(uid) = &opts.individual {
            let found = self
                .state
                .borrow()
                .individuals
                .iter()
                .find(|i| i.contains_uid(uid))
                .cloned();
            match found {
                Some(ind) => {
                    self.state.borrow_mut().selected = Some(ind.members[0].clone());
                    self.refresh_list();
                    self.split.set_show_content(true);
                }
                None => self.toast(tr("Contact not found")),
            }
        }
        if let Some(email) = &opts.email {
            let found = self
                .state
                .borrow()
                .individuals
                .iter()
                .find(|i| {
                    i.contact
                        .emails
                        .iter()
                        .any(|e| e.value.eq_ignore_ascii_case(email))
                })
                .cloned();
            match found {
                Some(ind) => {
                    self.state.borrow_mut().selected = Some(ind.members[0].clone());
                    self.refresh_list();
                    self.split.set_show_content(true);
                }
                None => self.new_contact(Some(email)),
            }
        }
        // vCard files given on the command line go into the default book,
        // which is only known once the books are open.
        for f in &opts.files {
            self.import_file(f.clone());
        }
    }
}

/// Formats a vCard date for display ("17 May 1990" or "17 May").
fn format_vcard_date(v: &str) -> Option<String> {
    let (y, m, d) = vcard::parse_date(v)?;
    if !(1..=12).contains(&m) {
        return None;
    }
    let month = helix_app_common::i18n::month_name(m);
    Some(match y {
        Some(y) => format!("{d} {month} {y}"),
        None => format!("{d} {month}"),
    })
}

/// What activating a detail row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    Email,
    Call,
    Map,
    Website,
}

impl RowAction {
    /// The button's name and tooltip, and the row's description.
    fn label(self) -> &'static str {
        match self {
            RowAction::Email => tr("Send Email"),
            RowAction::Call => tr("Call"),
            RowAction::Map => tr("Show on Map"),
            RowAction::Website => tr("Open Website"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            RowAction::Email => "mail-send-symbolic",
            RowAction::Call => "call-start-symbolic",
            RowAction::Map => "mark-location-symbolic",
            RowAction::Website => "web-browser-symbolic",
        }
    }

    /// Toast shown when no application handles the link.
    fn failure(self) -> &'static str {
        match self {
            RowAction::Email => tr("No app available to send email"),
            RowAction::Call => tr("No app available to make calls"),
            RowAction::Map => tr("No app available to show maps"),
            RowAction::Website => tr("No app available to open websites"),
        }
    }
}

/// Accessible name of a detail row's Copy button, naming the value on one
/// line: "Copy jane@example.org".
fn copy_name(value: &str) -> String {
    let one_line: Vec<&str> = value
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    trf!("Copy {}", one_line.join(", "))
}

#[cfg(test)]
mod tests {
    #[test]
    fn copy_names_its_value() {
        assert_eq!(
            super::copy_name("jane@example.org"),
            "Copy jane@example.org"
        );
        assert_eq!(
            super::copy_name("1 Main St\nSpringfield\n"),
            "Copy 1 Main St, Springfield"
        );
    }

    #[test]
    fn date_formatting() {
        assert_eq!(
            super::format_vcard_date("1990-05-17").as_deref(),
            Some("17 May 1990")
        );
        assert_eq!(
            super::format_vcard_date("--1224").as_deref(),
            Some("24 December")
        );
        assert_eq!(super::format_vcard_date("garbage"), None);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::selection_title;

    #[test]
    fn selection_titles() {
        assert_eq!(selection_title(false, 3), "Contacts");
        assert_eq!(selection_title(true, 0), "Select Contacts");
        assert_eq!(selection_title(true, 1), "1 Selected");
        assert_eq!(selection_title(true, 12), "12 Selected");
    }
}
