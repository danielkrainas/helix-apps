//! Automatic location: position from the Location portal / GeoClue
//! ([`crate::location`]) and reverse geocoding to a named [`Place`].
//!
//! Reverse geocoding tries the offline GWeather database first (nearest
//! city within [`OFFLINE_RADIUS_KM`]), then OpenStreetMap Nominatim, and
//! finally falls back to an unnamed "Current Location".

use serde::Deserialize;
use std::time::Duration;

use crate::api::USER_AGENT;
use crate::gwdb;
use crate::location::{self, Fix};
use crate::store::Place;

pub const OFFLINE_RADIUS_KM: f64 = 50.0;
const NOMINATIM_URL: &str = "https://nominatim.openstreetmap.org/reverse";
pub const FALLBACK_NAME: &str = "Current Location";

/// Nearest GWeather city within the radius.
pub fn offline(db: &gwdb::Db, fix: Fix) -> Option<Place> {
    let c = db.nearest_city((fix.latitude, fix.longitude), OFFLINE_RADIUS_KM)?;
    Some(Place {
        admin1: c.state.clone(),
        country: c.country.clone(),
        timezone: c.tz.clone(),
        ..Place::at(c.name.clone(), fix.latitude, fix.longitude)
    })
}

#[derive(Deserialize, Default)]
struct Address {
    city: Option<String>,
    town: Option<String>,
    village: Option<String>,
    municipality: Option<String>,
    hamlet: Option<String>,
    suburb: Option<String>,
    county: Option<String>,
    state: Option<String>,
    country: Option<String>,
    country_code: Option<String>,
}

#[derive(Deserialize)]
struct Reverse {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    address: Option<Address>,
    #[serde(default)]
    error: Option<String>,
}

/// Parses a Nominatim `format=jsonv2` reverse response.
pub fn parse_nominatim(body: &str, fix: Fix) -> Result<Place, String> {
    let r: Reverse = serde_json::from_str(body)
        .map_err(|e| format!("Unexpected reverse geocoding data: {e}"))?;
    if let Some(e) = r.error {
        return Err(e);
    }
    let a = r.address.unwrap_or_default();
    let name = [
        a.city,
        a.town,
        a.village,
        a.municipality,
        a.hamlet,
        a.suburb,
        a.county,
        r.name,
    ]
    .into_iter()
    .flatten()
    .find(|s| !s.trim().is_empty())
    .ok_or("No place name at this position")?;
    Ok(Place {
        admin1: a.state,
        country: a.country,
        country_code: a.country_code.map(|c| c.to_uppercase()),
        ..Place::at(name, fix.latitude, fix.longitude)
    })
}

fn nominatim(fix: Fix, language: &str) -> Result<Place, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build();
    let req = agent
        .get(NOMINATIM_URL)
        .query("format", "jsonv2")
        .query("lat", &format!("{:.4}", fix.latitude))
        .query("lon", &format!("{:.4}", fix.longitude))
        .query("zoom", "10")
        .query("accept-language", language);
    let body = req
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    parse_nominatim(&body, fix)
}

/// Names `fix` (blocking; may use the network).
pub fn name_place(fix: Fix, db: Option<&gwdb::Db>, language: &str) -> Place {
    db.and_then(|d| offline(d, fix))
        .or_else(|| {
            nominatim(fix, language)
                .map_err(|e| eprintln!("helix-weather: reverse geocoding: {e}"))
                .ok()
        })
        .unwrap_or_else(|| Place::at(FALLBACK_NAME, fix.latitude, fix.longitude))
}

/// Locates and names the current place (blocking).
pub fn current_place(app_id: &str, language: &str) -> Result<Place, String> {
    let fix = location::locate(app_id, Duration::from_secs(30))?;
    let db = gwdb::Db::load();
    let mut p = name_place(fix, db.as_ref(), language);
    p.automatic = true;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(lat: f64, lon: f64) -> Fix {
        Fix {
            latitude: lat,
            longitude: lon,
            accuracy: None,
        }
    }

    #[test]
    fn offline_nearest_city() {
        let db = gwdb::Db::parse(gwdb::FIXTURE).unwrap();
        let p = offline(&db, fix(48.2, 11.6)).unwrap();
        assert_eq!(p.name, "Munich");
        assert_eq!(p.subtitle(), "Bavaria, Germany");
        assert_eq!(p.timezone.as_deref(), Some("Europe/Berlin"));
        assert_eq!(
            (p.latitude, p.longitude),
            (48.2, 11.6),
            "keeps the real position"
        );
        assert!(offline(&db, fix(0.0, 0.0)).is_none());
        // Without any source the fallback name is used (no network here:
        // a far-away fix with the database would hit Nominatim).
    }

    #[test]
    fn nominatim_response() {
        let body = r#"{"place_id":1,"lat":"52.5","lon":"13.4","name":"Mitte","display_name":"Mitte, Berlin, Deutschland",
            "address":{"suburb":"Mitte","city":"Berlin","state":"Berlin","country":"Deutschland","country_code":"de"}}"#;
        let p = parse_nominatim(body, fix(52.5, 13.4)).unwrap();
        assert_eq!(p.name, "Berlin");
        assert_eq!(p.country.as_deref(), Some("Deutschland"));
        assert_eq!(p.country_code.as_deref(), Some("DE"));
        let village = r#"{"address":{"village":"Kleinort","county":"Kreis","country":"X"}}"#;
        assert_eq!(
            parse_nominatim(village, fix(1.0, 1.0)).unwrap().name,
            "Kleinort"
        );
        assert!(parse_nominatim(r#"{"error":"Unable to geocode"}"#, fix(0.0, 0.0)).is_err());
        assert!(parse_nominatim(r#"{"address":{}}"#, fix(0.0, 0.0)).is_err());
        assert!(parse_nominatim("nope", fix(0.0, 0.0)).is_err());
    }
}
