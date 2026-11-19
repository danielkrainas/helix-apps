//! Manual benchmark (ignored): main-thread stalls loading and re-sorting
//! a big folder through Files' model chain, sorting incrementally or not.
//!
//! `N=200000 tools/testing/run-tests.sh cargo test --release -p helix-files
//! --test sort_bench -- --ignored --nocapture` (the folder is made once in
//! the temporary folder and kept).

#[allow(dead_code)]
#[path = "../src/sorting.rs"]
mod sorting;

use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

const ATTRS: &str = "standard::name,standard::display-name,standard::type,standard::size,time::modified,standard::is-hidden,standard::is-backup";

fn cmp(a: &gio::FileInfo, b: &gio::FileInfo, key: sorting::SortKey) -> std::cmp::Ordering {
    let (an, bn) = (a.display_name(), b.display_name());
    fn e<'a>(i: &gio::FileInfo, n: &'a str) -> sorting::Entry<'a> {
        sorting::Entry {
            name: n,
            is_dir: i.file_type() == gio::FileType::Directory,
            size: i.size().max(0) as u64,
            type_desc: "",
            mtime: i.attribute_uint64("time::modified") as i64,
        }
    }
    let ea = e(a, an.as_str());
    let eb = e(b, bn.as_str());
    sorting::compare(&ea, &eb, key, false, true)
}

fn run(dir: &std::path::Path, incremental: bool) {
    let key = Rc::new(Cell::new(sorting::SortKey::Name));
    let list = gtk::DirectoryList::new(Some(ATTRS), None::<&gio::File>);
    list.set_io_priority(glib::Priority::DEFAULT_IDLE);
    let filter = gtk::CustomFilter::new(|o| {
        let i = o.downcast_ref::<gio::FileInfo>().unwrap();
        !(i.boolean("standard::is-hidden") || i.boolean("standard::is-backup"))
    });
    let fm = gtk::FilterListModel::new(Some(list.clone()), Some(filter));
    fm.set_incremental(false);
    let k = key.clone();
    let sorter = gtk::CustomSorter::new(move |a, b| {
        cmp(
            a.downcast_ref().unwrap(),
            b.downcast_ref().unwrap(),
            k.get(),
        )
        .into()
    });
    let sm = gtk::SortListModel::new(Some(fm), Some(sorter.clone()));
    sm.set_incremental(incremental);
    let _sel = gtk::MultiSelection::new(Some(sm.clone()));

    // Longest gap between 1 ms ticks = longest main-thread stall.
    let worst = Rc::new(Cell::new(Duration::ZERO));
    let last = Rc::new(Cell::new(Instant::now()));
    let (w, l) = (worst.clone(), last.clone());
    let tick = glib::timeout_add_local(Duration::from_millis(1), move || {
        let now = Instant::now();
        w.set(w.get().max(now - l.get()));
        l.set(now);
        glib::ControlFlow::Continue
    });
    let main = glib::MainLoop::new(None, false);
    let wait = |done: Rc<dyn Fn() -> bool>| {
        let m = main.clone();
        let t0 = Instant::now();
        glib::timeout_add_local(Duration::from_millis(2), move || {
            if done() || t0.elapsed() > Duration::from_secs(60) {
                m.quit();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        main.run();
        t0.elapsed()
    };
    let first_visible = Rc::new(RefCell::new(None::<Duration>));
    let t0 = Instant::now();
    let fv = first_visible.clone();
    sm.connect_items_changed(move |m, _, _, _| {
        if fv.borrow().is_none() && m.n_items() > 0 {
            *fv.borrow_mut() = Some(t0.elapsed());
        }
    });
    last.set(Instant::now());
    list.set_file(Some(&gio::File::for_path(dir)));
    let (l2, s2) = (list.clone(), sm.clone());
    let load = wait(Rc::new(move || !l2.is_loading() && s2.pending() == 0));
    let load_worst = worst.replace(Duration::ZERO);
    println!(
        "incremental={incremental}: first items {:?}, loaded+sorted {:?}, worst stall {:?}, items {}",
        first_visible.borrow().unwrap_or_default(),
        load,
        load_worst,
        sm.n_items()
    );
    // Re-sort by modification date, then by name again.
    for k2 in [sorting::SortKey::Modified, sorting::SortKey::Name] {
        last.set(Instant::now());
        key.set(k2);
        sorter.changed(gtk::SorterChange::Different);
        let s2 = sm.clone();
        let t = wait(Rc::new(move || s2.pending() == 0));
        println!(
            "  resort {:?}: {:?}, worst stall {:?}",
            k2,
            t,
            worst.replace(Duration::ZERO)
        );
    }
    tick.remove();
}

#[test]
#[ignore]
fn sort_bench() {
    let Some(_d) = helix_app_common::testing::headless_gtk() else {
        return;
    };
    let n: usize = std::env::var("N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let dir = std::env::temp_dir().join(format!("helix-sort-bench-{n}"));
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            std::fs::write(
                dir.join(format!("IMG_{:05} copy {}.jpg", (i * 7919) % n, i % 13)),
                "",
            )
            .unwrap();
        }
    }
    run(&dir, false);
    run(&dir, true);
}
