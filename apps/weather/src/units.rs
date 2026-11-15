//! Unit handling: GWeather unit preferences, locale defaults, conversion and
//! formatting. Raw data is always metric (°C, km/h, hPa).

use helix_app_common::i18n::trc;
use helix_app_common::trf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temperature {
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    MetersPerSecond,
    Kph,
    Mph,
    Knots,
    Beaufort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    Kpa,
    Hpa,
    Mb,
    MmHg,
    InHg,
    Atm,
}

/// Whether a locale string (e.g. `en_US.UTF-8`) uses imperial/US customary
/// measurements by default.
pub fn locale_is_imperial(locale: &str) -> bool {
    let tag = locale.split(['.', '@']).next().unwrap_or("");
    matches!(tag, "en_US" | "en_LR" | "my_MM" | "es_US")
}

/// The effective measurement locale from the environment.
pub fn measurement_locale() -> String {
    for var in ["LC_ALL", "LC_MEASUREMENT", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    String::new()
}

impl Temperature {
    /// Resolves a GWeather `temperature-unit` nick.
    pub fn from_nick(nick: &str, imperial: bool) -> Self {
        match nick {
            "fahrenheit" => Temperature::Fahrenheit,
            "centigrade" | "kelvin" => Temperature::Celsius,
            _ if imperial => Temperature::Fahrenheit,
            _ => Temperature::Celsius,
        }
    }

    pub fn nick(self) -> &'static str {
        match self {
            Temperature::Celsius => "centigrade",
            Temperature::Fahrenheit => "fahrenheit",
        }
    }

    pub fn convert(self, celsius: f64) -> f64 {
        match self {
            Temperature::Celsius => celsius,
            Temperature::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Temperature::Celsius => "°C",
            Temperature::Fahrenheit => "°F",
        }
    }

    /// Rounded value with a bare degree sign, e.g. `21°`.
    pub fn format_short(self, celsius: f64) -> String {
        format!("{}°", round_i(self.convert(celsius)))
    }

    /// Rounded value with unit, e.g. `21 °C`.
    pub fn format(self, celsius: f64) -> String {
        format!("{} {}", round_i(self.convert(celsius)), self.symbol())
    }
}

fn round_i(v: f64) -> i64 {
    // Avoid printing "-0".
    let r = v.round() as i64;
    if r == 0 {
        0
    } else {
        r
    }
}

impl Speed {
    pub fn from_nick(nick: &str, imperial: bool) -> Self {
        match nick {
            "ms" => Speed::MetersPerSecond,
            "kph" => Speed::Kph,
            "mph" => Speed::Mph,
            "knots" => Speed::Knots,
            "bft" => Speed::Beaufort,
            _ if imperial => Speed::Mph,
            _ => Speed::Kph,
        }
    }

    pub fn convert(self, kph: f64) -> f64 {
        match self {
            Speed::MetersPerSecond => kph / 3.6,
            Speed::Kph => kph,
            Speed::Mph => kph / 1.609_344,
            Speed::Knots => kph / 1.852,
            Speed::Beaufort => beaufort(kph) as f64,
        }
    }

    pub fn format(self, kph: f64) -> String {
        let v = self.convert(kph);
        match self {
            Speed::MetersPerSecond => format!("{v:.1} m/s"),
            Speed::Kph => format!("{} km/h", v.round() as i64),
            Speed::Mph => trf!("{} mph", v.round() as i64),
            Speed::Knots => format!("{} kn", v.round() as i64),
            Speed::Beaufort => trf!("Beaufort {}", v as i64),
        }
    }
}

/// Beaufort force number for a wind speed in km/h.
pub fn beaufort(kph: f64) -> u8 {
    const LIMITS: [f64; 12] = [
        1.0, 6.0, 12.0, 20.0, 29.0, 39.0, 50.0, 62.0, 75.0, 89.0, 103.0, 118.0,
    ];
    LIMITS.iter().position(|&l| kph < l).unwrap_or(12) as u8
}

impl Pressure {
    pub fn from_nick(nick: &str, imperial: bool) -> Self {
        match nick {
            "kpa" => Pressure::Kpa,
            "hpa" => Pressure::Hpa,
            "mb" => Pressure::Mb,
            "mm-hg" => Pressure::MmHg,
            "inch-hg" => Pressure::InHg,
            "atm" => Pressure::Atm,
            _ if imperial => Pressure::InHg,
            _ => Pressure::Hpa,
        }
    }

    pub fn convert(self, hpa: f64) -> f64 {
        match self {
            Pressure::Kpa => hpa / 10.0,
            Pressure::Hpa | Pressure::Mb => hpa,
            Pressure::MmHg => hpa * 0.750_061_68,
            Pressure::InHg => hpa * 0.029_529_983,
            Pressure::Atm => hpa / 1013.25,
        }
    }

    pub fn format(self, hpa: f64) -> String {
        let v = self.convert(hpa);
        match self {
            Pressure::Kpa => trf!("{v} kPa", v = helix_app_common::i18n::format_number(v, 1)),
            Pressure::Hpa => trf!("{v} hPa", v = helix_app_common::i18n::format_number(v, 0)),
            Pressure::Mb => trf!("{v} mb", v = helix_app_common::i18n::format_number(v, 0)),
            Pressure::MmHg => trf!("{v} mmHg", v = helix_app_common::i18n::format_number(v, 0)),
            Pressure::InHg => trf!("{v} inHg", v = helix_app_common::i18n::format_number(v, 2)),
            Pressure::Atm => trf!("{v} atm", v = helix_app_common::i18n::format_number(v, 3)),
        }
    }
}

/// 16-point compass abbreviation for a direction in degrees (where the wind
/// comes from).
pub fn compass(degrees: f64) -> String {
    let d = degrees.rem_euclid(360.0);
    match ((d / 22.5).round() as usize) % 16 {
        // Translators: compass points, abbreviated (N = north, NNE =
        // north-northeast, …), after a wind speed: "12 km/h NNE".
        0 => trc("wind direction", "N"),
        1 => trc("wind direction", "NNE"),
        2 => trc("wind direction", "NE"),
        3 => trc("wind direction", "ENE"),
        4 => trc("wind direction", "E"),
        5 => trc("wind direction", "ESE"),
        6 => trc("wind direction", "SE"),
        7 => trc("wind direction", "SSE"),
        8 => trc("wind direction", "S"),
        9 => trc("wind direction", "SSW"),
        10 => trc("wind direction", "SW"),
        11 => trc("wind direction", "WSW"),
        12 => trc("wind direction", "W"),
        13 => trc("wind direction", "WNW"),
        14 => trc("wind direction", "NW"),
        _ => trc("wind direction", "NNW"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_defaults() {
        assert!(locale_is_imperial("en_US.UTF-8"));
        assert!(locale_is_imperial("en_US"));
        assert!(!locale_is_imperial("en_GB.UTF-8"));
        assert!(!locale_is_imperial("de_DE.UTF-8@euro"));
        assert!(!locale_is_imperial("C"));
        assert!(!locale_is_imperial(""));
        assert_eq!(
            Temperature::from_nick("default", true),
            Temperature::Fahrenheit
        );
        assert_eq!(
            Temperature::from_nick("default", false),
            Temperature::Celsius
        );
        assert_eq!(
            Temperature::from_nick("fahrenheit", false),
            Temperature::Fahrenheit
        );
        assert_eq!(Temperature::from_nick("kelvin", true), Temperature::Celsius);
        assert_eq!(Speed::from_nick("default", true), Speed::Mph);
        assert_eq!(Speed::from_nick("default", false), Speed::Kph);
        assert_eq!(Pressure::from_nick("default", true), Pressure::InHg);
        assert_eq!(Pressure::from_nick("bogus", false), Pressure::Hpa);
    }

    #[test]
    fn temperature_conversion() {
        assert_eq!(Temperature::Fahrenheit.convert(0.0), 32.0);
        assert_eq!(Temperature::Fahrenheit.convert(100.0), 212.0);
        assert_eq!(Temperature::Fahrenheit.convert(-40.0), -40.0);
        assert_eq!(Temperature::Celsius.convert(21.4), 21.4);
        assert_eq!(Temperature::Celsius.format_short(-0.3), "0°");
        assert_eq!(Temperature::Fahrenheit.format(20.0), "68 °F");
        assert_eq!(Temperature::Celsius.format(3.6), "4 °C");
    }

    #[test]
    fn speed_conversion() {
        assert!((Speed::MetersPerSecond.convert(36.0) - 10.0).abs() < 1e-9);
        assert!((Speed::Mph.convert(160.9344) - 100.0).abs() < 1e-9);
        assert!((Speed::Knots.convert(18.52) - 10.0).abs() < 1e-9);
        assert_eq!(Speed::Kph.format(11.9), "12 km/h");
        assert_eq!(Speed::Mph.format(16.09344), "10 mph");
        assert_eq!(beaufort(0.5), 0);
        assert_eq!(beaufort(15.0), 3);
        assert_eq!(beaufort(130.0), 12);
        assert_eq!(Speed::Beaufort.format(15.0), "Beaufort 3");
    }

    #[test]
    fn pressure_conversion() {
        assert!((Pressure::Atm.convert(1013.25) - 1.0).abs() < 1e-9);
        assert!((Pressure::InHg.convert(1013.25) - 29.92).abs() < 0.01);
        assert!((Pressure::MmHg.convert(1013.25) - 760.0).abs() < 0.1);
        assert_eq!(Pressure::Hpa.format(997.5), "998 hPa");
        assert_eq!(Pressure::Kpa.format(997.5), "99.8 kPa");
    }

    #[test]
    fn compass_points() {
        assert_eq!(compass(0.0), "N");
        assert_eq!(compass(359.0), "N");
        assert_eq!(compass(45.0), "NE");
        assert_eq!(compass(256.0), "WSW");
        assert_eq!(compass(315.0), "NW");
        assert_eq!(compass(-90.0), "W");
    }
}
