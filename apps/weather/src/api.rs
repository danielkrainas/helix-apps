//! Open-Meteo forecast and geocoding: request building, HTTP and response
//! parsing into the app's model.

use serde::Deserialize;
use std::time::Duration;

use crate::store::Place;
use crate::timefmt::DateTime;

pub const USER_AGENT: &str = "helix-weather/0.1";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const GEOCODING_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const CURRENT_FIELDS: &str =
    "temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m,wind_direction_10m,pressure_msl";
const HOURLY_FIELDS: &str = "temperature_2m,weather_code,precipitation_probability,is_day";
const DAILY_FIELDS: &str =
    "weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset,precipitation_probability_max";
pub const FORECAST_DAYS: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct Current {
    pub time: DateTime,
    pub temperature: f64,
    pub apparent: Option<f64>,
    pub humidity: Option<f64>,
    pub is_day: bool,
    pub code: u8,
    pub wind_speed: Option<f64>,
    pub wind_direction: Option<f64>,
    pub pressure: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hour {
    pub time: DateTime,
    pub temperature: f64,
    pub code: u8,
    pub precipitation: Option<f64>,
    pub is_day: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub date: DateTime,
    pub code: u8,
    pub max: f64,
    pub min: f64,
    pub sunrise: Option<DateTime>,
    pub sunset: Option<DateTime>,
    pub precipitation: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Forecast {
    pub utc_offset: i64,
    pub timezone: String,
    pub current: Current,
    pub hourly: Vec<Hour>,
    pub daily: Vec<Day>,
}

impl Forecast {
    /// Up to `n` hours starting at the hour containing `now` (location-local).
    /// Falls back to the start of the data when `now` precedes it, and to an
    /// empty slice when all data is in the past.
    pub fn hours_from(&self, now: &DateTime, n: usize) -> &[Hour] {
        let start = now.hour_floor();
        let i = self.hourly.iter().position(|h| h.time >= start).unwrap_or(self.hourly.len());
        let end = (i + n).min(self.hourly.len());
        &self.hourly[i..end]
    }

    /// Daily entries from the day containing `now` onwards.
    pub fn days_from(&self, now: &DateTime) -> &[Day] {
        let i = self.daily.iter().position(|d| d.date.days() >= now.days()).unwrap_or(self.daily.len());
        &self.daily[i..]
    }

    /// The daily entry for `now`'s date.
    pub fn today(&self, now: &DateTime) -> Option<&Day> {
        self.daily.iter().find(|d| d.date.same_day(now))
    }
}

// ---- raw response shapes ----

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    error: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct RawForecast {
    #[serde(default)]
    utc_offset_seconds: i64,
    #[serde(default)]
    timezone: String,
    current: RawCurrent,
    hourly: RawHourly,
    daily: RawDaily,
}

#[derive(Deserialize)]
struct RawCurrent {
    time: String,
    temperature_2m: Option<f64>,
    relative_humidity_2m: Option<f64>,
    apparent_temperature: Option<f64>,
    is_day: Option<u8>,
    weather_code: Option<f64>,
    wind_speed_10m: Option<f64>,
    wind_direction_10m: Option<f64>,
    pressure_msl: Option<f64>,
}

#[derive(Deserialize)]
struct RawHourly {
    time: Vec<String>,
    temperature_2m: Vec<Option<f64>>,
    weather_code: Vec<Option<f64>>,
    #[serde(default)]
    precipitation_probability: Vec<Option<f64>>,
    #[serde(default)]
    is_day: Vec<Option<u8>>,
}

#[derive(Deserialize)]
struct RawDaily {
    time: Vec<String>,
    weather_code: Vec<Option<f64>>,
    temperature_2m_max: Vec<Option<f64>>,
    temperature_2m_min: Vec<Option<f64>>,
    #[serde(default)]
    sunrise: Vec<Option<String>>,
    #[serde(default)]
    sunset: Vec<Option<String>>,
    #[serde(default)]
    precipitation_probability_max: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct RawGeocoding {
    #[serde(default)]
    results: Vec<Place>,
}

fn code(v: Option<f64>) -> u8 {
    v.map(|c| c.clamp(0.0, 255.0) as u8).unwrap_or(255)
}

fn at<T: Clone>(v: &[Option<T>], i: usize) -> Option<T> {
    v.get(i).cloned().flatten()
}

/// Checks for Open-Meteo's `{"error": true, "reason": …}` body.
fn api_error(body: &str) -> Option<String> {
    let e: ApiError = serde_json::from_str(body).ok()?;
    e.error.then(|| e.reason.unwrap_or_else(|| "The weather service reported an error".into()))
}

/// Parses a forecast response body.
pub fn parse_forecast(body: &str) -> Result<Forecast, String> {
    if let Some(reason) = api_error(body) {
        return Err(reason);
    }
    let raw: RawForecast = serde_json::from_str(body).map_err(|e| format!("Unexpected forecast data: {e}"))?;
    let c = raw.current;
    let current = Current {
        time: DateTime::parse(&c.time).ok_or("Invalid time in forecast data")?,
        temperature: c.temperature_2m.ok_or("Forecast data has no current temperature")?,
        apparent: c.apparent_temperature,
        humidity: c.relative_humidity_2m,
        is_day: c.is_day.unwrap_or(1) != 0,
        code: code(c.weather_code),
        wind_speed: c.wind_speed_10m,
        wind_direction: c.wind_direction_10m,
        pressure: c.pressure_msl,
    };
    let h = raw.hourly;
    let hourly = h
        .time
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            Some(Hour {
                time: DateTime::parse(t)?,
                temperature: at(&h.temperature_2m, i)?,
                code: code(at(&h.weather_code, i)),
                precipitation: at(&h.precipitation_probability, i),
                is_day: at(&h.is_day, i).unwrap_or(1) != 0,
            })
        })
        .collect();
    let d = raw.daily;
    let daily = d
        .time
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            Some(Day {
                date: DateTime::parse(t)?,
                code: code(at(&d.weather_code, i)),
                max: at(&d.temperature_2m_max, i)?,
                min: at(&d.temperature_2m_min, i)?,
                sunrise: at(&d.sunrise, i).and_then(|s| DateTime::parse(&s)),
                sunset: at(&d.sunset, i).and_then(|s| DateTime::parse(&s)),
                precipitation: at(&d.precipitation_probability_max, i),
            })
        })
        .collect();
    Ok(Forecast { utc_offset: raw.utc_offset_seconds, timezone: raw.timezone, current, hourly, daily })
}

/// Parses a geocoding response body. An absent `results` key means no match.
pub fn parse_geocoding(body: &str) -> Result<Vec<Place>, String> {
    if let Some(reason) = api_error(body) {
        return Err(reason);
    }
    let raw: RawGeocoding = serde_json::from_str(body).map_err(|e| format!("Unexpected search data: {e}"))?;
    Ok(raw.results)
}

// ---- HTTP ----

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).user_agent(USER_AGENT).build()
}

fn get(req: ureq::Request) -> Result<String, String> {
    match req.call() {
        Ok(resp) => resp.into_string().map_err(|e| format!("Could not read response: {e}")),
        // Open-Meteo returns a JSON body with a reason on 4xx.
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(api_error(&body).unwrap_or_else(|| format!("The weather service returned HTTP {code}")))
        }
        Err(ureq::Error::Transport(t)) => Err(format!("Could not connect to the weather service: {t}")),
    }
}

/// Fetches the raw forecast body for a place (blocking).
pub fn fetch_forecast(place: &Place) -> Result<String, String> {
    let req = agent()
        .get(FORECAST_URL)
        .query("latitude", &format!("{:.4}", place.latitude))
        .query("longitude", &format!("{:.4}", place.longitude))
        .query("current", CURRENT_FIELDS)
        .query("hourly", HOURLY_FIELDS)
        .query("daily", DAILY_FIELDS)
        .query("timezone", "auto")
        .query("forecast_days", &FORECAST_DAYS.to_string());
    let body = get(req)?;
    // Validate before the caller caches it.
    parse_forecast(&body)?;
    Ok(body)
}

/// Searches places by name (blocking).
pub fn search(query: &str, language: &str) -> Result<Vec<Place>, String> {
    let req = agent()
        .get(GEOCODING_URL)
        .query("name", query)
        .query("count", "10")
        .query("language", language)
        .query("format", "json");
    parse_geocoding(&get(req)?)
}

/// Two-letter UI language for geocoding results, from the locale.
pub fn ui_language(locale: &str) -> String {
    let lang: String = locale.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase();
    if lang.len() == 2 || lang.len() == 3 {
        lang
    } else {
        "en".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORECAST: &str = include_str!("fixtures/forecast.json");
    const GEOCODING: &str = include_str!("fixtures/geocoding.json");

    #[test]
    fn parses_forecast_fixture() {
        let f = parse_forecast(FORECAST).unwrap();
        assert_eq!(f.utc_offset, 3600);
        assert_eq!(f.timezone, "Europe/Berlin");
        let c = &f.current;
        assert_eq!(c.time, DateTime::parse("2026-11-12T23:30").unwrap());
        assert_eq!(c.temperature, 3.6);
        assert_eq!(c.apparent, Some(0.1));
        assert_eq!(c.humidity, Some(90.0));
        assert!(!c.is_day);
        assert_eq!(c.code, 1);
        assert_eq!(c.wind_speed, Some(11.9));
        assert_eq!(c.wind_direction, Some(256.0));
        assert_eq!(c.pressure, Some(997.5));

        assert_eq!(f.hourly.len(), 72);
        assert_eq!(f.hourly[0].time, DateTime::parse("2026-11-12T00:00").unwrap());
        assert_eq!(f.hourly[0].temperature, 4.1);
        assert_eq!(f.hourly[5].precipitation, None, "null values are tolerated");
        assert!(!f.hourly[0].is_day);
        assert!(f.hourly[8].is_day);

        assert_eq!(f.daily.len(), 10);
        let d0 = &f.daily[0];
        assert_eq!(d0.date, DateTime::parse("2026-11-12").unwrap());
        assert_eq!(d0.code, 61);
        assert_eq!((d0.min, d0.max), (3.5, 6.2));
        assert_eq!(d0.sunrise, DateTime::parse("2026-11-12T07:22"));
        assert_eq!(d0.sunset, DateTime::parse("2026-11-12T16:17"));
        assert_eq!(d0.precipitation, Some(10.0));
        assert_eq!(f.daily[9].min, -0.7);
    }

    #[test]
    fn hourly_window() {
        let f = parse_forecast(FORECAST).unwrap();
        let now = f.current.time;
        let hours = f.hours_from(&now, 48);
        assert_eq!(hours.len(), 48);
        assert_eq!(hours[0].time, DateTime::parse("2026-11-12T23:00").unwrap());
        // Near the end of the data the window is truncated.
        let late = DateTime::parse("2026-11-14T22:10").unwrap();
        assert_eq!(f.hours_from(&late, 48).len(), 2);
        // Entirely past the data.
        let later = DateTime::parse("2026-12-01T00:00").unwrap();
        assert!(f.hours_from(&later, 48).is_empty());
        // Before the data starts.
        let early = DateTime::parse("2026-11-01T00:00").unwrap();
        assert_eq!(f.hours_from(&early, 48)[0].time, f.hourly[0].time);
    }

    #[test]
    fn daily_window_and_today() {
        let f = parse_forecast(FORECAST).unwrap();
        let now = DateTime::parse("2026-11-14T10:00").unwrap();
        assert_eq!(f.days_from(&now).len(), 8);
        assert_eq!(f.today(&now).unwrap().date, DateTime::parse("2026-11-14").unwrap());
        assert!(f.today(&DateTime::parse("2027-01-01").unwrap()).is_none());
    }

    #[test]
    fn forecast_errors() {
        let e = parse_forecast(r#"{"error":true,"reason":"Latitude must be in range of -90 to 90°. Given: 999.0."}"#);
        assert_eq!(e.unwrap_err(), "Latitude must be in range of -90 to 90°. Given: 999.0.");
        assert!(parse_forecast("{}").unwrap_err().starts_with("Unexpected forecast data"));
        assert!(parse_forecast("<html>").is_err());
    }

    #[test]
    fn parses_geocoding_fixture() {
        let places = parse_geocoding(GEOCODING).unwrap();
        assert_eq!(places.len(), 3);
        assert_eq!(places[0].name, "Berlin");
        assert_eq!(places[0].id, Some(2950159));
        assert_eq!(places[0].country.as_deref(), Some("Germany"));
        assert_eq!(places[0].timezone.as_deref(), Some("Europe/Berlin"));
        assert_eq!(places[1].subtitle(), "New Hampshire, United States");
        assert_eq!(places[2].country, None);
        assert_eq!(places[2].longitude, 170.25);
    }

    #[test]
    fn geocoding_no_results() {
        assert!(parse_geocoding(r#"{"generationtime_ms":0.07}"#).unwrap().is_empty());
        assert!(parse_geocoding(r#"{"error":true}"#).is_err());
    }

    #[test]
    fn language() {
        assert_eq!(ui_language("de_DE.UTF-8"), "de");
        assert_eq!(ui_language("en"), "en");
        assert_eq!(ui_language("C"), "en");
        assert_eq!(ui_language(""), "en");
    }
}
