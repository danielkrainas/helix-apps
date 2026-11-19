//! Manual benchmark (ignored): how long GtkDirectoryList takes to load a
//! big folder with Files' attributes, and which attributes cost what.
//! `N=50000 tools/testing/run-tests.sh cargo test --release -p helix-files
//! --test load_bench -- --ignored --nocapture`

use gtk::prelude::*;
use gtk::{gio, glib};
use std::time::{Duration, Instant};

const FULL: &str = "standard::name,standard::display-name,standard::edit-name,standard::type,\
standard::size,standard::icon,standard::symbolic-icon,standard::content-type,standard::fast-content-type,\
standard::is-hidden,standard::is-backup,standard::is-symlink,standard::symlink-target,standard::target-uri,\
time::modified,unix::mode,access::can-read,access::can-write,access::can-trash,access::can-delete,\
access::can-rename,thumbnail::path,thumbnail::is-valid,trash::orig-path,trash::deletion-date";

fn without(drop: &[&str]) -> String {
    FULL.split(',')
        .filter(|a| !drop.contains(a))
        .collect::<Vec<_>>()
        .join(",")
}

fn load(dir: &std::path::Path, attrs: &str, prio: glib::Priority) -> (Duration, u32) {
    let list = gtk::DirectoryList::new(Some(attrs), None::<&gio::File>);
    list.set_monitored(true);
    list.set_io_priority(prio);
    let main = glib::MainLoop::new(None, false);
    let t0 = Instant::now();
    list.set_file(Some(&gio::File::for_path(dir)));
    let (m, l) = (main.clone(), list.clone());
    glib::timeout_add_local(Duration::from_millis(1), move || {
        if !l.is_loading() {
            m.quit();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    main.run();
    (t0.elapsed(), list.n_items())
}

#[test]
#[ignore]
fn load_bench() {
    let n: usize = std::env::var("N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let base = std::env::temp_dir().join(format!("helix-load-bench-{n}"));
    let dir = base.join("photos");
    let cache = base.join("cache");
    // Before GLib reads it.
    unsafe { std::env::set_var("XDG_CACHE_HOME", &cache) };
    let Some(_d) = helix_app_common::testing::headless_gtk() else {
        return;
    };
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
        let thumbs = cache.join("thumbnails/large");
        std::fs::create_dir_all(&thumbs).unwrap();
        // A tiny real JPEG header so sniffing has something to read.
        let jpeg: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0,
            0xFF, 0xD9,
        ];
        let png = std::fs::read("/usr/share/icons/hicolor/48x48/apps/org.gnome.Nautilus.png")
            .or_else(|_| std::fs::read("/usr/share/pixmaps/debian-logo.png"))
            .unwrap_or_default();
        for i in 0..n {
            let p = dir.join(format!("IMG_{i:06}.jpg"));
            std::fs::write(&p, jpeg).unwrap();
            if i % 2 == 0 && !png.is_empty() {
                let uri = gio::File::for_path(&p).uri();
                let sum = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri.as_str())
                    .unwrap();
                std::fs::write(thumbs.join(format!("{sum}.png")), &png).unwrap();
            }
        }
    }
    // Warm the page cache once.
    load(&dir, FULL, glib::Priority::DEFAULT);
    let cases: Vec<(&str, String)> = vec![
        ("full (Files today)", FULL.to_string()),
        (
            "minimal (name,type,size,mtime)",
            "standard::name,standard::display-name,standard::type,standard::size,time::modified"
                .into(),
        ),
        (
            "full − thumbnail::is-valid",
            without(&["thumbnail::is-valid"]),
        ),
        (
            "full − thumbnail::*",
            without(&["thumbnail::path", "thumbnail::is-valid"]),
        ),
        (
            "full − icons",
            without(&["standard::icon", "standard::symbolic-icon"]),
        ),
        (
            "full − content-type (fast only)",
            without(&["standard::content-type"]),
        ),
        (
            "full − access::*",
            without(&[
                "access::can-read",
                "access::can-write",
                "access::can-trash",
                "access::can-delete",
                "access::can-rename",
            ]),
        ),
    ];
    for (name, attrs) in &cases {
        let (t, items) = load(&dir, attrs, glib::Priority::DEFAULT_IDLE);
        println!(
            "{name:36} {:>8.0} ms  ({items} items)",
            t.as_secs_f64() * 1000.0
        );
    }
    let (t, _) = load(&dir, FULL, glib::Priority::DEFAULT);
    println!(
        "{:36} {:>8.0} ms",
        "full, DEFAULT priority",
        t.as_secs_f64() * 1000.0
    );
}
