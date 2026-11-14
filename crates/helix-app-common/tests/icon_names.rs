//! Every `"…-symbolic"` icon name written in an app's source must resolve
//! on a stock system: in the Adwaita icon theme (including its
//! `symbolic/legacy` directory, which the adwaita-icon-theme package
//! installs), in hicolor, in Helix's bundled icons
//! (`crates/helix-app-common/icons`) or in the app's own `data/` directory.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn names_in(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            names_in(&p, out);
        } else if let Some(stem) = p.file_name().and_then(|n| n.to_str()) {
            for ext in [".svg", ".png"] {
                if let Some(name) = stem.strip_suffix(ext) {
                    out.insert(name.to_string());
                }
            }
        }
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `"…-symbolic"` string literals (plain icon-name characters only).
fn symbolic_literals(src: &str) -> Vec<String> {
    let mut v = Vec::new();
    for part in src.split('"').skip(1).step_by(2) {
        if part.ends_with("-symbolic")
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        {
            v.push(part.to_string());
        }
    }
    v
}

#[test]
fn literal_extraction() {
    let src = r#"x.icon_name("go-up-symbolic"); format!("{}-symbolic", a); "Plain text""#;
    assert_eq!(symbolic_literals(src), vec!["go-up-symbolic".to_string()]);
}

#[test]
fn every_symbolic_icon_resolves() {
    let adwaita = Path::new("/usr/share/icons/Adwaita");
    if !adwaita.is_dir() {
        eprintln!("Adwaita icon theme not installed; skipping");
        return;
    }
    let mut stock = BTreeSet::new();
    names_in(adwaita, &mut stock);
    names_in(Path::new("/usr/share/icons/hicolor"), &mut stock);
    helix_app_common::register_icon_resource();
    stock.extend(helix_app_common::bundled_icon_names());
    assert!(stock.contains("globe-symbolic"), "bundled icons not found");

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut missing = Vec::new();
    for app in std::fs::read_dir(root.join("apps")).unwrap().flatten() {
        let src = app.path().join("src");
        if !src.is_dir() {
            continue;
        }
        let mut own = BTreeSet::new();
        names_in(&app.path().join("data"), &mut own);
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for name in symbolic_literals(&text) {
                if !stock.contains(&name) && !own.contains(&name) {
                    missing.push(format!("{}: {name}", f.display()));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "icons missing from Adwaita, hicolor and the bundled set:\n{}",
        missing.join("\n")
    );
}
