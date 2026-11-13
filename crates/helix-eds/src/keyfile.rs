//! Minimal GKeyFile reader/writer for EDS source descriptions (the `Data`
//! property of `org.gnome.evolution.dataserver.Source`).

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyFile {
    /// Groups in file order; each holds `(key, raw value)` in order. Keys keep
    /// their locale suffix (`DisplayName[de]`).
    groups: Vec<(String, Vec<(String, String)>)>,
}

impl KeyFile {
    pub fn parse(text: &str) -> KeyFile {
        let mut kf = KeyFile::default();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(name) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                kf.groups.push((name.to_string(), Vec::new()));
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                if kf.groups.is_empty() {
                    continue;
                }
                let group = kf.groups.last_mut().unwrap();
                group.1.push((k.trim_end().to_string(), unescape(v.trim_start())));
            }
        }
        kf
    }

    pub fn has_group(&self, group: &str) -> bool {
        self.groups.iter().any(|(g, _)| g == group)
    }

    pub fn groups(&self) -> impl Iterator<Item = &str> {
        self.groups.iter().map(|(g, _)| g.as_str())
    }

    pub fn get(&self, group: &str, key: &str) -> Option<&str> {
        self.groups
            .iter()
            .find(|(g, _)| g == group)
            .and_then(|(_, kv)| kv.iter().find(|(k, _)| k == key))
            .map(|(_, v)| v.as_str())
    }

    /// Looks up a localized key, trying each language (e.g. `de_AT`, `de`) and
    /// then the untranslated key.
    pub fn get_locale(&self, group: &str, key: &str, languages: &[String]) -> Option<&str> {
        for lang in languages {
            if let Some(v) = self.get(group, &format!("{key}[{lang}]")) {
                return Some(v);
            }
        }
        self.get(group, key)
    }

    pub fn get_bool(&self, group: &str, key: &str) -> Option<bool> {
        match self.get(group, key)? {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }

    pub fn set(&mut self, group: &str, key: &str, value: &str) {
        let idx = match self.groups.iter().position(|(g, _)| g == group) {
            Some(i) => i,
            None => {
                self.groups.push((group.to_string(), Vec::new()));
                self.groups.len() - 1
            }
        };
        let kv = &mut self.groups[idx].1;
        match kv.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.to_string(),
            None => kv.push((key.to_string(), value.to_string())),
        }
    }

    /// Sets a key and drops all its translations, so the new value shows in
    /// every locale.
    pub fn set_unlocalized(&mut self, group: &str, key: &str, value: &str) {
        let prefix = format!("{key}[");
        if let Some((_, kv)) = self.groups.iter_mut().find(|(g, _)| g == group) {
            kv.retain(|(k, _)| !k.starts_with(&prefix));
        }
        self.set(group, key, value);
    }

    pub fn set_bool(&mut self, group: &str, key: &str, value: bool) {
        self.set(group, key, if value { "true" } else { "false" });
    }
}

impl std::fmt::Display for KeyFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, (g, kv)) in self.groups.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            writeln!(f, "[{g}]")?;
            for (k, v) in kv {
                writeln!(f, "{k}={}", escape(v))?;
            }
        }
        Ok(())
    }
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => out.push(' '),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for (i, c) in v.chars().enumerate() {
        match c {
            ' ' if i == 0 => out.push_str("\\s"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

/// The user's preferred languages for localized keys, from `LANGUAGE`,
/// `LC_ALL`, `LC_MESSAGES`, `LANG` (e.g. `de_AT.UTF-8` → `de_AT`, `de`).
pub fn user_languages() -> Vec<String> {
    let mut raw = Vec::new();
    for var in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                raw.extend(v.split(':').map(str::to_string));
                if var != "LANGUAGE" {
                    break;
                }
            }
        }
    }
    let mut out = Vec::new();
    for l in raw {
        let base = l.split(['.', '@']).next().unwrap_or("").to_string();
        if base.is_empty() || base == "C" || base == "POSIX" {
            continue;
        }
        if !out.contains(&base) {
            out.push(base.clone());
        }
        if let Some((lang, _)) = base.split_once('_') {
            if !out.contains(&lang.to_string()) {
                out.push(lang.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "# comment\n\n[Data Source]\nDisplayName[de]=Persönlich\nDisplayName=Personal\nEnabled=true\nParent=local-stub\n\n[Calendar]\nBackendName=local\nColor=#62a0ea\nSelected=false\n";

    #[test]
    fn parses_groups_and_locales() {
        let kf = KeyFile::parse(DATA);
        assert_eq!(kf.get("Data Source", "DisplayName"), Some("Personal"));
        assert_eq!(kf.get_locale("Data Source", "DisplayName", &["de".into()]), Some("Persönlich"));
        assert_eq!(kf.get_locale("Data Source", "DisplayName", &["fr".into()]), Some("Personal"));
        assert_eq!(kf.get_bool("Calendar", "Selected"), Some(false));
        assert!(kf.has_group("Calendar"));
        assert!(!kf.has_group("Address Book"));
    }

    #[test]
    fn set_and_serialize_round_trip() {
        let mut kf = KeyFile::parse(DATA);
        kf.set_bool("Calendar", "Selected", true);
        kf.set_unlocalized("Data Source", "DisplayName", " Work\nstuff");
        kf.set("Alarms", "IncludeMe", "true");
        let text = kf.to_string();
        let again = KeyFile::parse(&text);
        assert_eq!(again.get_bool("Calendar", "Selected"), Some(true));
        assert_eq!(again.get_locale("Data Source", "DisplayName", &["de".into()]), Some(" Work\nstuff"));
        assert_eq!(again.get("Alarms", "IncludeMe"), Some("true"));
        assert_eq!(again, kf);
    }
}
