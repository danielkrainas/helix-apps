//! GTK user interface: windows, header bar, sidebar, actions.

use crate::dialogs;
use crate::docview::{DocView, Model, THUMB_WIDTH};
use crate::layout::{self, ZoomMode};
use crate::links::{Named, OutlineItem, Target};
use crate::presentation;
use crate::render::{DocInfo, JobKey, Renderer, Reply, TextPurpose};
use crate::state::{self, DocState, StateStore};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

pub const APP_ID: &str = "org.helix.DocumentViewer";
const APP_NAME: &str = "Document Viewer";
const BIN: &str = "helix-document-viewer";

/// Where to start in a newly opened document.
#[derive(Clone, Debug)]
pub enum Start {
    Label(String),
    Index(usize),
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub uris: Vec<String>,
    pub start: Option<Start>,
    pub fullscreen: bool,
    pub presentation: bool,
}

thread_local! {
    static STORE: Rc<RefCell<StateStore>> = Rc::new(RefCell::new(StateStore::load(
        &helix_app_common::data_dir(BIN).join("state.json"),
    )));
}

fn store() -> Rc<RefCell<StateStore>> {
    STORE.with(|s| s.clone())
}

pub fn run(opts: Options) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(&app, APP_NAME, "View PDF documents");
    app.connect_startup(|app| {
        load_css();
        let new_window = gio::ActionEntry::builder("new-window")
            .activate(|app: &adw::Application, _, _| Win::new(app).window.present())
            .build();
        app.add_action_entries([new_window]);
        helix_app_common::shortcuts::install(app, ACCELS, crate::shortcuts::GROUPS);
    });
    let initial = RefCell::new(Some(opts));
    app.connect_activate(move |app| {
        let opts = initial.borrow_mut().take().unwrap_or_default();
        if opts.uris.is_empty() {
            Win::new(app).window.present();
            return;
        }
        for (i, uri) in opts.uris.iter().enumerate() {
            let win = Win::new(app);
            let first = i == 0;
            win.open_uri(
                uri,
                if first { opts.start.clone() } else { None },
                first && opts.presentation,
            );
            if first && opts.fullscreen {
                win.window.fullscreen();
            }
            win.window.present();
        }
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

pub(crate) const ACCELS: &[(&str, &[&str])] = &[
    ("app.new-window", &["<Control>n"]),
    ("win.open", &["<Control>o"]),
    ("win.print", &["<Control>p"]),
    ("win.properties", &["<Alt>Return"]),
    ("win.sidebar", &["F9"]),
    ("win.search", &["<Control>f"]),
    ("win.find-next", &["<Control>g"]),
    ("win.find-prev", &["<Control><Shift>g"]),
    (
        "win.zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
    ),
    ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
    ("win.zoom-auto", &["<Control>0", "<Control>KP_0"]),
    ("win.rotate-left", &["<Control>Left"]),
    ("win.rotate-right", &["<Control>Right"]),
    ("win.night", &["<Control>i"]),
    ("win.go-to-page", &["<Control>l"]),
    ("win.back", &["<Alt>Left"]),
    ("win.forward", &["<Alt>Right"]),
    ("win.presentation", &["F5"]),
    ("win.fullscreen", &["F11"]),
    ("win.caret", &["F7"]),
];

/// Actions that need a loaded document.
const DOC_ACTIONS: &[&str] = &[
    "print",
    "properties",
    "sidebar",
    "search",
    "find-next",
    "find-prev",
    "zoom-in",
    "zoom-out",
    "zoom-auto",
    "zoom-mode",
    "zoom",
    "rotate-left",
    "rotate-right",
    "dual",
    "caret",
    "go-to-page",
    "back",
    "forward",
    "presentation",
    "copy",
    "select-all",
];

fn load_css() {
    let css = gtk::CssProvider::new();
    // Pages and thumbnails are paper, white in every style (night mode
    // inverts the rendered pages instead); presentations are on black.
    css.load_from_string(
        "docview { background-color: mix(@window_bg_color, @window_fg_color, 0.10); }
         .presentation-window, presentation { background-color: black; }
         .dv-thumb picture { background-color: white; box-shadow: 0 1px 3px alpha(black, 0.3); }
         .dv-thumb { padding: 6px; }
         .dv-page-entry { min-width: 0; }
         docview > doclink:focus-visible {
             outline: 2px solid @accent_color; outline-offset: 1px;
             border-radius: 2px; background-color: alpha(@accent_bg_color, 0.15); }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

struct Pending {
    id: u64,
    uri: String,
    start: Option<Start>,
    password: Option<String>,
    presentation: bool,
}

type ActionFn = Box<dyn Fn(&Rc<Win>, Option<&glib::Variant>)>;
type ToggleFn = Box<dyn Fn(&Rc<Win>, bool)>;

pub struct Win {
    app: adw::Application,
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    title: adw::WindowTitle,
    header_doc: Vec<gtk::Widget>,
    split: adw::OverlaySplitView,
    side_stack: adw::ViewStack,
    view: DocView,
    model: Rc<Model>,
    page_entry: gtk::Entry,
    page_of: gtk::Label,
    zoom_btn: gtk::MenuButton,
    zoom_entry: gtk::Entry,
    recent_list: gtk::ListBox,
    recent_group: gtk::Box,
    thumbs_store: gtk::StringList,
    thumbs_sel: gtk::SingleSelection,
    thumbs_view: gtk::ListView,
    thumbs_bound: RefCell<HashMap<usize, glib::WeakRef<gtk::Picture>>>,
    thumbs_idle: Cell<bool>,
    outline_store: gio::ListStore,
    outline_page: adw::ViewStackPage,
    search_entry: gtk::SearchEntry,
    case_btn: gtk::CheckButton,
    words_btn: gtk::CheckButton,
    results_store: gtk::StringList,
    results_sel: gtk::SingleSelection,
    results_view: gtk::ListView,
    search_status: gtk::Label,
    search_gen: Cell<u64>,
    uri: RefCell<Option<String>>,
    password: RefCell<Option<String>>,
    pending: RefCell<Option<Pending>>,
    next_id: Cell<u64>,
    history: RefCell<(Vec<usize>, Vec<usize>)>,
    save_source: RefCell<Option<glib::SourceId>>,
    presentation: RefCell<Option<gtk::Window>>,
    print_settings: Rc<RefCell<Option<gtk::PrintSettings>>>,
    syncing: Cell<bool>,
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build()
        .labelled(helix_app_common::a11y::name_from_tooltip(tooltip))
}

impl Win {
    pub fn new(app: &adw::Application) -> Rc<Win> {
        let (renderer, rx) = Renderer::spawn();
        let model = Model::new(renderer);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(1000)
            .default_height(800)
            .width_request(360)
            .height_request(294)
            .build();
        helix_app_common::window_state::persist(
            &window,
            helix_app_common::window_state::Store::file("helix-document-viewer"),
        );

        // ----- header -----
        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new(APP_NAME, "");
        header.set_title_widget(Some(&title));
        let open_btn = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Open…")
            .action_name("win.open")
            .build()
            .labelled("Open…");
        header.pack_start(&open_btn);
        let sidebar_btn = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Sidebar (F9)")
            .action_name("win.sidebar")
            .build()
            .labelled("Sidebar");
        header.pack_start(&sidebar_btn);

        let page_entry = gtk::Entry::builder()
            .width_chars(4)
            .max_width_chars(8)
            .xalign(1.0)
            .tooltip_text("Go to page (Ctrl+L)")
            .build();
        page_entry.add_css_class("dv-page-entry");
        let page_of = gtk::Label::new(None);
        page_of.add_css_class("dim-label");
        // "Current Page", described by the "of 12" next to it.
        helix_app_common::set_accessible_label(&page_entry, "Current Page");
        page_entry.update_relation(&[gtk::accessible::Relation::DescribedBy(&[
            page_of.upcast_ref()
        ])]);
        let page_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        page_box.append(&page_entry);
        page_box.append(&page_of);

        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some("_Open…"), Some("win.open"));
        s1.append(Some("New _Window"), Some("app.new-window"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append(Some("_Dual Page"), Some("win.dual"));
        s2.append(Some("_Night Mode"), Some("win.night"));
        s2.append(Some("_Caret Navigation"), Some("win.caret"));
        s2.append(Some("Rotate _Left"), Some("win.rotate-left"));
        s2.append(Some("Rotate _Right"), Some("win.rotate-right"));
        menu.append_section(None, &s2);
        let s3 = gio::Menu::new();
        s3.append(Some("_Presentation"), Some("win.presentation"));
        s3.append(Some("_Fullscreen"), Some("win.fullscreen"));
        menu.append_section(None, &s3);
        let s4 = gio::Menu::new();
        s4.append(Some("_Print…"), Some("win.print"));
        s4.append(Some("P_roperties"), Some("win.properties"));
        menu.append_section(None, &s4);
        let s5 = gio::Menu::new();
        s5.append(
            Some(helix_app_common::shortcuts::MENU_LABEL),
            Some(helix_app_common::shortcuts::ACTION),
        );
        s5.append(Some("_About Document Viewer"), Some("app.about"));
        menu.append_section(None, &s5);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Main Menu")
            .primary(true)
            .build()
            .labelled("Main Menu");
        header.pack_end(&menu_btn);

        // Zoom menu.
        let zmenu = gio::Menu::new();
        let zc = gio::Menu::new();
        let custom = gio::MenuItem::new(None, None);
        custom.set_attribute_value("custom", Some(&"zoom-controls".to_variant()));
        zc.append_item(&custom);
        zmenu.append_section(None, &zc);
        let zm = gio::Menu::new();
        for (label, mode) in [
            ("Fit _Width", "fit-width"),
            ("Fit _Page", "fit-page"),
            ("_Automatic", "automatic"),
        ] {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some("win.zoom-mode"), Some(&mode.to_variant()));
            zm.append_item(&item);
        }
        zmenu.append_section(None, &zm);
        let zp = gio::Menu::new();
        for z in [0.5f64, 0.75, 1.0, 1.5, 2.0, 4.0] {
            let item = gio::MenuItem::new(Some(&format!("{:.0}%", z * 100.0)), None);
            item.set_action_and_target_value(Some("win.zoom"), Some(&z.to_variant()));
            zp.append_item(&item);
        }
        zmenu.append_section(None, &zp);
        let zpop = gtk::PopoverMenu::from_model(Some(&zmenu));
        let zbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        zbox.set_margin_start(6);
        zbox.set_margin_end(6);
        let zoom_entry = gtk::Entry::builder()
            .width_chars(6)
            .xalign(1.0)
            .tooltip_text("Zoom (%)")
            .build()
            .labelled("Zoom Level");
        let zout = icon_button("zoom-out-symbolic", "Zoom Out", "win.zoom-out");
        let zin = icon_button("zoom-in-symbolic", "Zoom In", "win.zoom-in");
        zout.add_css_class("flat");
        zin.add_css_class("flat");
        zbox.append(&zout);
        zbox.append(&zoom_entry);
        zbox.append(&zin);
        zpop.add_child(&zbox, "zoom-controls");
        let zoom_btn = gtk::MenuButton::builder()
            .label("100%")
            .popover(&zpop)
            .tooltip_text("Zoom")
            .build();
        header.pack_end(&zoom_btn);
        header.pack_end(&page_box);
        let search_btn = gtk::Button::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search (Ctrl+F)")
            .action_name("win.search")
            .build()
            .labelled("Search");
        header.pack_end(&search_btn);
        let header_doc: Vec<gtk::Widget> = vec![
            sidebar_btn.upcast(),
            page_box.upcast(),
            zoom_btn.clone().upcast(),
            search_btn.upcast(),
        ];

        // ----- start page -----
        let status = adw::StatusPage::builder()
            .icon_name("x-office-document-symbolic")
            .title("Open a Document")
            .description("Drag and drop a PDF here, or choose one")
            .build();
        let open_big = gtk::Button::builder()
            .label("_Open…")
            .use_underline(true)
            .action_name("win.open")
            .halign(gtk::Align::Center)
            .build();
        open_big.add_css_class("pill");
        open_big.add_css_class("suggested-action");
        let recent_list = gtk::ListBox::new();
        recent_list.add_css_class("boxed-list");
        recent_list.set_selection_mode(gtk::SelectionMode::None);
        let recent_label = gtk::Label::builder()
            .label("Recent Documents")
            .xalign(0.0)
            .build();
        recent_label.add_css_class("heading");
        let recent_group = gtk::Box::new(gtk::Orientation::Vertical, 12);
        recent_group.append(&recent_label);
        recent_group.append(&recent_list);
        let start_box = gtk::Box::new(gtk::Orientation::Vertical, 24);
        start_box.append(&open_big);
        start_box.append(&recent_group);
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .child(&start_box)
            .build();
        status.set_child(Some(&clamp));

        // ----- document view -----
        let view = DocView::default();
        view.set_model(model.clone());
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .hexpand(true)
            .vexpand(true)
            .build();

        // Sidebar: thumbnails.
        let thumbs_store = gtk::StringList::new(&[]);
        let thumbs_sel = gtk::SingleSelection::new(Some(thumbs_store.clone()));
        thumbs_sel.set_autoselect(false);
        thumbs_sel.set_can_unselect(true);
        let thumbs_view =
            gtk::ListView::new(Some(thumbs_sel.clone()), None::<gtk::ListItemFactory>);
        thumbs_view.add_css_class("navigation-sidebar");
        let thumbs_scroll = gtk::ScrolledWindow::builder()
            .child(&thumbs_view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        // Outline.
        let outline_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let tree = gtk::TreeListModel::new(outline_store.clone(), false, false, |obj| {
            let b = obj.downcast_ref::<glib::BoxedAnyObject>()?;
            let item = b.borrow::<Rc<OutlineItem>>();
            if item.children.is_empty() {
                return None;
            }
            let store = gio::ListStore::new::<glib::BoxedAnyObject>();
            for c in &item.children {
                store.append(&glib::BoxedAnyObject::new(Rc::new(c.clone())));
            }
            Some(store.upcast())
        });
        let outline_sel = gtk::NoSelection::new(Some(tree));
        let outline_factory = gtk::SignalListItemFactory::new();
        outline_factory.connect_setup(|_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let exp = gtk::TreeExpander::new();
            exp.set_child(Some(&label));
            li.set_child(Some(&exp));
        });
        outline_factory.connect_bind(|_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(row) = li.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            let Some(exp) = li.child().and_downcast::<gtk::TreeExpander>() else {
                return;
            };
            exp.set_list_row(Some(&row));
            if let Some(b) = row.item().and_downcast::<glib::BoxedAnyObject>() {
                let item = b.borrow::<Rc<OutlineItem>>();
                if let Some(l) = exp.child().and_downcast::<gtk::Label>() {
                    l.set_label(&item.title);
                    l.set_tooltip_text(Some(&item.title));
                }
            }
        });
        let outline_view = gtk::ListView::new(Some(outline_sel), Some(outline_factory));
        outline_view.set_single_click_activate(true);
        outline_view.add_css_class("navigation-sidebar");
        let outline_scroll = gtk::ScrolledWindow::builder()
            .child(&outline_view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        // Search.
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search document")
            .search_delay(300)
            .build()
            .labelled("Search Document");
        let case_btn = gtk::CheckButton::with_label("Match case");
        let words_btn = gtk::CheckButton::with_label("Whole words only");
        let search_status = gtk::Label::builder().xalign(0.0).build();
        search_status.add_css_class("dim-label");
        let results_store = gtk::StringList::new(&[]);
        let results_sel = gtk::SingleSelection::new(Some(results_store.clone()));
        results_sel.set_autoselect(false);
        results_sel.set_can_unselect(true);
        let results_view =
            gtk::ListView::new(Some(results_sel.clone()), None::<gtk::ListItemFactory>);
        results_view.set_single_click_activate(true);
        results_view.add_css_class("navigation-sidebar");
        let results_scroll = gtk::ScrolledWindow::builder()
            .child(&results_view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let search_top = gtk::Box::new(gtk::Orientation::Vertical, 6);
        search_top.set_margin_start(6);
        search_top.set_margin_end(6);
        search_top.set_margin_top(6);
        search_top.append(&search_entry);
        let opts_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        opts_box.append(&case_btn);
        opts_box.append(&words_btn);
        search_top.append(&opts_box);
        search_top.append(&search_status);
        search_box.append(&search_top);
        search_box.append(&results_scroll);

        let side_stack = adw::ViewStack::new();
        side_stack.add_titled_with_icon(
            &thumbs_scroll,
            Some("thumbnails"),
            "Pages",
            "view-grid-symbolic",
        );
        let outline_page = side_stack.add_titled_with_icon(
            &outline_scroll,
            Some("outline"),
            "Outline",
            "view-list-symbolic",
        );
        side_stack.add_titled_with_icon(
            &search_box,
            Some("search"),
            "Search",
            "system-search-symbolic",
        );
        let switcher = adw::ViewSwitcher::builder()
            .stack(&side_stack)
            .policy(adw::ViewSwitcherPolicy::Narrow)
            .build();
        let side_tv = adw::ToolbarView::new();
        let side_header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .show_start_title_buttons(false)
            .title_widget(&switcher)
            .build();
        side_tv.add_top_bar(&side_header);
        side_tv.set_content(Some(&side_stack));

        let split = adw::OverlaySplitView::builder()
            .sidebar(&side_tv)
            .content(&scroller)
            .show_sidebar(false)
            .min_sidebar_width(200.0)
            .max_sidebar_width(300.0)
            .build();

        let stack = gtk::Stack::new();
        stack.add_named(&status, Some("start"));
        stack.add_named(&split, Some("doc"));
        stack.set_visible_child_name("start");

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&stack));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&tv));
        window.set_content(Some(&toasts));

        // Narrow windows: overlay sidebar.
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            640.0,
            adw::LengthUnit::Sp,
        ));
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let win = Rc::new(Win {
            app: app.clone(),
            window,
            toasts,
            stack,
            title,
            header_doc,
            split,
            side_stack,
            view,
            model,
            page_entry,
            page_of,
            zoom_btn,
            zoom_entry,
            recent_list,
            recent_group,
            thumbs_store,
            thumbs_sel,
            thumbs_view,
            thumbs_bound: RefCell::default(),
            thumbs_idle: Cell::new(false),
            outline_store,
            outline_page,
            search_entry,
            case_btn,
            words_btn,
            results_store,
            results_sel,
            results_view,
            search_status,
            search_gen: Cell::new(0),
            uri: RefCell::new(None),
            password: RefCell::new(None),
            pending: RefCell::new(None),
            next_id: Cell::new(1),
            history: RefCell::default(),
            save_source: RefCell::new(None),
            presentation: RefCell::new(None),
            print_settings: Rc::default(),
            syncing: Cell::new(false),
        });
        win.setup_actions();
        win.setup_view();
        win.setup_sidebar(&outline_view);
        win.setup_misc();
        win.set_doc_ui(false);
        win.refresh_recent();

        // The window keeps its controller alive until it is destroyed.
        let keep = RefCell::new(Some(win.clone()));
        win.window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });

        let weak = Rc::downgrade(&win);
        glib::spawn_future_local(async move {
            while let Ok(r) = rx.recv().await {
                let Some(win) = weak.upgrade() else { break };
                win.on_reply(r);
            }
        });
        win
    }

    fn weak(self: &Rc<Self>) -> Weak<Win> {
        Rc::downgrade(self)
    }

    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    fn action(&self, name: &str) -> Option<gio::SimpleAction> {
        self.window
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
    }

    fn set_doc_ui(&self, loaded: bool) {
        for a in DOC_ACTIONS {
            if let Some(a) = self.action(a) {
                a.set_enabled(loaded);
            }
        }
        for w in &self.header_doc {
            w.set_visible(loaded);
        }
        self.stack
            .set_visible_child_name(if loaded { "doc" } else { "start" });
    }

    // ----- actions -----

    fn setup_actions(self: &Rc<Self>) {
        let add = |name: &str, f: ActionFn| {
            let a = gio::SimpleAction::new(name, None);
            let w = self.weak();
            a.connect_activate(move |_, p| {
                if let Some(win) = w.upgrade() {
                    f(&win, p);
                }
            });
            self.window.add_action(&a);
        };
        add("open", Box::new(|w, _| w.choose_file()));
        add("print", Box::new(|w, _| w.print()));
        add(
            "properties",
            Box::new(|w, _| {
                if let Some(info) = w.model.info() {
                    dialogs::show_properties(&w.window, &info);
                }
            }),
        );
        add(
            "search",
            Box::new(|w, _| {
                w.split.set_show_sidebar(true);
                w.side_stack.set_visible_child_name("search");
                w.search_entry.grab_focus();
            }),
        );
        add("find-next", Box::new(|w, _| w.find(true)));
        add("find-prev", Box::new(|w, _| w.find(false)));
        add("zoom-in", Box::new(|w, _| w.view.zoom_in(None)));
        add("zoom-out", Box::new(|w, _| w.view.zoom_out(None)));
        add(
            "zoom-auto",
            Box::new(|w, _| w.view.set_zoom_mode(ZoomMode::Automatic)),
        );
        add(
            "rotate-left",
            Box::new(|w, _| {
                // Ctrl+Left moves the caret by a word in caret mode.
                if w.view.caret_mode() && w.view.has_focus() {
                    w.view.caret_word(false);
                } else {
                    w.view.set_rotation(w.view.rotation() + 270)
                }
            }),
        );
        add(
            "rotate-right",
            Box::new(|w, _| {
                if w.view.caret_mode() && w.view.has_focus() {
                    w.view.caret_word(true);
                } else {
                    w.view.set_rotation(w.view.rotation() + 90)
                }
            }),
        );
        add(
            "go-to-page",
            Box::new(|w, _| {
                w.page_entry.grab_focus();
            }),
        );
        add("back", Box::new(|w, _| w.history_step(true)));
        add("forward", Box::new(|w, _| w.history_step(false)));
        add("presentation", Box::new(|w, _| w.start_presentation()));
        add(
            "fullscreen",
            Box::new(|w, _| w.window.set_fullscreened(!w.window.is_fullscreen())),
        );
        add(
            "copy",
            Box::new(|w, _| w.view.request_selected_text(TextPurpose::Clipboard)),
        );
        add("select-all", Box::new(|w, _| w.view.select_all()));

        let zoom = gio::SimpleAction::new("zoom", Some(glib::VariantTy::DOUBLE));
        let w = self.weak();
        zoom.connect_activate(move |_, p| {
            if let (Some(win), Some(z)) = (w.upgrade(), p.and_then(|p| p.get::<f64>())) {
                win.view.set_zoom(z, None);
            }
        });
        self.window.add_action(&zoom);

        let mode = gio::SimpleAction::new_stateful(
            "zoom-mode",
            Some(glib::VariantTy::STRING),
            &"automatic".to_variant(),
        );
        let w = self.weak();
        mode.connect_activate(move |a, p| {
            let Some(win) = w.upgrade() else { return };
            if let Some(m) = p
                .and_then(|p| p.get::<String>())
                .and_then(|s| ZoomMode::parse(&s))
            {
                a.set_state(&m.as_str().to_variant());
                win.view.set_zoom_mode(m);
            }
        });
        self.window.add_action(&mode);

        let toggle = |name: &str, f: ToggleFn| {
            let a = gio::SimpleAction::new_stateful(name, None, &false.to_variant());
            let w = self.weak();
            a.connect_activate(move |a, _| {
                let v = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
                a.set_state(&v.to_variant());
                if let Some(win) = w.upgrade() {
                    f(&win, v);
                }
            });
            self.window.add_action(&a);
        };
        toggle(
            "dual",
            Box::new(|w, v| {
                w.view.set_dual(v);
                w.schedule_save();
            }),
        );
        toggle(
            "caret",
            Box::new(|w, v| {
                w.view.set_caret_mode(v);
                w.toast(if v {
                    "Caret navigation on: arrow keys move the caret, Shift+arrows select"
                } else {
                    "Caret navigation off"
                });
            }),
        );
        toggle(
            "night",
            Box::new(|w, v| {
                w.model.night.set(v);
                w.view.queue_draw();
                if let Some(p) = w.presentation.borrow().as_ref() {
                    if let Some(c) = p.child() {
                        c.queue_draw();
                    }
                }
            }),
        );

        let sidebar = gio::SimpleAction::new_stateful("sidebar", None, &false.to_variant());
        let w = self.weak();
        sidebar.connect_activate(move |_, _| {
            if let Some(win) = w.upgrade() {
                win.split.set_show_sidebar(!win.split.shows_sidebar());
            }
        });
        self.window.add_action(&sidebar);
        let w = self.weak();
        self.split.connect_show_sidebar_notify(move |s| {
            if let Some(win) = w.upgrade() {
                if let Some(a) = win.action("sidebar") {
                    a.set_state(&s.shows_sidebar().to_variant());
                }
                win.schedule_save();
            }
        });
    }

    fn set_toggle_state(&self, name: &str, v: bool) {
        if let Some(a) = self.action(name) {
            a.set_state(&v.to_variant());
        }
    }

    // ----- view wiring -----

    fn setup_view(self: &Rc<Self>) {
        let mut cbs = self.view.callbacks();
        let w = self.weak();
        cbs.page_changed = Some(Box::new(move |p| {
            if let Some(win) = w.upgrade() {
                win.on_page_changed(p);
            }
        }));
        let w = self.weak();
        cbs.zoom_changed = Some(Box::new(move |z, mode| {
            if let Some(win) = w.upgrade() {
                let label = format!("{:.0}%", z * 100.0);
                win.zoom_btn.set_label(&label);
                if !win.zoom_entry.has_focus() {
                    win.zoom_entry.set_text(&label);
                }
                if let Some(a) = win.action("zoom-mode") {
                    a.set_state(&mode.as_str().to_variant());
                }
                win.schedule_save();
            }
        }));
        let w = self.weak();
        cbs.link_activated = Some(Box::new(move |t| {
            if let Some(win) = w.upgrade() {
                win.follow(t);
            }
        }));
        let w = self.weak();
        cbs.caret_mode_changed = Some(Box::new(move |on| {
            if let Some(win) = w.upgrade() {
                win.set_toggle_state("caret", on);
            }
        }));
        drop(cbs);

        // Ctrl+C / Ctrl+A while the document has focus.
        let keys = gtk::EventControllerKey::new();
        let w = self.weak();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(win) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if state.contains(gdk::ModifierType::CONTROL_MASK)
                && !state.contains(gdk::ModifierType::SHIFT_MASK)
            {
                match key.to_lower() {
                    gdk::Key::c | gdk::Key::Insert => {
                        win.view.request_selected_text(TextPurpose::Clipboard);
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::a => {
                        win.view.select_all();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }
            glib::Propagation::Proceed
        });
        self.view.add_controller(keys);

        // Context menu, from the secondary button or Menu / Shift+F10.
        let cm = gio::Menu::new();
        cm.append(Some("_Copy"), Some("win.copy"));
        cm.append(Some("Select _All"), Some("win.select-all"));
        cm.append(Some("_Caret Navigation"), Some("win.caret"));
        let pop = gtk::PopoverMenu::from_model(Some(&cm));
        pop.set_parent(&self.view);
        pop.set_has_arrow(false);
        pop.set_halign(gtk::Align::Start);
        let popup = move |x: f64, y: f64| {
            pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.popup();
        };
        let popup = Rc::new(popup);
        let rc = gtk::GestureClick::new();
        rc.set_button(gdk::BUTTON_SECONDARY);
        let p = popup.clone();
        rc.connect_pressed(move |_, _, x, y| p(x, y));
        self.view.add_controller(rc);
        let menu_keys = gtk::EventControllerKey::new();
        let view = self.view.downgrade();
        menu_keys.connect_key_pressed(move |_, key, _, state| {
            let shift_f10 = key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK);
            if key == gdk::Key::Menu || shift_f10 {
                if let Some(view) = view.upgrade() {
                    let (x, y) = view.context_point();
                    popup(x, y);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        self.view.add_controller(menu_keys);
    }

    fn on_page_changed(self: &Rc<Self>, p: usize) {
        let n = self.model.n_pages();
        let Some(info) = self.model.info() else {
            return;
        };
        let label = info
            .labels
            .get(p)
            .cloned()
            .unwrap_or_else(|| (p + 1).to_string());
        if !self.page_entry.has_focus() {
            self.page_entry.set_text(&label);
        }
        let of = if info.has_labels && label != (p + 1).to_string() {
            format!("({} of {})", p + 1, n)
        } else {
            format!("of {n}")
        };
        self.page_of.set_label(&of);
        helix_app_common::announce_settled(
            &self.view,
            &crate::docview::page_description(&label, p, n),
        );
        if self.thumbs_sel.selected() != p as u32 {
            self.syncing.set(true);
            self.thumbs_sel.set_selected(p as u32);
            if self.split.shows_sidebar() {
                self.thumbs_view
                    .scroll_to(p as u32, gtk::ListScrollFlags::NONE, None);
            }
            self.syncing.set(false);
        }
        self.schedule_save();
    }

    fn jump(&self, page: usize, top: Option<f64>, left: Option<f64>) {
        let cur = self.view.current_page();
        {
            let mut h = self.history.borrow_mut();
            if h.0.last() != Some(&cur) {
                h.0.push(cur);
            }
            h.1.clear();
        }
        self.view.go_to(page, top, left);
    }

    fn history_step(&self, back: bool) {
        let cur = self.view.current_page();
        let target = {
            let mut h = self.history.borrow_mut();
            let (from, to) = if back { (&mut h.0, 0) } else { (&mut h.1, 1) };
            let t = from.pop();
            if t.is_some() {
                if to == 0 {
                    h.1.push(cur);
                } else {
                    h.0.push(cur);
                }
            }
            t
        };
        if let Some(p) = target {
            self.view.go_to(p, None, None);
        }
    }

    fn follow(self: &Rc<Self>, t: Target) {
        match t {
            Target::Page { page, top, left } => self.jump(page, top, left),
            Target::Uri(uri) => {
                gtk::UriLauncher::new(&uri).launch(
                    Some(&self.window),
                    None::<&gio::Cancellable>,
                    |_| {},
                );
            }
            Target::Remote { file, page } => {
                let base = self.uri.borrow().clone().map(|u| gio::File::for_uri(&u));
                let f = match base.and_then(|b| b.parent()) {
                    Some(dir) if !file.contains("://") => dir.resolve_relative_path(&file),
                    _ => gio::File::for_commandline_arg(&file),
                };
                let w = Win::new(&self.app);
                w.open_uri(&f.uri(), page.map(Start::Index), false);
                w.window.present();
            }
            Target::Named(n) => match n {
                Named::NextPage => self.view.next_page(),
                Named::PrevPage => self.view.prev_page(),
                Named::FirstPage => self.jump(0, None, None),
                Named::LastPage => self.jump(self.model.n_pages().saturating_sub(1), None, None),
                Named::GoBack => self.history_step(true),
                Named::GoForward => self.history_step(false),
            },
        }
    }

    // ----- sidebar -----

    fn setup_sidebar(self: &Rc<Self>, outline_view: &gtk::ListView) {
        // Thumbnails factory.
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(|_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            b.add_css_class("dv-thumb");
            let pic = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Fill)
                .halign(gtk::Align::Center)
                .build();
            let l = gtk::Label::new(None);
            l.add_css_class("caption");
            b.append(&pic);
            b.append(&l);
            li.set_child(Some(&b));
        });
        let w = self.weak();
        f.connect_bind(move |_, li| {
            let Some(win) = w.upgrade() else { return };
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            let pos = li.position() as usize;
            let Some(b) = li.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(pic) = b.first_child().and_downcast::<gtk::Picture>() else {
                return;
            };
            let Some(l) = b.last_child().and_downcast::<gtk::Label>() else {
                return;
            };
            let (pw, ph) = win.model.size(pos);
            let tw = THUMB_WIDTH as i32;
            pic.set_size_request(tw, (tw as f64 * ph / pw).round() as i32);
            l.set_label(&win.model.label(pos));
            let tex = win.model.thumbs.borrow().peek(&(pos as u32)).cloned();
            pic.set_paintable(tex.as_ref());
            win.thumbs_bound.borrow_mut().insert(pos, pic.downgrade());
            win.schedule_thumbs();
        });
        let w = self.weak();
        f.connect_unbind(move |_, li| {
            let Some(win) = w.upgrade() else { return };
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            win.thumbs_bound
                .borrow_mut()
                .remove(&(li.position() as usize));
            win.schedule_thumbs();
        });
        self.thumbs_view.set_factory(Some(&f));
        let w = self.weak();
        self.thumbs_sel.connect_selected_notify(move |s| {
            let Some(win) = w.upgrade() else { return };
            if win.syncing.get() || s.selected() == gtk::INVALID_LIST_POSITION {
                return;
            }
            win.jump(s.selected() as usize, None, None);
        });

        // Outline activation.
        let w = self.weak();
        outline_view.connect_activate(move |lv, pos| {
            let Some(win) = w.upgrade() else { return };
            let Some(row) = lv
                .model()
                .and_then(|m| m.item(pos))
                .and_downcast::<gtk::TreeListRow>()
            else {
                return;
            };
            let Some(b) = row.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let target = b.borrow::<Rc<OutlineItem>>().target.clone();
            if let Some(t) = target {
                win.follow(t);
                if win.split.is_collapsed() {
                    win.split.set_show_sidebar(false);
                }
            }
        });

        // Search results factory.
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(|_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let p = gtk::Label::builder().xalign(0.0).build();
            p.add_css_class("dim-label");
            p.add_css_class("dv-result-page");
            p.add_css_class("caption");
            // The snippet starts a few words before the match, so the match
            // stays visible when the end is ellipsized.
            let c = gtk::Label::builder()
                .xalign(0.0)
                .use_markup(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            b.append(&p);
            b.append(&c);
            li.set_child(Some(&b));
        });
        let w = self.weak();
        f.connect_bind(move |_, li| {
            let Some(win) = w.upgrade() else { return };
            let li = li.downcast_ref::<gtk::ListItem>().unwrap();
            let pos = li.position() as usize;
            let Some(b) = li.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let search = win.model.search.borrow();
            let Some(hit) = search.hits().get(pos) else {
                return;
            };
            let page = format!("Page {}", win.model.label(hit.page));
            if let Some(p) = b.first_child().and_downcast::<gtk::Label>() {
                p.set_label(&page);
            }
            if let Some(c) = b.last_child().and_downcast::<gtk::Label>() {
                c.set_markup(&hit.context_markup());
                c.set_tooltip_text(Some(&hit.context));
            }
            li.set_accessible_label(&format!("{page}: {}", hit.context));
        });
        self.results_view.set_factory(Some(&f));
        let w = self.weak();
        self.results_view.connect_activate(move |_, pos| {
            if let Some(win) = w.upgrade() {
                win.model
                    .search
                    .borrow_mut()
                    .set_current(Some(pos as usize));
                win.show_hit(pos as usize);
            }
        });

        let w = self.weak();
        self.search_entry.connect_search_changed(move |_| {
            if let Some(win) = w.upgrade() {
                win.start_search();
            }
        });
        for b in [&self.case_btn, &self.words_btn] {
            let w = self.weak();
            b.connect_toggled(move |_| {
                if let Some(win) = w.upgrade() {
                    win.start_search();
                }
            });
        }
        let w = self.weak();
        self.search_entry.connect_activate(move |_| {
            if let Some(win) = w.upgrade() {
                win.find(true);
            }
        });
        let w = self.weak();
        self.search_entry.connect_next_match(move |_| {
            if let Some(win) = w.upgrade() {
                win.find(true);
            }
        });
        let w = self.weak();
        self.search_entry.connect_previous_match(move |_| {
            if let Some(win) = w.upgrade() {
                win.find(false);
            }
        });
        let w = self.weak();
        self.search_entry.connect_stop_search(move |e| {
            if let Some(win) = w.upgrade() {
                e.set_text("");
                win.view.grab_focus();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let w = self.weak();
        keys.connect_key_pressed(move |_, key, _, state| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                && state.contains(gdk::ModifierType::SHIFT_MASK)
            {
                if let Some(win) = w.upgrade() {
                    win.find(false);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.search_entry.add_controller(keys);
    }

    fn schedule_thumbs(self: &Rc<Self>) {
        if self.thumbs_idle.replace(true) {
            return;
        }
        let w = self.weak();
        glib::idle_add_local_once(move || {
            let Some(win) = w.upgrade() else { return };
            win.thumbs_idle.set(false);
            let sf = win.window.scale_factor().max(1) as u32;
            let mut pages: Vec<usize> = win.thumbs_bound.borrow().keys().copied().collect();
            pages.sort_unstable();
            let thumbs = win.model.thumbs.borrow();
            let wants = pages
                .into_iter()
                .filter(|p| !thumbs.contains(&(*p as u32)))
                .map(|p| JobKey::Thumb {
                    page: p as u32,
                    width: THUMB_WIDTH * sf,
                })
                .collect();
            drop(thumbs);
            win.model
                .renderer
                .set_wants(crate::render::Client::Thumbs, wants);
        });
    }

    fn rebuild_sidebar(&self, info: &DocInfo) {
        self.thumbs_bound.borrow_mut().clear();
        let labels: Vec<&str> = info.labels.iter().map(|s| s.as_str()).collect();
        self.syncing.set(true);
        self.thumbs_store
            .splice(0, self.thumbs_store.n_items(), &labels);
        self.thumbs_sel.set_selected(gtk::INVALID_LIST_POSITION);
        self.syncing.set(false);
        self.outline_store.remove_all();
        for item in &info.outline {
            self.outline_store
                .append(&glib::BoxedAnyObject::new(Rc::new(item.clone())));
        }
        self.outline_page.set_visible(!info.outline.is_empty());
        self.reset_search_results();
    }

    // ----- search -----

    fn reset_search_results(&self) {
        self.model.search.borrow_mut().clear();
        self.results_store
            .splice(0, self.results_store.n_items(), &[]);
        self.search_status.set_label("");
    }

    fn start_search(&self) {
        let gen = self.search_gen.get() + 1;
        self.search_gen.set(gen);
        self.reset_search_results();
        let q = self.search_entry.text().to_string();
        if q.trim().is_empty() || self.model.info().is_none() {
            self.model.renderer.cancel_search();
        } else {
            self.search_status.set_label("Searching…");
            self.model.renderer.search(
                gen,
                &q,
                self.case_btn.is_active(),
                self.words_btn.is_active(),
            );
        }
        self.view.queue_draw();
    }

    fn find(&self, forward: bool) {
        let page = self.view.current_page();
        let i = {
            let mut s = self.model.search.borrow_mut();
            if forward {
                s.next(page)
            } else {
                s.prev(page)
            }
        };
        if let Some(i) = i {
            self.show_hit(i);
        }
    }

    fn show_hit(&self, i: usize) {
        let hit = self.model.search.borrow().hits().get(i).cloned();
        if let Some(h) = hit {
            self.view.show_rect(h.page, &h.rect);
            self.syncing.set(true);
            self.results_sel.set_selected(i as u32);
            self.results_view
                .scroll_to(i as u32, gtk::ListScrollFlags::NONE, None);
            self.syncing.set(false);
            self.view.queue_draw();
        }
    }

    // ----- misc -----

    fn setup_misc(self: &Rc<Self>) {
        // Page entry.
        let w = self.weak();
        self.page_entry.connect_activate(move |e| {
            let Some(win) = w.upgrade() else { return };
            let Some(info) = win.model.info() else { return };
            let text = e.text().trim().to_string();
            let page = info.labels.iter().position(|l| *l == text).or_else(|| {
                text.parse::<usize>()
                    .ok()
                    .filter(|n| *n >= 1 && *n <= info.labels.len())
                    .map(|n| n - 1)
            });
            match page {
                Some(p) => {
                    win.jump(p, None, None);
                    win.view.grab_focus();
                }
                None => {
                    e.error_bell();
                    let cur = win.view.current_page();
                    e.set_text(&win.model.label(cur));
                }
            }
        });
        let w = self.weak();
        self.zoom_entry.connect_activate(move |e| {
            let Some(win) = w.upgrade() else { return };
            match parse_zoom_percent(&e.text()) {
                Some(z) => {
                    win.view.set_zoom(z, None);
                    win.zoom_btn.popdown();
                }
                _ => e.error_bell(),
            }
        });

        // Drag and drop.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let w = self.weak();
        drop.connect_drop(move |_, value, _, _| {
            let Some(win) = w.upgrade() else { return false };
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            match list.files().first() {
                Some(f) => {
                    win.open_uri(&f.uri(), None, false);
                    true
                }
                None => false,
            }
        });
        self.window.add_controller(drop);

        // Escape leaves fullscreen.
        let keys = gtk::EventControllerKey::new();
        let w = self.weak();
        keys.connect_key_pressed(move |_, key, _, _| {
            if let Some(win) = w.upgrade() {
                if key == gdk::Key::Escape && win.window.is_fullscreen() {
                    win.window.unfullscreen();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);

        let w = self.weak();
        self.recent_list.connect_row_activated(move |_, row| {
            if let Some(win) = w.upgrade() {
                let uri = row.widget_name().to_string();
                win.open_uri(&uri, None, false);
            }
        });

        let w = self.weak();
        self.window.connect_close_request(move |_| {
            if let Some(win) = w.upgrade() {
                win.save_state();
                if let Some(p) = win.presentation.borrow_mut().take() {
                    p.close();
                }
            }
            glib::Propagation::Proceed
        });
    }

    fn refresh_recent(&self) {
        while let Some(row) = self.recent_list.first_child() {
            self.recent_list.remove(&row);
        }
        let mut items: Vec<gtk::RecentInfo> = gtk::RecentManager::default()
            .items()
            .into_iter()
            .filter(|i| i.mime_type() == "application/pdf" && i.exists())
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.modified().to_unix()));
        for info in items.iter().take(20) {
            let uri = info.uri().to_string();
            let folder = gio::File::for_uri(&uri)
                .parent()
                .map(|p| p.parse_name().to_string())
                .unwrap_or_default();
            let when = info
                .modified()
                .to_local()
                .ok()
                .and_then(|d| d.format("%x").ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&info.display_name()))
                .subtitle(glib::markup_escape_text(&folder))
                .activatable(true)
                .title_lines(1)
                .subtitle_lines(1)
                .build();
            row.set_widget_name(&uri);
            row.add_prefix(&gtk::Image::from_icon_name("x-office-document-symbolic"));
            let l = gtk::Label::new(Some(&when));
            l.add_css_class("dim-label");
            row.add_suffix(&l);
            self.recent_list.append(&row);
        }
        self.recent_group.set_visible(!items.is_empty());
    }

    fn choose_file(self: &Rc<Self>) {
        let pdf = gtk::FileFilter::new();
        pdf.set_name(Some("PDF Documents"));
        pdf.add_mime_type("application/pdf");
        pdf.add_suffix("pdf");
        let all = gtk::FileFilter::new();
        all.set_name(Some("All Files"));
        all.add_pattern("*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&pdf);
        filters.append(&all);
        let dialog = gtk::FileDialog::builder()
            .title("Open Document")
            .modal(true)
            .filters(&filters)
            .default_filter(&pdf)
            .build();
        if let Some(dir) = self
            .uri
            .borrow()
            .as_ref()
            .and_then(|u| gio::File::for_uri(u).parent())
        {
            dialog.set_initial_folder(Some(&dir));
        }
        let w = self.weak();
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if let (Some(win), Ok(file)) = (w.upgrade(), res) {
                win.open_uri(&file.uri(), None, false);
            }
        });
    }

    /// Starts opening `uri` on the render thread.
    pub fn open_uri(self: &Rc<Self>, uri: &str, start: Option<Start>, presentation: bool) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        *self.pending.borrow_mut() = Some(Pending {
            id,
            uri: uri.to_string(),
            start,
            password: None,
            presentation,
        });
        self.model.renderer.open(id, uri, None);
    }

    fn display_name(uri: &str) -> String {
        let f = gio::File::for_uri(uri);
        f.basename()
            .map(|b| b.display().to_string())
            .unwrap_or_else(|| uri.to_string())
    }

    fn on_reply(self: &Rc<Self>, r: Reply) {
        let pending_id = self.pending.borrow().as_ref().map(|p| p.id);
        let doc_id = self.model.doc_id.get();
        match r {
            Reply::Opened { id, info } if Some(id) == pending_id => {
                let p = self.pending.borrow_mut().take().expect("pending");
                self.on_opened(*info, p);
            }
            Reply::NeedPassword { id, uri, wrong } if Some(id) == pending_id => {
                let w = self.weak();
                dialogs::ask_password(&self.window, &Self::display_name(&uri), wrong, move |pw| {
                    if let Some(win) = w.upgrade() {
                        if let Some(p) = win.pending.borrow_mut().as_mut() {
                            p.password = Some(pw.clone());
                            win.model.renderer.open(p.id, &p.uri, Some(pw));
                        }
                    }
                });
            }
            Reply::OpenFailed { id, uri, message } if Some(id) == pending_id => {
                self.pending.borrow_mut().take();
                self.toast(&format!(
                    "Unable to open “{}”: {message}",
                    Self::display_name(&uri)
                ));
            }
            Reply::Image {
                id,
                key,
                width,
                height,
                stride,
                data,
            } if id == doc_id => {
                self.model.insert_image(key, width, height, stride, data);
                if let JobKey::Thumb { page, .. } = key {
                    let pic = self
                        .thumbs_bound
                        .borrow()
                        .get(&(page as usize))
                        .and_then(|w| w.upgrade());
                    if let Some(pic) = pic {
                        let tex = self.model.thumbs.borrow().peek(&page).cloned();
                        pic.set_paintable(tex.as_ref());
                    }
                }
                self.view.queue_draw();
                if let Some(p) = self.presentation.borrow().as_ref() {
                    if let Some(c) = p.child() {
                        c.queue_draw();
                    }
                }
            }
            Reply::Links { id, page, links } if id == doc_id => {
                self.model.links.borrow_mut().insert(page, links);
                self.view.links_changed();
            }
            Reply::PageText { id, page, text } if id == doc_id => {
                self.model.texts.borrow_mut().insert(page, Rc::new(text));
                self.view.text_arrived(page);
            }
            Reply::Selection { id, gen, regions }
                if id == doc_id && gen == self.view.selection_gen() =>
            {
                *self.model.selection.borrow_mut() = regions;
                self.view.queue_draw();
            }
            Reply::Text { id, purpose, text } if id == doc_id && !text.is_empty() => {
                let display = WidgetExt::display(&self.window);
                match purpose {
                    TextPurpose::Clipboard => display.clipboard().set_text(&text),
                    TextPurpose::Primary => display.primary_clipboard().set_text(&text),
                }
            }
            Reply::SearchHits {
                id,
                gen,
                page,
                hits,
            } if id == doc_id && gen == self.search_gen.get() => {
                let n = hits.len();
                let at = self.model.search.borrow_mut().add_page(page, hits);
                let blanks = vec![""; n];
                self.results_store.splice(at as u32, 0, &blanks);
                let total = self.model.search.borrow().len();
                self.search_status
                    .set_label(&format!("Searching… {total} found"));
                let first = self.model.search.borrow().current().is_none();
                if first {
                    // Jump to the first match at or after the current page.
                    let cur = self.view.current_page();
                    if page >= cur {
                        self.find(true);
                    }
                }
                self.view.queue_draw();
            }
            Reply::SearchProgress { id, gen, page }
                if id == doc_id && gen == self.search_gen.get() =>
            {
                let total = self.model.search.borrow().len();
                self.search_status.set_label(&format!(
                    "Searching page {page} of {}… {total} found",
                    self.model.n_pages()
                ));
            }
            Reply::SearchDone { id, gen } if id == doc_id && gen == self.search_gen.get() => {
                let total = self.model.search.borrow().len();
                let text = match total {
                    0 => "No results found".to_string(),
                    1 => "1 result".to_string(),
                    n => format!("{n} results"),
                };
                helix_app_common::announce(
                    &self.search_status,
                    &text,
                    helix_app_common::Urgency::Polite,
                );
                self.search_status.set_label(&text);
                if self.model.search.borrow().current().is_none() {
                    self.find(true);
                }
            }
            _ => {}
        }
    }

    fn on_opened(self: &Rc<Self>, info: DocInfo, p: Pending) {
        self.save_state();
        if let Some(pw) = self.presentation.borrow_mut().take() {
            pw.close();
        }
        let uri = info.uri.clone();
        self.model.doc_id.set(p.id);
        self.model.set_document(info.clone());
        *self.uri.borrow_mut() = Some(uri.clone());
        *self.password.borrow_mut() = p.password.clone();
        *self.history.borrow_mut() = Default::default();
        self.rebuild_sidebar(&info);
        self.view.clear_selection();

        let saved = store().borrow().get(&uri).cloned().unwrap_or_default();
        let mode = ZoomMode::parse(&saved.zoom_mode).unwrap_or_default();
        self.view
            .document_changed(mode, saved.zoom, saved.rotation, saved.dual);
        self.set_toggle_state("dual", saved.dual);
        let n = info.sizes.len();
        let page = match &p.start {
            Some(Start::Index(i)) => (*i).min(n.saturating_sub(1)),
            Some(Start::Label(l)) => info.labels.iter().position(|x| x == l).unwrap_or(0),
            None => (saved.page as usize).min(n.saturating_sub(1)),
        };
        self.view.go_to(page, None, None);

        let name = Self::display_name(&uri);
        match &info.title {
            Some(t) => {
                self.title.set_title(t);
                self.title.set_subtitle(&name);
                self.window.set_title(Some(t));
            }
            None => {
                self.title.set_title(&name);
                self.title.set_subtitle("");
                self.window.set_title(Some(&name));
            }
        }
        // The document view is named after the document.
        self.view
            .update_property(&[gtk::accessible::Property::Label(
                info.title.as_deref().unwrap_or(&name),
            )]);
        let (show, side) = if info.wants_outline {
            (true, "outline")
        } else {
            (saved.sidebar, "thumbnails")
        };
        self.side_stack.set_visible_child_name(side);
        if self.split.is_mapped() {
            self.split.set_show_sidebar(show);
        } else {
            // Showing the sidebar of a split view that has never been
            // allocated makes libadwaita allocate it with a bogus width;
            // wait until it is mapped.
            self.split.set_show_sidebar(false);
            if show {
                let id: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
                let id2 = id.clone();
                *id.borrow_mut() = Some(self.split.connect_map(move |s| {
                    if let Some(h) = id2.borrow_mut().take() {
                        s.disconnect(h);
                    }
                    let s = s.clone();
                    glib::idle_add_local_once(move || s.set_show_sidebar(true));
                }));
            }
        }
        self.set_doc_ui(true);
        self.on_page_changed(page);
        if !self.search_entry.text().is_empty() {
            self.start_search();
        }
        gtk::RecentManager::default().add_item(&uri);
        self.view.grab_focus();
        if p.presentation {
            self.start_presentation();
        }
    }

    fn current_state(&self) -> Option<(String, DocState)> {
        let uri = self.uri.borrow().clone()?;
        Some((
            uri,
            DocState {
                page: self.view.current_page() as u32,
                zoom_mode: self.view.zoom_mode().as_str().to_string(),
                zoom: self.view.zoom(),
                rotation: self.view.rotation(),
                dual: self.view.dual(),
                sidebar: self.split.shows_sidebar(),
                last_used: state::now(),
            },
        ))
    }

    fn save_state(&self) {
        if let Some(id) = self.save_source.borrow_mut().take() {
            id.remove();
        }
        if let Some((uri, st)) = self.current_state() {
            let s = store();
            let mut s = s.borrow_mut();
            s.set(&uri, st);
            if let Err(e) = s.save() {
                eprintln!("{BIN}: cannot save state: {e}");
            }
        }
    }

    fn schedule_save(self: &Rc<Self>) {
        if self.uri.borrow().is_none() || self.save_source.borrow().is_some() {
            return;
        }
        let w = self.weak();
        let id = glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(win) = w.upgrade() {
                win.save_source.borrow_mut().take();
                win.save_state();
            }
        });
        *self.save_source.borrow_mut() = Some(id);
    }

    fn start_presentation(self: &Rc<Self>) {
        if self.model.info().is_none() || self.presentation.borrow().is_some() {
            return;
        }
        let w = self.weak();
        let win = presentation::present(
            &self.window,
            self.model.clone(),
            self.view.current_page(),
            self.view.rotation(),
            move |page| {
                if let Some(win) = w.upgrade() {
                    win.presentation.borrow_mut().take();
                    win.view.go_to(page, None, None);
                }
            },
        );
        *self.presentation.borrow_mut() = Some(win);
    }

    fn print(self: &Rc<Self>) {
        let (Some(uri), Some(info)) = (self.uri.borrow().clone(), self.model.info()) else {
            return;
        };
        let title = info
            .title
            .clone()
            .unwrap_or_else(|| Self::display_name(&uri));
        let w = self.weak();
        dialogs::print(
            &self.window,
            &uri,
            self.password.borrow().clone(),
            &title,
            self.view.current_page(),
            self.print_settings.clone(),
            move |msg| {
                if let Some(win) = w.upgrade() {
                    win.toast(&format!("Printing failed: {msg}"));
                }
            },
        );
    }
}

/// Clamp helper shared with tests.
pub fn parse_zoom_percent(s: &str) -> Option<f64> {
    let t = s.trim().trim_end_matches('%').trim().replace(',', ".");
    t.parse::<f64>()
        .ok()
        .filter(|v| *v > 0.0)
        .map(|v| layout::clamp_zoom(v / 100.0))
}
