//! Sunrise / sunset (NOAA "sunrise equation" approximation, ~1–2 min).

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SunTimes {
    Normal {
        rise: DateTime<Utc>,
        set: DateTime<Utc>,
    },
    /// The sun stays above the horizon all day.
    PolarDay,
    /// The sun stays below the horizon all day.
    PolarNight,
}

const J2000: f64 = 2_451_545.0;
const UNIX_EPOCH_JD: f64 = 2_440_587.5;

fn jd_to_utc(jd: f64) -> DateTime<Utc> {
    let ms = ((jd - UNIX_EPOCH_JD) * 86_400_000.0).round() as i64;
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

/// Sunrise and sunset for the solar day of `date` at (lat, lon) degrees
/// (east positive).
pub fn sun_times(date: NaiveDate, lat: f64, lon: f64) -> SunTimes {
    let noon_utc = date.and_hms_opt(12, 0, 0).unwrap().and_utc();
    let jd_noon = noon_utc.timestamp() as f64 / 86_400.0 + UNIX_EPOCH_JD;
    let n = (jd_noon - J2000 + 0.0008).round();
    let j_star = n - lon / 360.0;
    let m = (357.5291 + 0.985_600_28 * j_star).rem_euclid(360.0);
    let mr = m.to_radians();
    let c = 1.9148 * mr.sin() + 0.02 * (2.0 * mr).sin() + 0.0003 * (3.0 * mr).sin();
    let lambda = (m + c + 180.0 + 102.9372).rem_euclid(360.0).to_radians();
    let transit = J2000 + j_star + 0.0053 * mr.sin() - 0.0069 * (2.0 * lambda).sin();
    let sin_d = lambda.sin() * 23.4397f64.to_radians().sin();
    let cos_d = sin_d.asin().cos();
    let phi = lat.to_radians();
    let cos_w = ((-0.833f64).to_radians().sin() - phi.sin() * sin_d) / (phi.cos() * cos_d);
    if cos_w > 1.0 {
        return SunTimes::PolarNight;
    }
    if cos_w < -1.0 {
        return SunTimes::PolarDay;
    }
    let w = cos_w.acos().to_degrees();
    SunTimes::Normal {
        rise: jd_to_utc(transit - w / 360.0),
        set: jd_to_utc(transit + w / 360.0),
    }
}

/// Whether the sun is up at `now` for a place whose local date is
/// `local_date`.
pub fn is_day(now: DateTime<Utc>, local_date: NaiveDate, lat: f64, lon: f64) -> bool {
    match sun_times(local_date, lat, lon) {
        SunTimes::Normal { rise, set } => now >= rise && now < set,
        SunTimes::PolarDay => true,
        SunTimes::PolarNight => false,
    }
}

/// Fallback when coordinates are unknown: 07:00–19:00 local.
pub fn is_day_by_hour(hour: u32) -> bool {
    (7..19).contains(&hour)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: DateTime<Utc>, b: DateTime<Utc>, mins: i64) -> bool {
        (a - b).num_minutes().abs() <= mins
    }

    #[test]
    fn london_midsummer() {
        let d = NaiveDate::from_ymd_opt(2024, 6, 21).unwrap();
        let SunTimes::Normal { rise, set } = sun_times(d, 51.5074, -0.1278) else {
            panic!()
        };
        // Published: sunrise 04:43 BST (03:43 UTC), sunset 21:21 BST (20:21 UTC).
        assert!(
            close(
                rise,
                Utc.with_ymd_and_hms(2024, 6, 21, 3, 43, 0).unwrap(),
                3
            ),
            "{rise}"
        );
        assert!(
            close(
                set,
                Utc.with_ymd_and_hms(2024, 6, 21, 20, 21, 0).unwrap(),
                3
            ),
            "{set}"
        );
    }

    #[test]
    fn tokyo_winter() {
        let d = NaiveDate::from_ymd_opt(2024, 12, 21).unwrap();
        let SunTimes::Normal { rise, set } = sun_times(d, 35.6895, 139.6917) else {
            panic!()
        };
        // Published: sunrise 06:47 JST (21:47 UTC prev day), sunset 16:32 JST (07:32 UTC).
        assert!(
            close(
                rise,
                Utc.with_ymd_and_hms(2024, 12, 20, 21, 47, 0).unwrap(),
                3
            ),
            "{rise}"
        );
        assert!(
            close(
                set,
                Utc.with_ymd_and_hms(2024, 12, 21, 7, 32, 0).unwrap(),
                3
            ),
            "{set}"
        );
        assert!(is_day(
            Utc.with_ymd_and_hms(2024, 12, 21, 3, 0, 0).unwrap(),
            d,
            35.6895,
            139.6917
        ));
        assert!(!is_day(
            Utc.with_ymd_and_hms(2024, 12, 21, 12, 0, 0).unwrap(),
            d,
            35.6895,
            139.6917
        ));
    }

    #[test]
    fn polar() {
        let summer = NaiveDate::from_ymd_opt(2024, 6, 21).unwrap();
        let winter = NaiveDate::from_ymd_opt(2024, 12, 21).unwrap();
        // Longyearbyen, Svalbard.
        assert_eq!(sun_times(summer, 78.22, 15.65), SunTimes::PolarDay);
        assert_eq!(sun_times(winter, 78.22, 15.65), SunTimes::PolarNight);
        // McMurdo is the opposite.
        assert_eq!(sun_times(summer, -77.85, 166.67), SunTimes::PolarNight);
    }

    #[test]
    fn hour_fallback() {
        assert!(!is_day_by_hour(6));
        assert!(is_day_by_hour(7));
        assert!(is_day_by_hour(18));
        assert!(!is_day_by_hour(19));
    }
}
