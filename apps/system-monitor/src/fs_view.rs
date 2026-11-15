//! File Systems page.

use adw::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

use crate::format;
use crate::model::{self, FsColumn};
use crate::sampler::FsEntry;
use helix_app_common::i18n::{n_, tr};

pub struct FsView {
    pub page: gtk::ScrolledWindow,
    store: gio::ListStore,
}

fn entry(obj: &glib::Object) -> Option<FsEntry> {
    obj.downcast_ref::<glib::BoxedAnyObject>()
        .map(|b| b.borrow::<FsEntry>().clone())
}

const COLUMNS: [(&str, FsColumn, bool); 7] = [
    (n_("Device"), FsColumn::Device, false),
    (n_("Directory"), FsColumn::Directory, true),
    (n_("Type"), FsColumn::Type, false),
    (n_("Total"), FsColumn::Total, false),
    (n_("Free"), FsColumn::Free, false),
    (n_("Available"), FsColumn::Available, false),
    (n_("Used"), FsColumn::Used, true),
];

fn cell_text(e: &FsEntry, col: FsColumn) -> String {
    match col {
        FsColumn::Device => e.device.clone(),
        FsColumn::Directory => e.dir.clone(),
        FsColumn::Type => e.fstype.clone(),
        FsColumn::Total => format::size(e.total, false),
        FsColumn::Free => format::size(e.free, false),
        FsColumn::Available => format::size(e.available, false),
        FsColumn::Used => format!(
            "{} ({:.0} %)",
            format::size(e.used(), false),
            e.used_fraction() * 100.0
        ),
    }
}

impl FsView {
    pub fn new() -> Rc<Self> {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let sorted = gtk::SortListModel::new(Some(store.clone()), None::<gtk::Sorter>);
        let selection = gtk::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        let view = gtk::ColumnView::new(Some(selection));
        view.add_css_class("data-table");
        sorted.set_sorter(view.sorter().as_ref());

        let mut dir_column = None;
        for (title, col, expand) in COLUMNS {
            let factory = gtk::SignalListItemFactory::new();
            factory.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
                let numeric = matches!(col, FsColumn::Total | FsColumn::Free | FsColumn::Available);
                let label = gtk::Label::builder()
                    .xalign(if numeric { 1.0 } else { 0.0 })
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .hexpand(true)
                    .build();
                if col == FsColumn::Used {
                    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let bar = gtk::LevelBar::builder()
                        .min_value(0.0)
                        .max_value(1.0)
                        .hexpand(true)
                        .valign(gtk::Align::Center)
                        .build();
                    // Turn red only when nearly full.
                    bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 0.9);
                    bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, 0.95);
                    bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_FULL, 1.0);
                    label.set_hexpand(false);
                    label.set_width_chars(16);
                    label.set_xalign(1.0);
                    b.append(&bar);
                    b.append(&label);
                    item.set_child(Some(&b));
                } else {
                    item.set_child(Some(&label));
                }
            });
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
                let Some(e) = item.item().as_ref().and_then(entry) else {
                    return;
                };
                let child = item.child();
                let label = match child
                    .as_ref()
                    .and_then(|c| c.downcast_ref::<gtk::Box>().cloned())
                {
                    Some(b) => {
                        if let Some(bar) = b.first_child().and_downcast::<gtk::LevelBar>() {
                            bar.set_value(e.used_fraction());
                        }
                        b.last_child().and_downcast::<gtk::Label>()
                    }
                    None => child.and_downcast::<gtk::Label>(),
                };
                if let Some(label) = label {
                    label.set_label(&cell_text(&e, col));
                    if matches!(col, FsColumn::Device | FsColumn::Directory) {
                        label.set_tooltip_text(Some(&cell_text(&e, col)));
                    }
                }
            });
            let sorter = gtk::CustomSorter::new(move |a, b| match (entry(a), entry(b)) {
                (Some(a), Some(b)) => model::compare_fs(&a, &b, col).into(),
                _ => gtk::Ordering::Equal,
            });
            let column = gtk::ColumnViewColumn::builder()
                .title(tr(title))
                .factory(&factory)
                .sorter(&sorter)
                .resizable(true)
                .expand(expand)
                .build();
            if col == FsColumn::Directory {
                dir_column = Some(column.clone());
            }
            view.append_column(&column);
        }
        view.sort_by_column(dir_column.as_ref(), gtk::SortType::Ascending);
        view.connect_activate(|view, pos| {
            let Some(e) = view
                .model()
                .and_then(|m| m.item(pos))
                .as_ref()
                .and_then(entry)
            else {
                return;
            };
            let uri = gio::File::for_path(&e.dir).uri();
            if let Err(err) =
                gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
            {
                eprintln!("helix-system-monitor: cannot open {}: {err}", e.dir);
            }
        });

        let page = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .build();
        Rc::new(FsView { page, store })
    }

    pub fn update(&self, entries: Vec<FsEntry>) {
        let current: Vec<FsEntry> = (0..self.store.n_items())
            .filter_map(|i| self.store.item(i).as_ref().and_then(entry))
            .collect();
        if current == entries {
            return;
        }
        let objs: Vec<glib::BoxedAnyObject> =
            entries.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objs);
    }
}
