//! Default apps and removable media choices (pure logic; the panel
//! applies them through GIO and `org.gnome.desktop.media-handling`).

use helix_app_common::i18n::n_;

/// A kind of default app: its name and the content types it is the
/// default for (the first one decides what is shown).
pub struct Category {
    pub label: &'static str,
    pub types: &'static [&'static str],
}

pub static CATEGORIES: [Category; 6] = [
    Category {
        label: n_("Web"),
        types: &[
            "x-scheme-handler/http",
            "x-scheme-handler/https",
            "text/html",
            "application/xhtml+xml",
        ],
    },
    Category {
        label: n_("Mail"),
        types: &["x-scheme-handler/mailto"],
    },
    Category {
        label: n_("Calendar"),
        types: &["text/calendar"],
    },
    Category {
        label: n_("Music"),
        types: &[
            "audio/mpeg",
            "audio/x-vorbis+ogg",
            "audio/flac",
            "audio/x-flac",
            "audio/mp4",
            "audio/x-wav",
            "audio/ogg",
        ],
    },
    Category {
        label: n_("Video"),
        types: &[
            "video/mp4",
            "video/x-matroska",
            "video/webm",
            "video/x-msvideo",
            "video/quicktime",
            "video/ogg",
        ],
    },
    Category {
        label: n_("Photos"),
        types: &[
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "image/tiff",
            "image/bmp",
        ],
    },
];

/// Kinds of removable media and their `x-content` types.
pub const MEDIA: [(&str, &str); 5] = [
    ("x-content/audio-cdda", n_("CD Audio")),
    ("x-content/video-dvd", n_("DVD Video")),
    ("x-content/audio-player", n_("Music Player")),
    ("x-content/image-dcf", n_("Photos")),
    ("x-content/unix-software", n_("Software")),
];

/// What happens when a medium of one kind is inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Ask,
    Nothing,
    OpenFolder,
    /// Start this app (desktop id); GIO's default for the type.
    App(String),
}

/// The three lists of `org.gnome.desktop.media-handling`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lists {
    pub start_app: Vec<String>,
    pub ignore: Vec<String>,
    pub open_folder: Vec<String>,
}

impl Lists {
    /// What the lists say for `kind`; `default_app` is GIO's default for it.
    pub fn action(&self, kind: &str, default_app: Option<&str>) -> Action {
        if self.ignore.iter().any(|k| k == kind) {
            Action::Nothing
        } else if self.open_folder.iter().any(|k| k == kind) {
            Action::OpenFolder
        } else if let (true, Some(app)) = (self.start_app.iter().any(|k| k == kind), default_app) {
            Action::App(app.to_owned())
        } else {
            Action::Ask
        }
    }

    /// Records `action` for `kind` (in at most one list).
    pub fn set(&mut self, kind: &str, action: &Action) {
        for l in [&mut self.start_app, &mut self.ignore, &mut self.open_folder] {
            l.retain(|k| k != kind);
        }
        match action {
            Action::Ask => {}
            Action::Nothing => self.ignore.push(kind.into()),
            Action::OpenFolder => self.open_folder.push(kind.into()),
            Action::App(_) => self.start_app.push(kind.into()),
        }
    }
}

/// Content types (`x-scheme-handler/…` left out) shown as "Opens: …".
pub fn file_types(types: &[String]) -> Vec<&str> {
    types
        .iter()
        .map(String::as_str)
        .filter(|t| !t.starts_with("x-scheme-handler/") && !t.starts_with("x-content/"))
        .collect()
}

/// Link schemes (`https`, `mailto`…) an app handles.
pub fn schemes(types: &[String]) -> Vec<&str> {
    types
        .iter()
        .filter_map(|t| t.strip_prefix("x-scheme-handler/"))
        .collect()
}

/// The desktop id of the app whose search provider file names it, from
/// the `.ini` files of `search-providers` directories.
pub fn search_provider_ids(dirs: &[std::path::PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(entries) = std::fs::read_dir(d.join("gnome-shell/search-providers")) else {
            continue;
        };
        for e in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(e.path()) {
                if let Some(id) = text
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("DesktopId="))
                {
                    out.push(id.trim().to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Bytes as "12.3 MB".
pub fn size_label(bytes: u64) -> String {
    gtk::glib::format_size(bytes).to_string()
}

/// The total size of the files under `dir` (links not followed).
pub fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// A Flatpak app's installation, from its exported desktop file
/// (`<installation>/exports/share/applications/<id>.desktop`).
#[derive(Debug, Clone, PartialEq)]
pub struct FlatpakApp {
    pub id: String,
    /// The installation (`~/.local/share/flatpak` or `/var/lib/flatpak`).
    pub installation: std::path::PathBuf,
    /// Whether it is the user's own installation (else the system's).
    pub user: bool,
}

impl FlatpakApp {
    pub fn from_desktop_file(path: &std::path::Path, home: &std::path::Path) -> Option<Self> {
        let text = path.to_str()?;
        let (root, rest) = text.split_once("/exports/share/applications/")?;
        let id = rest.strip_suffix(".desktop")?.to_string();
        if id.is_empty() || id.contains('/') {
            return None;
        }
        let installation = std::path::PathBuf::from(root);
        Some(FlatpakApp {
            user: installation.starts_with(home),
            installation,
            id,
        })
    }

    /// The deployed files of the active version (`app/<id>/current/active`,
    /// links resolved), whose size is the app's installed size.
    pub fn deploy_dir(&self) -> Option<std::path::PathBuf> {
        self.installation
            .join("app")
            .join(&self.id)
            .join("current/active")
            .canonicalize()
            .ok()
    }

    /// The command that uninstalls it from its installation (the system
    /// installation asks for authorization through polkit).
    pub fn uninstall_command(&self) -> Vec<String> {
        vec![
            "flatpak".into(),
            "uninstall".into(),
            "--noninteractive".into(),
            if self.user { "--user" } else { "--system" }.into(),
            self.id.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_apps() {
        let home = std::path::Path::new("/home/u");
        let user = FlatpakApp::from_desktop_file(
            std::path::Path::new(
                "/home/u/.local/share/flatpak/exports/share/applications/org.x.App.desktop",
            ),
            home,
        )
        .unwrap();
        assert_eq!(user.id, "org.x.App");
        assert!(user.user);
        assert_eq!(
            user.uninstall_command(),
            [
                "flatpak",
                "uninstall",
                "--noninteractive",
                "--user",
                "org.x.App"
            ]
        );
        let system = FlatpakApp::from_desktop_file(
            std::path::Path::new("/var/lib/flatpak/exports/share/applications/org.y.App.desktop"),
            home,
        )
        .unwrap();
        assert!(!system.user);
        assert_eq!(
            system.installation,
            std::path::Path::new("/var/lib/flatpak")
        );
        assert_eq!(system.uninstall_command()[3], "--system");
        assert!(FlatpakApp::from_desktop_file(
            std::path::Path::new("/usr/share/applications/org.z.desktop"),
            home
        )
        .is_none());

        // The installed size counts the active deployment, through the
        // installation's links.
        let root = std::env::temp_dir().join(format!("helix-flatpak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let deploy = root.join("app/org.x.App/x86_64/stable/abc123");
        std::fs::create_dir_all(deploy.join("files/bin")).unwrap();
        std::fs::write(deploy.join("files/bin/app"), [0u8; 100]).unwrap();
        std::os::unix::fs::symlink("x86_64/stable", root.join("app/org.x.App/current")).unwrap();
        std::os::unix::fs::symlink("abc123", root.join("app/org.x.App/x86_64/stable/active"))
            .unwrap();
        let app = FlatpakApp {
            id: "org.x.App".into(),
            installation: root.clone(),
            user: true,
        };
        let dir = app.deploy_dir().unwrap();
        assert_eq!(dir_size(&dir), 100);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn media_lists() {
        let mut l = Lists {
            start_app: vec!["x-content/audio-cdda".into()],
            ignore: vec!["x-content/video-dvd".into()],
            open_folder: vec![],
        };
        assert_eq!(
            l.action("x-content/audio-cdda", Some("player.desktop")),
            Action::App("player.desktop".into())
        );
        assert_eq!(
            l.action("x-content/audio-cdda", None),
            Action::Ask,
            "no app left: ask"
        );
        assert_eq!(l.action("x-content/video-dvd", None), Action::Nothing);
        assert_eq!(l.action("x-content/image-dcf", None), Action::Ask);
        l.set("x-content/video-dvd", &Action::OpenFolder);
        l.set("x-content/audio-cdda", &Action::Ask);
        assert_eq!(
            l,
            Lists {
                start_app: vec![],
                ignore: vec![],
                open_folder: vec!["x-content/video-dvd".into()],
            }
        );
    }

    #[test]
    fn types_and_providers() {
        let t: Vec<String> = [
            "text/plain",
            "x-scheme-handler/https",
            "x-content/audio-cdda",
            "x-scheme-handler/mailto",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(file_types(&t), ["text/plain"]);
        assert_eq!(schemes(&t), ["https", "mailto"]);
        let dir = std::env::temp_dir().join(format!("helix-sp-{}", std::process::id()));
        let sp = dir.join("gnome-shell/search-providers");
        std::fs::create_dir_all(&sp).unwrap();
        std::fs::write(
            sp.join("a.ini"),
            "[Shell Search Provider]\nDesktopId=org.x.Files.desktop\nBusName=x\n",
        )
        .unwrap();
        std::fs::write(sp.join("b.ini"), "junk").unwrap();
        assert_eq!(
            search_provider_ids(std::slice::from_ref(&dir)),
            ["org.x.Files.desktop"]
        );
        std::fs::write(dir.join("f"), [0u8; 10]).unwrap();
        assert_eq!(
            dir_size(&dir),
            10 + std::fs::metadata(sp.join("a.ini")).unwrap().len() + 4
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
