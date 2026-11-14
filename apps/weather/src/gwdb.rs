//! Read-only access to the GWeather location database
//! (`$XDG_DATA_DIRS/libgweather-4/Locations.xml`, a public data file).
//!
//! GNOME Weather's serialized locations refer to a weather station by its
//! METAR code; the database resolves it to a region, country and time zone
//! on import, and names the automatic location offline (nearest city).
//! Everything degrades gracefully when the file is missing.

use std::path::PathBuf;

/// A weather station (`<location>` with a `<code>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Station {
    pub name: String,
    pub code: String,
    /// (lat, lon) degrees.
    pub coords: (f64, f64),
    pub tz: Option<String>,
    pub country: Option<String>,
    pub state: Option<String>,
}

/// A city (`<city>`).
#[derive(Debug, Clone, PartialEq)]
pub struct City {
    pub name: String,
    pub coords: (f64, f64),
    pub tz: Option<String>,
    pub country: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Default)]
pub struct Db {
    pub stations: Vec<Station>,
    pub cities: Vec<City>,
}

#[derive(Clone, Default)]
struct Ctx {
    tz: Option<String>,
    country: Option<String>,
    state: Option<String>,
}

fn child_text<'a>(n: roxmltree::Node<'a, 'a>, tag: &str) -> Option<&'a str> {
    n.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .map(str::trim)
}

fn node_name(n: roxmltree::Node) -> Option<String> {
    child_text(n, "_name")
        .or_else(|| child_text(n, "name"))
        .map(str::to_string)
}

fn coords(n: roxmltree::Node) -> Option<(f64, f64)> {
    let t = child_text(n, "coordinates")?;
    let mut it = t.split_whitespace().map(|v| v.parse::<f64>().ok());
    Some((it.next()??, it.next()??))
}

/// The node's own time zone: `<tz-hint>`, else the only `<timezone>`.
fn own_tz(n: roxmltree::Node) -> Option<String> {
    if let Some(h) = child_text(n, "tz-hint").filter(|h| !h.is_empty()) {
        return Some(h.to_string());
    }
    let zones: Vec<&str> = n
        .children()
        .filter(|c| c.has_tag_name("timezones"))
        .flat_map(|c| {
            c.children()
                .filter(|z| z.has_tag_name("timezone"))
                .filter_map(|z| z.attribute("id"))
        })
        .collect();
    (zones.len() == 1).then(|| zones[0].to_string())
}

fn walk(n: roxmltree::Node, ctx: &Ctx, db: &mut Db) {
    for c in n.children().filter(|c| c.is_element()) {
        let tag = c.tag_name().name();
        if !matches!(tag, "region" | "country" | "state" | "city" | "location") {
            continue;
        }
        let mut sub = ctx.clone();
        if let Some(tz) = own_tz(c) {
            sub.tz = Some(tz);
        }
        match tag {
            "country" => {
                sub.country = node_name(c);
                sub.state = None;
            }
            "state" => sub.state = node_name(c),
            "city" => {
                if let (Some(name), Some(co)) = (node_name(c), coords(c)) {
                    db.cities.push(City {
                        name,
                        coords: co,
                        tz: sub.tz.clone(),
                        country: sub.country.clone(),
                        state: sub.state.clone(),
                    });
                }
            }
            "location" => {
                if let (Some(name), Some(code), Some(co)) =
                    (node_name(c), child_text(c, "code"), coords(c))
                {
                    db.stations.push(Station {
                        name,
                        code: code.to_string(),
                        coords: co,
                        tz: sub.tz.clone(),
                        country: sub.country.clone(),
                        state: sub.state.clone(),
                    });
                }
            }
            _ => {}
        }
        walk(c, &sub, db);
    }
}

/// Great-circle distance in kilometres.
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (
        a.0.to_radians(),
        a.1.to_radians(),
        b.0.to_radians(),
        b.1.to_radians(),
    );
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

impl Db {
    pub fn parse(xml: &str) -> Option<Db> {
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(xml, opts).ok()?;
        let mut db = Db::default();
        walk(doc.root_element(), &Ctx::default(), &mut db);
        Some(db)
    }

    /// Loads the installed database, if any.
    pub fn load() -> Option<Db> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(v) = std::env::var("XDG_DATA_DIRS") {
            dirs.extend(v.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
        }
        dirs.extend(["/usr/local/share", "/usr/share"].map(PathBuf::from));
        dirs.iter()
            .map(|d| d.join("libgweather-4/Locations.xml"))
            .find_map(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| Db::parse(&s))
    }

    pub fn station(&self, code: &str) -> Option<&Station> {
        (!code.is_empty())
            .then(|| self.stations.iter().find(|s| s.code == code))
            .flatten()
    }

    /// The nearest city within `max_km`.
    pub fn nearest_city(&self, at: (f64, f64), max_km: f64) -> Option<&City> {
        self.cities
            .iter()
            .map(|c| (distance_km(c.coords, at), c))
            .filter(|(d, _)| *d <= max_km)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, c)| c)
    }
}

#[cfg(test)]
pub(crate) const FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE gweather SYSTEM "locations.dtd">
<gweather format="1.0">
  <region>
    <_name>Europe</_name>
    <country>
      <_name>Germany</_name>
      <iso-code>DE</iso-code>
      <timezones><timezone id="Europe/Berlin" /></timezones>
      <tz-hint>Europe/Berlin</tz-hint>
      <state>
        <_name>Bavaria</_name>
        <location>
          <name>Munich</name>
          <code>EDDM</code>
          <coordinates>48.350000 11.783333</coordinates>
        </location>
        <city>
          <!-- comment -->
          <_name>Munich</_name>
          <coordinates>48.150000 11.583333</coordinates>
        </city>
      </state>
    </country>
    <country>
      <_name>United States</_name>
      <timezones><timezone id="America/New_York" /><timezone id="America/Chicago" /></timezones>
      <state>
        <_name>Illinois</_name>
        <tz-hint>America/Chicago</tz-hint>
        <location>
          <name>Chicago O'Hare</name>
          <code>KORD</code>
          <coordinates>41.983333 -87.916667</coordinates>
        </location>
      </state>
      <location>
        <name>No zone</name>
        <code>XXXX</code>
        <coordinates>40 -100</coordinates>
      </location>
    </country>
  </region>
</gweather>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hierarchy_and_inherits_zones() {
        let db = Db::parse(FIXTURE).unwrap();
        assert_eq!(db.stations.len(), 3);
        assert_eq!(db.cities.len(), 1);
        let m = db.station("EDDM").unwrap();
        assert_eq!(m.tz.as_deref(), Some("Europe/Berlin"));
        assert_eq!(m.country.as_deref(), Some("Germany"));
        assert_eq!(m.state.as_deref(), Some("Bavaria"));
        assert_eq!(
            db.station("KORD").unwrap().tz.as_deref(),
            Some("America/Chicago")
        );
        assert_eq!(
            db.station("XXXX").unwrap().tz,
            None,
            "ambiguous country zone"
        );
        assert!(db.station("").is_none());
        let c = &db.cities[0];
        assert_eq!((c.name.as_str(), c.coords), ("Munich", (48.15, 11.583333)));
    }

    #[test]
    fn nearest_lookups() {
        let db = Db::parse(FIXTURE).unwrap();
        assert_eq!(db.nearest_city((48.2, 11.6), 50.0).unwrap().name, "Munich");
        assert!(db.nearest_city((0.0, 0.0), 50.0).is_none());
    }

    #[test]
    fn installed_database_if_present() {
        let t = std::time::Instant::now();
        let Some(db) = Db::load() else { return };
        eprintln!(
            "Locations.xml: {} stations, {} cities in {:?}",
            db.stations.len(),
            db.cities.len(),
            t.elapsed()
        );
        let m = db.station("EDDM").unwrap();
        assert_eq!(m.tz.as_deref(), Some("Europe/Berlin"));
        let c = db.nearest_city((48.14, 11.58), 50.0).unwrap();
        assert_eq!(c.name, "Munich");
    }

    #[test]
    fn junk() {
        assert!(Db::parse("<nope").is_none());
        assert!(Db::parse("<gweather/>").unwrap().stations.is_empty());
    }
}
