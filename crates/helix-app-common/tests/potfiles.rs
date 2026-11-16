//! po/POTFILES lists every file with translatable text, so the template
//! extracted from it is complete. Same rule as tools/i18n/potfiles.py.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Whether `text` calls tr/trc/trn/n_ with a literal, or trf!/trnf!/nc_!.
fn has_messages(text: &str) -> bool {
    let b = text.as_bytes();
    let starts_call =
        |i: usize, name: &str| b[i..].starts_with(name.as_bytes()) && (i == 0 || !ident(b[i - 1]));
    for i in 0..b.len() {
        for name in ["trf!(", "trnf!(", "nc_!("] {
            if starts_call(i, name) {
                return true;
            }
        }
        for name in ["tr(", "trc(", "trn(", "n_("] {
            if starts_call(i, name) {
                let rest = text[i + name.len()..].trim_start();
                if rest.starts_with('"') {
                    return true;
                }
            }
        }
    }
    false
}

fn walk(dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name != "tests" && name != "target" {
                walk(&p, out);
            }
        } else if name.ends_with(".rs") {
            let rel = p
                .strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel == "crates/helix-app-common/src/i18n.rs" {
                continue;
            }
            if has_messages(&std::fs::read_to_string(&p).unwrap()) {
                out.push(rel);
            }
        }
    }
}

#[test]
fn potfiles_lists_every_source_with_messages() {
    let mut expected = Vec::new();
    walk(&root().join("apps"), &mut expected);
    walk(&root().join("crates"), &mut expected);
    expected.sort();
    let listed = std::fs::read_to_string(root().join("po/POTFILES")).unwrap();
    let listed: Vec<String> = listed
        .lines()
        .filter(|l| l.ends_with(".rs"))
        .map(str::to_string)
        .collect();
    let missing: Vec<_> = expected.iter().filter(|f| !listed.contains(f)).collect();
    let stale: Vec<_> = listed.iter().filter(|f| !expected.contains(f)).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "po/POTFILES is out of date (run tools/i18n/update-pot.sh): missing {missing:?}, stale {stale:?}"
    );
    assert!(expected.len() > 100, "only {} files", expected.len());
}

#[test]
fn every_app_initialises_translations() {
    let apps = root().join("apps");
    let mut n = 0;
    for e in std::fs::read_dir(&apps).unwrap().flatten() {
        let main = e.path().join("src/main.rs");
        if main.is_file() {
            let text = std::fs::read_to_string(&main).unwrap();
            assert!(
                text.contains("helix_app_common::i18n::init()"),
                "{} does not call i18n::init()",
                main.display()
            );
            n += 1;
        }
    }
    assert!(n >= 25, "only {n} apps");
}
