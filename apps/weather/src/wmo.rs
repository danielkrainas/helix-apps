//! WMO weather interpretation codes (as used by Open-Meteo) mapped to
//! human-readable descriptions and standard symbolic icon names.

use helix_app_common::i18n::tr;

/// Human-readable description of a WMO weather code.
pub fn description(code: u8) -> &'static str {
    match code {
        0 => tr("Clear sky"),
        1 => tr("Mainly clear"),
        2 => tr("Partly cloudy"),
        3 => tr("Overcast"),
        45 => tr("Fog"),
        48 => tr("Depositing rime fog"),
        51 => tr("Light drizzle"),
        53 => tr("Moderate drizzle"),
        55 => tr("Dense drizzle"),
        56 => tr("Light freezing drizzle"),
        57 => tr("Dense freezing drizzle"),
        61 => tr("Slight rain"),
        63 => tr("Moderate rain"),
        65 => tr("Heavy rain"),
        66 => tr("Light freezing rain"),
        67 => tr("Heavy freezing rain"),
        71 => tr("Slight snow fall"),
        73 => tr("Moderate snow fall"),
        75 => tr("Heavy snow fall"),
        77 => tr("Snow grains"),
        80 => tr("Slight rain showers"),
        81 => tr("Moderate rain showers"),
        82 => tr("Violent rain showers"),
        85 => tr("Slight snow showers"),
        86 => tr("Heavy snow showers"),
        95 => tr("Thunderstorm"),
        96 => tr("Thunderstorm with slight hail"),
        99 => tr("Thunderstorm with heavy hail"),
        _ => tr("Unknown"),
    }
}

/// Base icon name (without `-symbolic`) for a WMO code.
pub fn icon_base(code: u8, is_day: bool) -> &'static str {
    match code {
        0 => {
            if is_day {
                "weather-clear"
            } else {
                "weather-clear-night"
            }
        }
        1 | 2 => {
            if is_day {
                "weather-few-clouds"
            } else {
                "weather-few-clouds-night"
            }
        }
        3 => "weather-overcast",
        45 | 48 => "weather-fog",
        51..=57 | 61 | 80 => "weather-showers-scattered",
        63 | 65 | 66 | 67 | 81 | 82 => "weather-showers",
        71..=77 | 85 | 86 => "weather-snow",
        95 | 96 | 99 => "weather-storm",
        _ => "weather-severe-alert",
    }
}

/// Symbolic icon name for a WMO code.
pub fn icon_name(code: u8, is_day: bool) -> String {
    format!("{}-symbolic", icon_base(code, is_day))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions() {
        assert_eq!(description(0), "Clear sky");
        assert_eq!(description(3), "Overcast");
        assert_eq!(description(65), "Heavy rain");
        assert_eq!(description(99), "Thunderstorm with heavy hail");
        assert_eq!(description(42), "Unknown");
    }

    #[test]
    fn icons_day_night() {
        assert_eq!(icon_name(0, true), "weather-clear-symbolic");
        assert_eq!(icon_name(0, false), "weather-clear-night-symbolic");
        assert_eq!(icon_name(2, false), "weather-few-clouds-night-symbolic");
        assert_eq!(icon_name(3, false), "weather-overcast-symbolic");
    }

    #[test]
    fn icons_precipitation() {
        assert_eq!(icon_base(45, true), "weather-fog");
        assert_eq!(icon_base(53, true), "weather-showers-scattered");
        assert_eq!(icon_base(61, true), "weather-showers-scattered");
        assert_eq!(icon_base(63, true), "weather-showers");
        assert_eq!(icon_base(82, true), "weather-showers");
        assert_eq!(icon_base(73, true), "weather-snow");
        assert_eq!(icon_base(86, true), "weather-snow");
        assert_eq!(icon_base(95, true), "weather-storm");
        assert_eq!(icon_base(200, true), "weather-severe-alert");
    }

    #[test]
    fn every_known_code_has_description_and_icon() {
        for code in [
            0, 1, 2, 3, 45, 48, 51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 71, 73, 75, 77, 80, 81, 82,
            85, 86, 95, 96, 99,
        ] {
            assert_ne!(description(code), "Unknown", "code {code}");
            assert_ne!(icon_base(code, true), "weather-severe-alert", "code {code}");
        }
    }
}
