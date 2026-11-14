//! Installed icon themes, for Appearance's *Icon Theme* row: the
//! directories of the XDG icon theme search path holding an `index.theme`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Themes that only back others up and are never offered.
const NOT_OFFERED: [&str; 2] = ["hicolor", "default"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconTheme {
    /// The directory name: the value `icon-theme` takes.
    pub id: String,
    /// `Name` from `index.theme`, else the id.
    pub name: String,
}

/// What `index.theme` says: its name, and whether the theme is offered
/// (not `Hidden`, and it has icon directories: cursor-only themes have
/// none).
pub fn parse_index(text: &str) -> (Option<String>, bool) {
    let mut in_section = false;
    let (mut name, mut hidden, mut has_dirs) = (None, false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == "[Icon Theme]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim().to_owned()).filter(|n| !n.is_empty()),
            "Hidden" => hidden = value.trim().eq_ignore_ascii_case("true"),
            "Directories" => has_dirs = value.split(',').any(|d| !d.trim().is_empty()),
            _ => {}
        }
    }
    (name, !hidden && has_dirs)
}

/// The icon theme search path: `$XDG_DATA_HOME/icons`, `~/.icons`, then
/// `icons` in each of `$XDG_DATA_DIRS`.
pub fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .map(|d| d.join("icons"))
        .into_iter()
        .chain(home.map(|h| h.join(".icons")))
        .chain(
            data_dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(|d| Path::new(d).join("icons")),
        )
        .collect()
}

/// The themes offered in `dirs`, by name; a theme in an earlier directory
/// hides one of the same id in a later one.
pub fn installed(dirs: &[PathBuf]) -> Vec<IconTheme> {
    let mut found: BTreeMap<String, Option<IconTheme>> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if found.contains_key(&id) || NOT_OFFERED.contains(&id.as_str()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path().join("index.theme")) else {
                continue;
            };
            let (name, offered) = parse_index(&text);
            let theme = offered.then(|| IconTheme {
                name: name.unwrap_or_else(|| id.clone()),
                id: id.clone(),
            });
            found.insert(id, theme);
        }
    }
    let mut themes: Vec<IconTheme> = found.into_values().flatten().collect();
    themes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    themes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_files() {
        let theme = "[Icon Theme]\nName=Yaru\nInherits=Adwaita\nDirectories=16x16/actions,scalable/actions\n\n[16x16/actions]\nSize=16\n";
        assert_eq!(parse_index(theme), (Some("Yaru".into()), true));
        let cursors = "[Icon Theme]\nName=DMZ (White)\nInherits=DMZ-White\n";
        assert_eq!(
            parse_index(cursors),
            (Some("DMZ (White)".into()), false),
            "cursor-only"
        );
        let hidden = "[Icon Theme]\nName=Base\nHidden=true\nDirectories=a\n";
        assert!(!parse_index(hidden).1);
        let other_section = "[Other]\nName=X\n[Icon Theme]\nDirectories=a\n";
        assert_eq!(parse_index(other_section), (None, true));
    }

    #[test]
    fn scanning() {
        let root = std::env::temp_dir().join(format!("helix-icon-themes-{}", std::process::id()));
        let (user, system) = (root.join("user"), root.join("system"));
        let theme = |dir: &Path, id: &str, index: &str| {
            std::fs::create_dir_all(dir.join(id)).unwrap();
            std::fs::write(dir.join(id).join("index.theme"), index).unwrap();
        };
        theme(
            &system,
            "Adwaita",
            "[Icon Theme]\nName=Adwaita\nDirectories=a\n",
        );
        theme(&system, "zed", "[Icon Theme]\nName=Alpha\nDirectories=a\n");
        theme(
            &system,
            "hicolor",
            "[Icon Theme]\nName=Hicolor\nDirectories=a\n",
        );
        theme(&system, "cursors", "[Icon Theme]\nName=Cursors\n");
        theme(
            &system,
            "mine",
            "[Icon Theme]\nName=System Copy\nDirectories=a\n",
        );
        theme(&user, "mine", "[Icon Theme]\nName=Mine\nDirectories=a\n");
        std::fs::create_dir_all(system.join("no-index")).unwrap();
        let got: Vec<(String, String)> = installed(&[user, system, root.join("missing")])
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();
        let want = [("Adwaita", "Adwaita"), ("zed", "Alpha"), ("mine", "Mine")];
        assert_eq!(got, want.map(|(a, b)| (a.to_owned(), b.to_owned())));
        let _ = std::fs::remove_dir_all(&root);
    }
}
