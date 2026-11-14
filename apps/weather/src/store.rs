//! Saved locations and the on-disk forecast cache.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A geographic place (from geocoding or the saved list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub admin1: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub timezone: Option<String>,
    /// The automatically located place (at most one, kept first).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub automatic: bool,
}

impl Place {
    /// A place with just a name and coordinates.
    pub fn at(name: impl Into<String>, latitude: f64, longitude: f64) -> Place {
        Place {
            id: None,
            name: name.into(),
            admin1: None,
            country: None,
            country_code: None,
            latitude,
            longitude,
            timezone: None,
            automatic: false,
        }
    }

    /// "Region, Country" (whatever parts are known).
    pub fn subtitle(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if let Some(a) = self
            .admin1
            .as_deref()
            .filter(|a| !a.is_empty() && *a != self.name)
        {
            parts.push(a);
        }
        if let Some(c) = self.country.as_deref().filter(|c| !c.is_empty()) {
            parts.push(c);
        }
        parts.join(", ")
    }

    /// Whether two places denote the same spot (≈100 m).
    pub fn same_as(&self, other: &Place) -> bool {
        (self.latitude - other.latitude).abs() < 0.001
            && (self.longitude - other.longitude).abs() < 0.001
    }

    /// Stable cache key derived from the coordinates.
    pub fn cache_key(&self) -> String {
        format!("{:.3}_{:.3}", self.latitude, self.longitude)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Locations {
    #[serde(default)]
    pub selected: usize,
    #[serde(default)]
    pub locations: Vec<Place>,
    /// Mirrors GNOME Weather's `automatic-location`.
    #[serde(default)]
    pub automatic_location: bool,
}

impl Locations {
    pub fn selected_place(&self) -> Option<&Place> {
        self.locations
            .get(self.selected)
            .or_else(|| self.locations.first())
    }

    /// Adds `place` (unless already present) and selects it.
    pub fn add(&mut self, place: Place) {
        match self.locations.iter().position(|p| p.same_as(&place)) {
            Some(i) => self.selected = i,
            None => {
                self.locations.push(place);
                self.selected = self.locations.len() - 1;
            }
        }
    }

    /// Replaces the automatic place (kept at index 0) with `place`, or
    /// removes it. The selection follows the place it pointed at; a new
    /// automatic place is selected when nothing else was (empty list or the
    /// previous automatic place was selected).
    pub fn set_automatic(&mut self, place: Option<Place>) {
        let was_auto_selected = self
            .locations
            .get(self.selected)
            .is_some_and(|p| p.automatic);
        let had_auto = self.locations.first().is_some_and(|p| p.automatic);
        let was_empty = self.locations.iter().all(|p| p.automatic);
        if had_auto {
            self.locations.remove(0);
            if self.selected > 0 {
                self.selected -= 1;
            }
        }
        if let Some(mut p) = place {
            p.automatic = true;
            self.locations.insert(0, p);
            if was_auto_selected || was_empty {
                self.selected = 0;
            } else {
                self.selected += 1;
            }
        } else if was_auto_selected {
            self.selected = 0;
        }
        if self.selected >= self.locations.len() {
            self.selected = 0;
        }
    }

    /// Removes the location at `index`, keeping the selection sensible.
    pub fn remove(&mut self, index: usize) {
        if index >= self.locations.len() {
            return;
        }
        self.locations.remove(index);
        if self.selected == index {
            self.selected = 0;
        } else if self.selected > index {
            self.selected -= 1;
        }
        if self.selected >= self.locations.len() {
            self.selected = 0;
        }
    }

    pub fn load(path: &Path) -> Locations {
        let mut l: Locations = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if l.selected >= l.locations.len() {
            l.selected = 0;
        }
        l
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(path, &json)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// A raw forecast response with its fetch time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedForecast {
    pub fetched: i64,
    pub body: String,
}

pub fn cache_path(dir: &Path, place: &Place) -> PathBuf {
    dir.join(format!("forecast-{}.json", place.cache_key()))
}

pub fn load_cache(dir: &Path, place: &Place) -> Option<CachedForecast> {
    let bytes = std::fs::read(cache_path(dir, place)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save_cache(dir: &Path, place: &Place, entry: &CachedForecast) -> std::io::Result<()> {
    let json = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
    write_atomic(&cache_path(dir, place), &json)
}

pub fn remove_cache(dir: &Path, place: &Place) {
    let _ = std::fs::remove_file(cache_path(dir, place));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str, lat: f64, lon: f64) -> Place {
        Place {
            admin1: Some("Region".into()),
            country: Some("Country".into()),
            ..Place::at(name, lat, lon)
        }
    }

    #[test]
    fn automatic_place() {
        let mut l = Locations::default();
        l.set_automatic(Some(place("Here", 5.0, 5.0)));
        assert_eq!((l.selected, l.locations.len()), (0, 1));
        assert!(l.locations[0].automatic);
        l.add(place("A", 1.0, 1.0));
        assert_eq!(l.selected, 1);
        // Moving: the manual selection stays on A.
        l.set_automatic(Some(place("There", 6.0, 6.0)));
        assert_eq!(l.locations.len(), 2);
        assert_eq!(l.locations[0].name, "There");
        assert_eq!(l.selected_place().unwrap().name, "A");
        // Selected automatic place follows the move.
        l.selected = 0;
        l.set_automatic(Some(place("Elsewhere", 7.0, 7.0)));
        assert_eq!(l.selected_place().unwrap().name, "Elsewhere");
        // Turning it off.
        l.set_automatic(None);
        assert_eq!(l.locations.len(), 1);
        assert_eq!(l.selected_place().unwrap().name, "A");
        assert!(!l.locations.iter().any(|p| p.automatic));
        // Added while a manual place is selected and none was automatic.
        l.set_automatic(Some(place("Here", 5.0, 5.0)));
        assert_eq!(l.selected_place().unwrap().name, "A");
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(
            json.matches("\"automatic\":true").count(),
            1,
            "manual places omit the flag"
        );
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("helix-weather-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn subtitle() {
        assert_eq!(place("A", 0.0, 0.0).subtitle(), "Region, Country");
        let mut p = place("Berlin", 0.0, 0.0);
        p.admin1 = Some("Berlin".into());
        assert_eq!(p.subtitle(), "Country");
        p.country = None;
        assert_eq!(p.subtitle(), "");
    }

    #[test]
    fn add_remove_select() {
        let mut l = Locations::default();
        assert!(l.selected_place().is_none());
        l.add(place("A", 1.0, 1.0));
        l.add(place("B", 2.0, 2.0));
        l.add(place("C", 3.0, 3.0));
        assert_eq!(l.selected, 2);
        l.add(place("A again", 1.00001, 1.0));
        assert_eq!(l.locations.len(), 3);
        assert_eq!(l.selected, 0);
        l.selected = 2;
        l.remove(0);
        assert_eq!(l.selected, 1);
        assert_eq!(l.selected_place().unwrap().name, "C");
        l.remove(1);
        assert_eq!(l.selected, 0);
        assert_eq!(l.selected_place().unwrap().name, "B");
        l.remove(5);
        l.remove(0);
        assert!(l.locations.is_empty());
        assert_eq!(l.selected, 0);
    }

    #[test]
    fn locations_roundtrip() {
        let dir = temp_dir("loc");
        let path = dir.join("locations.json");
        assert_eq!(Locations::load(&path), Locations::default());
        let mut l = Locations::default();
        l.add(place("A", 1.0, 1.0));
        l.add(place("B", 2.5, -3.25));
        l.save(&path).unwrap();
        assert_eq!(Locations::load(&path), l);
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Locations::load(&path), Locations::default());
        std::fs::write(
            &path,
            br#"{"selected":7,"locations":[{"name":"X","latitude":1,"longitude":2}]}"#,
        )
        .unwrap();
        let l = Locations::load(&path);
        assert_eq!(l.selected, 0);
        assert_eq!(l.locations[0].name, "X");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cache_roundtrip() {
        let dir = temp_dir("cache");
        let p = place("A", 52.52437, 13.41053);
        assert!(load_cache(&dir, &p).is_none());
        let entry = CachedForecast {
            fetched: 1234,
            body: "{\"x\":1}".into(),
        };
        save_cache(&dir, &p, &entry).unwrap();
        assert_eq!(load_cache(&dir, &p), Some(entry));
        assert!(cache_path(&dir, &p).ends_with("forecast-52.524_13.411.json"));
        remove_cache(&dir, &p);
        assert!(load_cache(&dir, &p).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
