//! Conversion between the app model and `org.gnome.clocks` GSettings values
//! (`aa{sv}`). See SPEC.md for the dictionary layouts.

use crate::alarm::{Alarm, Weekdays};
use crate::gwdb;
use crate::timer::Timer;
use crate::tz::{self, Location};
use chrono_tz::Tz;
use gtk::glib::prelude::*;
use gtk::glib::{self, Variant, VariantDict};
use std::time::Duration;

/// A configured world clock.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldClock {
    pub name: String,
    pub zone: Tz,
    /// (lat, lon) degrees.
    pub coords: Option<(f64, f64)>,
    pub country: Option<String>,
    /// GWeather weather station (METAR code, (lat, lon) degrees), kept so
    /// GNOME-written entries round-trip unchanged.
    pub station: Option<(String, (f64, f64))>,
}

impl WorldClock {
    pub fn from_location(l: &Location) -> Self {
        WorldClock {
            name: l.city.clone(),
            zone: l.zone,
            coords: l.coords,
            country: l.country.clone(),
            station: None,
        }
    }
}

const GWEATHER_FORMAT_VERSION: u32 = 2;

type SerializedLocation = (String, String, bool, Vec<(f64, f64)>, Vec<(f64, f64)>);

fn dicts(v: &Variant) -> Vec<VariantDict> {
    if !v.is_container() || v.type_().as_str() != "aa{sv}" {
        return Vec::new();
    }
    (0..v.n_children())
        .map(|i| VariantDict::new(Some(&v.child_value(i))))
        .collect()
}

fn array(items: Vec<Variant>) -> Variant {
    Variant::array_from_iter_with_type(glib::VariantTy::VARDICT, items)
}

fn radians(c: Option<(f64, f64)>) -> Vec<(f64, f64)> {
    c.map(|(la, lo)| vec![(la.to_radians(), lo.to_radians())])
        .unwrap_or_default()
}

/// Serializes like GNOME Clocks: `{'location': <(uint32 2, <(name, code,
/// true, [city], [station])>)>}`. The station is the entry's own or, with
/// the GWeather database, the one nearest to the city.
pub fn world_clock_to_variant(c: &WorldClock, gw: Option<&gwdb::Db>) -> Variant {
    let station = c.station.clone().or_else(|| {
        let s = gw?.nearest_station(c.coords?)?;
        Some((s.code.clone(), s.coords))
    });
    let (code, st_coords) = match station {
        Some((code, co)) => (code, Some(co)),
        None => (String::new(), None),
    };
    let ser: SerializedLocation = (
        c.name.clone(),
        code,
        true,
        radians(c.coords),
        radians(st_coords),
    );
    let location = ToVariant::to_variant(&(GWEATHER_FORMAT_VERSION, ToVariant::to_variant(&ser)));
    let d = VariantDict::new(None);
    d.insert_value("location", &location);
    d.insert_value("helix-timezone", &ToVariant::to_variant(c.zone.name()));
    d.end()
}

pub fn world_clocks_to_variant(cs: &[WorldClock], gw: Option<&gwdb::Db>) -> Variant {
    array(cs.iter().map(|c| world_clock_to_variant(c, gw)).collect())
}

/// Reads one world-clock dictionary. The zone comes from `helix-timezone`,
/// else the GWeather station's zone (`gw`), else the `zone1970.tab` city
/// nearest to the coordinates (`db`).
pub fn world_clock_from_dict(
    d: &VariantDict,
    db: &[Location],
    gw: Option<&gwdb::Db>,
) -> Option<WorldClock> {
    let explicit = d
        .lookup::<String>("helix-timezone")
        .ok()
        .flatten()
        .and_then(|z| tz::zone_by_id(&z));
    let loc = d.lookup_value("location", None).map(|v| {
        if v.type_() == glib::VariantTy::VARIANT {
            v.as_variant().unwrap_or(v)
        } else {
            v
        }
    });
    let mut name = None;
    let mut coords = None;
    let mut station = None;
    if let Some(loc) = loc {
        if let Some((_version, inner)) = loc.get::<(u32, Variant)>() {
            if let Some((n, code, _is_city, city, st)) = inner.get::<SerializedLocation>() {
                name = Some(n);
                let deg = |v: &Vec<(f64, f64)>| {
                    v.first().map(|(la, lo)| (la.to_degrees(), lo.to_degrees()))
                };
                coords = deg(&city).or(deg(&st));
                if !code.is_empty() {
                    station = deg(&st).map(|c| (code, c));
                }
            }
        }
    }
    let gw_station = station.as_ref().and_then(|(code, _)| gw?.station(code));
    let gw_zone = gw_station
        .and_then(|s| s.tz.as_deref())
        .and_then(tz::zone_by_id);
    let nearest = coords.and_then(|c| tz::nearest(db, c));
    let zone = explicit.or(gw_zone).or(nearest.map(|l| l.zone))?;
    let known = db.iter().find(|l| l.zone == zone);
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| tz::city_from_zone(zone.name()));
    // The country is known for the zone's own principal city or from the
    // station; a coordinate-resolved place may lie in a neighbouring country.
    let country = known
        .filter(|l| l.city == name)
        .and_then(|l| l.country.clone())
        .or_else(|| gw_station.and_then(|s| s.country.clone()));
    Some(WorldClock {
        name,
        zone,
        coords: coords.or(known.and_then(|l| l.coords)),
        country,
        station,
    })
}

pub fn world_clocks_from_variant(
    v: &Variant,
    db: &[Location],
    gw: Option<&gwdb::Db>,
) -> Vec<WorldClock> {
    let mut out: Vec<WorldClock> = Vec::new();
    for d in dicts(v) {
        if let Some(c) = world_clock_from_dict(&d, db, gw) {
            if !out.iter().any(|o| o.zone == c.zone && o.name == c.name) {
                out.push(c);
            }
        }
    }
    out
}

pub fn alarm_to_variant(a: &Alarm) -> Variant {
    let d = VariantDict::new(None);
    d.insert_value("id", &a.id.to_variant());
    d.insert_value("name", &a.name.to_variant());
    d.insert_value("active", &a.active.to_variant());
    d.insert_value("hour", &(a.hour as i32).to_variant());
    d.insert_value("minute", &(a.minute as i32).to_variant());
    d.insert_value("days", &a.days.to_numbers().to_variant());
    d.insert_value("ring_minutes", &(a.ring_minutes as i32).to_variant());
    d.insert_value("snooze_minutes", &(a.snooze_minutes as i32).to_variant());
    d.end()
}

pub fn alarms_to_variant(alarms: &[Alarm]) -> Variant {
    array(alarms.iter().map(alarm_to_variant).collect())
}

pub fn alarm_from_dict(d: &VariantDict, index: usize) -> Option<Alarm> {
    let int = |k: &str| d.lookup::<i32>(k).ok().flatten();
    let hour = int("hour")?;
    let minute = int("minute")?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    let def = Alarm::default();
    Some(Alarm {
        id: d
            .lookup::<String>("id")
            .ok()
            .flatten()
            .unwrap_or_else(|| format!("imported-{index}")),
        name: d
            .lookup::<String>("name")
            .ok()
            .flatten()
            .unwrap_or(def.name),
        active: d.lookup::<bool>("active").ok().flatten().unwrap_or(true),
        hour: hour as u32,
        minute: minute as u32,
        days: Weekdays::from_numbers(
            &d.lookup::<Vec<i32>>("days")
                .ok()
                .flatten()
                .unwrap_or_default(),
        ),
        ring_minutes: int("ring_minutes")
            .filter(|m| *m > 0)
            .map(|m| m as u32)
            .unwrap_or(def.ring_minutes),
        snooze_minutes: int("snooze_minutes")
            .filter(|m| *m > 0)
            .map(|m| m as u32)
            .unwrap_or(def.snooze_minutes),
    })
}

pub fn alarms_from_variant(v: &Variant) -> Vec<Alarm> {
    dicts(v)
        .iter()
        .enumerate()
        .filter_map(|(i, d)| alarm_from_dict(d, i))
        .collect()
}

pub fn timer_to_variant(t: &Timer) -> Variant {
    let d = VariantDict::new(None);
    d.insert_value(
        "duration",
        &(t.duration.as_secs().min(i32::MAX as u64) as i32).to_variant(),
    );
    d.insert_value("name", &t.name.to_variant());
    d.end()
}

pub fn timers_to_variant(ts: &[Timer]) -> Variant {
    array(ts.iter().map(timer_to_variant).collect())
}

pub fn timers_from_variant(v: &Variant) -> Vec<Timer> {
    dicts(v)
        .iter()
        .filter_map(|d| {
            let secs = d
                .lookup::<i32>("duration")
                .ok()
                .flatten()
                .filter(|s| *s > 0)?;
            let name = d
                .lookup::<String>("name")
                .ok()
                .flatten()
                .unwrap_or_default();
            Some(Timer::new(name, Duration::from_secs(secs as u64)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Vec<Location> {
        vec![
            Location {
                city: "Berlin".into(),
                zone: chrono_tz::Europe::Berlin,
                country: Some("Germany".into()),
                country_code: Some("DE".into()),
                coords: Some((52.5, 13.37)),
            },
            Location {
                city: "Tokyo".into(),
                zone: chrono_tz::Asia::Tokyo,
                country: Some("Japan".into()),
                country_code: Some("JP".into()),
                coords: Some((35.65, 139.74)),
            },
        ]
    }

    #[test]
    fn world_clocks_roundtrip() {
        let cs = vec![
            WorldClock::from_location(&db()[1]),
            WorldClock::from_location(&db()[0]),
        ];
        let v = world_clocks_to_variant(&cs, None);
        assert_eq!(v.type_().as_str(), "aa{sv}");
        let back = world_clocks_from_variant(&v, &db(), None);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].zone, chrono_tz::Asia::Tokyo);
        assert_eq!(back[0].name, "Tokyo");
        let (la, lo) = back[0].coords.unwrap();
        assert!((la - 35.65).abs() < 1e-9 && (lo - 139.74).abs() < 1e-9);
        // The location payload is GWeather-shaped.
        let d = VariantDict::new(Some(&v.child_value(0)));
        let loc = d.lookup_value("location", None).unwrap();
        assert_eq!(loc.type_().as_str(), "(uv)");
        assert_eq!(
            loc.child_value(1).as_variant().unwrap().type_().as_str(),
            "(ssba(dd)a(dd))"
        );
    }

    #[test]
    fn gnome_written_location_resolves_by_coordinates() {
        // Munich, as GNOME would write it: city coordinates in radians.
        let ser: SerializedLocation = (
            "Munich".into(),
            "EDDM".into(),
            true,
            vec![(48.13f64.to_radians(), 11.57f64.to_radians())],
            vec![(48.35f64.to_radians(), 11.78f64.to_radians())],
        );
        let loc = (2u32, ser.to_variant()).to_variant();
        let d = VariantDict::new(None);
        d.insert_value("location", &loc);
        let v = array(vec![d.end()]);
        let back = world_clocks_from_variant(&v, &db(), None);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "Munich");
        assert_eq!(back[0].zone, chrono_tz::Europe::Berlin);
        assert_eq!(
            back[0].country, None,
            "country is not guessed from coordinates"
        );
        // With the GWeather database the station gives zone and country, and
        // the entry is written back unchanged.
        let gw = gwdb::Db::parse(gwdb::FIXTURE).unwrap();
        let back = world_clocks_from_variant(&v, &[], Some(&gw));
        assert_eq!(back[0].zone, chrono_tz::Europe::Berlin);
        assert_eq!(back[0].country.as_deref(), Some("Germany"));
        let again = world_clocks_to_variant(&back, Some(&gw));
        let inner = VariantDict::new(Some(&again.child_value(0)))
            .lookup_value("location", None)
            .unwrap();
        let got = inner
            .child_value(1)
            .as_variant()
            .unwrap()
            .get::<SerializedLocation>()
            .unwrap();
        assert_eq!(
            (got.0.as_str(), got.1.as_str(), got.2),
            ("Munich", "EDDM", true)
        );
        assert!((got.3[0].0 - ser.3[0].0).abs() < 1e-12 && (got.4[0].1 - ser.4[0].1).abs() < 1e-12);
    }

    #[test]
    fn helix_entries_get_nearest_station_code() {
        let gw = gwdb::Db::parse(gwdb::FIXTURE).unwrap();
        let mut c = WorldClock::from_location(&db()[0]);
        c.name = "Munich".into();
        c.coords = Some((48.15, 11.583333));
        let v = world_clocks_to_variant(&[c], Some(&gw));
        let loc = VariantDict::new(Some(&v.child_value(0)))
            .lookup_value("location", None)
            .unwrap();
        let (_, code, is_city, city, st) = loc
            .child_value(1)
            .as_variant()
            .unwrap()
            .get::<SerializedLocation>()
            .unwrap();
        assert_eq!(code, "EDDM");
        assert!(is_city);
        assert!((city[0].0.to_degrees() - 48.15).abs() < 1e-9);
        assert!((st[0].0.to_degrees() - 48.35).abs() < 1e-9);
    }

    #[test]
    fn junk_is_ignored() {
        let d = VariantDict::new(None);
        d.insert_value("location", &Variant::from_variant(&"nonsense".to_variant()));
        let v = array(vec![d.end()]);
        assert!(world_clocks_from_variant(&v, &db(), None).is_empty());
        assert!(world_clocks_from_variant(&5i32.to_variant(), &db(), None).is_empty());
        assert!(alarms_from_variant(&"x".to_variant()).is_empty());
    }

    #[test]
    fn alarms_roundtrip() {
        let a = Alarm {
            id: "abc".into(),
            name: "Wake up".into(),
            active: false,
            hour: 6,
            minute: 45,
            days: Weekdays::from_numbers(&[0, 1, 2, 3, 4]),
            ring_minutes: 3,
            snooze_minutes: 7,
        };
        let v = alarms_to_variant(std::slice::from_ref(&a));
        assert_eq!(v.type_().as_str(), "aa{sv}");
        let d = VariantDict::new(Some(&v.child_value(0)));
        assert_eq!(d.lookup_value("days", None).unwrap().type_().as_str(), "ai");
        assert_eq!(alarms_from_variant(&v), vec![a]);
    }

    #[test]
    fn alarm_defaults_and_validation() {
        let d = VariantDict::new(None);
        d.insert_value("hour", &7i32.to_variant());
        d.insert_value("minute", &0i32.to_variant());
        let bad = VariantDict::new(None);
        bad.insert_value("hour", &25i32.to_variant());
        bad.insert_value("minute", &0i32.to_variant());
        let v = array(vec![d.end(), bad.end()]);
        let alarms = alarms_from_variant(&v);
        assert_eq!(alarms.len(), 1);
        assert_eq!(alarms[0].id, "imported-0");
        assert_eq!(alarms[0].name, "Alarm");
        assert!(alarms[0].active);
        assert_eq!((alarms[0].ring_minutes, alarms[0].snooze_minutes), (5, 10));
    }

    #[test]
    fn timers_roundtrip() {
        let ts = vec![
            Timer::new("Tea", Duration::from_secs(180)),
            Timer::new("", Duration::from_secs(3600)),
        ];
        let v = timers_to_variant(&ts);
        assert_eq!(v.type_().as_str(), "aa{sv}");
        assert_eq!(timers_from_variant(&v), ts);
    }
}
