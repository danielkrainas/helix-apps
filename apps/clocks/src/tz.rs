//! Time zone database, location search and time zone math.

use chrono::{DateTime, NaiveDate, NaiveTime, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;
use std::str::FromStr;

const ZONEINFO: &str = "/usr/share/zoneinfo";

/// A searchable place: a time zone's principal city.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    /// City name, e.g. "Buenos Aires".
    pub city: String,
    /// IANA zone.
    pub zone: Tz,
    /// Country (or region) name, e.g. "Argentina".
    pub country: Option<String>,
    /// ISO 3166 country code of the principal country.
    pub country_code: Option<String>,
    /// (latitude, longitude) in degrees, east and north positive.
    pub coords: Option<(f64, f64)>,
}

impl Location {
    pub fn zone_id(&self) -> &'static str {
        self.zone.name()
    }

    /// Secondary text: the country or the zone's region.
    pub fn subtitle(&self) -> String {
        match &self.country {
            Some(c) => c.clone(),
            None => region_of(self.zone.name()).replace('_', " "),
        }
    }
}

/// "America/Argentina/Buenos_Aires" → "Buenos Aires".
pub fn city_from_zone(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).replace('_', " ")
}

/// "America/Argentina/Buenos_Aires" → "America".
pub fn region_of(id: &str) -> &str {
    id.split('/').next().unwrap_or(id)
}

/// Parses an ISO 6709 coordinate pair as used in zone.tab:
/// `±DDMM±DDDMM` or `±DDMMSS±DDDMMSS`. Returns degrees.
pub fn parse_coords(s: &str) -> Option<(f64, f64)> {
    let split = s[1..].find(['+', '-'])? + 1;
    let (lat, lon) = s.split_at(split);
    Some((parse_angle(lat, 2)?, parse_angle(lon, 3)?))
}

fn parse_angle(s: &str, deg_digits: usize) -> Option<f64> {
    let sign = match s.as_bytes().first()? {
        b'+' => 1.0,
        b'-' => -1.0,
        _ => return None,
    };
    let digits = &s[1..];
    if !digits.bytes().all(|b| b.is_ascii_digit()) || digits.len() < deg_digits + 2 {
        return None;
    }
    let d: f64 = digits[..deg_digits].parse().ok()?;
    let m: f64 = digits[deg_digits..deg_digits + 2].parse().ok()?;
    let sec: f64 = if digits.len() >= deg_digits + 4 { digits[deg_digits + 2..deg_digits + 4].parse().ok()? } else { 0.0 };
    Some(sign * (d + m / 60.0 + sec / 3600.0))
}

/// Parses `iso3166.tab` into code → country name.
pub fn parse_iso3166(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split('\t');
            Some((it.next()?.trim().to_string(), it.next()?.trim().to_string()))
        })
        .collect()
}

/// Parses `zone1970.tab` / `zone.tab` (both have codes, coordinates, TZ).
pub fn parse_zone_tab(text: &str, countries: &HashMap<String, String>) -> Vec<Location> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let Ok(zone) = Tz::from_str(cols[2].trim()) else { continue };
        let code = cols[0].split(',').next().map(|c| c.trim().to_string());
        out.push(Location {
            city: city_from_zone(cols[2].trim()),
            zone,
            country: code.as_ref().and_then(|c| countries.get(c).cloned()),
            country_code: code,
            coords: parse_coords(cols[1].trim()),
        });
    }
    out
}

/// Builds the location database from the system's zoneinfo tables, falling
/// back to the zones compiled into chrono-tz. `Etc/` zones and `UTC` are
/// always included.
pub fn load_locations() -> Vec<Location> {
    let countries = std::fs::read_to_string(format!("{ZONEINFO}/iso3166.tab"))
        .map(|t| parse_iso3166(&t))
        .unwrap_or_default();
    let tab = std::fs::read_to_string(format!("{ZONEINFO}/zone1970.tab"))
        .or_else(|_| std::fs::read_to_string(format!("{ZONEINFO}/zone.tab")))
        .unwrap_or_default();
    let mut locs = parse_zone_tab(&tab, &countries);
    let have_tab = !locs.is_empty();
    for tz in chrono_tz::TZ_VARIANTS {
        let name = tz.name();
        let extra = name == "UTC" || name.starts_with("Etc/GMT") && name.len() > 7;
        let fallback = !have_tab
            && matches!(
                region_of(name),
                "Africa" | "America" | "Antarctica" | "Asia" | "Atlantic" | "Australia" | "Europe" | "Indian" | "Pacific"
            );
        if (extra || fallback) && !locs.iter().any(|l| l.zone == tz) {
            let city = if name == "UTC" { "Coordinated Universal Time (UTC)".to_string() } else { city_from_zone(name) };
            locs.push(Location { city, zone: tz, country: None, country_code: None, coords: None });
        }
    }
    locs.sort_by(|a, b| a.city.cmp(&b.city));
    locs
}

/// Lowercases and strips common Latin diacritics for matching.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let r = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'č' => "c",
            'ď' | 'đ' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
            'ğ' => "g",
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' => "i",
            'ł' => "l",
            'ñ' | 'ń' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
            'œ' => "oe",
            'ř' => "r",
            'ś' | 'š' | 'ş' | 'ș' => "s",
            'ß' => "ss",
            'ť' | 'ţ' | 'ț' => "t",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
            'ý' | 'ÿ' => "y",
            'ž' | 'ź' | 'ż' => "z",
            '_' => " ",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(r);
    }
    out
}

/// Returns indices into `locs` matching `query`, best matches first.
/// Every whitespace-separated query word must match the city, zone ID,
/// country name or (exactly) the country code.
pub fn search(locs: &[Location], query: &str) -> Vec<usize> {
    let q = normalize(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<(u8, usize)> = Vec::new();
    for (i, l) in locs.iter().enumerate() {
        let city = normalize(&l.city);
        let zone = normalize(l.zone.name());
        let country = l.country.as_deref().map(normalize).unwrap_or_default();
        let code = l.country_code.as_deref().map(str::to_lowercase).unwrap_or_default();
        let all = words.iter().all(|w| city.contains(w) || zone.contains(w) || country.contains(w) || code == *w);
        if !all {
            continue;
        }
        let rank = if city == q {
            0
        } else if city.starts_with(&q) {
            1
        } else if city.split(' ').any(|p| p.starts_with(words[0])) {
            2
        } else if country.starts_with(&q) || code == q {
            3
        } else {
            4
        };
        hits.push((rank, i));
    }
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| locs[a.1].city.cmp(&locs[b.1].city)));
    hits.into_iter().map(|(_, i)| i).collect()
}

/// Great-circle distance in kilometres between two (lat, lon) degree pairs.
pub fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

/// The location (with coordinates) nearest to `coords` (degrees).
pub fn nearest(locs: &[Location], coords: (f64, f64)) -> Option<&Location> {
    locs.iter()
        .filter_map(|l| l.coords.map(|c| (distance_km(c, coords), l)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, l)| l)
}

/// Looks up a zone by IANA ID.
pub fn zone_by_id(id: &str) -> Option<Tz> {
    Tz::from_str(id).ok()
}

/// The system's local zone: `$TZ`, then `/etc/localtime`'s symlink target,
/// then `/etc/timezone`.
pub fn local_zone() -> Option<Tz> {
    if let Ok(tz) = std::env::var("TZ") {
        if let Some(z) = zone_by_id(tz.trim_start_matches(':')) {
            return Some(z);
        }
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(idx) = s.find("zoneinfo/") {
            if let Some(z) = zone_by_id(&s[idx + 9..]) {
                return Some(z);
            }
        }
    }
    std::fs::read_to_string("/etc/timezone").ok().and_then(|s| zone_by_id(s.trim()))
}

/// UTC offset of `zone` at `now`, in seconds.
pub fn utc_offset(zone: Tz, now: DateTime<Utc>) -> i32 {
    zone.offset_from_utc_datetime(&now.naive_utc()).fix().local_minus_utc()
}

/// Abbreviation (e.g. "CET", "EDT") of `zone` at `now`.
pub fn abbreviation(zone: Tz, now: DateTime<Utc>) -> String {
    now.with_timezone(&zone).format("%Z").to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeDay {
    Yesterday,
    Today,
    Tomorrow,
}

impl RelativeDay {
    pub fn label(self) -> &'static str {
        match self {
            RelativeDay::Yesterday => "Yesterday",
            RelativeDay::Today => "Today",
            RelativeDay::Tomorrow => "Tomorrow",
        }
    }
}

/// Compares the calendar date at `remote` with that at `local`.
pub fn relative_day(local_date: NaiveDate, remote_date: NaiveDate) -> RelativeDay {
    match remote_date.signed_duration_since(local_date).num_days() {
        d if d > 0 => RelativeDay::Tomorrow,
        d if d < 0 => RelativeDay::Yesterday,
        _ => RelativeDay::Today,
    }
}

/// Relative day of `zone` compared with `local` at `now`.
pub fn relative_day_at(zone: Tz, local: Tz, now: DateTime<Utc>) -> RelativeDay {
    relative_day(now.with_timezone(&local).date_naive(), now.with_timezone(&zone).date_naive())
}

/// Describes an offset difference (`remote - local`, seconds):
/// "9 hours ahead", "3½ hours behind", "45 minutes ahead",
/// "5 hours 45 minutes ahead", "Current time zone".
pub fn describe_offset(diff: i32) -> String {
    if diff == 0 {
        return "Current time zone".into();
    }
    let dir = if diff > 0 { "ahead" } else { "behind" };
    let mins = diff.unsigned_abs() / 60;
    let (h, m) = (mins / 60, mins % 60);
    let hours = |n: u32, frac: &str| {
        let plural = if n == 1 && frac.is_empty() { "hour" } else { "hours" };
        format!("{n}{frac} {plural}")
    };
    let amount = match (h, m) {
        (0, m) => format!("{m} minute{}", if m == 1 { "" } else { "s" }),
        (h, 0) => hours(h, ""),
        (h, 30) => hours(h, "½"),
        (h, 15) => hours(h, "¼"),
        (h, 45) => hours(h, "¾"),
        (h, m) => format!("{} {m} minutes", hours(h, "")),
    };
    format!("{amount} {dir}")
}

/// Formats a UTC offset as "UTC+05:30" / "UTC−03:00" / "UTC".
pub fn format_utc_offset(secs: i32) -> String {
    if secs == 0 {
        return "UTC".into();
    }
    let sign = if secs > 0 { '+' } else { '−' };
    let m = secs.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// Formats a wall time as "14:05" or "2:05 PM".
pub fn format_time(t: NaiveTime, twelve_hour: bool) -> String {
    if twelve_hour {
        let (pm, h) = t.hour12();
        format!("{h}:{:02} {}", t.minute(), if pm { "PM" } else { "AM" })
    } else {
        format!("{:02}:{:02}", t.hour(), t.minute())
    }
}

/// Like [`format_time`] with seconds.
pub fn format_time_secs(t: NaiveTime, twelve_hour: bool) -> String {
    if twelve_hour {
        let (pm, h) = t.hour12();
        format!("{h}:{:02}:{:02} {}", t.minute(), t.second(), if pm { "PM" } else { "AM" })
    } else {
        format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    const ZONE_TAB: &str = "# comment\n\
AR\t-3436-05827\tAmerica/Argentina/Buenos_Aires\tBuenos Aires (BA, CF)\n\
DE,DK,NO,SE,SJ\t+5230+01322\tEurope/Berlin\tmost of Germany\n\
GB,GG,IM,JE\t+513030-0000731\tEurope/London\n\
JP\t+353916+1394441\tAsia/Tokyo\n\
US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)\n\
IN\t+2232+08822\tAsia/Kolkata\n\
NP\t+2743+08519\tAsia/Kathmandu\n\
XX\t+0000+00000\tNot/AZone\n";
    const ISO: &str = "# c\nAR\tArgentina\nDE\tGermany\nGB\tBritain (UK)\nJP\tJapan\nUS\tUnited States\nIN\tIndia\nNP\tNepal\n";

    fn db() -> Vec<Location> {
        parse_zone_tab(ZONE_TAB, &parse_iso3166(ISO))
    }

    #[test]
    fn coords_parse() {
        let (lat, lon) = parse_coords("-3436-05827").unwrap();
        assert!((lat + 34.6).abs() < 1e-9);
        assert!((lon + 58.45).abs() < 1e-9);
        let (lat, lon) = parse_coords("+513030-0000731").unwrap();
        assert!((lat - (51.0 + 30.0 / 60.0 + 30.0 / 3600.0)).abs() < 1e-9);
        assert!((lon + (7.0 / 60.0 + 31.0 / 3600.0)).abs() < 1e-9);
        assert!(parse_coords("bogus").is_none());
    }

    #[test]
    fn zone_tab_parse() {
        let d = db();
        assert_eq!(d.len(), 7, "invalid zones are skipped");
        assert_eq!(d[0].city, "Buenos Aires");
        assert_eq!(d[0].country.as_deref(), Some("Argentina"));
        assert_eq!(d[1].country_code.as_deref(), Some("DE"));
        assert_eq!(city_from_zone("Etc/GMT+5"), "GMT+5");
    }

    #[test]
    fn search_ranks_and_normalizes() {
        let d = db();
        let names = |q: &str| search(&d, q).into_iter().map(|i| d[i].city.clone()).collect::<Vec<_>>();
        assert_eq!(names("tok"), vec!["Tokyo"]);
        assert_eq!(names("BUENOS"), vec!["Buenos Aires"]);
        assert_eq!(names("aires"), vec!["Buenos Aires"]);
        assert_eq!(names("japan"), vec!["Tokyo"]);
        assert_eq!(names("us"), vec!["New York"]);
        assert_eq!(names("new_york"), vec!["New York"]);
        assert_eq!(names("europe"), vec!["Berlin", "London"]);
        assert_eq!(names("tōkyo"), vec!["Tokyo"]);
        assert_eq!(normalize("Zürich São Paulo"), "zurich sao paulo");
        assert!(names("").is_empty());
        assert!(names("zzzz").is_empty());
    }

    #[test]
    fn nearest_zone() {
        let d = db();
        // Munich is nearest to Berlin among these.
        assert_eq!(nearest(&d, (48.14, 11.58)).unwrap().zone, chrono_tz::Europe::Berlin);
        // Yokohama → Tokyo.
        assert_eq!(nearest(&d, (35.44, 139.64)).unwrap().zone, chrono_tz::Asia::Tokyo);
        let km = distance_km((51.5, -0.12), (48.85, 2.35));
        assert!((km - 343.0).abs() < 5.0, "{km}");
    }

    #[test]
    fn offsets_and_dst() {
        let winter = Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap();
        let summer = Utc.with_ymd_and_hms(2024, 7, 15, 12, 0, 0).unwrap();
        assert_eq!(utc_offset(chrono_tz::America::New_York, winter), -5 * 3600);
        assert_eq!(utc_offset(chrono_tz::America::New_York, summer), -4 * 3600);
        assert_eq!(utc_offset(chrono_tz::Asia::Kolkata, summer), 5 * 3600 + 1800);
        assert_eq!(utc_offset(chrono_tz::Asia::Kathmandu, summer), 5 * 3600 + 2700);
        assert_eq!(abbreviation(chrono_tz::Europe::Berlin, summer), "CEST");
        assert_eq!(format_utc_offset(19800), "UTC+05:30");
        assert_eq!(format_utc_offset(-3 * 3600), "UTC−03:00");
        assert_eq!(format_utc_offset(0), "UTC");
    }

    #[test]
    fn relative_days() {
        use chrono_tz::{America::Los_Angeles, Asia::Tokyo, Europe::Berlin, Pacific::Kiritimati, Pacific::Pago_Pago};
        // 22:00 in Berlin (winter) = 06:00 next day in Tokyo, 13:00 same day in LA.
        let now = Utc.with_ymd_and_hms(2024, 1, 15, 21, 0, 0).unwrap();
        assert_eq!(relative_day_at(Tokyo, Berlin, now), RelativeDay::Tomorrow);
        assert_eq!(relative_day_at(Los_Angeles, Berlin, now), RelativeDay::Today);
        // 00:30 in Berlin = previous day in LA.
        let now = Utc.with_ymd_and_hms(2024, 1, 14, 23, 30, 0).unwrap();
        assert_eq!(relative_day_at(Los_Angeles, Berlin, now), RelativeDay::Yesterday);
        // Across the date line: UTC+14 vs UTC-11 are 25 h apart.
        let now = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap();
        assert_eq!(relative_day_at(Kiritimati, Pago_Pago, now), RelativeDay::Tomorrow);
        assert_eq!(relative_day_at(Pago_Pago, Kiritimati, now), RelativeDay::Yesterday);
        // Year boundary.
        assert_eq!(
            relative_day(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap(), NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
            RelativeDay::Tomorrow
        );
        let tokyo_date = now.with_timezone(&Tokyo).date_naive();
        assert_eq!(tokyo_date.day(), 15);
    }

    #[test]
    fn offset_descriptions() {
        assert_eq!(describe_offset(0), "Current time zone");
        assert_eq!(describe_offset(9 * 3600), "9 hours ahead");
        assert_eq!(describe_offset(-3600), "1 hour behind");
        assert_eq!(describe_offset(-(3 * 3600 + 1800)), "3½ hours behind");
        assert_eq!(describe_offset(1800), "30 minutes ahead");
        assert_eq!(describe_offset(5 * 3600 + 45 * 60), "5¾ hours ahead");
        assert_eq!(describe_offset(5 * 3600 + 20 * 60), "5 hours 20 minutes ahead");
        assert_eq!(describe_offset(60), "1 minute ahead");
    }

    #[test]
    fn time_formats() {
        let t = NaiveTime::from_hms_opt(14, 5, 9).unwrap();
        assert_eq!(format_time(t, false), "14:05");
        assert_eq!(format_time(t, true), "2:05 PM");
        assert_eq!(format_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap(), true), "12:00 AM");
        assert_eq!(format_time_secs(t, false), "14:05:09");
        assert_eq!(format_time_secs(t, true), "2:05:09 PM");
    }

    #[test]
    fn system_db_loads_utc() {
        let all = load_locations();
        assert!(all.iter().any(|l| l.zone == chrono_tz::UTC));
        assert!(all.iter().any(|l| l.zone.name() == "Etc/GMT+5"));
    }
}
