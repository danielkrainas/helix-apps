//! End-to-end check of the translation set-up: a catalogue written to a
//! scratch locale directory is found through `HELIX_LOCALEDIR` and used by
//! `tr`, `trn`, `trf!` and `trnf!`, plural rules included.
//!
//! This is the only test in this binary: it sets the process locale.

use helix_app_common::i18n::{self, tr, trn};
use helix_app_common::{trf, trnf};

#[test]
fn translations_from_helix_localedir() {
    let dir = std::env::temp_dir().join(format!("helix-i18n-test-{}", std::process::id()));
    let messages = dir.join("xx/LC_MESSAGES");
    std::fs::create_dir_all(&messages).unwrap();
    i18n::write_test_catalogue(
        &messages.join(format!("{}.mo", i18n::DOMAIN)),
        &[
            (
                "",
                "Content-Type: text/plain; charset=UTF-8\n\
                 Plural-Forms: nplurals=2; plural=(n > 1);\n",
            ),
            ("Keyboard Shortcuts", "Raccourcis clavier"),
            ("{n} item\0{n} items", "{n} élément\0{n} éléments"),
            ("Open {name}", "Ouvrir {name}"),
            ("{} of {}", "{1} sur {0}"),
        ],
    );
    std::env::set_var("HELIX_LOCALEDIR", &dir);
    std::env::set_var("LANGUAGE", "xx");
    std::env::remove_var("LC_ALL");
    std::env::remove_var("LC_MESSAGES");
    // LANGUAGE is ignored in the C locale; use any installed real locale.
    let real = ["en_US.UTF-8", "C.UTF-8", "en_GB.UTF-8"];
    let mut ok = false;
    for l in real {
        std::env::set_var("LANG", l);
        i18n::init_for_tests();
        if tr("Keyboard Shortcuts") != "Keyboard Shortcuts" {
            ok = true;
            break;
        }
    }
    if !ok {
        eprintln!("no locale accepts LANGUAGE here; skipping");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert_eq!(tr("Keyboard Shortcuts"), "Raccourcis clavier");
    assert_eq!(tr("Not in the catalogue"), "Not in the catalogue");
    // French plural rule: 0 and 1 are singular.
    assert_eq!(trn("{n} item", "{n} items", 0), "0 élément");
    assert_eq!(trn("{n} item", "{n} items", 1), "1 élément");
    assert_eq!(trn("{n} item", "{n} items", 5), "5 éléments");
    assert_eq!(trnf!("{n} item", "{n} items", 2), "2 éléments");
    assert_eq!(trf!("Open {name}", name = "a.txt"), "Ouvrir a.txt");
    // Translators may reorder positional placeholders.
    assert_eq!(trf!("{} of {}", 3, 9), "9 sur 3");
    let _ = std::fs::remove_dir_all(&dir);
}
