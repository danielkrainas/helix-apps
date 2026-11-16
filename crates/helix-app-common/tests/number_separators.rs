//! `i18n::number_separators` in real locales: a decimal comma and `.`
//! groups in German, a decimal comma and a narrow no-break space in
//! French, `.` and `,` in English. The locales are compiled into a
//! scratch directory with `localedef` (skipped when that or the locale
//! sources are missing), so nothing is installed.
//!
//! The only test in this binary: it sets the process locale.

use helix_app_common::i18n;

fn compile(dir: &std::path::Path, name: &str, source: &str) -> bool {
    std::process::Command::new("localedef")
        .args(["-i", source, "-f", "UTF-8"])
        .arg(dir.join(name))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success() || s.code() == Some(1))
        && dir.join(name).exists()
}

#[test]
fn separators_of_comma_and_dot_locales() {
    let dir = std::env::temp_dir().join(format!("helix-locales-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cases = [
        ("de_DE.UTF-8", "de_DE", (',', Some('.'))),
        ("fr_FR.UTF-8", "fr_FR", (',', Some('\u{202f}'))),
        ("en_US.UTF-8", "en_US", ('.', Some(','))),
    ];
    if !cases.iter().all(|(n, s, _)| compile(&dir, n, s)) {
        eprintln!("localedef or locale sources unavailable; skipping");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    std::env::set_var("LOCPATH", &dir);
    std::env::remove_var("LC_ALL");
    std::env::remove_var("LC_NUMERIC");
    for (name, _, expected) in cases {
        std::env::set_var("LANG", name);
        i18n::init_for_tests();
        assert_eq!(i18n::number_separators(), expected, "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
