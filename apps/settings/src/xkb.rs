//! XKB layout names from the rules `.lst` file, and input source helpers.

use helix_app_common::trf;
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
            let Some((name, rest)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let rest = rest.trim();
            let (id, desc) = match section {
                "layout" => (name.to_string(), rest.to_string()),
                "variant" => {
                    let Some((layout, desc)) = rest.split_once(':') else {
                        continue;
                    };
                    (
                        format!("{}+{}", layout.trim(), name),
                        desc.trim().to_string(),
                    )
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
                info.description(layout)
                    .map(|d| format!("{d} ({})", id.split('+').nth(1).unwrap_or("")))
            })
            .unwrap_or_else(|| id.to_string()),
        "ibus" => trf!("{id} (IBus)", id = id),
        _ => id.to_string(),
    }
}

/// Where IBus components describe their engines.
pub const IBUS_COMPONENT_DIR: &str = "/usr/share/ibus/component";

/// An IBus input method: engine name, long name, language.
#[derive(Debug, Clone, PartialEq)]
pub struct IbusEngine {
    pub name: String,
    pub longname: String,
    pub language: String,
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn xml_element<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let start = block.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = block[start..].find(&format!("</{tag}>"))? + start;
    Some(block[start..end].trim())
}

/// The engines an IBus component file lists statically, without the
/// `xkb:` ones (plain layouts are offered from XKB directly). Engines a
/// component only reports by running a program (`<engines exec=…>`) are
/// not listed.
pub fn ibus_engines(component_xml: &str) -> Vec<IbusEngine> {
    let mut out = Vec::new();
    let mut rest = component_xml;
    while let Some(i) = rest.find("<engine>") {
        let after = &rest[i + "<engine>".len()..];
        let Some(end) = after.find("</engine>") else {
            break;
        };
        let block = &after[..end];
        rest = &after[end..];
        let Some(name) = xml_element(block, "name").map(xml_unescape) else {
            continue;
        };
        if name.is_empty() || name.starts_with("xkb:") {
            continue;
        }
        let longname = xml_element(block, "longname")
            .map(xml_unescape)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| name.clone());
        let language = xml_element(block, "language")
            .map(xml_unescape)
            .unwrap_or_default();
        out.push(IbusEngine {
            name,
            longname,
            language,
        });
    }
    out
}

/// IBus engines from every installed component file (read from disk;
/// no IBus daemon is contacted). Like IBus, `IBUS_COMPONENT_PATH`
/// (colon-separated) replaces the default directory.
pub fn load_ibus_engines() -> Vec<IbusEngine> {
    let dirs: Vec<std::path::PathBuf> = match std::env::var_os("IBUS_COMPONENT_PATH") {
        Some(p) => std::env::split_paths(&p).collect(),
        None => vec![IBUS_COMPONENT_DIR.into()],
    };
    let mut engines: Vec<IbusEngine> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|dir| dir.flatten())
        .filter(|e| e.path().extension().is_some_and(|x| x == "xml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .flat_map(|text| ibus_engines(&text))
        .collect();
    engines.sort_by_key(|e| e.longname.to_lowercase());
    engines.dedup_by(|a, b| a.name == b.name);
    engines
}

/// Case-insensitive substring filter on an engine's names and language.
pub fn ibus_matches(engine: &IbusEngine, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || engine.longname.to_lowercase().contains(&q)
        || engine.name.to_lowercase().contains(&q)
        || engine.language.to_lowercase().contains(&q)
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
        .filter(|(id, desc)| {
            q.is_empty() || desc.to_lowercase().contains(&q) || id.to_lowercase().contains(&q)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "! model\n  pc105 Generic 105-key PC\n\n! layout\n  us              English (US)\n  de              German\n\n! variant\n  dvorak          us: English (Dvorak)\n  nodeadkeys      de: German (no dead keys)\n\n! option\n  grp             Switching\n";

    #[test]
    fn ibus_component_engines() {
        let xml = r#"<component><name>org.freedesktop.IBus.Test</name><engines>
            <engine><name>xkb:us::eng</name><longname>English (US)</longname></engine>
            <engine><name>anthy</name><longname>Anthy &amp; friends</longname><language>ja</language></engine>
            <engine><name>pinyin</name><language>zh_CN</language></engine>
            </engines></component>"#;
        let e = ibus_engines(xml);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "anthy");
        assert_eq!(e[0].longname, "Anthy & friends");
        assert_eq!(e[1].longname, "pinyin");
        assert!(ibus_matches(&e[0], "JA"));
        assert!(ibus_matches(&e[1], "pin"));
        assert!(!ibus_matches(&e[1], "anthy"));
        // Engines listed by running a program are skipped.
        assert!(ibus_engines("<engines exec='/usr/libexec/x --xml'/>").is_empty());
    }

    #[test]
    fn parses_sections() {
        let info = XkbInfo::parse(SAMPLE);
        assert_eq!(info.entries.len(), 4);
        assert_eq!(info.description("us"), Some("English (US)"));
        assert_eq!(info.description("us+dvorak"), Some("English (Dvorak)"));
        assert_eq!(
            info.description("de+nodeadkeys"),
            Some("German (no dead keys)")
        );
        assert_eq!(info.description("pc105"), None);
        assert_eq!(info.description("grp"), None);
    }

    #[test]
    fn names() {
        let info = XkbInfo::parse(SAMPLE);
        assert_eq!(source_name(&info, "xkb", "de"), "German");
        assert_eq!(source_name(&info, "xkb", "us+dvorak"), "English (Dvorak)");
        assert_eq!(
            source_name(&info, "xkb", "us+weird"),
            "English (US) (weird)"
        );
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
