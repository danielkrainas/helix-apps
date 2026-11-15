//! Coordinates, `geo:` URIs, launch arguments and human-readable
//! distance / duration formatting.

use helix_app_common::i18n::{format_number, tr};
use helix_app_common::trf;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub fn new(lat: f64, lon: f64) -> Option<Self> {
        let valid = lat.is_finite()
            && lon.is_finite()
            && (-90.0..=90.0).contains(&lat)
            && (-180.0..=180.0).contains(&lon);
        valid.then_some(LatLon { lat, lon })
    }
}

/// What the app should do on startup, from the command line.
#[derive(Debug, Clone, PartialEq)]
pub enum LaunchTarget {
    Point {
        at: LatLon,
        zoom: Option<f64>,
        label: Option<String>,
    },
    Search(String),
}

/// Percent-decodes a URI component (`+` is treated as a space).
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes a query component.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Parses `"lat, lon"`, `"lat lon"` or `"lat;lon"` into a coordinate.
pub fn parse_coordinates(text: &str) -> Option<LatLon> {
    let text = text.trim();
    let parts: Vec<&str> = text
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 2 {
        return None;
    }
    let lat = parts[0].trim_end_matches('°').parse().ok()?;
    let lon = parts[1].trim_end_matches('°').parse().ok()?;
    LatLon::new(lat, lon)
}

/// Parses a `geo:` URI (RFC 5870 plus the widespread `?q=` extension).
pub fn parse_geo_uri(uri: &str) -> Option<LaunchTarget> {
    let rest = uri.strip_prefix("geo:").or_else(|| {
        uri.get(..4)
            .filter(|p| p.eq_ignore_ascii_case("geo:"))
            .map(|_| &uri[4..])
    })?;
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    // Coordinates are before any `;param`.
    let coords = path.split(';').next().unwrap_or("");
    let mut nums = coords.split(',').map(|s| s.trim().parse::<f64>());
    let lat = nums.next()?.ok()?;
    let lon = nums.next()?.ok()?;
    let point = LatLon::new(lat, lon)?;

    let mut zoom = None;
    let mut q = None;
    for pair in query.unwrap_or("").split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "z" => zoom = v.parse::<f64>().ok().filter(|z| (0.0..=22.0).contains(z)),
            "q" => q = Some(percent_decode(v)),
            _ => {}
        }
    }

    if let Some(q) = q.filter(|q| !q.trim().is_empty()) {
        // `q=lat,lon(Label)` names a point; anything else is a search.
        let (coord_part, label) = match q.find('(') {
            Some(i) if q.ends_with(')') => (
                q[..i].to_string(),
                Some(q[i + 1..q.len() - 1].trim().to_string()),
            ),
            _ => (q.clone(), None),
        };
        if let Some(at) = parse_coordinates(&coord_part) {
            return Some(LaunchTarget::Point {
                at,
                zoom,
                label: label.filter(|l| !l.is_empty()),
            });
        }
        return Some(LaunchTarget::Search(q.trim().to_string()));
    }
    Some(LaunchTarget::Point {
        at: point,
        zoom,
        label: None,
    })
}

/// Parses a `maps:q=…` URI.
pub fn parse_maps_uri(uri: &str) -> Option<LaunchTarget> {
    let rest = uri.strip_prefix("maps:")?;
    let rest = rest.trim_start_matches("//").trim_start_matches('?');
    for pair in rest.split('&') {
        if let Some(v) = pair.strip_prefix("q=") {
            let q = percent_decode(v);
            if !q.trim().is_empty() {
                return Some(LaunchTarget::Search(q.trim().to_string()));
            }
        }
    }
    None
}

/// Interprets one positional command-line argument.
pub fn parse_launch_arg(arg: &str) -> Option<LaunchTarget> {
    if arg.len() >= 4 && arg[..4].eq_ignore_ascii_case("geo:") {
        parse_geo_uri(arg)
    } else if arg.starts_with("maps:") {
        parse_maps_uri(arg)
    } else {
        None
    }
}

/// Zoom level at which a bounding box (south, north, west, east) fits in a
/// viewport of `width`×`height` pixels with 256-px tiles.
pub fn zoom_for_bounds(
    south: f64,
    north: f64,
    west: f64,
    east: f64,
    width: f64,
    height: f64,
) -> f64 {
    fn merc_y(lat: f64) -> f64 {
        let lat = lat.clamp(-85.0511, 85.0511).to_radians();
        (PI / 4.0 + lat / 2.0).tan().ln() / (2.0 * PI)
    }
    let dx = ((east - west).abs() / 360.0).max(1e-9);
    let dy = (merc_y(north) - merc_y(south)).abs().max(1e-9);
    let zx = (width.max(1.0) / 256.0 / dx).log2();
    let zy = (height.max(1.0) / 256.0 / dy).log2();
    zx.min(zy).clamp(2.0, 18.0)
}

/// Zoom level suited to a location with the given accuracy radius.
pub fn zoom_for_accuracy(accuracy_m: f64) -> f64 {
    match accuracy_m {
        a if !a.is_finite() || a <= 0.0 => 15.0,
        a if a < 100.0 => 17.0,
        a if a < 1_000.0 => 15.0,
        a if a < 10_000.0 => 12.0,
        a if a < 100_000.0 => 9.0,
        _ => 5.0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    Metric,
    Imperial,
}

impl Units {
    pub fn from_name(s: &str) -> Units {
        if s == "imperial" {
            Units::Imperial
        } else {
            Units::Metric
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Units::Metric => "metric",
            Units::Imperial => "imperial",
        }
    }
}

pub fn format_distance(m: f64, units: Units) -> String {
    match units {
        Units::Metric => {
            if m < 1000.0 {
                // Translators: a distance in metres.
                trf!(
                    "{} m",
                    round_to(m, if m < 100.0 { 5.0 } else { 10.0 }) as i64
                )
            } else if m < 10_000.0 {
                // Translators: a distance in kilometres.
                trf!("{} km", format_number(m / 1000.0, 1))
            } else {
                trf!("{} km", (m / 1000.0).round() as i64)
            }
        }
        Units::Imperial => {
            let ft = m / 0.3048;
            let mi = m / 1609.344;
            if mi < 0.1 {
                // Translators: a distance in feet.
                trf!("{} ft", round_to(ft, 10.0) as i64)
            } else if mi < 10.0 {
                // Translators: a distance in miles.
                trf!("{} mi", format_number(mi, 1))
            } else {
                trf!("{} mi", mi.round() as i64)
            }
        }
    }
}

fn round_to(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

pub fn format_duration(secs: f64) -> String {
    let mins = (secs / 60.0).round() as i64;
    if mins < 1 {
        tr("less than a minute").to_string()
    } else if mins < 60 {
        trf!("{mins} min", mins = mins)
    } else {
        let h = mins / 60;
        let m = mins % 60;
        if m == 0 {
            // Translators: a travel time in hours, "2 h".
            trf!("{h} h", h = h)
        } else {
            trf!("{h} h {m} min", h = h, m = m)
        }
    }
}

pub fn format_coordinates(p: LatLon) -> String {
    format!("{:.6}, {:.6}", p.lat, p.lon)
}

pub fn geo_uri(p: LatLon) -> String {
    format!("geo:{:.6},{:.6}", p.lat, p.lon)
}

/// Human-readable degrees/minutes form, e.g. `52°31′12″ N, 13°24′00″ E`.
pub fn format_dms(p: LatLon) -> String {
    fn part(v: f64, pos: char, neg: char) -> String {
        let hemi = if v < 0.0 { neg } else { pos };
        let total = (v.abs() * 3600.0).round() as i64;
        format!(
            "{}°{:02}′{:02}″ {}",
            total / 3600,
            (total / 60) % 60,
            total % 60,
            hemi
        )
    }
    format!("{}, {}", part(p.lat, 'N', 'S'), part(p.lon, 'E', 'W'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    #[test]
    fn coordinates() {
        assert_eq!(parse_coordinates("52.52, 13.405"), Some(pt(52.52, 13.405)));
        assert_eq!(parse_coordinates(" -33.9 18.4 "), Some(pt(-33.9, 18.4)));
        assert_eq!(parse_coordinates("1;2"), Some(pt(1.0, 2.0)));
        assert_eq!(parse_coordinates("91, 0"), None);
        assert_eq!(parse_coordinates("Berlin"), None);
        assert_eq!(parse_coordinates("1, 2, 3"), None);
    }

    #[test]
    fn geo_uris() {
        assert_eq!(
            parse_geo_uri("geo:48.2010,16.3695,183;crs=wgs84;u=40"),
            Some(LaunchTarget::Point {
                at: pt(48.201, 16.3695),
                zoom: None,
                label: None
            })
        );
        assert_eq!(
            parse_geo_uri("geo:1,2?z=12"),
            Some(LaunchTarget::Point {
                at: pt(1.0, 2.0),
                zoom: Some(12.0),
                label: None
            })
        );
        assert_eq!(
            parse_geo_uri("geo:0,0?q=34.99,-106.61(Treasure)"),
            Some(LaunchTarget::Point {
                at: pt(34.99, -106.61),
                zoom: None,
                label: Some("Treasure".into())
            })
        );
        assert_eq!(
            parse_geo_uri("geo:0,0?q=1600+Amphitheatre%20Parkway"),
            Some(LaunchTarget::Search("1600 Amphitheatre Parkway".into()))
        );
        assert_eq!(parse_geo_uri("geo:abc"), None);
        assert_eq!(parse_geo_uri("geo:100,0"), None);
        assert!(parse_launch_arg("GEO:1,2").is_some());
    }

    #[test]
    fn maps_uris() {
        assert_eq!(
            parse_maps_uri("maps:q=Caf%C3%A9"),
            Some(LaunchTarget::Search("Café".into()))
        );
        assert_eq!(
            parse_maps_uri("maps:?q=x&z=1"),
            Some(LaunchTarget::Search("x".into()))
        );
        assert_eq!(parse_maps_uri("maps:z=1"), None);
        assert_eq!(parse_launch_arg("http://x"), None);
    }

    #[test]
    fn encoding_roundtrip() {
        let s = "Straße 1/2 & co";
        assert_eq!(percent_decode(&percent_encode(s)), s);
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn distances() {
        assert_eq!(format_distance(42.0, Units::Metric), "40 m");
        assert_eq!(format_distance(456.0, Units::Metric), "460 m");
        assert_eq!(format_distance(2345.0, Units::Metric), "2.3 km");
        assert_eq!(format_distance(23_456.0, Units::Metric), "23 km");
        assert_eq!(format_distance(50.0, Units::Imperial), "160 ft");
        assert_eq!(format_distance(5000.0, Units::Imperial), "3.1 mi");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(20.0), "less than a minute");
        assert_eq!(format_duration(600.0), "10 min");
        assert_eq!(format_duration(3600.0), "1 h");
        assert_eq!(format_duration(5400.0), "1 h 30 min");
    }

    #[test]
    fn formatting() {
        assert_eq!(format_coordinates(pt(1.5, -2.25)), "1.500000, -2.250000");
        assert_eq!(geo_uri(pt(1.5, -2.25)), "geo:1.500000,-2.250000");
        assert_eq!(format_dms(pt(52.52, -13.4)), "52°31′12″ N, 13°24′00″ W");
    }

    #[test]
    fn zooms() {
        // A city-sized box (~0.2°) should land around zoom 11-12 in 1000px.
        let z = zoom_for_bounds(52.4, 52.6, 13.2, 13.5, 1000.0, 700.0);
        assert!((10.0..13.0).contains(&z), "{z}");
        // Whole world clamps to the minimum.
        assert_eq!(
            zoom_for_bounds(-85.0, 85.0, -180.0, 180.0, 500.0, 500.0),
            2.0
        );
        // A point clamps to the maximum.
        assert_eq!(zoom_for_bounds(1.0, 1.0, 1.0, 1.0, 500.0, 500.0), 18.0);
        assert_eq!(zoom_for_accuracy(20.0), 17.0);
        assert_eq!(zoom_for_accuracy(50_000.0), 9.0);
    }
}
