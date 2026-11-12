//! XKB layout names from the rules `.lst` file, and input source helpers.

use std::collections::HashMap;

pub const EVDEV_LST: &str = "/usr/share/X11/xkb/rules/evdev.lst";

/// Layout / variant descriptions keyed by `layout` or `layout+variant`.
#[derive(Debug, Default)]
pub struct XkbInfo {
    /// `(id, description)` in file order.
    pub entries: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl XkbInfo {
    pub fn parse(text: &str) -> Self {
        let mut info = XkbInfo::default();
        let mut section = "";
        for line in text.lines() {
            if let Some(s) = line.strip_prefix('!') {
                section = match s.trim() {
                    "layout" => "layout",
                    "variant" => "variant",
                    _ => "",
                };
                continue;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, rest)) = line.split_once(char::is_whitespace) else { continue };
            let rest = rest.trim();
            let (id, desc) = match section {
                "layout" => (name.to_string(), rest.to_string()),
                "variant" => {
                    let Some((layout, desc)) = rest.split_once(':') else { continue };
                    (format!("{}+{}", layout.trim(), name), desc.trim().to_string())
                }
                _ => continue,
            };
            info.index.insert(id.clone(), info.entries.len());
            info.entries.push((id, desc));
        }
        info
    }

    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(EVDEV_LST).unwrap_or_default())
    }

    pub fn description(&self, id: &str) -> Option<&str> {
        self.index.get(id).map(|&i| self.entries[i].1.as_str())
    }
}

/// Display name of an input source `(type, id)`.
pub fn source_name(info: &XkbInfo, kind: &str, id: &str) -> String {
    match kind {
        "xkb" => info
            .description(id)
            .map(str::to_string)
            .or_else(|| {
                // Unknown variant: fall back to the layout's name.
                let layout = id.split('+').next()?;
                info.description(layout).map(|d| format!("{d} ({})", id.split('+').nth(1).unwrap_or("")))
            })
            .unwrap_or_else(|| id.to_string()),
        "ibus" => format!("{id} (IBus)"),
        _ => id.to_string(),
    }
}

/// Moves item `i` by `delta` positions, returning whether it moved.
pub fn move_item<T>(items: &mut [T], i: usize, delta: isize) -> bool {
    let j = i as isize + delta;
    if i >= items.len() || j < 0 || j as usize >= items.len() {
        return false;
    }
    items.swap(i, j as usize);
    true
}

/// Case-insensitive substring filter on description or id.
pub fn filter<'a>(info: &'a XkbInfo, query: &str) -> Vec<&'a (String, String)> {
    let q = query.trim().to_lowercase();
    info.entries
        .iter()
        .filter(|(id, desc)| q.is_empty() || desc.to_lowercase().contains(&q) || id.to_lowercase().contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "! model\n  pc105 Generic 105-key PC\n\n! layout\n  us              English (US)\n  de              German\n\n! variant\n  dvorak          us: English (Dvorak)\n  nodeadkeys      de: German (no dead keys)\n\n! option\n  grp             Switching\n";

    #[test]
    fn parses_sections() {
        let info = XkbInfo::parse(SAMPLE);
        assert_eq!(info.entries.len(), 4);
        assert_eq!(info.description("us"), Some("English (US)"));
        assert_eq!(info.description("us+dvorak"), Some("English (Dvorak)"));
        assert_eq!(info.description("de+nodeadkeys"), Some("German (no dead keys)"));
        assert_eq!(info.description("pc105"), None);
        assert_eq!(info.description("grp"), None);
    }

    #[test]
    fn names() {
        let info = XkbInfo::parse(SAMPLE);
        assert_eq!(source_name(&info, "xkb", "de"), "German");
        assert_eq!(source_name(&info, "xkb", "us+dvorak"), "English (Dvorak)");
        assert_eq!(source_name(&info, "xkb", "us+weird"), "English (US) (weird)");
        assert_eq!(source_name(&info, "xkb", "zz"), "zz");
        assert_eq!(source_name(&info, "ibus", "anthy"), "anthy (IBus)");
    }

    #[test]
    fn moving() {
        let mut v = vec![1, 2, 3];
        assert!(move_item(&mut v, 0, 1));
        assert_eq!(v, [2, 1, 3]);
        assert!(!move_item(&mut v, 0, -1));
        assert!(!move_item(&mut v, 2, 1));
        assert!(move_item(&mut v, 2, -1));
        assert_eq!(v, [2, 3, 1]);
    }

    #[test]
    fn filtering() {
        let info = XkbInfo::parse(SAMPLE);
        assert_eq!(filter(&info, "").len(), 4);
        assert_eq!(filter(&info, "german").len(), 2);
        assert_eq!(filter(&info, "DVORAK")[0].0, "us+dvorak");
    }
}
