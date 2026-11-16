//! OSRM-compatible routing: request URL and response parsing.

use crate::geo::LatLon;
use helix_app_common::i18n::{tr, trc};
use helix_app_common::trf;
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
            Mode::Car => tr("Car"),
            Mode::Bike => tr("Bike"),
            Mode::Walk => tr("Walk"),
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
        .map_err(|_| tr("Invalid response from the routing service").to_string())?;
    let code = v.get("code").and_then(Value::as_str).unwrap_or("");
    if code != "Ok" {
        return Err(match code {
            "NoRoute" => tr("No route found").to_string(),
            "NoSegment" => tr("One of the points is too far from any road").to_string(),
            _ => v
                .get("message")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| tr("Routing failed").to_string()),
        });
    }
    let r = v
        .get("routes")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or(tr("No route found"))?;
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

/// The direction of a turn (OSRM's maneuver `modifier`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Left,
    Right,
    SlightLeft,
    SlightRight,
    SharpLeft,
    SharpRight,
    UTurn,
}

fn direction(modifier: &str) -> Option<Dir> {
    Some(match modifier {
        "left" => Dir::Left,
        "right" => Dir::Right,
        "slight left" => Dir::SlightLeft,
        "slight right" => Dir::SlightRight,
        "sharp left" => Dir::SharpLeft,
        "sharp right" => Dir::SharpRight,
        "uturn" => Dir::UTurn,
        _ => return None,
    })
}

/// "Turn left", "Turn left onto {name}", …
fn turn(dir: Dir, name: &str) -> String {
    let n = name;
    match (dir, name.is_empty()) {
        (Dir::Left, true) => tr("Turn left").into(),
        (Dir::Left, false) => trf!("Turn left onto {name}", name = n),
        (Dir::Right, true) => tr("Turn right").into(),
        (Dir::Right, false) => trf!("Turn right onto {name}", name = n),
        (Dir::SlightLeft, true) => tr("Turn slightly left").into(),
        (Dir::SlightLeft, false) => trf!("Turn slightly left onto {name}", name = n),
        (Dir::SlightRight, true) => tr("Turn slightly right").into(),
        (Dir::SlightRight, false) => trf!("Turn slightly right onto {name}", name = n),
        (Dir::SharpLeft, true) => tr("Turn sharp left").into(),
        (Dir::SharpLeft, false) => trf!("Turn sharp left onto {name}", name = n),
        (Dir::SharpRight, true) => tr("Turn sharp right").into(),
        (Dir::SharpRight, false) => trf!("Turn sharp right onto {name}", name = n),
        (Dir::UTurn, true) => tr("Make a U-turn").into(),
        (Dir::UTurn, false) => trf!("Make a U-turn onto {name}", name = n),
    }
}

/// "Keep left", "Keep left onto {name}", … (a bend in a road that goes on).
fn keep(dir: Dir, name: &str) -> String {
    let n = name;
    match (dir, name.is_empty()) {
        (Dir::Left | Dir::SharpLeft, true) => tr("Keep left").into(),
        (Dir::Left | Dir::SharpLeft, false) => trf!("Keep left onto {name}", name = n),
        (Dir::Right | Dir::SharpRight, true) => tr("Keep right").into(),
        (Dir::Right | Dir::SharpRight, false) => trf!("Keep right onto {name}", name = n),
        (Dir::SlightLeft, true) => tr("Keep slightly left").into(),
        (Dir::SlightLeft, false) => trf!("Keep slightly left onto {name}", name = n),
        (Dir::SlightRight, true) => tr("Keep slightly right").into(),
        (Dir::SlightRight, false) => trf!("Keep slightly right onto {name}", name = n),
        (Dir::UTurn, _) => turn(dir, name),
    }
}

/// "At the end of the road, turn left", …
fn end_of_road(dir: Dir, name: &str) -> String {
    let n = name;
    let left = matches!(dir, Dir::Left | Dir::SlightLeft | Dir::SharpLeft);
    match (dir, left, name.is_empty()) {
        (Dir::UTurn, _, _) => turn(dir, name),
        (_, true, true) => tr("At the end of the road, turn left").into(),
        (_, true, false) => trf!("At the end of the road, turn left onto {name}", name = n),
        (_, false, true) => tr("At the end of the road, turn right").into(),
        (_, false, false) => trf!("At the end of the road, turn right onto {name}", name = n),
    }
}

/// Builds the instruction for an OSRM maneuver, in the interface language.
pub fn instruction(kind: &str, modifier: &str, name: &str, exit: Option<u64>) -> String {
    let n = name;
    let named = !name.is_empty();
    let dir = direction(modifier);
    match kind {
        "depart" => {
            if named {
                trf!("Start on {name}", name = n)
            } else {
                trc("route instruction", "Start")
            }
        }
        "arrive" => tr("Arrive at destination").to_string(),
        "roundabout" | "rotary" => match (exit, named) {
            (Some(x), true) => trf!(
                "At the roundabout, take exit {n} onto {name}",
                n = x,
                name = n
            ),
            (Some(x), false) => trf!("At the roundabout, take exit {n}", n = x),
            (None, true) => trf!("Enter the roundabout and continue onto {name}", name = n),
            (None, false) => tr("Enter the roundabout").to_string(),
        },
        "exit roundabout" | "exit rotary" => {
            if named {
                trf!("Exit the roundabout onto {name}", name = n)
            } else {
                tr("Exit the roundabout").to_string()
            }
        }
        "merge" => {
            if named {
                trf!("Merge onto {name}", name = n)
            } else {
                tr("Merge").to_string()
            }
        }
        "on ramp" => {
            if named {
                trf!("Take the ramp onto {name}", name = n)
            } else {
                tr("Take the ramp").to_string()
            }
        }
        "off ramp" => {
            if named {
                trf!("Take the exit onto {name}", name = n)
            } else {
                tr("Take the exit").to_string()
            }
        }
        "fork" => match (modifier.contains("left"), named) {
            (true, true) => trf!("Keep left at the fork onto {name}", name = n),
            (true, false) => tr("Keep left at the fork").to_string(),
            (false, true) => trf!("Keep right at the fork onto {name}", name = n),
            (false, false) => tr("Keep right at the fork").to_string(),
        },
        "end of road" if dir.is_some() => end_of_road(dir.unwrap_or(Dir::Left), name),
        "continue" | "new name" => match dir {
            Some(d) if modifier != "straight" => keep(d, name),
            _ => {
                if named {
                    trf!("Continue on {name}", name = n)
                } else {
                    tr("Continue").to_string()
                }
            }
        },
        _ => match dir {
            Some(d) => turn(d, name),
            None => {
                if named {
                    trf!("Continue straight onto {name}", name = n)
                } else {
                    tr("Continue straight").to_string()
                }
            }
        },
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
