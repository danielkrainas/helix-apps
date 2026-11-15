//! GTK user interface.

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use helix_app_common::Labelled;
use helix_app_common::Prefs;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::chart::{self, ChartKind, Hit, Layout};
use crate::mounts::{self, Location};
use crate::scanner::{self, Kind, NodeId, Progress, ScanError, ScanOptions, ScanTree, ROOT};
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;
use helix_app_common::trnf;

pub const APP_ID: &str = "org.helix.DiskUsage";
/// `excluded-uris` lives here.
const SCHEMA_PREFERENCES: &str = "org.gnome.baobab.preferences";
/// `active-chart`, `window-size`, `is-maximized` live here.
const SCHEMA_UI: &str = "org.gnome.baobab.ui";
/// Upstream default of `org.gnome.baobab.ui window-size`.
const DEFAULT_WINDOW_SIZE: (i32, i32) = (960, 600);
const APP_NAME: &str = n_("Disk Usage Analyzer");
const COMMENTS: &str = n_("Check folder sizes and available disk space");

/// Options that have no `org.gnome.baobab` key (see SPEC.md). Older
/// versions also stored the window state here; those fields are ignored.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(default)]
struct LocalState {
    cross_filesystems: bool,
}

fn state_path() -> PathBuf {
    helix_app_common::config_dir("helix-disk-usage").join("state.json")
}

impl LocalState {
    fn load() -> Self {
        std::fs::read(state_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(state_path(), json);
        }
    }
}

fn size_text(bytes: u64) -> String {
    glib::format_size(bytes).to_string()
}

fn node_id(obj: &glib::Object) -> Option<NodeId> {
    let obj = match obj.downcast_ref::<gtk::TreeListRow>() {
        Some(row) => row.item()?,
        None => obj.clone(),
    };
    obj.downcast_ref::<glib::BoxedAnyObject>()
        .map(|b| *b.borrow::<NodeId>())
}

type CellRef = (Col, glib::WeakRef<gtk::Widget>);
type ActionFn = Box<dyn Fn(&Rc<App>)>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Col {
    Name,
    Size,
    Contents,
    Modified,
}

struct App {
    window: adw::ApplicationWindow,
    /// `org.gnome.baobab.preferences`.
    prefs: Prefs,
    /// `org.gnome.baobab.ui`.
    ui_prefs: Prefs,
    local: RefCell<LocalState>,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    locations_list: gtk::ListBox,
    locations: RefCell<Vec<Location>>,
    results_page: adw::NavigationPage,
    results_title: adw::WindowTitle,
    results_stack: gtk::Stack,
    progress_bar: gtk::ProgressBar,
    progress_label: gtk::Label,
    progress_current: gtk::Label,
    scan_progress: RefCell<Option<Arc<Progress>>>,
    scan_path: RefCell<Option<PathBuf>>,
    generation: Cell<u64>,
    tree: RefCell<Option<ScanTree>>,
    stores: RefCell<HashMap<NodeId, gio::ListStore>>,
    /// Bound cells per node, refreshed when sizes change after trashing.
    cells: RefCell<HashMap<NodeId, Vec<CellRef>>>,
    column_view: gtk::ColumnView,
    selection: gtk::SingleSelection,
    status: gtk::Label,
    chart: gtk::DrawingArea,
    chart_layout: RefCell<Layout>,
    chart_root: Cell<NodeId>,
    chart_kind: Cell<ChartKind>,
    chart_menu: gtk::PopoverMenu,
    hover: Cell<Option<NodeId>>,
    /// The chart item with keyboard focus (an item or the chart root).
    chart_focus: Cell<Option<NodeId>>,
    breadcrumbs: gtk::Box,
    up_button: gtk::Button,
    target: Cell<Option<NodeId>>,
}

pub fn run(initial: Option<String>) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    helix_app_common::install_standard_actions(&app, tr(APP_NAME), tr(COMMENTS));
    let initial = RefCell::new(initial);
    app.connect_startup(|_| load_css());
    app.connect_activate(move |gapp| {
        let ui = App::build(gapp);
        ui.window.present();
        ui.reload_locations();
        if let Some(arg) = initial.borrow_mut().take() {
            match gio::File::for_commandline_arg(&arg).path() {
                Some(p) => ui.start_scan(p),
                None => ui.toast(&trf!("“{arg}” is not a local folder", arg = arg)),
            }
        }
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}

fn load_css() {
    helix_app_common::style::add_css(
        ".du-share { min-height: 6px; }
         .du-share block { min-height: 6px; }
         .du-capacity block { min-height: 8px; }
         .du-breadcrumbs button { padding: 2px 6px; min-height: 0; }
         .du-chart:focus-visible { outline: 2px solid alpha(@accent_color, 0.5);
                                   outline-offset: -2px; border-radius: 6px; }",
        ".du-chart:focus-visible { outline-color: @accent_color; }",
    );
}

fn clean_level_bar(bar: &gtk::LevelBar) {
    for name in [
        gtk::LEVEL_BAR_OFFSET_LOW,
        gtk::LEVEL_BAR_OFFSET_HIGH,
        gtk::LEVEL_BAR_OFFSET_FULL,
    ] {
        bar.remove_offset_value(Some(name));
    }
}

fn item_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some(tr("_Open Folder")), Some("win.open-item"));
    s1.append(Some(tr("_Copy Path")), Some("win.copy-path"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    let trash = gio::MenuItem::new(Some(tr("Move to _Trash…")), Some("win.trash-item"));
    // Delete is a bubble-phase shortcut, not an application accelerator,
    // so the menu is told about it explicitly.
    trash.set_attribute_value("accel", Some(&tr("Delete").to_variant()));
    s2.append_item(&trash);
    menu.append_section(None, &s2);
    menu
}

fn primary_menu() -> gtk::MenuButton {
    let menu = gio::Menu::new();
    let s = gio::Menu::new();
    s.append(Some(tr("_Preferences")), Some("win.preferences"));
    s.append(
        Some(helix_app_common::i18n::tr(
            helix_app_common::shortcuts::MENU_LABEL,
        )),
        Some(helix_app_common::shortcuts::ACTION),
    );
    s.append(Some(tr("_About Disk Usage Analyzer")), Some("app.about"));
    menu.append_section(None, &s);
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text(tr("Main Menu"))
        .primary(true)
        .build()
        .labelled(tr("Main Menu"))
}

impl App {
    fn build(gapp: &adw::Application) -> Rc<App> {
        let prefs = Prefs::new(SCHEMA_PREFERENCES);
        let ui_prefs = Prefs::new(SCHEMA_UI);
        let local = LocalState::load();
        let window = adw::ApplicationWindow::builder()
            .application(gapp)
            .title(tr(APP_NAME))
            .default_width(DEFAULT_WINDOW_SIZE.0)
            .default_height(DEFAULT_WINDOW_SIZE.1)
            .width_request(helix_app_common::MIN_WIDTH)
            .height_request(helix_app_common::MIN_HEIGHT)
            .build();
        {
            use helix_app_common::window_state::{persist, SizeKeys, Store};
            persist(
                &window,
                Store::settings(
                    &ui_prefs,
                    SizeKeys::Pair("window-size"),
                    "is-maximized",
                    "helix-disk-usage",
                ),
            );
        }
        let nav = adw::NavigationView::new();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&nav));
        window.set_content(Some(&toasts));

        // ---- Start page -------------------------------------------------
        let start_header = adw::HeaderBar::new();
        let scan_btn = gtk::Button::builder()
            .label(tr("Scan Folder…"))
            .action_name("win.scan-folder")
            .tooltip_text(tr("Scan a folder (Ctrl+O)"))
            .build();
        start_header.pack_start(&scan_btn);
        // In narrow windows the button shows only its icon.
        let scan_icon_btn = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .action_name("win.scan-folder")
            .tooltip_text(tr("Scan a folder (Ctrl+O)"))
            .visible(false)
            .build()
            .labelled(tr("Scan Folder"));
        start_header.pack_start(&scan_icon_btn);
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            450.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&scan_btn, "visible", Some(&false.to_value()));
        narrow.add_setter(&scan_icon_btn, "visible", Some(&true.to_value()));
        window.add_breakpoint(narrow);
        start_header.pack_end(&primary_menu());
        let reload_btn = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .action_name("win.rescan")
            .tooltip_text(tr("Reload Locations"))
            .build()
            .labelled(tr("Reload Locations"));
        start_header.pack_end(&reload_btn);
        let locations_list = gtk::ListBox::new();
        locations_list.add_css_class("boxed-list");
        locations_list.set_selection_mode(gtk::SelectionMode::None);
        let title = gtk::Label::builder()
            .label(tr("Devices &amp; Locations"))
            .use_markup(true)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        vbox.set_margin_top(24);
        vbox.set_margin_bottom(24);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.append(&title);
        vbox.append(&locations_list);
        let clamp = adw::Clamp::builder().maximum_size(720).child(&vbox).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .vexpand(true)
            .build();
        let start_tv = adw::ToolbarView::new();
        start_tv.add_top_bar(&start_header);
        start_tv.set_content(Some(&scroller));
        let start_page = adw::NavigationPage::builder()
            .title(tr(APP_NAME))
            .tag("start")
            .child(&start_tv)
            .build();
        nav.add(&start_page);

        // ---- Results page -----------------------------------------------
        let results_header = adw::HeaderBar::new();
        let results_title = adw::WindowTitle::new("", "");
        results_header.set_title_widget(Some(&results_title));
        results_header.pack_end(&primary_menu());
        let rescan_btn = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .action_name("win.rescan")
            .tooltip_text(tr("Rescan (Ctrl+R)"))
            .build()
            .labelled(tr("Rescan"));
        results_header.pack_end(&rescan_btn);

        // Progress view.
        let progress_bar = gtk::ProgressBar::new();
        let progress_label = gtk::Label::new(None);
        progress_label.add_css_class("title-4");
        let progress_current = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["dim-label"])
            .build();
        let cancel_btn = gtk::Button::builder()
            .label(tr("_Cancel"))
            .use_underline(true)
            .action_name("win.cancel-scan")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        let pbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        pbox.set_valign(gtk::Align::Center);
        pbox.set_margin_start(24);
        pbox.set_margin_end(24);
        let scanning = gtk::Label::builder()
            .label(tr("Scanning…"))
            .css_classes(["title-1"])
            .build();
        for w in [
            scanning.upcast_ref::<gtk::Widget>(),
            progress_bar.upcast_ref(),
            progress_label.upcast_ref(),
            progress_current.upcast_ref(),
            cancel_btn.upcast_ref(),
        ] {
            pbox.append(w);
        }
        let pclamp = adw::Clamp::builder().maximum_size(480).child(&pbox).build();

        // Tree.
        let selection = gtk::SingleSelection::new(None::<gio::ListModel>);
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let column_view = gtk::ColumnView::new(Some(selection.clone()));
        column_view.add_css_class("data-table");
        let status = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .margin_start(8)
            .margin_end(8)
            .margin_top(4)
            .margin_bottom(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let tree_scroller = gtk::ScrolledWindow::builder()
            .child(&column_view)
            .vexpand(true)
            .hexpand(true)
            .build();
        let tree_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tree_box.append(&tree_scroller);
        tree_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        tree_box.append(&status);

        // Chart.
        let breadcrumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        breadcrumbs.add_css_class("du-breadcrumbs");
        let crumb_scroller = gtk::ScrolledWindow::builder()
            .child(&breadcrumbs)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .build();
        let up_button = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .action_name("win.chart-up")
            .tooltip_text(tr("Up (Alt+Up)"))
            .css_classes(["flat"])
            .build()
            .labelled(tr("Up"));
        let rings_btn = gtk::ToggleButton::builder()
            .label(tr("Rings"))
            .tooltip_text(tr("Rings Chart"))
            .build();
        let treemap_btn = gtk::ToggleButton::builder()
            .label(tr("Treemap"))
            .tooltip_text(tr("Treemap Chart"))
            .group(&rings_btn)
            .build();
        let switch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        switch.add_css_class("linked");
        switch.append(&rings_btn);
        switch.append(&treemap_btn);
        let chart_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chart_bar.set_margin_start(6);
        chart_bar.set_margin_end(6);
        chart_bar.set_margin_top(6);
        chart_bar.append(&up_button);
        chart_bar.append(&crumb_scroller);
        chart_bar.append(&switch);
        // A picture of the folder's contents: reachable with Tab, its
        // items navigable with the arrow keys (see `setup_chart`), and
        // summarised in text for screen readers.
        let chart_area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .has_tooltip(true)
            .focusable(true)
            .accessible_role(gtk::AccessibleRole::Img)
            .css_classes(["du-chart"])
            .build();
        chart_area.update_property(&[gtk::accessible::Property::Label(tr("Usage chart"))]);
        chart_area.set_size_request(240, 240);
        let chart_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        chart_box.append(&chart_bar);
        chart_box.append(&chart_area);
        let chart_menu = gtk::PopoverMenu::from_model(Some(&item_menu()));
        chart_menu.set_has_arrow(false);
        chart_menu.set_parent(&chart_area);

        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&tree_box)
            .end_child(&chart_box)
            .position(520)
            .resize_start_child(true)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .build();

        let results_stack = gtk::Stack::new();
        results_stack.add_named(&pclamp, Some("progress"));
        results_stack.add_named(&paned, Some("results"));
        let results_tv = adw::ToolbarView::new();
        results_tv.add_top_bar(&results_header);
        results_tv.set_content(Some(&results_stack));
        let results_page = adw::NavigationPage::builder()
            .title(tr("Results"))
            .tag("results")
            .child(&results_tv)
            .build();

        let chart_kind = ChartKind::from_nick(&ui_prefs.get_enum_nick("active-chart", "rings"));
        let app = Rc::new(App {
            window,
            prefs,
            ui_prefs,
            local: RefCell::new(local),
            nav,
            toasts,
            locations_list,
            locations: RefCell::default(),
            results_page,
            results_title,
            results_stack,
            progress_bar,
            progress_label,
            progress_current,
            scan_progress: RefCell::default(),
            scan_path: RefCell::default(),
            generation: Cell::new(0),
            tree: RefCell::default(),
            stores: RefCell::default(),
            cells: RefCell::default(),
            column_view,
            selection,
            status,
            chart: chart_area,
            chart_layout: RefCell::new(Layout::Empty),
            chart_root: Cell::new(ROOT),
            chart_kind: Cell::new(chart_kind),
            chart_menu,
            hover: Cell::new(None),
            chart_focus: Cell::new(None),
            breadcrumbs,
            up_button,
            target: Cell::new(None),
        });

        match chart_kind {
            ChartKind::Rings => rings_btn.set_active(true),
            ChartKind::Treemap => treemap_btn.set_active(true),
        }
        for (btn, kind) in [
            (rings_btn, ChartKind::Rings),
            (treemap_btn, ChartKind::Treemap),
        ] {
            let a = app.clone();
            btn.connect_toggled(move |b| {
                if b.is_active() {
                    a.chart_kind.set(kind);
                    a.ui_prefs.set_enum_nick("active-chart", kind.nick());
                    a.chart.queue_draw();
                }
            });
        }

        app.setup_columns();
        app.setup_chart();
        app.setup_actions();

        let a = app.clone();
        app.locations_list.connect_row_activated(move |_, row| {
            let path = a
                .locations
                .borrow()
                .get(row.index() as usize)
                .map(|l| l.path.clone());
            if let Some(p) = path {
                a.start_scan(p);
            }
        });
        let a = app.clone();
        app.selection.connect_selection_changed(move |sel, _, _| {
            let id = sel.selected_item().and_then(|o| node_id(&o));
            a.target.set(id);
            a.update_action_state();
            if let Some(id) = id {
                let root = {
                    let tree = a.tree.borrow();
                    let Some(t) = tree.as_ref() else { return };
                    let n = t.node(id);
                    if n.kind == Kind::Dir {
                        id
                    } else {
                        n.parent.unwrap_or(ROOT)
                    }
                };
                a.set_chart_root(root);
            }
        });
        let a = app.clone();
        app.nav.connect_popped(move |_, page| {
            if page == &a.results_page {
                a.cancel_scan();
                a.clear_results();
            }
        });
        let a = app.clone();
        app.window.connect_close_request(move |_| {
            a.cancel_scan();
            glib::Propagation::Proceed
        });
        app.update_action_state();
        app
    }

    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    // ---- Locations ----------------------------------------------------

    fn reload_locations(self: &Rc<Self>) {
        let home = glib::home_dir();
        let a = self.clone();
        glib::spawn_future_local(async move {
            let Ok(locs) = gio::spawn_blocking(move || mounts::locations(&home)).await else {
                return;
            };
            while let Some(row) = a.locations_list.row_at_index(0) {
                a.locations_list.remove(&row);
            }
            for loc in &locs {
                a.locations_list.append(&location_row(loc));
            }
            *a.locations.borrow_mut() = locs;
        });
    }

    // ---- Scanning -----------------------------------------------------

    fn excluded_uris(&self) -> Vec<String> {
        self.prefs.get::<Vec<String>>("excluded-uris", Vec::new())
    }

    fn scan_options(&self) -> ScanOptions {
        let excluded = self
            .excluded_uris()
            .iter()
            .filter_map(|u| gio::File::for_uri(u).path())
            .collect();
        ScanOptions {
            cross_filesystems: self.local.borrow().cross_filesystems,
            excluded,
        }
    }

    fn start_scan(self: &Rc<Self>, path: PathBuf) {
        self.cancel_scan();
        self.clear_results();
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let progress = Arc::new(Progress::default());
        *self.scan_progress.borrow_mut() = Some(progress.clone());
        *self.scan_path.borrow_mut() = Some(path.clone());

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.results_page.set_title(&name);
        self.results_title.set_title(&name);
        self.results_title.set_subtitle(&path.display().to_string());
        self.results_stack.set_visible_child_name("progress");
        self.progress_label.set_text("");
        self.progress_current.set_text("");
        if self.nav.visible_page().as_ref() != Some(&self.results_page) {
            self.nav.push(&self.results_page);
        }
        self.update_action_state();

        let opts = self.scan_options();
        let result: Arc<Mutex<Option<Result<ScanTree, ScanError>>>> = Arc::default();
        let (r2, p2, path2) = (result.clone(), progress.clone(), path.clone());
        let spawned = std::thread::Builder::new()
            .name("scanner".into())
            .stack_size(64 << 20)
            .spawn(move || {
                let res = scanner::scan(&path2, &opts, &p2);
                if let Ok(mut slot) = r2.lock() {
                    *slot = Some(res);
                }
            });
        if let Err(e) = spawned {
            self.toast(&trf!("Could not start scan: {e}", e = e));
            return;
        }
        let a = self.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            if a.generation.get() != gen {
                return glib::ControlFlow::Break;
            }
            let done = result.lock().ok().and_then(|mut s| s.take());
            if let Some(res) = done {
                a.finish_scan(res);
                return glib::ControlFlow::Break;
            }
            use std::sync::atomic::Ordering::Relaxed;
            a.progress_bar.pulse();
            a.progress_label.set_text(&trf!(
                "{} items, {}",
                progress.items.load(Relaxed),
                size_text(progress.bytes.load(Relaxed))
            ));
            if let Ok(cur) = progress.current.lock() {
                a.progress_current.set_text(&cur.display().to_string());
            }
            glib::ControlFlow::Continue
        });
    }

    fn cancel_scan(&self) {
        if let Some(p) = self.scan_progress.borrow_mut().take() {
            p.cancel();
        }
        self.generation.set(self.generation.get() + 1);
    }

    fn finish_scan(self: &Rc<Self>, res: Result<ScanTree, ScanError>) {
        self.scan_progress.borrow_mut().take();
        match res {
            Ok(tree) => {
                *self.tree.borrow_mut() = Some(tree);
                self.show_results();
            }
            Err(ScanError::Cancelled) => {}
            Err(e) => {
                let path = self.scan_path.borrow().clone().unwrap_or_default();
                self.toast(&trf!(
                    "Could not scan “{path}”: {e}",
                    path = path.display(),
                    e = e
                ));
                self.nav.pop_to_tag("start");
            }
        }
        self.update_action_state();
    }

    fn clear_results(&self) {
        self.selection.set_model(None::<&gio::ListModel>);
        self.stores.borrow_mut().clear();
        self.cells.borrow_mut().clear();
        self.tree.borrow_mut().take();
        *self.chart_layout.borrow_mut() = Layout::Empty;
        self.chart_root.set(ROOT);
        self.hover.set(None);
        self.target.set(None);
        self.update_action_state();
    }

    fn children_store(self: &Rc<Self>, id: NodeId) -> Option<gio::ListModel> {
        if let Some(s) = self.stores.borrow().get(&id) {
            return Some(s.clone().upcast());
        }
        let tree = self.tree.borrow();
        let t = tree.as_ref()?;
        let n = t.node(id);
        if n.kind != Kind::Dir || n.children.is_empty() {
            return None;
        }
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        for &c in &n.children {
            store.append(&glib::BoxedAnyObject::new(c));
        }
        self.stores.borrow_mut().insert(id, store.clone());
        Some(store.upcast())
    }

    fn show_results(self: &Rc<Self>) {
        let root_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        root_store.append(&glib::BoxedAnyObject::new(ROOT));
        let a = self.clone();
        let tlm = gtk::TreeListModel::new(root_store, false, false, move |obj| {
            node_id(obj).and_then(|id| a.children_store(id))
        });
        let sorter = gtk::TreeListRowSorter::new(self.column_view.sorter());
        let sorted = gtk::SortListModel::new(Some(tlm.clone()), Some(sorter));
        self.selection.set_model(Some(&sorted));
        if let Some(row) = tlm.row(0) {
            row.set_expanded(true);
        }
        self.results_stack.set_visible_child_name("results");
        self.set_chart_root(ROOT);
        self.update_status();
    }

    fn update_status(&self) {
        let tree = self.tree.borrow();
        let Some(t) = tree.as_ref() else { return };
        let r = t.root();
        let mut s = trf!(
            "{} on disk · {} apparent · {} items",
            size_text(r.size),
            size_text(r.apparent),
            r.items
        );
        if t.errors > 0 {
            s.push_str(&trf!(" · {} folders could not be read", t.errors));
        }
        self.status.set_text(&s);
    }

    // ---- Tree columns ---------------------------------------------------

    fn register_cell(&self, col: Col, widget: &gtk::Widget, id: NodeId) {
        let mut cells = self.cells.borrow_mut();
        let v = cells.entry(id).or_default();
        v.retain(|(_, w)| w.upgrade().is_some());
        v.push((col, widget.downgrade()));
    }

    fn unregister_cell(&self, widget: &gtk::Widget, id: NodeId) {
        if let Some(v) = self.cells.borrow_mut().get_mut(&id) {
            v.retain(|(_, w)| w.upgrade().is_some_and(|w| &w != widget));
        }
    }

    /// Fills a cell widget for node `id`.
    fn fill_cell(&self, col: Col, widget: &gtk::Widget, id: NodeId) {
        let tree = self.tree.borrow();
        let Some(t) = tree.as_ref() else { return };
        let n = t.node(id);
        match col {
            Col::Name => {
                let bx = widget.downcast_ref::<gtk::Box>().expect("name cell");
                let icon = bx.first_child().and_downcast::<gtk::Image>().expect("icon");
                let label = icon
                    .next_sibling()
                    .and_downcast::<gtk::Label>()
                    .expect("label");
                icon.set_icon_name(Some(match n.kind {
                    Kind::Dir if n.error => "dialog-warning-symbolic",
                    Kind::Dir => "folder-symbolic",
                    Kind::Symlink => "insert-link-symbolic",
                    _ => "text-x-generic-symbolic",
                }));
                label.set_text(&n.name);
                bx.set_tooltip_text(n.error.then_some(tr("This folder could not be fully read")));
            }
            Col::Size => {
                let bx = widget.downcast_ref::<gtk::Box>().expect("size cell");
                let label = bx
                    .first_child()
                    .and_downcast::<gtk::Label>()
                    .expect("label");
                let bar = label
                    .next_sibling()
                    .and_downcast::<gtk::LevelBar>()
                    .expect("bar");
                label.set_text(&size_text(n.size));
                let share = n
                    .parent
                    .map(|p| t.node(p).size)
                    .filter(|&s| s > 0)
                    .map(|s| n.size as f64 / s as f64)
                    .unwrap_or(1.0);
                bar.set_value(share.clamp(0.0, 1.0));
                bx.set_tooltip_text(Some(&trf!(
                    "{} on disk, {} apparent",
                    size_text(n.size),
                    size_text(n.apparent)
                )));
            }
            Col::Contents => {
                let label = widget.downcast_ref::<gtk::Label>().expect("label");
                label.set_text(&if n.kind == Kind::Dir {
                    trnf!("{n} item", "{n} items", n.items)
                } else {
                    String::new()
                });
            }
            Col::Modified => {
                let label = widget.downcast_ref::<gtk::Label>().expect("label");
                let text = glib::DateTime::from_unix_local(n.mtime)
                    .ok()
                    .and_then(|d| d.format("%x").ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                label.set_text(&text);
            }
        }
    }

    fn make_factory(self: &Rc<Self>, col: Col) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let a = self.clone();
        factory.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().expect("ListItem");
            let widget: gtk::Widget = match col {
                Col::Name => {
                    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    bx.append(&gtk::Image::new());
                    bx.append(
                        &gtk::Label::builder()
                            .xalign(0.0)
                            .ellipsize(gtk::pango::EllipsizeMode::End)
                            .build(),
                    );
                    let exp = gtk::TreeExpander::new();
                    exp.set_child(Some(&bx));
                    exp.upcast()
                }
                Col::Size => {
                    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    bx.append(&gtk::Label::builder().xalign(1.0).width_chars(9).build());
                    let bar = gtk::LevelBar::builder()
                        .min_value(0.0)
                        .max_value(1.0)
                        .width_request(48)
                        .valign(gtk::Align::Center)
                        .css_classes(["du-share"])
                        .build();
                    clean_level_bar(&bar);
                    bx.append(&bar);
                    bx.upcast()
                }
                Col::Contents | Col::Modified => gtk::Label::builder()
                    .xalign(if col == Col::Contents { 1.0 } else { 0.0 })
                    .build()
                    .upcast(),
            };
            // Right-click or long press: select the row and show the item
            // menu (Menu and Shift+F10 are handled on the column view).
            let gesture = gtk::GestureClick::builder()
                .button(gdk::BUTTON_SECONDARY)
                .build();
            let (a2, weak_item) = (a.clone(), item.downgrade());
            gesture.connect_pressed(move |g, _, x, y| {
                if let (Some(item), Some(w)) = (weak_item.upgrade(), g.widget()) {
                    a2.row_menu(&w, item.position(), Some((x, y)));
                }
            });
            widget.add_controller(gesture);
            let long_press = gtk::GestureLongPress::new();
            let (a2, weak_item) = (a.clone(), item.downgrade());
            long_press.connect_pressed(move |g, x, y| {
                if let (Some(item), Some(w)) = (weak_item.upgrade(), g.widget()) {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    a2.row_menu(&w, item.position(), Some((x, y)));
                }
            });
            widget.add_controller(long_press);
            item.set_child(Some(&widget));
        });
        let a = self.clone();
        factory.connect_bind(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().expect("ListItem");
            let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            let Some(id) = node_id(row.upcast_ref()) else {
                return;
            };
            let Some(child) = item.child() else { return };
            let target = if col == Col::Name {
                let exp = child.downcast_ref::<gtk::TreeExpander>().expect("expander");
                exp.set_list_row(Some(&row));
                exp.child().expect("expander child")
            } else {
                child
            };
            a.fill_cell(col, &target, id);
            a.register_cell(col, &target, id);
        });
        let a = self.clone();
        factory.connect_unbind(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().expect("ListItem");
            let Some(id) = item
                .item()
                .and_downcast::<gtk::TreeListRow>()
                .and_then(|r| node_id(r.upcast_ref()))
            else {
                return;
            };
            let Some(child) = item.child() else { return };
            let target = match child.downcast_ref::<gtk::TreeExpander>() {
                Some(exp) => {
                    let c = exp.child();
                    exp.set_list_row(None);
                    c
                }
                None => Some(child),
            };
            if let Some(t) = target {
                a.unregister_cell(&t, id);
            }
        });
        factory
    }

    /// Selects row `pos`, makes its item the target of the item actions and
    /// shows the item menu on `widget`, at `at` or below the whole widget.
    fn row_menu(&self, widget: &gtk::Widget, pos: u32, at: Option<(f64, f64)>) {
        self.selection.set_selected(pos);
        self.target
            .set(self.selection.item(pos).and_then(|o| node_id(&o)));
        self.update_action_state();
        let menu = gtk::PopoverMenu::from_model(Some(&item_menu()));
        menu.set_has_arrow(false);
        menu.set_parent(widget);
        if let Some((x, y)) = at {
            menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        }
        menu.connect_closed(|m| {
            let m = m.clone();
            glib::idle_add_local_once(move || m.unparent());
        });
        menu.popup();
    }

    /// Menu or Shift+F10 in the tree: the item menu for the selected row,
    /// shown on the row that has keyboard focus.
    fn tree_menu_from_keyboard(&self) -> bool {
        let pos = self.selection.selected();
        if pos == gtk::INVALID_LIST_POSITION {
            return false;
        }
        let focused = gtk::prelude::GtkWindowExt::focus(&self.window)
            .filter(|f| f.is_ancestor(&self.column_view));
        let Some(mut widget) = focused else {
            return false;
        };
        // Focus is on GTK's row (or cell) widget, which expects only cells
        // as children: put the menu on our own cell content inside it.
        while widget.type_().name().starts_with("GtkColumnView") {
            match widget.first_child() {
                Some(c) => widget = c,
                None => return false,
            }
        }
        self.row_menu(&widget, pos, None);
        true
    }

    fn node_sorter(self: &Rc<Self>, col: Col) -> gtk::CustomSorter {
        let a = self.clone();
        gtk::CustomSorter::new(move |x, y| {
            let tree = a.tree.borrow();
            let (Some(t), Some(i), Some(j)) = (tree.as_ref(), node_id(x), node_id(y)) else {
                return gtk::Ordering::Equal;
            };
            let (n, m) = (t.node(i), t.node(j));
            let ord = match col {
                Col::Name => n.name.to_lowercase().cmp(&m.name.to_lowercase()),
                Col::Size => n.size.cmp(&m.size),
                Col::Contents => n.items.cmp(&m.items),
                Col::Modified => n.mtime.cmp(&m.mtime),
            };
            ord.into()
        })
    }

    fn setup_columns(self: &Rc<Self>) {
        let mut size_col = None;
        for (col, title) in [
            (Col::Name, tr("Name")),
            (Col::Size, tr("Size")),
            (Col::Contents, tr("Contents")),
            (Col::Modified, tr("Modified")),
        ] {
            let c = gtk::ColumnViewColumn::new(Some(title), Some(self.make_factory(col)));
            c.set_sorter(Some(&self.node_sorter(col)));
            c.set_resizable(true);
            if col == Col::Name {
                c.set_expand(true);
            }
            if col == Col::Size {
                size_col = Some(c.clone());
            }
            self.column_view.append_column(&c);
        }
        self.column_view
            .sort_by_column(size_col.as_ref(), gtk::SortType::Descending);
        let keys = gtk::EventControllerKey::new();
        let a = self.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let menu_key = key == gdk::Key::Menu
                || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK));
            if menu_key && a.tree_menu_from_keyboard() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.column_view.add_controller(keys);
        let a = self.clone();
        self.column_view.connect_activate(move |_, pos| {
            let Some(row) = a.selection.item(pos).and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            if row.is_expandable() {
                row.set_expanded(!row.is_expanded());
            }
        });
    }

    // ---- Chart --------------------------------------------------------

    fn setup_chart(self: &Rc<Self>) {
        let a = self.clone();
        self.chart.set_draw_func(move |area, cr, w, h| {
            let tree = a.tree.borrow();
            let Some(t) = tree.as_ref() else {
                *a.chart_layout.borrow_mut() = Layout::Empty;
                return;
            };
            let root = a.chart_root.get();
            // The widget's Pango context carries the interface font and
            // the text-scaling factor.
            let pango = area.pango_context();
            let layout = chart::compute(
                t,
                root,
                a.chart_kind.get(),
                w as f64,
                h as f64,
                chart::line_height(&pango),
            );
            let fg = area.color();
            let theme = chart::Theme {
                fg: (fg.red() as f64, fg.green() as f64, fg.blue() as f64),
                style: chart::Style {
                    dark: helix_app_common::style::is_dark(),
                    high_contrast: helix_app_common::style::is_high_contrast(),
                },
            };
            chart::draw(
                cr,
                &pango,
                t,
                root,
                &layout,
                a.hover.get(),
                theme,
                &size_text,
            );
            // Only when focus came from the keyboard, like a focus ring.
            let focus_visible = area.state_flags().contains(gtk::StateFlags::FOCUS_VISIBLE);
            if let Some(f) = a
                .chart_focus
                .get()
                .filter(|_| area.has_focus() && focus_visible)
            {
                chart::draw_focus(cr, &layout, root, f, theme);
            }
            *a.chart_layout.borrow_mut() = layout;
        });
        helix_app_common::style::redraw_on_style_change(&self.chart);

        let hit_node = |a: &App, x: f64, y: f64| -> Option<NodeId> {
            match chart::hit(&a.chart_layout.borrow(), x, y)? {
                Hit::Root => Some(a.chart_root.get()),
                Hit::Node(n) => Some(n),
            }
        };

        let motion = gtk::EventControllerMotion::new();
        let a = self.clone();
        motion.connect_motion(move |_, x, y| {
            let h = hit_node(&a, x, y);
            if h != a.hover.get() {
                a.hover.set(h);
                a.chart.queue_draw();
                let pointer = h.is_some_and(|id| {
                    a.tree
                        .borrow()
                        .as_ref()
                        .is_some_and(|t| t.node(id).kind == Kind::Dir)
                });
                a.chart.set_cursor_from_name(pointer.then_some("pointer"));
            }
        });
        let a = self.clone();
        motion.connect_leave(move |_| {
            if a.hover.take().is_some() {
                a.chart.queue_draw();
            }
        });
        self.chart.add_controller(motion);

        let a = self.clone();
        self.chart
            .connect_query_tooltip(move |_, x, y, _, tooltip| {
                let Some(id) = hit_node(&a, x as f64, y as f64) else {
                    return false;
                };
                let tree = a.tree.borrow();
                let Some(t) = tree.as_ref() else { return false };
                let n = t.node(id);
                let mut text = format!(
                    "<b>{}</b>\n{}",
                    glib::markup_escape_text(&n.name),
                    glib::markup_escape_text(&trf!(
                        "{size} on disk ({apparent} apparent)",
                        size = size_text(n.size),
                        apparent = size_text(n.apparent)
                    ))
                );
                if n.kind == Kind::Dir {
                    text.push('\n');
                    text.push_str(&trnf!("{n} item", "{n} items", n.items));
                }
                if id == a.chart_root.get() && n.parent.is_some() {
                    text.push('\n');
                    text.push_str(tr("Click to go up"));
                }
                tooltip.set_markup(Some(&text));
                true
            });

        let click = gtk::GestureClick::builder().button(0).build();
        let a = self.clone();
        click.connect_pressed(move |g, _, x, y| {
            let hit = chart::hit(&a.chart_layout.borrow(), x, y);
            match g.current_button() {
                gdk::BUTTON_PRIMARY => match hit {
                    Some(Hit::Root) => a.chart_up(),
                    Some(Hit::Node(id)) => {
                        let is_dir = a.tree.borrow().as_ref().is_some_and(|t| {
                            t.node(id).kind == Kind::Dir && !t.node(id).children.is_empty()
                        });
                        if is_dir {
                            a.set_chart_root(id);
                        }
                    }
                    None => {}
                },
                gdk::BUTTON_SECONDARY => {
                    let id = match hit {
                        Some(Hit::Root) => Some(a.chart_root.get()),
                        Some(Hit::Node(id)) => Some(id),
                        None => None,
                    };
                    if let Some(id) = id {
                        a.target.set(Some(id));
                        a.update_action_state();
                        a.chart_menu
                            .set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                        a.chart_menu.popup();
                    }
                }
                _ => {}
            }
        });
        self.chart.add_controller(click);

        // Long press (touch): the item menu, like a right click.
        let long_press = gtk::GestureLongPress::new();
        let a = self.clone();
        long_press.connect_pressed(move |g, x, y| {
            let id = match chart::hit(&a.chart_layout.borrow(), x, y) {
                Some(Hit::Root) => a.chart_root.get(),
                Some(Hit::Node(id)) => id,
                None => return,
            };
            g.set_state(gtk::EventSequenceState::Claimed);
            a.target.set(Some(id));
            a.update_action_state();
            a.chart_menu
                .set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            a.chart_menu.popup();
        });
        self.chart.add_controller(long_press);

        // Keyboard: arrows move between items (Left/Right along a ring or
        // level, Up towards the centre, Down outwards), Enter opens a
        // folder (or goes up from the centre), Backspace goes up, and
        // Menu or Shift+F10 opens the item menu.
        let keys = gtk::EventControllerKey::new();
        let a = self.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            use chart::Move;
            let plain = !mods.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            );
            let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
            let rtl = a.chart.direction() == gtk::TextDirection::Rtl;
            let mv = match key {
                gdk::Key::Left | gdk::Key::KP_Left if plain => {
                    Some(if rtl { Move::Next } else { Move::Prev })
                }
                gdk::Key::Right | gdk::Key::KP_Right if plain => {
                    Some(if rtl { Move::Prev } else { Move::Next })
                }
                gdk::Key::Up | gdk::Key::KP_Up if plain => Some(Move::Out),
                gdk::Key::Down | gdk::Key::KP_Down if plain => Some(Move::In),
                gdk::Key::Home | gdk::Key::KP_Home if plain => Some(Move::First),
                gdk::Key::End | gdk::Key::KP_End if plain => Some(Move::Last),
                _ => None,
            };
            if let Some(m) = mv {
                a.chart_move(m);
                return glib::Propagation::Stop;
            }
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space | gdk::Key::KP_Space
                    if plain && !shift =>
                {
                    a.chart_activate();
                    glib::Propagation::Stop
                }
                gdk::Key::BackSpace if plain => {
                    a.chart_up();
                    glib::Propagation::Stop
                }
                gdk::Key::Menu => {
                    a.chart_popup_focused();
                    glib::Propagation::Stop
                }
                gdk::Key::F10 if shift => {
                    a.chart_popup_focused();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.chart.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let a = self.clone();
        focus.connect_enter(move |_| {
            let current = a.chart_focus.get();
            let valid = current.is_some_and(|f| {
                f == a.chart_root.get()
                    || chart::visible(&a.fresh_layout())
                        .iter()
                        .any(|(n, _)| *n == f)
            });
            match current {
                Some(f) if valid => a.focus_chart_item(f),
                _ => {
                    a.chart_focus.set(None);
                    a.chart_move(chart::Move::In);
                }
            }
        });
        let a = self.clone();
        focus.connect_leave(move |_| a.chart.queue_draw());
        self.chart.add_controller(focus);
    }

    /// The chart's current layout, computed now (the stored one is only
    /// refreshed when the chart is drawn).
    fn fresh_layout(&self) -> Layout {
        let tree = self.tree.borrow();
        let Some(t) = tree.as_ref() else {
            return Layout::Empty;
        };
        chart::compute(
            t,
            self.chart_root.get(),
            self.chart_kind.get(),
            self.chart.width() as f64,
            self.chart.height() as f64,
            chart::line_height(&self.chart.pango_context()),
        )
    }

    /// Moves the chart's keyboard focus; from nowhere, starts at the
    /// largest item (or the centre when the folder shows no items).
    fn chart_move(self: &Rc<Self>, m: chart::Move) {
        let root = self.chart_root.get();
        let next = {
            let tree = self.tree.borrow();
            let Some(t) = tree.as_ref() else { return };
            let layout = self.fresh_layout();
            match self.chart_focus.get() {
                Some(cur) => chart::navigate(t, &layout, root, cur, m),
                None => chart::navigate(t, &layout, root, root, chart::Move::In).or(Some(root)),
            }
        };
        match next {
            Some(n) => self.focus_chart_item(n),
            None => self.chart.error_bell(),
        }
    }

    /// Gives `id` the chart's keyboard focus, makes it the target of the
    /// item actions, and tells screen readers what it is.
    fn focus_chart_item(self: &Rc<Self>, id: NodeId) {
        self.chart_focus.set(Some(id));
        self.target.set(Some(id));
        self.update_action_state();
        self.chart.queue_draw();
        let text = {
            let tree = self.tree.borrow();
            let Some(t) = tree.as_ref() else { return };
            let n = t.node(id);
            let root = self.chart_root.get();
            if id == root {
                let up = if n.parent.is_some() {
                    tr(", press Enter to go up")
                } else {
                    ""
                };
                trf!(
                    "{name}, {size} in total{up}",
                    name = n.name,
                    size = size_text(n.size),
                    up = up
                )
            } else {
                let share = n.parent.map(|p| t.node(p)).filter(|p| p.size > 0);
                let what = match n.kind {
                    Kind::Dir if !n.children.is_empty() => tr("folder, press Enter to open"),
                    Kind::Dir => tr("empty folder"),
                    _ => tr("file"),
                };
                match share {
                    // Translators: spoken for an item of the chart: name,
                    // size, share of its parent folder, then "folder" or "file".
                    Some(p) => trf!(
                        "{name}, {size}, {percent} % of {parent}, {kind}",
                        name = n.name,
                        size = size_text(n.size),
                        percent = n.size * 100 / p.size,
                        parent = p.name,
                        kind = what
                    ),
                    None => trf!(
                        "{name}, {size}, {kind}",
                        name = n.name,
                        size = size_text(n.size),
                        kind = what
                    ),
                }
            }
        };
        helix_app_common::announce(&self.chart, &text, helix_app_common::Urgency::Polite);
    }

    /// Enter on the focused chart item: opens a folder, or goes up from
    /// the centre.
    fn chart_activate(self: &Rc<Self>) {
        let Some(id) = self.chart_focus.get() else {
            return;
        };
        if id == self.chart_root.get() {
            self.chart_up();
            return;
        }
        let opens = self
            .tree
            .borrow()
            .as_ref()
            .is_some_and(|t| t.node(id).kind == Kind::Dir && !t.node(id).children.is_empty());
        if opens {
            self.set_chart_root(id);
            if self.chart.has_focus() {
                self.chart_move(chart::Move::In);
            }
        } else {
            self.chart.error_bell();
        }
    }

    /// Opens the item menu for the keyboard-focused chart item.
    fn chart_popup_focused(self: &Rc<Self>) {
        let Some(id) = self.chart_focus.get() else {
            return;
        };
        let rect = chart::anchor(&self.fresh_layout(), self.chart_root.get(), id);
        self.target.set(Some(id));
        self.update_action_state();
        let r = rect.map_or_else(
            || gdk::Rectangle::new(self.chart.width() / 2, self.chart.height() / 2, 1, 1),
            |r| {
                gdk::Rectangle::new(
                    r.x as i32,
                    r.y as i32,
                    r.w.max(1.0) as i32,
                    r.h.max(1.0) as i32,
                )
            },
        );
        self.chart_menu.set_pointing_to(Some(&r));
        self.chart_menu.popup();
    }

    /// Names the chart after its folder and describes the largest items,
    /// for screen readers.
    fn update_chart_accessible(&self) {
        let tree = self.tree.borrow();
        let Some(t) = tree.as_ref() else { return };
        let root = t.node(self.chart_root.get());
        let label = trf!("Usage of {}", root.name);
        let mut kids: Vec<_> = root
            .children
            .iter()
            .map(|&c| t.node(c))
            .filter(|n| !n.removed && n.size > 0)
            .collect();
        kids.sort_by_key(|n| std::cmp::Reverse(n.size));
        let parts: Vec<String> = kids
            .iter()
            .take(5)
            .map(|n| {
                let pct = (n.size * 100).checked_div(root.size).unwrap_or(0);
                // Translators: one of the largest items in the chart's description.
                trf!(
                    "{name} {size}, {percent} %",
                    name = n.name,
                    size = size_text(n.size),
                    percent = pct
                )
            })
            .collect();
        let description = if parts.is_empty() {
            trf!("{size} in total; no items", size = size_text(root.size))
        } else {
            trf!(
                "{} in total. Largest: {}. Use the arrow keys to explore.",
                size_text(root.size),
                parts.join("; ")
            )
        };
        self.chart.update_property(&[
            gtk::accessible::Property::Label(&label),
            gtk::accessible::Property::Description(&description),
        ]);
    }

    fn set_chart_root(self: &Rc<Self>, id: NodeId) {
        self.chart_root.set(id);
        self.hover.set(None);
        self.chart_focus.set(None);
        self.rebuild_breadcrumbs();
        self.update_chart_accessible();
        self.chart.queue_draw();
        self.update_action_state();
    }

    fn chart_up(self: &Rc<Self>) {
        let old = self.chart_root.get();
        let parent = self.tree.borrow().as_ref().and_then(|t| t.node(old).parent);
        match parent {
            Some(p) => {
                self.set_chart_root(p);
                // Keep the keyboard on the folder just left.
                if self.chart.has_focus() {
                    self.focus_chart_item(old);
                }
            }
            None if self.chart.has_focus() => self.chart.error_bell(),
            None => {}
        }
    }

    fn rebuild_breadcrumbs(self: &Rc<Self>) {
        while let Some(c) = self.breadcrumbs.first_child() {
            self.breadcrumbs.remove(&c);
        }
        let tree = self.tree.borrow();
        let Some(t) = tree.as_ref() else { return };
        let chain = t.ancestry(self.chart_root.get());
        for (i, &id) in chain.iter().enumerate() {
            if i > 0 {
                let sep = gtk::Label::new(Some("›"));
                sep.add_css_class("dim-label");
                self.breadcrumbs.append(&sep);
            }
            let btn = gtk::Button::builder()
                .label(&t.node(id).name)
                .css_classes(["flat"])
                .build();
            if i + 1 == chain.len() {
                btn.add_css_class("heading");
            }
            let a = self.clone();
            btn.connect_clicked(move |_| a.set_chart_root(id));
            self.breadcrumbs.append(&btn);
        }
    }

    // ---- Actions ------------------------------------------------------

    fn setup_actions(self: &Rc<Self>) {
        let win = &self.window;
        let add = |name: &str, f: ActionFn| {
            let action = gio::SimpleAction::new(name, None);
            let a = self.clone();
            action.connect_activate(move |_, _| f(&a));
            win.add_action(&action);
        };
        add("scan-folder", Box::new(|a| a.choose_folder()));
        add(
            "rescan",
            Box::new(|a| {
                if a.nav.visible_page().as_ref() == Some(&a.results_page) {
                    let path = a.scan_path.borrow().clone();
                    if let Some(p) = path {
                        a.start_scan(p);
                    }
                } else {
                    a.reload_locations();
                }
            }),
        );
        add(
            "cancel-scan",
            Box::new(|a| {
                if a.scan_progress.borrow().is_some() {
                    a.cancel_scan();
                    a.nav.pop_to_tag("start");
                }
            }),
        );
        add("chart-up", Box::new(|a| a.chart_up()));
        add("preferences", Box::new(|a| a.show_preferences()));
        add("open-item", Box::new(|a| a.open_target()));
        add("copy-path", Box::new(|a| a.copy_target()));
        add("trash-item", Box::new(|a| a.trash_target()));
        let gapp = self.window.application().expect("application");
        helix_app_common::shortcuts::install(
            &gapp,
            crate::shortcuts::ACCELS,
            crate::shortcuts::GROUPS,
        );
        // Delete without modifiers must not reach past a focused text field
        // (the preferences search), so it is a bubble-phase shortcut.
        helix_app_common::add_bubble_shortcuts(
            &self.window,
            &[
                (tr("Delete"), "win.trash-item"),
                ("KP_Delete", "win.trash-item"),
            ],
        );
    }

    fn set_enabled(&self, name: &str, enabled: bool) {
        if let Some(a) = self
            .window
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            a.set_enabled(enabled);
        }
    }

    fn update_action_state(&self) {
        let has_tree = self.tree.borrow().is_some();
        let target = self.target.get().filter(|_| has_tree);
        self.set_enabled("open-item", target.is_some());
        self.set_enabled("copy-path", target.is_some());
        self.set_enabled("trash-item", target.is_some_and(|t| t != ROOT));
        let can_up = has_tree
            && self
                .tree
                .borrow()
                .as_ref()
                .is_some_and(|t| t.node(self.chart_root.get()).parent.is_some());
        self.set_enabled("chart-up", can_up);
        self.up_button.set_sensitive(can_up);
        self.set_enabled("cancel-scan", self.scan_progress.borrow().is_some());
    }

    fn target_path(&self) -> Option<(NodeId, PathBuf, Kind)> {
        let id = self.target.get()?;
        let tree = self.tree.borrow();
        let t = tree.as_ref()?;
        Some((id, t.path(id), t.node(id).kind))
    }

    fn choose_folder(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Select Folder to Scan"))
            .modal(true)
            .build();
        dialog.set_initial_folder(Some(&gio::File::for_path(glib::home_dir())));
        let a = self.clone();
        dialog.select_folder(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if let Ok(file) = res {
                match file.path() {
                    Some(p) => a.start_scan(p),
                    None => a.toast(tr("Only local folders can be scanned")),
                }
            }
        });
    }

    fn open_target(self: &Rc<Self>) {
        let Some((_, path, kind)) = self.target_path() else {
            return;
        };
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
        let a = self.clone();
        let done = move |res: Result<(), glib::Error>| {
            if let Err(e) = res {
                if !e.matches(gtk::DialogError::Dismissed)
                    && !e.matches(gtk::DialogError::Cancelled)
                {
                    a.toast(&trf!("Could not open folder: {}", e.message()));
                }
            }
        };
        if kind == Kind::Dir {
            launcher.launch(Some(&self.window), None::<&gio::Cancellable>, done);
        } else {
            launcher.open_containing_folder(Some(&self.window), None::<&gio::Cancellable>, done);
        }
    }

    fn copy_target(&self) {
        if let Some((_, path, _)) = self.target_path() {
            self.window.clipboard().set_text(&path.to_string_lossy());
            self.toast(tr("Path copied"));
        }
    }

    fn trash_target(self: &Rc<Self>) {
        let Some((id, path, _)) = self.target_path() else {
            return;
        };
        if id == ROOT {
            return;
        }
        let (name, size) = {
            let tree = self.tree.borrow();
            let Some(n) = tree.as_ref().map(|t| t.node(id)) else {
                return;
            };
            (n.name.clone(), n.size)
        };
        let dialog = adw::AlertDialog::new(
            Some(&trf!("Move “{name}” to the Trash?", name = name)),
            Some(&format!(
                "{}\n\n{}",
                path.display(),
                trf!(
                    "This frees up to {size} once the trash is emptied.",
                    size = size_text(size)
                )
            )),
        );
        dialog.add_responses(&[("cancel", tr("_Cancel")), ("trash", tr("_Move to Trash"))]);
        dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let a = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "trash" {
                return;
            }
            let (a, name) = (a.clone(), name.clone());
            let file = gio::File::for_path(&path);
            file.trash_async(
                glib::Priority::DEFAULT,
                None::<&gio::Cancellable>,
                move |res| match res {
                    Ok(()) => a.remove_node(id),
                    Err(e) => a.toast(&trf!(
                        "Could not move “{name}” to the trash: {error}",
                        name = name,
                        error = e.message()
                    )),
                },
            );
        });
        dialog.present(Some(&self.window));
    }

    fn remove_node(self: &Rc<Self>, id: NodeId) {
        let (parent, ancestors) = {
            let mut tree = self.tree.borrow_mut();
            let Some(t) = tree.as_mut() else { return };
            let parent = t.node(id).parent;
            if !t.remove(id) {
                return;
            }
            (parent, parent.map(|p| t.ancestry(p)).unwrap_or_default())
        };
        if let Some(p) = parent {
            let store = self.stores.borrow().get(&p).cloned();
            if let Some(store) = store {
                let pos = (0..store.n_items())
                    .find(|&i| store.item(i).and_then(|o| node_id(&o)) == Some(id));
                if let Some(pos) = pos {
                    store.remove(pos);
                }
            }
        }
        // Refresh the visible cells of every ancestor.
        let refresh: Vec<(NodeId, Col, gtk::Widget)> = {
            let cells = self.cells.borrow();
            ancestors
                .iter()
                .filter_map(|n| cells.get(n).map(|v| (n, v)))
                .flat_map(|(n, v)| {
                    v.iter()
                        .filter_map(move |(c, w)| w.upgrade().map(|w| (*n, *c, w)))
                })
                .collect()
        };
        for (n, c, w) in refresh {
            self.fill_cell(c, &w, n);
        }
        // Keep the chart root valid.
        let root = self.chart_root.get();
        let chart_ok = self
            .tree
            .borrow()
            .as_ref()
            .is_some_and(|t| t.ancestry(root).iter().all(|&n| !t.node(n).removed));
        if !chart_ok {
            self.set_chart_root(parent.unwrap_or(ROOT));
        } else {
            if self.chart_focus.get() == Some(id) {
                self.chart_focus.set(None);
            }
            self.update_chart_accessible();
            self.chart.queue_draw();
        }
        self.target.set(None);
        self.update_action_state();
        self.update_status();
        self.toast(tr("Moved to the trash"));
    }

    // ---- Preferences --------------------------------------------------

    fn show_preferences(self: &Rc<Self>) {
        let dialog = adw::PreferencesDialog::new();
        dialog.set_title(tr("Preferences"));
        let page = adw::PreferencesPage::new();
        let scan_group = adw::PreferencesGroup::builder()
            .title(tr("Scanning"))
            .build();
        let cross = adw::SwitchRow::builder()
            .title(tr("Cross File System Boundaries"))
            .subtitle(tr(
                "Also scan folders that are on other disks or partitions",
            ))
            .active(self.local.borrow().cross_filesystems)
            .build();
        let a = self.clone();
        cross.connect_active_notify(move |r| {
            let mut st = a.local.borrow_mut();
            st.cross_filesystems = r.is_active();
            st.save();
        });
        scan_group.add(&cross);
        page.add(&scan_group);

        let excl_group = adw::PreferencesGroup::builder()
            .title(tr("Excluded Locations"))
            .description(tr("These locations are skipped when scanning"))
            .build();
        let add_btn = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(tr("Add Location…"))
            .css_classes(["flat"])
            .build()
            .labelled(tr("Add Location…"));
        excl_group.set_header_suffix(Some(&add_btn));
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        excl_group.add(&list);
        page.add(&excl_group);
        dialog.add(&page);

        fill_excluded(self, &list);
        let (a, l, dlg) = (self.clone(), list.clone(), dialog.downgrade());
        add_btn.connect_clicked(move |_| {
            let fd = gtk::FileDialog::builder()
                .title(tr("Exclude Location"))
                .modal(true)
                .build();
            let (a2, l2) = (a.clone(), l.clone());
            let parent = dlg
                .upgrade()
                .and_then(|d| d.root())
                .and_downcast::<gtk::Window>();
            fd.select_folder(parent.as_ref(), None::<&gio::Cancellable>, move |res| {
                if let Ok(file) = res {
                    let mut v = a2.excluded_uris();
                    let uri = file.uri().to_string();
                    if !v.contains(&uri) {
                        v.push(uri);
                        a2.prefs.set("excluded-uris", v);
                    }
                    fill_excluded(&a2, &l2);
                }
            });
        });
        dialog.present(Some(&self.window));
    }
}

/// (Re)fills the excluded-locations list in the preferences dialog.
fn fill_excluded(a: &Rc<App>, list: &gtk::ListBox) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
    let uris = a.excluded_uris();
    if uris.is_empty() {
        list.append(
            &adw::ActionRow::builder()
                .title(tr("No excluded locations"))
                .sensitive(false)
                .build(),
        );
    }
    for uri in uris {
        let title = gio::File::for_uri(&uri)
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| uri.clone());
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&title).as_str())
            .build();
        let rm = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(tr("Remove"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build()
            .labelled(tr("Remove"));
        let (a2, l2) = (a.clone(), list.clone());
        rm.connect_clicked(move |_| {
            let v: Vec<String> = a2
                .excluded_uris()
                .into_iter()
                .filter(|u| *u != uri)
                .collect();
            a2.prefs.set("excluded-uris", v);
            fill_excluded(&a2, &l2);
        });
        row.add_suffix(&rm);
        list.append(&row);
    }
}

fn location_row(loc: &Location) -> gtk::ListBoxRow {
    let icon = gtk::Image::from_icon_name(if loc.is_home {
        "user-home-symbolic"
    } else {
        "drive-harddisk-symbolic"
    });
    icon.set_pixel_size(32);
    let name = gtk::Label::builder()
        .label(&loc.name)
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    let detail = gtk::Label::builder()
        .label(&loc.detail)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vbox.set_hexpand(true);
    vbox.append(&name);
    vbox.append(&detail);
    if let Some(u) = loc.usage {
        let bar = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(1.0)
            .value(u.fraction())
            .css_classes(["du-capacity"])
            .build();
        clean_level_bar(&bar);
        let text = gtk::Label::builder()
            .label(trf!(
                "{} used of {} ({} available)",
                size_text(u.used()),
                size_text(u.total),
                size_text(u.available)
            ))
            .xalign(0.0)
            .css_classes(["caption"])
            .build();
        vbox.append(&bar);
        vbox.append(&text);
    }
    let arrow = gtk::Image::from_icon_name("go-next-symbolic");
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hbox.set_margin_top(10);
    hbox.set_margin_bottom(10);
    hbox.set_margin_start(12);
    hbox.set_margin_end(12);
    hbox.append(&icon);
    hbox.append(&vbox);
    hbox.append(&arrow);
    let row = gtk::ListBoxRow::builder()
        .child(&hbox)
        .activatable(true)
        .build();
    row.set_tooltip_text(Some(&trf!("Scan {}", loc.path.display())));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_state_ignores_old_window_fields() {
        let old = r#"{"width": 800, "height": 500, "maximized": true, "cross_filesystems": true}"#;
        let st: LocalState = serde_json::from_str(old).unwrap();
        assert_eq!(
            st,
            LocalState {
                cross_filesystems: true
            }
        );
        assert_eq!(
            serde_json::from_str::<LocalState>("{}").unwrap(),
            LocalState::default()
        );
    }

    /// `(schema id, key name, type)` triples declared in a schema file.
    fn keys(xml: &str) -> Vec<(String, String, String)> {
        let attr = |tag: &str, name: &str| {
            let pat = format!("{name}=\"");
            tag.find(&pat).map(|i| {
                tag[i + pat.len()..]
                    .split('"')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
        };
        let mut out = Vec::new();
        let mut schema = String::new();
        for tag in xml.split('<').skip(1) {
            if tag.starts_with("schema ") {
                schema = attr(tag, "id").unwrap_or_default();
            } else if tag.starts_with("key ") {
                out.push((
                    schema.clone(),
                    attr(tag, "name").unwrap_or_default(),
                    attr(tag, "type").unwrap_or_default(),
                ));
            }
        }
        out
    }

    #[test]
    fn shipped_schema_matches_keys_used() {
        let ours = keys(include_str!("../data/org.gnome.baobab.gschema.xml"));
        let used = [
            (SCHEMA_PREFERENCES, "excluded-uris", "as"),
            (SCHEMA_UI, "active-chart", "s"),
            (SCHEMA_UI, "window-size", "(ii)"),
            (SCHEMA_UI, "is-maximized", "b"),
        ];
        for (schema, key, ty) in used {
            assert!(
                ours.contains(&(schema.into(), key.into(), ty.into())),
                "{schema} {key}"
            );
        }
        // When the real schema is installed, every key we ship must exist
        // there with the same type.
        if let Ok(installed) =
            std::fs::read_to_string("/usr/share/glib-2.0/schemas/org.gnome.baobab.gschema.xml")
        {
            let installed = keys(&installed);
            for k in &ours {
                assert!(
                    installed.contains(k),
                    "{k:?} not in installed org.gnome.baobab schema"
                );
            }
        }
    }
}
