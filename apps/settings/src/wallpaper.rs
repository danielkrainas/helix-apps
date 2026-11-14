//! Wallpaper discovery and light/dark pairing.

use std::path::{Path, PathBuf};

pub const SYSTEM_DIR: &str = "/usr/share/backgrounds";

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "svg", "jxl", "tif", "tiff", "bmp", "gif",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallpaper {
    pub light: PathBuf,
    pub dark: Option<PathBuf>,
}

impl Wallpaper {
    pub fn dark_or_light(&self) -> &Path {
        self.dark.as_deref().unwrap_or(&self.light)
    }
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Splits `name-dark` / `name-light` / `name-d` / `name-l` into
/// `(base, Some(is_dark))`; other stems give `(stem, None)`.
fn variant_of(stem: &str) -> (&str, Option<bool>) {
    for (suffix, dark) in [
        ("-dark", true),
        ("-light", false),
        ("-d", true),
        ("-l", false),
        ("_dark", true),
        ("_light", false),
    ] {
        if let Some(base) = stem.strip_suffix(suffix) {
            if !base.is_empty() {
                return (base, Some(dark));
            }
        }
    }
    (stem, None)
}

/// Groups image paths into wallpapers, pairing light/dark variants that share
/// a directory, base name and extension. Output is sorted by light path.
pub fn pair(paths: &[PathBuf]) -> Vec<Wallpaper> {
    use std::collections::BTreeMap;
    // key: (dir, base, ext) → (light, dark)
    type Slot = (Option<PathBuf>, Option<PathBuf>);
    let mut groups: BTreeMap<(PathBuf, String, String), Slot> = BTreeMap::new();
    let mut singles = Vec::new();
    for p in paths.iter().filter(|p| is_image(p)) {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
        match variant_of(stem) {
            (base, Some(dark)) => {
                let slot = groups.entry((dir, base.to_string(), ext)).or_default();
                if dark {
                    slot.1 = Some(p.clone());
                } else {
                    slot.0 = Some(p.clone());
                }
            }
            (_, None) => singles.push(Wallpaper {
                light: p.clone(),
                dark: None,
            }),
        }
    }
    for (_, (light, dark)) in groups {
        match (light, dark) {
            (Some(l), d) => singles.push(Wallpaper { light: l, dark: d }),
            (None, Some(d)) => singles.push(Wallpaper {
                light: d,
                dark: None,
            }),
            (None, None) => {}
        }
    }
    singles.sort_by(|a, b| a.light.cmp(&b.light));
    singles
}

/// Lists image files in `dir` and its direct subdirectories.
pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&p) {
                out.extend(
                    sub.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_file() && is_image(p)),
                );
            }
        } else if is_image(&p) {
            out.push(p);
        }
    }
    out
}

/// Whether `wp` is the wallpaper currently set (by either URI).
pub fn is_current(wp: &Wallpaper, light_path: Option<&Path>, dark_path: Option<&Path>) -> bool {
    let light = light_path.is_some_and(|p| p == wp.light);
    let dark = dark_path.is_some_and(|p| p == wp.dark_or_light());
    light || (dark && light_path.is_none())
}

/// A destination in `dir` named like `name` that does not exist yet:
/// `pic.jpg`, `pic-1.jpg`, `pic-2.jpg`, …
pub fn unique_destination(dir: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let candidate = dir.join(name);
    if !exists(&candidate) {
        return candidate;
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("picture");
    let ext = p.extension().and_then(|s| s.to_str());
    (1..)
        .map(|i| match ext {
            Some(e) => dir.join(format!("{stem}-{i}.{e}")),
            None => dir.join(format!("{stem}-{i}")),
        })
        .find(|c| !exists(c))
        .expect("unbounded search")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn images() {
        assert!(is_image(&p("/a/b.JPG")));
        assert!(is_image(&p("/a/b.webp")));
        assert!(!is_image(&p("/a/b.xml")));
        assert!(!is_image(&p("/a/b")));
    }

    #[test]
    fn pairing() {
        let ws = pair(&[
            p("/bg/fuwa-dark.png"),
            p("/bg/fuwa-light.png"),
            p("/bg/clouds.jpg"),
            p("/bg/adwaita-d.jxl"),
            p("/bg/adwaita-l.jxl"),
            p("/bg/lonely-dark.png"),
            p("/bg/readme.xml"),
            p("/bg/mixed-dark.png"),
            p("/bg/mixed-light.jpg"),
        ]);
        assert!(ws.contains(&Wallpaper {
            light: p("/bg/fuwa-light.png"),
            dark: Some(p("/bg/fuwa-dark.png"))
        }));
        assert!(ws.contains(&Wallpaper {
            light: p("/bg/adwaita-l.jxl"),
            dark: Some(p("/bg/adwaita-d.jxl"))
        }));
        assert!(ws.contains(&Wallpaper {
            light: p("/bg/clouds.jpg"),
            dark: None
        }));
        assert!(ws.contains(&Wallpaper {
            light: p("/bg/lonely-dark.png"),
            dark: None
        }));
        // Different extensions are not paired.
        assert!(ws.contains(&Wallpaper {
            light: p("/bg/mixed-light.jpg"),
            dark: None
        }));
        assert!(ws.contains(&Wallpaper {
            light: p("/bg/mixed-dark.png"),
            dark: None
        }));
        assert_eq!(ws.len(), 6);
        let mut sorted = ws.clone();
        sorted.sort_by(|a, b| a.light.cmp(&b.light));
        assert_eq!(ws, sorted);
    }

    #[test]
    fn current_detection() {
        let wp = Wallpaper {
            light: p("/a-l.png"),
            dark: Some(p("/a-d.png")),
        };
        assert!(is_current(&wp, Some(&p("/a-l.png")), None));
        assert!(is_current(
            &wp,
            Some(&p("/a-l.png")),
            Some(&p("/other.png"))
        ));
        assert!(!is_current(&wp, Some(&p("/b.png")), Some(&p("/a-d.png"))));
        assert!(is_current(&wp, None, Some(&p("/a-d.png"))));
        let single = Wallpaper {
            light: p("/s.png"),
            dark: None,
        };
        assert!(is_current(&single, None, Some(&p("/s.png"))));
        assert!(!is_current(&single, None, None));
    }

    #[test]
    fn unique_names() {
        let taken = [p("/d/pic.jpg"), p("/d/pic-1.jpg")];
        let exists = |c: &Path| taken.iter().any(|t| t == c);
        assert_eq!(
            unique_destination(Path::new("/d"), "new.jpg", exists),
            p("/d/new.jpg")
        );
        assert_eq!(
            unique_destination(Path::new("/d"), "pic.jpg", exists),
            p("/d/pic-2.jpg")
        );
    }

    #[test]
    fn scan_temp_dir() {
        let dir = std::env::temp_dir().join(format!("helix-settings-wp-{}", std::process::id()));
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join("a.png"), b"").unwrap();
        std::fs::write(dir.join("b.txt"), b"").unwrap();
        std::fs::write(sub.join("c.jpg"), b"").unwrap();
        let mut found = scan(&dir);
        found.sort();
        assert_eq!(found, vec![dir.join("a.png"), sub.join("c.jpg")]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(scan(&dir).is_empty());
    }
}
