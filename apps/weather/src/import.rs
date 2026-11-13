//! Best-effort, read-only import of GNOME Weather's saved locations on first
//! run. GNOME Weather stores `org.gnome.Weather locations` (`av`) as
//! GWeather-serialized locations: each item is `(uv)`, version 2 being
//! `(name, station code, is-city, [(lat, lon)] city, [(lat, lon)] station)`
//! with radians (`(ssba(dd)a(dd))`); version 1 used maybes instead of arrays
//! (`(ssbm(dd)m(dd))`).

use gtk::gio::prelude::*;
use gtk::glib::{self, Variant};
use std::path::{Path, PathBuf};

use crate::gwdb;
use crate::store::Place;

type V2 = (String, String, bool, Vec<(f64, f64)>, Vec<(f64, f64)>);
type V1 = (String, String, bool, Option<(f64, f64)>, Option<(f64, f64)>);

/// One serialized location (`v` holding `(uv)`, or `(uv)` itself).
pub fn place_from_serialized(v: &Variant, gw: Option<&gwdb::Db>) -> Option<Place> {
    let v = if v.type_() == glib::VariantTy::VARIANT { v.as_variant()? } else { v.clone() };
    let (version, inner) = v.get::<(u32, Variant)>()?;
    let (name, code, city, station) = match version {
        2 => {
            let (n, c, _, city, st) = inner.get::<V2>()?;
            (n, c, city.first().copied(), st.first().copied())
        }
        1 => {
            let (n, c, _, city, st) = inner.get::<V1>()?;
            (n, c, city, st)
        }
        _ => return None,
    };
    let (lat, lon) = city.or(station).map(|(a, b)| (a.to_degrees(), b.to_degrees()))?;
    if !(lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
        return None;
    }
    let st = gw.and_then(|g| g.station(&code));
    let name = if name.trim().is_empty() { st.map(|s| s.name.clone())? } else { name };
    Some(Place {
        admin1: st.and_then(|s| s.state.clone()),
        country: st.and_then(|s| s.country.clone()),
        timezone: st.and_then(|s| s.tz.clone()),
        ..Place::at(name, lat, lon)
    })
}

/// All usable places of an `av` value, without duplicates.
pub fn places_from_list(v: &Variant, gw: Option<&gwdb::Db>) -> Vec<Place> {
    let mut out: Vec<Place> = Vec::new();
    if v.type_().as_str() != "av" {
        return out;
    }
    for i in 0..v.n_children() {
        if let Some(p) = place_from_serialized(&v.child_value(i), gw) {
            if !out.iter().any(|o| o.same_as(&p)) {
                out.push(p);
            }
        }
    }
    out
}

/// Parses an `av` from GVariant text (dconf / keyfile syntax).
pub fn parse_text(text: &str) -> Option<Variant> {
    Variant::parse(Some(glib::VariantTy::new("av").ok()?), text.trim()).ok()
}

/// The `locations` value of a GSettings keyfile (`[org/gnome/Weather]`).
pub fn from_keyfile(path: &Path, group: &str) -> Option<Variant> {
    let kf = glib::KeyFile::new();
    kf.load_from_file(path, glib::KeyFileFlags::NONE).ok()?;
    parse_text(&kf.value(group, "locations").ok()?)
}

fn from_schema(id: &str) -> Option<Variant> {
    let s = helix_app_common::settings(id)?;
    s.settings_schema()?.has_key("locations").then(|| s.value("locations"))
}

fn from_dconf() -> Option<Variant> {
    let out = std::process::Command::new("dconf").args(["read", "/org/gnome/Weather/locations"]).output().ok()?;
    out.status.success().then_some(())?;
    parse_text(&String::from_utf8_lossy(&out.stdout))
}

fn flatpak_keyfile() -> PathBuf {
    glib::home_dir().join(".var/app/org.gnome.Weather/config/glib-2.0/settings/keyfile")
}

/// Reads GNOME Weather's locations (never writes): the `org.gnome.Weather`
/// schema, the dconf database (works without the schema), the Flatpak
/// app's keyfile, and finally GNOME Shell's copy
/// (`org.gnome.shell.weather locations`). The first non-empty source wins.
pub fn import(gw: Option<&gwdb::Db>) -> Vec<Place> {
    let sources: [&dyn Fn() -> Option<Variant>; 4] = [
        &|| from_schema("org.gnome.Weather"),
        &from_dconf,
        &|| from_keyfile(&flatpak_keyfile(), "org/gnome/Weather"),
        &|| from_schema("org.gnome.shell.weather"),
    ];
    for src in sources {
        if let Some(v) = src() {
            let places = places_from_list(&v, gw);
            if !places.is_empty() {
                return places;
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GNOME: &str = "[<(uint32 2, <('Munich', 'EDDM', true, [(0.8403, 0.2021)], [(0.8438, 0.2056)])>)>, \
        <(uint32 2, <('', 'EDDM', false, @a(dd) [], [(0.8438, 0.2056)])>)>, \
        <(uint32 1, <('Old', 'KORD', true, @m(dd) nothing, just (0.7327, -1.5344))>)>, \
        <(uint32 9, <'future'>)>, <'junk'>]";

    #[test]
    fn parses_gnome_list() {
        let gw = gwdb::Db::parse(gwdb::FIXTURE).unwrap();
        let v = parse_text(GNOME).unwrap();
        let places = places_from_list(&v, Some(&gw));
        // The station-only entry has other coordinates, so it is kept.
        assert_eq!(places.len(), 3);
        let m = &places[0];
        assert_eq!(m.name, "Munich");
        assert!((m.latitude - 48.146).abs() < 0.01 && (m.longitude - 11.579).abs() < 0.01);
        assert_eq!(m.country.as_deref(), Some("Germany"));
        assert_eq!(m.admin1.as_deref(), Some("Bavaria"));
        assert_eq!(m.timezone.as_deref(), Some("Europe/Berlin"));
        assert_eq!(places[1].name, "Munich", "empty name falls back to the station");
        assert_eq!(places[2].name, "Old");
        assert_eq!(places[2].timezone.as_deref(), Some("America/Chicago"));
        assert!(!places.iter().any(|p| p.automatic));
    }

    #[test]
    fn without_database_and_junk() {
        let v = parse_text(GNOME).unwrap();
        let places = places_from_list(&v, None);
        assert_eq!(places.len(), 2, "the nameless entry needs the database");
        assert!(places[0].country.is_none());
        assert!(parse_text("not a variant").is_none());
        assert!(places_from_list(&"x".to_variant(), None).is_empty());
    }

    #[test]
    fn keyfile_source() {
        let dir = std::env::temp_dir().join(format!("helix-weather-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keyfile");
        std::fs::write(&path, format!("[org/gnome/Weather]\nlocations={GNOME}\n")).unwrap();
        let v = from_keyfile(&path, "org/gnome/Weather").unwrap();
        assert_eq!(places_from_list(&v, None).len(), 2);
        assert!(from_keyfile(&dir.join("missing"), "org/gnome/Weather").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
