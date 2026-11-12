//! Locale fallback for help lookup (`LANG` → language → `C`).

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
    let locale = get("LC_ALL").or_else(|| get("LC_MESSAGES")).or_else(|| get("LANG"));
    candidates(get("LANGUAGE").as_deref(), locale.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_locale() {
        assert_eq!(variants("pt_BR.UTF-8@euro"), ["pt_BR@euro", "pt_BR", "pt@euro", "pt"]);
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
        assert_eq!(candidates(Some("sv:fi_FI"), Some("fi_FI.UTF-8")), ["sv", "fi_FI", "fi", "C"]);
    }
}
