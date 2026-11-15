//! Per-origin permission decisions.

use helix_app_common::i18n::tr;
use helix_app_common::trf;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Geolocation,
    Notifications,
    Microphone,
    Camera,
    Clipboard,
    /// "Never save passwords for this site" (only ever stored as deny).
    SavePassword,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Geolocation => tr("Location"),
            Kind::Notifications => tr("Notifications"),
            Kind::Microphone => tr("Microphone"),
            Kind::Camera => tr("Camera"),
            Kind::Clipboard => tr("Clipboard"),
            Kind::SavePassword => tr("Save Passwords"),
        }
    }

    /// Maps a Permissions API name (`query-permission-state`) to a kind.
    pub fn from_api_name(name: &str) -> Option<Kind> {
        Some(match name {
            "geolocation" => Kind::Geolocation,
            "notifications" | "push" => Kind::Notifications,
            "microphone" => Kind::Microphone,
            "camera" => Kind::Camera,
            "clipboard-read" | "clipboard" => Kind::Clipboard,
            _ => return None,
        })
    }
}

/// Builds the "<host> wants to …" sentence for a request.
pub fn request_text(host: &str, kinds: &[Kind]) -> String {
    let what = match kinds {
        [Kind::Geolocation] => "know your location".to_string(),
        [Kind::Notifications] => "show notifications".to_string(),
        [Kind::Clipboard] => "read your clipboard".to_string(),
        k if k.contains(&Kind::Camera) && k.contains(&Kind::Microphone) => {
            "use your camera and microphone".to_string()
        }
        [Kind::Camera] => "use your camera".to_string(),
        [Kind::Microphone] => "use your microphone".to_string(),
        _ => "use additional permissions".to_string(),
    };
    trf!("{host} wants to {what}", host = host, what = what)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    origins: BTreeMap<String, BTreeMap<Kind, bool>>,
}

pub struct Permissions {
    path: Option<PathBuf>,
    origins: BTreeMap<String, BTreeMap<Kind, bool>>,
}

impl Permissions {
    #[cfg(test)]
    pub fn memory() -> Self {
        Permissions {
            path: None,
            origins: BTreeMap::new(),
        }
    }

    pub fn load(path: &Path) -> Self {
        let f: File = crate::store::load(path);
        Permissions {
            path: Some(path.to_owned()),
            origins: f.origins,
        }
    }

    pub fn serialize(&self) -> Option<(PathBuf, String)> {
        let path = self.path.clone()?;
        Some((
            path,
            crate::store::to_json(&File {
                origins: self.origins.clone(),
            }),
        ))
    }

    pub fn get(&self, origin: &str, kind: Kind) -> Decision {
        match self.origins.get(origin).and_then(|m| m.get(&kind)) {
            Some(true) => Decision::Allow,
            Some(false) => Decision::Deny,
            None => Decision::Ask,
        }
    }

    /// Combined decision for a multi-kind request: any deny → deny; all
    /// allowed → allow; otherwise ask.
    pub fn get_all(&self, origin: &str, kinds: &[Kind]) -> Decision {
        let ds: Vec<Decision> = kinds.iter().map(|&k| self.get(origin, k)).collect();
        if ds.contains(&Decision::Deny) {
            Decision::Deny
        } else if !ds.is_empty() && ds.iter().all(|&d| d == Decision::Allow) {
            Decision::Allow
        } else {
            Decision::Ask
        }
    }

    pub fn set(&mut self, origin: &str, kind: Kind, allow: bool) {
        self.origins
            .entry(origin.to_string())
            .or_default()
            .insert(kind, allow);
    }

    pub fn reset(&mut self, origin: &str, kind: Kind) {
        if let Some(m) = self.origins.get_mut(origin) {
            m.remove(&kind);
            if m.is_empty() {
                self.origins.remove(origin);
            }
        }
    }

    pub fn for_origin(&self, origin: &str) -> Vec<(Kind, bool)> {
        self.origins
            .get(origin)
            .map(|m| m.iter().map(|(&k, &v)| (k, v)).collect())
            .unwrap_or_default()
    }

    pub fn clear(&mut self) {
        self.origins.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.origins.values().all(|m| m.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions() {
        let mut p = Permissions::memory();
        let o = "https://maps.example";
        assert_eq!(p.get(o, Kind::Geolocation), Decision::Ask);
        p.set(o, Kind::Geolocation, true);
        p.set(o, Kind::Camera, false);
        assert_eq!(p.get(o, Kind::Geolocation), Decision::Allow);
        assert_eq!(p.get("https://other", Kind::Geolocation), Decision::Ask);
        assert_eq!(
            p.get_all(o, &[Kind::Camera, Kind::Microphone]),
            Decision::Deny
        );
        assert_eq!(p.get_all(o, &[Kind::Microphone]), Decision::Ask);
        p.set(o, Kind::Microphone, true);
        p.set(o, Kind::Camera, true);
        assert_eq!(
            p.get_all(o, &[Kind::Camera, Kind::Microphone]),
            Decision::Allow
        );
        assert_eq!(p.for_origin(o).len(), 3);
        p.reset(o, Kind::Camera);
        assert_eq!(p.get(o, Kind::Camera), Decision::Ask);
        p.clear();
        assert!(p.for_origin(o).is_empty());
    }

    #[test]
    fn persists() {
        let dir = crate::store::temp_dir("perms");
        let path = dir.join("permissions.json");
        let mut p = Permissions::load(&path);
        p.set("https://a", Kind::Notifications, false);
        p.set("https://a", Kind::Clipboard, true);
        let (f, s) = p.serialize().unwrap();
        crate::store::write_atomic(&f, &s).unwrap();
        let q = Permissions::load(&path);
        assert_eq!(q.get("https://a", Kind::Notifications), Decision::Deny);
        assert_eq!(q.get("https://a", Kind::Clipboard), Decision::Allow);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn texts_and_names() {
        assert_eq!(
            request_text("a.org", &[Kind::Geolocation]),
            "a.org wants to know your location"
        );
        assert_eq!(
            request_text("a.org", &[Kind::Microphone, Kind::Camera]),
            "a.org wants to use your camera and microphone"
        );
        assert_eq!(Kind::from_api_name("camera"), Some(Kind::Camera));
        assert_eq!(Kind::from_api_name("midi"), None);
    }
}
