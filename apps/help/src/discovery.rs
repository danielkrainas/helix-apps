//! Finding help documents in `$XDG_DATA_DIRS/help/<locale>/<doc>/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mallard,
    DocBook,
    Html,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Directory holding the document (resources are relative to it).
    pub dir: PathBuf,
    /// Entry file (`index.page`, `index.docbook`, `<doc>.xml`, `index.html`).
    pub entry: PathBuf,
    pub format: Format,
}

/// `help` directories in lookup order: user data dir first, then the
/// system data dirs.
pub fn help_dirs_from(
    data_home: Option<&str>,
    data_dirs: Option<&str>,
    home: Option<&str>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match data_home.filter(|s| !s.is_empty()) {
        Some(h) => out.push(PathBuf::from(h).join("help")),
        None => {
            if let Some(home) = home {
                out.push(PathBuf::from(home).join(".local/share/help"));
            }
        }
    }
    let dirs = data_dirs
        .filter(|s| !s.is_empty())
        .unwrap_or("/usr/local/share:/usr/share");
    for d in dirs.split(':').filter(|d| !d.is_empty()) {
        let p = PathBuf::from(d).join("help");
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

pub fn help_dirs() -> Vec<PathBuf> {
    let get = |k: &str| std::env::var(k).ok();
    help_dirs_from(
        get("XDG_DATA_HOME").as_deref(),
        get("XDG_DATA_DIRS").as_deref(),
        get("HOME").as_deref(),
    )
}

/// Detects a document's format in a directory. `id` is the document name
/// (used for `<id>.xml`).
pub fn detect_dir(dir: &Path, id: &str) -> Option<Found> {
    let candidates = [
        ("index.page".to_string(), Format::Mallard),
        ("index.docbook".to_string(), Format::DocBook),
        (format!("{id}.xml"), Format::DocBook),
        ("index.html".to_string(), Format::Html),
    ];
    candidates.into_iter().find_map(|(name, format)| {
        let entry = dir.join(name);
        entry.is_file().then(|| Found {
            dir: dir.to_path_buf(),
            entry,
            format,
        })
    })
}

/// Detects a document from a user-supplied directory or file.
pub fn detect_path(path: &Path) -> Option<Found> {
    if path.is_dir() {
        let id = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return detect_dir(path, &id).or_else(|| {
            // A Mallard directory without index.page is still browsable.
            let has_pages = std::fs::read_dir(path)
                .ok()?
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "page"));
            has_pages.then(|| Found {
                dir: path.to_path_buf(),
                entry: path.join("index.page"),
                format: Format::Mallard,
            })
        });
    }
    if !path.is_file() {
        return None;
    }
    let dir = path.parent()?.to_path_buf();
    let format = match path.extension()?.to_str()? {
        "page" => Format::Mallard,
        "docbook" | "xml" => Format::DocBook,
        "html" | "htm" => Format::Html,
        _ => return None,
    };
    let entry = if format == Format::Mallard {
        dir.join("index.page")
    } else {
        path.to_path_buf()
    };
    Some(Found { dir, entry, format })
}

/// Resolves document `id`, trying each help dir and, within it, each locale
/// in order. Locales take precedence over directories, so a translation in
/// `/usr/share` beats an untranslated copy in `~/.local/share`.
pub fn find(id: &str, dirs: &[PathBuf], locales: &[String]) -> Option<Found> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return None;
    }
    for loc in locales {
        for d in dirs {
            if let Some(f) = detect_dir(&d.join(loc).join(id), id) {
                return Some(f);
            }
        }
    }
    None
}

/// Every document id available in any of the locales.
pub fn list_ids(dirs: &[PathBuf], locales: &[String]) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for d in dirs {
        for loc in locales {
            if let Ok(rd) = std::fs::read_dir(d.join(loc)) {
                for e in rd.flatten() {
                    if e.path().is_dir() {
                        ids.insert(e.file_name().to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    ids.into_iter()
        .filter(|id| find(id, dirs, locales).is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("helix-help-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dirs_from_env() {
        assert_eq!(
            help_dirs_from(Some("/h/.local/share"), Some("/a:/b/"), None),
            [
                PathBuf::from("/h/.local/share/help"),
                "/a/help".into(),
                "/b/help".into()
            ]
        );
        assert_eq!(
            help_dirs_from(None, None, Some("/home/u")),
            [
                PathBuf::from("/home/u/.local/share/help"),
                "/usr/local/share/help".into(),
                "/usr/share/help".into()
            ]
        );
    }

    #[test]
    fn locale_fallback() {
        let root = tmp("loc");
        let user = root.join("user/help");
        let sys = root.join("sys/help");
        fs::create_dir_all(sys.join("C/app")).unwrap();
        fs::write(sys.join("C/app/index.page"), "<page/>").unwrap();
        fs::create_dir_all(sys.join("de/app")).unwrap();
        fs::write(sys.join("de/app/index.page"), "<page/>").unwrap();
        fs::create_dir_all(user.join("C/app")).unwrap();
        fs::write(user.join("C/app/index.docbook"), "<book/>").unwrap();
        fs::create_dir_all(sys.join("C/old")).unwrap();
        fs::write(sys.join("C/old/old.xml"), "<book/>").unwrap();
        fs::create_dir_all(sys.join("C/web")).unwrap();
        fs::write(sys.join("C/web/index.html"), "<html/>").unwrap();
        fs::create_dir_all(sys.join("C/empty")).unwrap();
        let dirs = vec![user.clone(), sys.clone()];

        let de: Vec<String> = ["de_AT", "de", "C"].map(String::from).to_vec();
        let f = find("app", &dirs, &de).unwrap();
        assert_eq!(f.dir, sys.join("de/app"));
        assert_eq!(f.format, Format::Mallard);

        let c = vec!["fr".to_string(), "C".to_string()];
        let f = find("app", &dirs, &c).unwrap();
        assert_eq!(
            f.dir,
            user.join("C/app"),
            "user dir wins within the same locale"
        );
        assert_eq!(f.format, Format::DocBook);

        assert_eq!(
            find("old", &dirs, &c).unwrap().entry,
            sys.join("C/old/old.xml")
        );
        assert_eq!(find("web", &dirs, &c).unwrap().format, Format::Html);
        assert!(find("empty", &dirs, &c).is_none());
        assert!(find("../C/app", &dirs, &c).is_none());
        assert_eq!(list_ids(&dirs, &c), ["app", "old", "web"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn detect_user_paths() {
        let root = tmp("paths");
        fs::write(root.join("index.page"), "<page/>").unwrap();
        fs::write(root.join("a.page"), "<page/>").unwrap();
        fs::write(root.join("manual.docbook"), "<book/>").unwrap();
        assert_eq!(detect_path(&root).unwrap().format, Format::Mallard);
        assert_eq!(
            detect_path(&root.join("a.page")).unwrap().entry,
            root.join("index.page")
        );
        assert_eq!(
            detect_path(&root.join("manual.docbook")).unwrap().entry,
            root.join("manual.docbook")
        );
        assert!(detect_path(&root.join("missing")).is_none());
        let _ = fs::remove_dir_all(root);
    }
}
