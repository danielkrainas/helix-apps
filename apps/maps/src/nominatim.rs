//! Nominatim-compatible geocoding: request URLs and response parsing.

use crate::geo::{percent_encode, LatLon};
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A place shown on the map (search result, favorite, reverse-geocoded
/// point or a bare coordinate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Place {
    pub name: String,
    pub address: String,
    pub lat: f64,
    pub lon: f64,
    pub category: String,
    pub osm_type: String,
    pub osm_id: u64,
    /// south, north, west, east
    pub bbox: Option<[f64; 4]>,
    #[serde(skip)]
    pub extras: Vec<(String, String)>,
}

impl Place {
    pub fn at(p: LatLon, name: &str) -> Place {
        Place {
            name: name.to_string(),
            lat: p.lat,
            lon: p.lon,
            ..Default::default()
        }
    }

    pub fn location(&self) -> LatLon {
        LatLon {
            lat: self.lat,
            lon: self.lon,
        }
    }

    /// Identity for favorites: OSM object if known, else rounded coordinates.
    pub fn key(&self) -> String {
        if !self.osm_type.is_empty() && self.osm_id != 0 {
            format!("{}/{}", self.osm_type, self.osm_id)
        } else {
            format!("{:.5},{:.5}", self.lat, self.lon)
        }
    }

    pub fn osm_url(&self) -> String {
        if !self.osm_type.is_empty() && self.osm_id != 0 {
            format!(
                "https://www.openstreetmap.org/{}/{}",
                self.osm_type, self.osm_id
            )
        } else {
            format!(
                "https://www.openstreetmap.org/?mlat={:.6}&mlon={:.6}#map=17/{:.6}/{:.6}",
                self.lat, self.lon, self.lat, self.lon
            )
        }
    }

    /// Short secondary line (address without the name).
    pub fn short_address(&self) -> String {
        let parts: Vec<&str> = self
            .address
            .split(", ")
            .filter(|p| *p != self.name)
            .collect();
        parts.into_iter().take(3).collect::<Vec<_>>().join(", ")
    }
}

/// Builds a forward-search URL. `viewbox` is (west, south, east, north).
pub fn search_url(
    base: &str,
    query: &str,
    limit: u32,
    viewbox: Option<(f64, f64, f64, f64)>,
) -> String {
    let mut url = format!(
        "{}/search?format=jsonv2&addressdetails=1&extratags=1&limit={}&q={}",
        base.trim_end_matches('/'),
        limit,
        percent_encode(query)
    );
    if let Some((w, s, e, n)) = viewbox {
        url.push_str(&format!("&viewbox={w:.5},{n:.5},{e:.5},{s:.5}"));
    }
    url
}

pub fn reverse_url(base: &str, p: LatLon, zoom: u32) -> String {
    format!(
        "{}/reverse?format=jsonv2&addressdetails=1&extratags=1&zoom={}&lat={:.6}&lon={:.6}",
        base.trim_end_matches('/'),
        zoom.min(18),
        p.lat,
        p.lon
    )
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn parse_place(v: &Value) -> Option<Place> {
    let lat = num(v.get("lat")?)?;
    let lon = num(v.get("lon")?)?;
    LatLon::new(lat, lon)?;
    let display = str_of(v, "display_name").to_string();
    let mut name = str_of(v, "name").to_string();
    if name.is_empty() {
        name = address_title(v.get("address"))
            .unwrap_or_else(|| display.split(", ").next().unwrap_or("").to_string());
    }
    let category = humanize_category(
        v.get("category")
            .or_else(|| v.get("class"))
            .and_then(Value::as_str)
            .unwrap_or(""),
        str_of(v, "type"),
    );
    let bbox = v
        .get("boundingbox")
        .and_then(Value::as_array)
        .and_then(|a| {
            if a.len() != 4 {
                return None;
            }
            let n: Vec<f64> = a.iter().filter_map(num).collect();
            (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
        });
    let osm_id = v
        .get("osm_id")
        .and_then(|i| i.as_u64().or_else(|| num(i).map(|f| f as u64)));
    let mut extras = Vec::new();
    if let Some(Value::Object(tags)) = v.get("extratags") {
        for (key, label) in [
            ("opening_hours", tr("Opening hours")),
            ("phone", tr("Phone")),
            ("contact:phone", tr("Phone")),
            ("website", tr("Website")),
            ("contact:website", tr("Website")),
            ("wikipedia", tr("Wikipedia")),
            ("cuisine", tr("Cuisine")),
            ("wheelchair", tr("Wheelchair access")),
            ("population", tr("Population")),
            ("ele", tr("Elevation")),
        ] {
            if let Some(val) = tags.get(key).and_then(Value::as_str) {
                if !extras.iter().any(|(l, _): &(String, String)| l == label) {
                    let val = match key {
                        "cuisine" => val.replace(['_', ';'], " "),
                        "ele" => trf!("{} m", val),
                        _ => val.to_string(),
                    };
                    extras.push((label.to_string(), val));
                }
            }
        }
    }
    Some(Place {
        name,
        address: display,
        lat,
        lon,
        category,
        osm_type: str_of(v, "osm_type").to_string(),
        osm_id: osm_id.unwrap_or(0),
        bbox,
        extras,
    })
}

/// Picks a title from an `address` object when the place has no name.
fn address_title(addr: Option<&Value>) -> Option<String> {
    let addr = addr?;
    let road = str_of(addr, "road");
    let number = str_of(addr, "house_number");
    if !road.is_empty() {
        return Some(if number.is_empty() {
            road.to_string()
        } else {
            format!("{road} {number}")
        });
    }
    ["village", "town", "city", "county", "state", "country"]
        .iter()
        .map(|k| str_of(addr, k))
        .find(|s| !s.is_empty())
        .map(String::from)
}

pub fn parse_search(json: &str) -> Result<Vec<Place>, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|_| tr("Invalid response from the search service").to_string())?;
    match v {
        Value::Array(items) => Ok(items.iter().filter_map(parse_place).collect()),
        Value::Object(_) if v.get("error").is_some() => Err(error_text(&v)),
        _ => Err(tr("Invalid response from the search service").into()),
    }
}

pub fn parse_reverse(json: &str) -> Result<Option<Place>, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|_| tr("Invalid response from the search service").to_string())?;
    if v.get("error").is_some() {
        // "Unable to geocode" means open sea etc.: not an error for the user.
        return Ok(None);
    }
    Ok(parse_place(&v))
}

fn error_text(v: &Value) -> String {
    match v.get("error") {
        Some(Value::String(s)) => s.clone(),
        Some(e) => e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(tr("Search failed"))
            .to_string(),
        None => tr("Search failed").into(),
    }
}

/// Turns an OSM class/type pair into a readable label.
pub fn humanize_category(class: &str, kind: &str) -> String {
    let special = match (class, kind) {
        ("railway", "station") => Some(tr("Railway station")),
        ("railway", "halt") => Some(tr("Railway stop")),
        ("highway", "bus_stop") => Some(tr("Bus stop")),
        ("amenity", "fuel") => Some(tr("Gas station")),
        ("amenity", "place_of_worship") => Some(tr("Place of worship")),
        ("boundary", "administrative") => Some(tr("Administrative area")),
        ("place", "house") => Some(tr("Address")),
        ("building", "yes") => Some(tr("Building")),
        ("highway", k) if !k.is_empty() => Some(tr("Road")),
        _ => None,
    };
    if let Some(s) = special {
        return s.to_string();
    }
    let raw = if kind.is_empty() || kind == "yes" {
        class
    } else {
        kind
    };
    let text = raw.replace('_', " ");
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"[
      {"place_id":1,"osm_type":"node","osm_id":240109189,"lat":"52.5170365","lon":"13.3888599",
       "category":"place","type":"city","name":"Berlin",
       "display_name":"Berlin, Deutschland",
       "address":{"city":"Berlin","country":"Deutschland"},
       "boundingbox":["52.3570365","52.6770365","13.2288599","13.5488599"],
       "extratags":{"population":"3755251","wikipedia":"de:Berlin"}},
      {"lat":"bad","lon":"1"},
      {"osm_type":"way","osm_id":"5","lat":"1.0","lon":"2.0","category":"highway","type":"residential",
       "name":"","display_name":"Main Street, Town","address":{"road":"Main Street"}}
    ]"#;

    #[test]
    fn search_results() {
        let places = parse_search(SEARCH).unwrap();
        assert_eq!(places.len(), 2);
        let b = &places[0];
        assert_eq!(b.name, "Berlin");
        assert_eq!(b.category, "City");
        assert_eq!(b.osm_url(), "https://www.openstreetmap.org/node/240109189");
        assert_eq!(
            b.bbox,
            Some([52.3570365, 52.6770365, 13.2288599, 13.5488599])
        );
        assert_eq!(
            b.extras[0],
            ("Wikipedia".to_string(), "de:Berlin".to_string())
        );
        assert_eq!(b.short_address(), "Deutschland");
        let s = &places[1];
        assert_eq!(s.name, "Main Street");
        assert_eq!(s.category, "Road");
        assert_eq!(s.osm_id, 5);
        assert_eq!(s.key(), "way/5");
    }

    #[test]
    fn errors_and_reverse() {
        assert!(parse_search(r#"{"error":{"message":"Bad request"}}"#)
            .unwrap_err()
            .contains("Bad"));
        assert!(parse_search("<html>").is_err());
        assert_eq!(
            parse_reverse(r#"{"error":"Unable to geocode"}"#).unwrap(),
            None
        );
        let r = parse_reverse(
            r#"{"lat":"1","lon":"2","category":"place","type":"house","display_name":"1 A St, X",
                "address":{"road":"A St","house_number":"1"}}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(r.name, "A St 1");
        assert_eq!(r.category, "Address");
    }

    #[test]
    fn urls() {
        let u = search_url(
            "https://n.example/",
            "Café & Bar",
            5,
            Some((1.0, 2.0, 3.0, 4.0)),
        );
        assert_eq!(
            u,
            "https://n.example/search?format=jsonv2&addressdetails=1&extratags=1&limit=5&q=Caf%C3%A9%20%26%20Bar&viewbox=1.00000,4.00000,3.00000,2.00000"
        );
        let r = reverse_url("https://n.example", LatLon { lat: 1.0, lon: 2.0 }, 30);
        assert!(r.contains("zoom=18&lat=1.000000&lon=2.000000"));
    }

    #[test]
    fn bare_place() {
        let p = Place::at(LatLon { lat: 1.0, lon: 2.0 }, "Pin");
        assert_eq!(p.key(), "1.00000,2.00000");
        assert!(p.osm_url().contains("mlat=1.000000&mlon=2.000000"));
    }

    #[test]
    fn categories() {
        assert_eq!(humanize_category("amenity", "fast_food"), "Fast food");
        assert_eq!(humanize_category("building", "yes"), "Building");
        assert_eq!(humanize_category("shop", "yes"), "Shop");
        assert_eq!(humanize_category("", ""), "");
    }
}
