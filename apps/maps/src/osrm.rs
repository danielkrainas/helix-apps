//! OSRM-compatible routing: request URL and response parsing.

use crate::geo::LatLon;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Car,
    Bike,
    Walk,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Car, Mode::Bike, Mode::Walk];

    /// From an `org.gnome.Maps` `transportation-type` nick. `transit` has
    /// no OSRM profile and falls back to walking (the schema default).
    pub fn from_nick(s: &str) -> Mode {
        match s {
            "car" => Mode::Car,
            "bike" => Mode::Bike,
            _ => Mode::Walk,
        }
    }
    /// The `org.gnome.maps.TransportationType` nick.
    pub fn nick(self) -> &'static str {
        match self {
            Mode::Car => "car",
            Mode::Bike => "bike",
            Mode::Walk => "pedestrian",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Car => "Car",
            Mode::Bike => "Bike",
            Mode::Walk => "Walk",
        }
    }
    pub fn profile(self) -> &'static str {
        match self {
            Mode::Car => "driving",
            Mode::Bike => "cycling",
            Mode::Walk => "foot",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub instruction: String,
    pub distance: f64,
    pub location: LatLon,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub distance: f64,
    pub duration: f64,
    pub path: Vec<LatLon>,
    pub steps: Vec<Step>,
}

impl Route {
    /// (south, north, west, east) of the route geometry.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let first = self.path.first()?;
        let mut b = (first.lat, first.lat, first.lon, first.lon);
        for p in &self.path {
            b.0 = b.0.min(p.lat);
            b.1 = b.1.max(p.lat);
            b.2 = b.2.min(p.lon);
            b.3 = b.3.max(p.lon);
        }
        Some(b)
    }
}

pub fn route_url(base: &str, mode: Mode, from: LatLon, to: LatLon) -> String {
    format!(
        "{}/route/v1/{}/{:.6},{:.6};{:.6},{:.6}?overview=full&geometries=geojson&steps=true",
        base.trim_end_matches('/'),
        mode.profile(),
        from.lon,
        from.lat,
        to.lon,
        to.lat
    )
}

fn lonlat(v: &Value) -> Option<LatLon> {
    let a = v.as_array()?;
    LatLon::new(a.get(1)?.as_f64()?, a.first()?.as_f64()?)
}

pub fn parse_route(json: &str) -> Result<Route, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|_| "Invalid response from the routing service".to_string())?;
    let code = v.get("code").and_then(Value::as_str).unwrap_or("");
    if code != "Ok" {
        return Err(match code {
            "NoRoute" => "No route found".to_string(),
            "NoSegment" => "One of the points is too far from any road".to_string(),
            _ => v
                .get("message")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| "Routing failed".to_string()),
        });
    }
    let r = v
        .get("routes")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or("No route found")?;
    let path: Vec<LatLon> = r
        .pointer("/geometry/coordinates")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(lonlat).collect())
        .unwrap_or_default();
    let mut steps = Vec::new();
    for leg in r
        .get("legs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for s in leg
            .get("steps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let m = s.get("maneuver").cloned().unwrap_or(Value::Null);
            let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
            let modifier = m.get("modifier").and_then(Value::as_str).unwrap_or("");
            let name = s.get("name").and_then(Value::as_str).unwrap_or("");
            let exit = m.get("exit").and_then(Value::as_u64);
            let Some(location) = m.get("location").and_then(lonlat) else {
                continue;
            };
            steps.push(Step {
                instruction: instruction(kind, modifier, name, exit),
                distance: s.get("distance").and_then(Value::as_f64).unwrap_or(0.0),
                location,
            });
        }
    }
    Ok(Route {
        distance: r.get("distance").and_then(Value::as_f64).unwrap_or(0.0),
        duration: r.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
        path,
        steps,
    })
}

fn direction(modifier: &str) -> &'static str {
    match modifier {
        "left" => "left",
        "right" => "right",
        "slight left" => "slightly left",
        "slight right" => "slightly right",
        "sharp left" => "sharp left",
        "sharp right" => "sharp right",
        "uturn" => "around",
        _ => "",
    }
}

/// Builds an English instruction from an OSRM maneuver.
pub fn instruction(kind: &str, modifier: &str, name: &str, exit: Option<u64>) -> String {
    let onto = if name.is_empty() {
        String::new()
    } else {
        format!(" onto {name}")
    };
    let on = if name.is_empty() {
        String::new()
    } else {
        format!(" on {name}")
    };
    let dir = direction(modifier);
    match kind {
        "depart" => {
            if name.is_empty() {
                "Start".to_string()
            } else {
                format!("Start on {name}")
            }
        }
        "arrive" => "Arrive at destination".to_string(),
        "roundabout" | "rotary" => match exit {
            Some(n) => format!("At the roundabout, take exit {n}{onto}"),
            None => format!("Enter the roundabout{onto}"),
        },
        "exit roundabout" | "exit rotary" => format!("Exit the roundabout{onto}"),
        "merge" => format!("Merge{onto}"),
        "on ramp" => format!("Take the ramp{onto}"),
        "off ramp" => format!("Take the exit{onto}"),
        "fork" => {
            let side = if modifier.contains("left") {
                "left"
            } else {
                "right"
            };
            format!("Keep {side} at the fork{onto}")
        }
        "end of road" if !dir.is_empty() => format!("At the end of the road, turn {dir}{onto}"),
        "continue" | "new name" => {
            if modifier == "straight" || dir.is_empty() {
                format!("Continue{on}")
            } else {
                format!("Keep {dir}{onto}")
            }
        }
        _ => {
            if modifier == "uturn" {
                format!("Make a U-turn{onto}")
            } else if modifier == "straight" || dir.is_empty() {
                format!("Continue straight{onto}")
            } else {
                format!("Turn {dir}{onto}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTE: &str = r#"{"code":"Ok","routes":[{"distance":1234.5,"duration":321.0,
      "geometry":{"type":"LineString","coordinates":[[13.0,52.0],[13.1,52.1],[13.2,52.05]]},
      "legs":[{"steps":[
        {"distance":100,"name":"Main St","maneuver":{"type":"depart","location":[13.0,52.0]}},
        {"distance":800,"name":"Oak Ave","maneuver":{"type":"turn","modifier":"left","location":[13.1,52.1]}},
        {"distance":0,"name":"","maneuver":{"type":"arrive","location":[13.2,52.05]}}]}]}]}"#;

    #[test]
    fn parse() {
        let r = parse_route(ROUTE).unwrap();
        assert_eq!(r.distance, 1234.5);
        assert_eq!(r.path.len(), 3);
        assert_eq!(
            r.path[0],
            LatLon {
                lat: 52.0,
                lon: 13.0
            }
        );
        let instr: Vec<&str> = r.steps.iter().map(|s| s.instruction.as_str()).collect();
        assert_eq!(
            instr,
            [
                "Start on Main St",
                "Turn left onto Oak Ave",
                "Arrive at destination"
            ]
        );
        assert_eq!(r.bounds(), Some((52.0, 52.1, 13.0, 13.2)));
    }

    #[test]
    fn errors() {
        assert_eq!(
            parse_route(r#"{"code":"NoRoute"}"#).unwrap_err(),
            "No route found"
        );
        assert_eq!(
            parse_route(r#"{"code":"InvalidUrl","message":"bad"}"#).unwrap_err(),
            "bad"
        );
        assert!(parse_route("garbage").is_err());
    }

    #[test]
    fn url() {
        let u = route_url(
            "https://r.example/",
            Mode::Bike,
            LatLon { lat: 1.0, lon: 2.0 },
            LatLon { lat: 3.0, lon: 4.0 },
        );
        assert_eq!(
            u,
            "https://r.example/route/v1/cycling/2.000000,1.000000;4.000000,3.000000?overview=full&geometries=geojson&steps=true"
        );
    }

    #[test]
    fn instructions() {
        assert_eq!(
            instruction("turn", "slight right", "A", None),
            "Turn slightly right onto A"
        );
        assert_eq!(instruction("turn", "uturn", "", None), "Make a U-turn");
        assert_eq!(
            instruction("roundabout", "", "B", Some(2)),
            "At the roundabout, take exit 2 onto B"
        );
        assert_eq!(
            instruction("new name", "straight", "C", None),
            "Continue on C"
        );
        assert_eq!(
            instruction("fork", "slight left", "", None),
            "Keep left at the fork"
        );
        assert_eq!(instruction("depart", "", "", None), "Start");
        assert_eq!(Mode::from_nick("pedestrian").profile(), "foot");
        assert_eq!(Mode::from_nick("transit"), Mode::Walk);
        assert_eq!(Mode::from_nick("car").profile(), "driving");
        for m in Mode::ALL {
            assert_eq!(Mode::from_nick(m.nick()), m);
        }
    }
}
