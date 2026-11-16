//! Month names in real locales: Chinese, Japanese and Korean write months
//! as numbers ("11月"), never glibc's full Chinese names ("十一月"), while
//! other languages keep their names and the forms dates need.
//!
//! This is the only test in this binary: it sets the process locale. A
//! locale that is not installed is compiled with `localedef` into a
//! scratch `LOCPATH`; one that cannot be is skipped.

use helix_app_common::i18n::{self, month_abbr, month_name, month_name_in_date};
use helix_app_common::trf;
use std::path::{Path, PathBuf};

/// Switches the process to `locale`; false when it is unavailable.
fn use_locale(locale: &str) -> bool {
    std::env::set_var("LC_ALL", locale);
    // SAFETY: this binary's only test; no other thread uses the locale.
    unsafe { gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "") }.is_some()
}

/// Compiles `locale` (like "zh_CN.UTF-8") into `dir` with localedef.
fn compile(locale: &str, dir: &Path) -> bool {
    let (name, charset) = locale.split_once('.').unwrap();
    std::process::Command::new("localedef")
        .args(["-i", name, "-f", charset])
        .arg(dir.join(locale))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        && dir.join(locale).is_dir()
}

/// Compiles the shipped catalogue `po/<lang>.po` under `locale_dir`.
fn catalogue(lang: &str, locale_dir: &Path) -> bool {
    let po = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../po/{lang}.po"));
    let messages = locale_dir.join(lang).join("LC_MESSAGES");
    std::fs::create_dir_all(&messages).unwrap();
    std::process::Command::new("msgfmt")
        .arg("-o")
        .arg(messages.join(format!("{}.mo", i18n::DOMAIN)))
        .arg(po)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn month_names_in_cjk_and_other_locales() {
    // (locale, catalogue, November on its own, in a date, abbreviated,
    // Calendar's title for November 2026)
    let cases: &[(&str, &str, &str, &str, &str, &str)] = &[
        ("zh_CN.UTF-8", "zh_CN", "11月", "11月", "11月", "2026年11月"),
        ("ja_JP.UTF-8", "ja", "11月", "11月", "11月", "2026年11月"),
        ("ko_KR.UTF-8", "ko", "11월", "11월", "11월", "2026년 11월"),
        (
            "ru_RU.UTF-8",
            "ru",
            "Ноябрь",
            "ноября",
            "ноя",
            "Ноябрь 2026",
        ),
        (
            "de_DE.UTF-8",
            "de",
            "November",
            "November",
            "Nov",
            "November 2026",
        ),
    ];
    let scratch: PathBuf =
        std::env::temp_dir().join(format!("helix-month-names-{}", std::process::id()));
    let locale_dir = scratch.join("messages");
    std::env::set_var("HELIX_LOCALEDIR", &locale_dir);
    std::env::remove_var("LANGUAGE");
    let mut tested = Vec::new();
    for &(locale, lang, alone, in_date, abbr, title) in cases {
        if !use_locale(locale) {
            std::fs::create_dir_all(&scratch).unwrap();
            if !compile(locale, &scratch) {
                eprintln!("{locale}: not available; skipping");
                continue;
            }
            std::env::set_var("LOCPATH", &scratch);
            let ok = use_locale(locale);
            std::env::remove_var("LOCPATH");
            if !ok {
                eprintln!("{locale}: not available; skipping");
                continue;
            }
        }
        let translated = catalogue(lang, &locale_dir);
        i18n::init_for_tests();
        assert_eq!(month_name(11), alone, "{locale}");
        assert_eq!(month_name_in_date(11), in_date, "{locale}");
        assert!(
            month_abbr(11).starts_with(abbr),
            "{locale}: {}",
            month_abbr(11)
        );
        // Every month, not only November.
        if locale.starts_with("zh") || locale.starts_with("ja") {
            for m in 1..=12 {
                assert_eq!(month_name(m), format!("{m}月"), "{locale}");
                assert_eq!(month_name_in_date(m), format!("{m}月"), "{locale}");
            }
        }
        if translated {
            // As Calendar's dates::month_title builds it.
            let shown = trf!("{month} {year}", month = month_name(11), year = 2026);
            assert_eq!(shown, title, "{locale}");
        } else {
            eprintln!("{locale}: msgfmt unavailable; title not checked");
        }
        tested.push(locale);
    }
    std::fs::remove_dir_all(&scratch).ok();
    eprintln!("tested: {tested:?}");
}
