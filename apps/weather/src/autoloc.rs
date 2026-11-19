//! Automatic location: position from the Location portal / GeoClue
//! ([`crate::location`]) and reverse geocoding to a named [`Place`].
//!
//! Reverse geocoding tries the offline GWeather database first (nearest
//! city within [`OFFLINE_RADIUS_KM`]), then OpenStreetMap Nominatim, and
//! finally falls back to an unnamed "Current Location".
//!
//! As in GNOME Weather, an automatic place is the nearest city, at the
//! city's own position; without one nearby, the fix rounded to two
//! decimals (about a kilometre). The precise fix is not kept, sent
//! anywhere or logged.

use serde::Deserialize;
use std::time::Duration;

use crate::api::USER_AGENT;
use crate::gwdb;
use crate::location::{self, Fix};
use crate::store::Place;
use helix_app_common::i18n::{n_, tr};
use helix_app_common::trf;

pub const OFFLINE_RADIUS_KM: f64 = 50.0;
const NOMINATIM_URL: &str = "https://nominatim.openstreetmap.org/reverse";
pub const FALLBACK_NAME: &str = n_("Current Location");

/// `fix` rounded to two decimals (about 1.1 km), enough for a forecast.
pub fn coarse(fix: Fix) -> Fix {
    let round = |v: f64| (v * 100.0).round() / 100.0;
    Fix {
        latitude: round(fix.latitude),
        longitude: round(fix.longitude),
        accuracy: None,
    }
}

/// Nearest GWeather city within the radius, at the city's position.
pub fn offline(db: &gwdb::Db, fix: Fix) -> Option<Place> {
    let c = db.nearest_city((fix.latitude, fix.longitude), OFFLINE_RADIUS_KM)?;
    Some(Place {
        admin1: c.state.clone(),
        country: c.country.clone(),
        timezone: c.tz.clone(),
        ..Place::at(c.name.clone(), c.coords.0, c.coords.1)
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
        .map_err(|e| trf!("Unexpected reverse geocoding data: {e}", e = e))?;
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
    .ok_or(tr("No place name at this position"))?;
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
        .map_err(|e| crate::api::describe_error(&e))?
        .into_string()
        .map_err(|e| e.kind().to_string())?;
    parse_nominatim(&body, fix)
}

/// Names `fix` (blocking; may use the network).
pub fn name_place(fix: Fix, db: Option<&gwdb::Db>, language: &str) -> Place {
    let fix = coarse(fix);
    db.and_then(|d| offline(d, fix))
        .or_else(|| {
            nominatim(fix, language)
                .map_err(|e| eprintln!("helix-weather: reverse geocoding: {e}"))
                .ok()
        })
        .unwrap_or_else(|| Place::at(tr(FALLBACK_NAME), fix.latitude, fix.longitude))
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
        let munich = db
            .cities
            .iter()
            .find(|c| c.name == "Munich")
            .unwrap()
            .coords;
        assert_eq!(
            (p.latitude, p.longitude),
            munich,
            "the city's position, not the fix"
        );
        assert!(offline(&db, fix(0.0, 0.0)).is_none());
        // Without any source the fallback name is used (no network here:
        // a far-away fix with the database would hit Nominatim).
    }

    #[test]
    fn fixes_are_coarsened() {
        let f = coarse(Fix {
            latitude: 48.137_154,
            longitude: -11.575_382,
            accuracy: Some(5.0),
        });
        assert_eq!((f.latitude, f.longitude), (48.14, -11.58));
        assert_eq!(f.accuracy, None);
        // Far from any city in the database: no network is used for a
        // name here, but the place keeps only the coarse position.
        let p = Place::at(tr(FALLBACK_NAME), f.latitude, f.longitude);
        assert_eq!(format!("{:.4}", p.latitude), "48.1400");
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
