//! Processes page: a sortable, filterable `GtkColumnView`.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use helix_app_common::Labelled;
use std::cell::{Cell, Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::columns;
use crate::format;
use crate::model::{self, ProcColumn, Whose};
use crate::sampler::ProcessInfo;
use helix_app_common::i18n::{n_, tr};
use helix_app_common::Prefs;

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::ProcessObject)]
    pub struct ProcessObject {
        #[property(get, set)]
        pub name: RefCell<String>,
        #[property(get, set)]
        pub user: RefCell<String>,
        #[property(get, set)]
        pub cpu_text: RefCell<String>,
        #[property(get, set)]
        pub memory_text: RefCell<String>,
        #[property(get, set)]
        pub pid_text: RefCell<String>,
        #[property(get, set)]
        pub priority_text: RefCell<String>,
        #[property(get, set)]
        pub state_text: RefCell<String>,
        #[property(get, set)]
        pub cmdline: RefCell<String>,
        pub info: RefCell<ProcessInfo>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProcessObject {
        const NAME: &'static str = "HelixSmProcess";
        type Type = super::ProcessObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ProcessObject {}
}

glib::wrapper! {
    pub struct ProcessObject(ObjectSubclass<imp::ProcessObject>);
}

/// Display options that affect the formatted cell text.
#[derive(Clone, Copy, Default)]
pub struct DisplayOpts {
    pub iec: bool,
}

impl ProcessObject {
    fn new(info: ProcessInfo, opts: DisplayOpts) -> Self {
        let obj: Self = glib::Object::new();
        obj.update(info, opts);
        obj
    }

    pub fn info(&self) -> Ref<'_, ProcessInfo> {
        self.imp().info.borrow()
    }

    pub fn pid(&self) -> i32 {
        self.info().pid
    }

    /// Replaces the data and notifies only the texts that changed.
    fn update(&self, info: ProcessInfo, opts: DisplayOpts) {
        let texts = [
            ("name", info.name.clone()),
            ("user", info.user.clone()),
            ("cpu-text", format!("{:.1}", info.cpu)),
            (
                "memory-text",
                info.memory
                    .map(|m| format::size(m, opts.iec))
                    .unwrap_or_else(|| "N/A".into()),
            ),
            ("pid-text", info.pid.to_string()),
            (
                "priority-text",
                format!("{} ({})", format::priority(info.nice), info.nice),
            ),
            ("state-text", format::state(info.state).to_string()),
            ("cmdline", info.cmdline.clone()),
        ];
        *self.imp().info.borrow_mut() = info;
        for (prop, value) in texts {
            if self.property::<String>(prop) != value {
                self.set_property(prop, value);
            }
        }
    }
}

struct FilterState {
    whose: Whose,
    search: String,
    uid: u32,
}

pub struct ProcView {
    pub page: gtk::Box,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub view: gtk::ColumnView,
    store: gio::ListStore,
    objects: RefCell<HashMap<i32, ProcessObject>>,
    filter: gtk::CustomFilter,
    filter_state: Rc<RefCell<FilterState>>,
    sorters: Vec<gtk::CustomSorter>,
    pub selection: gtk::SingleSelection,
    menu: gtk::PopoverMenu,
    pub opts: Cell<DisplayOpts>,
}

/// Title, row property, column, expand, xalign.
const COLUMNS: [(&str, &str, ProcColumn, bool, f32); 8] = [
    (n_("Process Name"), "name", ProcColumn::Name, true, 0.0),
    (n_("User"), "user", ProcColumn::User, false, 0.0),
    ("% CPU", "cpu-text", ProcColumn::Cpu, false, 1.0),
    (n_("Memory"), "memory-text", ProcColumn::Memory, false, 1.0),
    ("ID", "pid-text", ProcColumn::Pid, false, 1.0),
    (
        n_("Priority"),
        "priority-text",
        ProcColumn::Priority,
        false,
        0.0,
    ),
    (n_("State"), "state-text", ProcColumn::State, false, 0.0),
    (
        n_("Command Line"),
        "cmdline",
        ProcColumn::Command,
        true,
        0.0,
    ),
];

pub fn context_menu_model() -> gio::Menu {
    let menu = gio::Menu::new();
    let s = gio::Menu::new();
    s.append(Some(tr("_Properties")), Some("win.proc-properties"));
    s.append(Some(tr("Change _Priority…")), Some("win.proc-priority"));
    menu.append_section(None, &s);
    let s = gio::Menu::new();
    s.append(Some(tr("_Stop")), Some("win.proc-stop"));
    s.append(Some(tr("_Continue")), Some("win.proc-continue"));
    menu.append_section(None, &s);
    let s = gio::Menu::new();
    s.append(Some(tr("_End")), Some("win.proc-end"));
    s.append(Some(tr("_Kill")), Some("win.proc-kill"));
    menu.append_section(None, &s);
    menu
}

impl ProcView {
    /// `tree` is the `org.gnome.gnome-system-monitor.proctree` settings
    /// holding column visibility, widths, order and sort state.
    pub fn new(whose: Whose, uid: u32, opts: DisplayOpts, tree: Prefs) -> Rc<Self> {
        let store = gio::ListStore::new::<ProcessObject>();
        let filter_state = Rc::new(RefCell::new(FilterState {
            whose,
            search: String::new(),
            uid,
        }));
        let fs = filter_state.clone();
        let filter = gtk::CustomFilter::new(move |obj| {
            let Some(p) = obj.downcast_ref::<ProcessObject>() else {
                return false;
            };
            let f = fs.borrow();
            model::matches(&p.info(), f.whose, f.uid, &f.search)
        });
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let sorted = gtk::SortListModel::new(Some(filtered), None::<gtk::Sorter>);
        let selection = gtk::SingleSelection::new(Some(sorted.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_show_column_separators(false);
        view.set_reorderable(true);
        view.add_css_class("data-table");
        sorted.set_sorter(view.sorter().as_ref());

        let menu = gtk::PopoverMenu::from_model(Some(&context_menu_model()));
        menu.set_parent(&view);
        menu.set_has_arrow(false);
        menu.set_halign(gtk::Align::Start);
        {
            let menu = menu.clone();
            view.connect_destroy(move |_| menu.unparent());
        }

        let mut sorters = Vec::new();
        let mut columns: Vec<(ProcColumn, gtk::ColumnViewColumn)> = Vec::new();
        let header_menu = gio::Menu::new();
        let order =
            columns::order_from_setting(&tree.get::<Vec<i32>>(columns::COLUMNS_ORDER_KEY, vec![0]));
        for col in order {
            let (title, prop, _, expand, xalign) = COLUMNS
                .into_iter()
                .find(|c| c.2 == col)
                .expect("column spec");
            let factory = gtk::SignalListItemFactory::new();
            let view_weak = view.downgrade();
            let menu_c = menu.clone();
            let sel = selection.clone();
            factory.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
                let label = gtk::Label::builder()
                    .xalign(xalign)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .hexpand(true)
                    .build();
                if col == ProcColumn::Cpu || col == ProcColumn::Memory || col == ProcColumn::Pid {
                    label.add_css_class("numeric");
                }
                item.property_expression("item")
                    .chain_property::<ProcessObject>(prop)
                    .bind(&label, "label", gtk::Widget::NONE);
                if col == ProcColumn::Command || col == ProcColumn::Name {
                    item.property_expression("item")
                        .chain_property::<ProcessObject>("cmdline")
                        .bind(&label, "tooltip-text", gtk::Widget::NONE);
                }
                // Right click selects the row and opens the context menu.
                let click = gtk::GestureClick::builder()
                    .button(gdk_button_secondary())
                    .build();
                let item_weak = item.downgrade();
                let label_weak = label.downgrade();
                let view_weak = view_weak.clone();
                let menu_c = menu_c.clone();
                let sel = sel.clone();
                click.connect_pressed(move |g, _, x, y| {
                    let (Some(item), Some(label), Some(view)) = (
                        item_weak.upgrade(),
                        label_weak.upgrade(),
                        view_weak.upgrade(),
                    ) else {
                        return;
                    };
                    g.set_state(gtk::EventSequenceState::Claimed);
                    sel.set_selected(item.position());
                    if let Some(p) =
                        label.compute_point(&view, &gtk::graphene::Point::new(x as f32, y as f32))
                    {
                        menu_c.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                            p.x() as i32,
                            p.y() as i32,
                            1,
                            1,
                        )));
                        menu_c.popup();
                    }
                });
                label.add_controller(click);
                item.set_child(Some(&label));
            });
            let sorter = gtk::CustomSorter::new(move |a, b| {
                let (Some(a), Some(b)) = (
                    a.downcast_ref::<ProcessObject>(),
                    b.downcast_ref::<ProcessObject>(),
                ) else {
                    return gtk::Ordering::Equal;
                };
                model::compare(&a.info(), &b.info(), col).into()
            });
            let column = gtk::ColumnViewColumn::builder()
                .id(col.id())
                .title(tr(title))
                .factory(&factory)
                .sorter(&sorter)
                .resizable(true)
                .expand(expand)
                .visible(tree.get(&col.visible_key(), col.default_visible()))
                .header_menu(&header_menu)
                .build();
            if let Some(w) =
                columns::width_from_setting(tree.get(&col.width_key(), col.default_width()))
            {
                column.set_fixed_width(w);
            }
            view.append_column(&column);
            sorters.push(sorter);
            columns.push((col, column));
        }
        // Header context menu: one check item per column, in default order.
        let col_actions = gio::SimpleActionGroup::new();
        for col in columns::ALL {
            let (title, ..) = COLUMNS
                .into_iter()
                .find(|c| c.2 == col)
                .expect("column spec");
            header_menu.append(Some(tr(title)), Some(&format!("proccol.{}", col.id())));
        }
        for (col, column) in &columns {
            let action =
                gio::SimpleAction::new_stateful(col.id(), None, &column.is_visible().to_variant());
            let column = column.clone();
            let all: Vec<gtk::ColumnViewColumn> = columns.iter().map(|(_, c)| c.clone()).collect();
            let tree = tree.clone();
            let key = col.visible_key();
            action.connect_change_state(move |a, v| {
                let Some(visible) = v.and_then(|v| v.get::<bool>()) else {
                    return;
                };
                // Never hide the last visible column.
                if !visible && all.iter().filter(|c| c.is_visible()).count() <= 1 {
                    return;
                }
                a.set_state(&visible.to_variant());
                column.set_visible(visible);
                tree.set(&key, visible);
            });
            col_actions.add_action(&action);
        }
        view.insert_action_group("proccol", Some(&col_actions));

        // Widths: saved shortly after the user stops resizing.
        for (col, column) in &columns {
            let pending = Rc::new(Cell::new(false));
            let tree = tree.clone();
            let key = col.width_key();
            let default = col.default_width();
            column.connect_fixed_width_notify(move |c| {
                if pending.replace(true) {
                    return;
                }
                let (pending, tree, key, c) =
                    (pending.clone(), tree.clone(), key.clone(), c.downgrade());
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    pending.set(false);
                    let Some(c) = c.upgrade() else { return };
                    let w = c.fixed_width();
                    if w > 0 && tree.get(&key, default) != w {
                        tree.set(&key, w);
                    }
                });
            });
        }

        let (sort_col, descending) = columns::sort_from_settings(
            tree.get(columns::SORT_COL_KEY, columns::DEFAULT_SORT_COL),
            tree.get(columns::SORT_ORDER_KEY, columns::DEFAULT_SORT_ORDER),
        );
        let sort_column = columns
            .iter()
            .find(|(c, _)| *c == sort_col)
            .map(|(_, c)| c.clone());
        let sort_type = if descending {
            gtk::SortType::Descending
        } else {
            gtk::SortType::Ascending
        };
        view.sort_by_column(sort_column.as_ref(), sort_type);
        if let Some(sorter) = view.sorter().and_downcast::<gtk::ColumnViewSorter>() {
            let tree = tree.clone();
            sorter.connect_changed(move |s, _| {
                let Some(col) = s
                    .primary_sort_column()
                    .and_then(|c| c.id())
                    .and_then(|id| column_by_id(&id))
                else {
                    return;
                };
                let (c, o) = columns::sort_to_settings(
                    col,
                    s.primary_sort_order() == gtk::SortType::Descending,
                );
                if tree.get(columns::SORT_COL_KEY, columns::DEFAULT_SORT_COL) != c {
                    tree.set(columns::SORT_COL_KEY, c);
                }
                if tree.get(columns::SORT_ORDER_KEY, columns::DEFAULT_SORT_ORDER) != o {
                    tree.set(columns::SORT_ORDER_KEY, o);
                }
            });
        }

        // Column order (the view is reorderable by dragging headers).
        {
            let tree = tree.clone();
            view.columns().connect_items_changed(move |list, _, _, _| {
                // A drag is a remove + insert; only save complete states.
                if list.n_items() as usize != columns::ALL.len() {
                    return;
                }
                let current: Vec<ProcColumn> = (0..list.n_items())
                    .filter_map(|i| list.item(i).and_downcast::<gtk::ColumnViewColumn>())
                    .filter_map(|c| c.id().and_then(|id| column_by_id(&id)))
                    .collect();
                let stored = tree.get::<Vec<i32>>(columns::COLUMNS_ORDER_KEY, vec![0]);
                let merged = columns::merge_order(&stored, &current);
                if merged != stored {
                    tree.set(columns::COLUMNS_ORDER_KEY, merged);
                }
            });
        }

        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .build();

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search processes"))
            .hexpand(true)
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(500)
            .child(&search_entry)
            .build();
        let search_bar = gtk::SearchBar::builder().child(&clamp).build();
        search_bar.connect_entry(&search_entry);

        let end_button = gtk::Button::builder()
            .label(tr("End Process"))
            .action_name("win.proc-end")
            .build();
        end_button.add_css_class("destructive-action");
        let props_button = gtk::Button::builder()
            .icon_name("document-properties-symbolic")
            .tooltip_text(tr("Properties"))
            .action_name("win.proc-properties")
            .build()
            .labelled(tr("Properties"));
        let action_bar = gtk::ActionBar::new();
        action_bar.pack_end(&end_button);
        action_bar.pack_end(&props_button);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.append(&search_bar);
        page.append(&scroller);
        page.append(&action_bar);

        let this = Rc::new(ProcView {
            page,
            search_bar,
            search_entry,
            view,
            store,
            objects: RefCell::default(),
            filter,
            filter_state,
            sorters,
            selection,
            menu,
            opts: Cell::new(opts),
        });
        let weak = Rc::downgrade(&this);
        this.search_entry.connect_search_changed(move |e| {
            if let Some(this) = weak.upgrade() {
                this.filter_state.borrow_mut().search = e.text().to_lowercase();
                this.filter.changed(gtk::FilterChange::Different);
            }
        });
        this
    }

    pub fn set_whose(&self, whose: Whose) {
        self.filter_state.borrow_mut().whose = whose;
        self.filter.changed(gtk::FilterChange::Different);
    }

    pub fn selected(&self) -> Option<ProcessObject> {
        self.selection
            .selected_item()
            .and_downcast::<ProcessObject>()
    }

    /// Opens the context menu for the selected row (keyboard Menu key).
    pub fn popup_menu(&self) {
        if self.selected().is_none() {
            return;
        }
        let w = self.view.width();
        self.menu
            .set_pointing_to(Some(&gtk::gdk::Rectangle::new(w / 3, 40, 1, 1)));
        self.menu.popup();
    }

    /// Merges a new snapshot into the model, keeping row objects (and so
    /// selection) for processes that still exist.
    pub fn update(&self, procs: Vec<ProcessInfo>) {
        let opts = self.opts.get();
        let selected_pid = self.selected().map(|p| p.pid());
        let alive: HashSet<i32> = procs.iter().map(|p| p.pid).collect();
        let mut new_objects = Vec::new();
        {
            let mut objects = self.objects.borrow_mut();
            for info in procs {
                match objects.get(&info.pid) {
                    Some(obj) => obj.update(info, opts),
                    None => {
                        let pid = info.pid;
                        let obj = ProcessObject::new(info, opts);
                        objects.insert(pid, obj.clone());
                        new_objects.push(obj);
                    }
                }
            }
            objects.retain(|pid, _| alive.contains(pid));
        }
        self.store.retain(|o| {
            o.downcast_ref::<ProcessObject>()
                .is_some_and(|p| alive.contains(&p.pid()))
        });
        self.store.extend_from_slice(&new_objects);
        self.filter.changed(gtk::FilterChange::Different);
        for s in &self.sorters {
            s.changed(gtk::SorterChange::Different);
        }
        if let Some(pid) = selected_pid {
            self.select_pid(pid);
        }
    }

    /// Re-formats every row (e.g. after a units preference change).
    pub fn reformat(&self) {
        let opts = self.opts.get();
        for obj in self.objects.borrow().values() {
            let info = obj.info().clone();
            obj.update(info, opts);
        }
    }

    fn select_pid(&self, pid: i32) {
        if self.selected().is_some_and(|p| p.pid() == pid) {
            return;
        }
        let model = self.selection.model().expect("model");
        for i in 0..model.n_items() {
            if model
                .item(i)
                .and_downcast::<ProcessObject>()
                .is_some_and(|p| p.pid() == pid)
            {
                self.selection.set_selected(i);
                return;
            }
        }
    }
}

fn column_by_id(id: &str) -> Option<ProcColumn> {
    columns::ALL.into_iter().find(|c| c.id() == id)
}

fn gdk_button_secondary() -> u32 {
    gtk::gdk::BUTTON_SECONDARY
}
