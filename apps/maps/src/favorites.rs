//! Favorite places, stored as JSON in the data dir.

use crate::nominatim::Place;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Favorites {
    path: Option<PathBuf>,
    pub places: Vec<Place>,
}

impl Favorites {
    pub fn load(path: &Path) -> Favorites {
        let places = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Place>>(&t).ok())
            .unwrap_or_default();
        Favorites { path: Some(path.to_path_buf()), places }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Ok(json) = serde_json::to_string_pretty(&self.places) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    pub fn contains(&self, place: &Place) -> bool {
        let key = place.key();
        self.places.iter().any(|p| p.key() == key)
    }

    /// Adds or removes `place`; returns whether it is now a favorite.
    pub fn toggle(&mut self, place: &Place) -> bool {
        let key = place.key();
        let now = if let Some(i) = self.places.iter().position(|p| p.key() == key) {
            self.places.remove(i);
            false
        } else {
            let mut p = place.clone();
            p.extras.clear();
            self.places.push(p);
            true
        };
        self.save();
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::LatLon;

    #[test]
    fn toggle_persists() {
        let dir = std::env::temp_dir().join(format!("helix-maps-fav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("favorites.json");
        let mut f = Favorites::load(&path);
        assert!(f.places.is_empty());
        let p = Place::at(LatLon { lat: 1.0, lon: 2.0 }, "Home");
        assert!(f.toggle(&p));
        assert!(f.contains(&p));
        let f2 = Favorites::load(&path);
        assert_eq!(f2.places.len(), 1);
        assert_eq!(f2.places[0].name, "Home");
        assert!(!f.toggle(&p));
        assert!(Favorites::load(&path).places.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
