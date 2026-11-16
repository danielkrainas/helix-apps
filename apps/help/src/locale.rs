//! Locale fallback for help lookup (`LANG` → language → `C`).

use std::path::Path;
use std::sync::OnceLock;

/// Expands one locale name (`pt_BR.UTF-8@euro`) into its fallback variants,
/// most specific first: `pt_BR@euro`, `pt_BR`, `pt@euro`, `pt`.
pub fn variants(locale: &str) -> Vec<String> {
    let locale = locale.trim();
    if locale.is_empty() || locale == "C" || locale == "POSIX" || locale.starts_with("C.") {
        return Vec::new();
    }
    let (rest, modifier) = match locale.split_once('@') {
        Some((r, m)) => (r, Some(m)),
        None => (locale, None),
    };
    let rest = rest.split('.').next().unwrap_or(rest);
    let (lang, territory) = match rest.split_once('_') {
        Some((l, t)) => (l, Some(t)),
        None => (rest, None),
    };
    if lang.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(t) = territory {
        if let Some(m) = modifier {
            out.push(format!("{lang}_{t}@{m}"));
        }
        out.push(format!("{lang}_{t}"));
    }
    if let Some(m) = modifier {
        out.push(format!("{lang}@{m}"));
    }
    out.push(lang.to_string());
    out
}

/// Builds the ordered, de-duplicated list of locale directories to try,
/// always ending in `C`. `language` is the colon-separated `LANGUAGE`
/// value; `locale` the effective `LC_ALL`/`LC_MESSAGES`/`LANG` value.
pub fn candidates(language: Option<&str>, locale: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |v: String| {
        if !out.contains(&v) {
            out.push(v);
        }
    };
    // LANGUAGE is only honoured when the locale isn't C, like gettext.
    let locale_is_c = locale.map(|l| variants(l).is_empty()).unwrap_or(true);
    if let (Some(lang), false) = (language, locale_is_c) {
        for item in lang.split(':') {
            variants(item).into_iter().for_each(&mut push);
        }
    }
    if let Some(l) = locale {
        variants(l).into_iter().for_each(&mut push);
    }
    push("C".to_string());
    out
}

/// Locale candidates from the process environment.
pub fn from_env() -> Vec<String> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let locale = get("LC_ALL")
        .or_else(|| get("LC_MESSAGES"))
        .or_else(|| get("LANG"));
    candidates(get("LANGUAGE").as_deref(), locale.as_deref())
}

/// The BCP 47 language tag (an HTML `lang`) of a locale name as help
/// directories and gettext use it: `C` → `en`, `pt_BR` → `pt-BR`,
/// `sr_RS@latin` → `sr-Latn-RS`.
pub fn bcp47(locale: &str) -> String {
    let locale = locale.trim();
    if locale.is_empty() || locale == "C" || locale == "POSIX" || locale.starts_with("C.") {
        return "en".into();
    }
    let (rest, modifier) = match locale.split_once('@') {
        Some((r, m)) => (r, Some(m)),
        None => (locale, None),
    };
    let rest = rest.split('.').next().unwrap_or(rest);
    let (lang, territory) = match rest.split_once('_') {
        Some((l, t)) => (l, Some(t)),
        None => (rest, None),
    };
    let mut tag = lang.to_string();
    match modifier {
        Some("latin") => tag.push_str("-Latn"),
        Some("cyrillic") => tag.push_str("-Cyrl"),
        _ => {}
    }
    if let Some(t) = territory {
        tag.push('-');
        tag.push_str(t);
    }
    // Other modifiers that are valid variant subtags (`ca@valencia`).
    if let Some(m) = modifier.filter(|m| {
        !matches!(*m, "latin" | "cyrillic")
            && (5..=8).contains(&m.len())
            && m.chars().all(|c| c.is_ascii_alphanumeric())
    }) {
        tag.push('-');
        tag.push_str(m);
    }
    tag
}

/// The language of a document found in `…/help/<locale>/<doc>`, from the
/// name of its locale directory; `None` for a document elsewhere.
pub fn dir_lang(doc_dir: &Path) -> Option<String> {
    let locale_dir = doc_dir.parent()?;
    if locale_dir.parent()?.file_name()? != "help" {
        return None;
    }
    Some(bcp47(&locale_dir.file_name()?.to_string_lossy()))
}

/// The language of Help's own text (the document list, search results,
/// headings and messages): the first of `candidates` with an installed
/// catalogue in `localedir`, as gettext picks it, else English.
pub fn ui_lang_from(candidates: &[String], localedir: &Path) -> String {
    candidates
        .iter()
        .find(|c| {
            *c == "C"
                || localedir
                    .join(c)
                    .join("LC_MESSAGES")
                    .join(format!("{}.mo", helix_app_common::i18n::DOMAIN))
                    .is_file()
        })
        .map_or_else(|| "en".into(), |c| bcp47(c))
}

/// [`ui_lang_from`] for this process.
pub fn ui_lang() -> &'static str {
    static LANG: OnceLock<String> = OnceLock::new();
    LANG.get_or_init(|| ui_lang_from(&from_env(), &helix_app_common::i18n::localedir()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_locale() {
        assert_eq!(
            variants("pt_BR.UTF-8@euro"),
            ["pt_BR@euro", "pt_BR", "pt@euro", "pt"]
        );
        assert_eq!(variants("de_DE.UTF-8"), ["de_DE", "de"]);
        assert_eq!(variants("fr"), ["fr"]);
        assert!(variants("C.UTF-8").is_empty());
        assert!(variants("POSIX").is_empty());
    }

    #[test]
    fn candidates_end_in_c() {
        assert_eq!(candidates(None, Some("en_GB.UTF-8")), ["en_GB", "en", "C"]);
        assert_eq!(candidates(None, None), ["C"]);
        assert_eq!(candidates(Some("de"), Some("C")), ["C"]);
        assert_eq!(
            candidates(Some("sv:fi_FI"), Some("fi_FI.UTF-8")),
            ["sv", "fi_FI", "fi", "C"]
        );
    }

    #[test]
    fn language_tags() {
        assert_eq!(bcp47("C"), "en");
        assert_eq!(bcp47("de"), "de");
        assert_eq!(bcp47("pt_BR"), "pt-BR");
        assert_eq!(bcp47("zh_CN.UTF-8"), "zh-CN");
        assert_eq!(bcp47("sr@latin"), "sr-Latn");
        assert_eq!(bcp47("sr_RS@latin"), "sr-Latn-RS");
        assert_eq!(bcp47("ca@valencia"), "ca-valencia");
        assert_eq!(bcp47("de_DE@euro"), "de-DE");
        assert_eq!(
            dir_lang(Path::new("/usr/share/help/pt_BR/gnome-help")).as_deref(),
            Some("pt-BR")
        );
        assert_eq!(
            dir_lang(Path::new("/usr/share/help/C/x")).as_deref(),
            Some("en")
        );
        assert_eq!(dir_lang(Path::new("/home/u/manual")), None);
    }

    #[test]
    fn ui_language_follows_installed_catalogues() {
        let dir = std::env::temp_dir().join(format!("helix-help-uilang-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("de/LC_MESSAGES")).unwrap();
        std::fs::write(dir.join("de/LC_MESSAGES/helix-apps.mo"), b"").unwrap();
        let c = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(ui_lang_from(&c(&["de_DE", "de", "C"]), &dir), "de");
        assert_eq!(ui_lang_from(&c(&["fr_FR", "fr", "C"]), &dir), "en");
        assert_eq!(ui_lang_from(&c(&["fr", "de", "C"]), &dir), "de");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
