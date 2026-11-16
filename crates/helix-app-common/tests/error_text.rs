//! Library errors show translated details: in a pseudo locale, the
//! common errors of zbus, GLib/GIO and `std::io` come out as the
//! catalogue's sentences, not the library's English.
//!
//! The only test in this binary: it sets the process locale.

use helix_app_common::errors::{self, Kind};
use helix_app_common::i18n;

fn pseudo(s: &str) -> String {
    format!("⟦{s}⟧")
}

#[test]
fn common_errors_are_translated() {
    let dir = std::env::temp_dir().join(format!("helix-errors-test-{}", std::process::id()));
    let messages = dir.join("en@pseudo/LC_MESSAGES");
    std::fs::create_dir_all(&messages).unwrap();
    let translated: Vec<(String, String)> = Kind::ALL
        .iter()
        .map(|k| (k.msgid().to_owned(), pseudo(k.msgid())))
        .collect();
    let mut entries: Vec<(&str, &str)> = vec![(
        "",
        "Content-Type: text/plain; charset=UTF-8\nPlural-Forms: nplurals=2; plural=(n != 1);\n",
    )];
    entries.extend(translated.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    i18n::write_test_catalogue(&messages.join(format!("{}.mo", i18n::DOMAIN)), &entries);
    std::env::set_var("HELIX_LOCALEDIR", &dir);
    std::env::set_var("LANGUAGE", "en@pseudo");
    std::env::remove_var("LC_ALL");
    std::env::remove_var("LC_MESSAGES");
    let mut ok = false;
    for l in ["en_US.UTF-8", "C.UTF-8", "en_GB.UTF-8"] {
        std::env::set_var("LANG", l);
        i18n::init_for_tests();
        if Kind::NotFound.sentence() != Kind::NotFound.msgid() {
            ok = true;
            break;
        }
    }
    if !ok {
        eprintln!("no locale accepts LANGUAGE here; skipping");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    // Disks: UDisks2 not running; a dismissed password prompt.
    assert_eq!(
        errors::detail(
            "org.freedesktop.DBus.Error.ServiceUnknown: The name org.freedesktop.UDisks2 \
             was not provided by any .service files"
        ),
        pseudo(Kind::ServiceMissing.msgid())
    );
    assert_eq!(
        errors::detail("org.freedesktop.UDisks2.Error.NotAuthorizedDismissed: Not authorized"),
        pseudo(Kind::AuthDismissed.msgid())
    );
    // Files: GIO errors.
    let e = glib::Error::new(
        gio::IOErrorEnum::PermissionDenied,
        "Error opening directory “/root”: Permission denied",
    );
    assert_eq!(
        errors::glib_detail(&e),
        pseudo(Kind::PermissionDenied.msgid())
    );
    let e = glib::Error::new(
        gio::IOErrorEnum::NotFound,
        "Error when getting information for file “/x”: No such file or directory",
    );
    assert_eq!(errors::glib_detail(&e), pseudo(Kind::NotFound.msgid()));
    // Files: bookmarks file (std::io).
    let e = std::io::Error::from_raw_os_error(28);
    assert_eq!(errors::io_detail(&e), pseudo(Kind::NoSpace.msgid()));
    // Contacts and Calendar: EDS over zbus.
    assert_eq!(
        errors::detail("org.freedesktop.DBus.Error.NoReply: Did not receive a reply"),
        pseudo(Kind::TimedOut.msgid())
    );
    assert_eq!(
        errors::detail("org.gnome.evolution.dataserver.Client.PermissionDenied: denied"),
        pseudo(Kind::PermissionDenied.msgid())
    );
    // Anything else stays as the library said it, without the D-Bus name.
    assert_eq!(
        errors::detail("org.freedesktop.UDisks2.Error.Failed: wrong fs type"),
        "wrong fs type"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
