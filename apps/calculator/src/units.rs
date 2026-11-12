//! Unit conversion: `<expr> <unit> in <unit>`.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Length,
    Area,
    Volume,
    Mass,
    Time,
    Temperature,
    Speed,
    Data,
    Angle,
}

pub struct Unit {
    pub category: Category,
    pub names: &'static [&'static str],
    /// Size of one unit in the category's base unit (unused for temperature).
    pub factor: f64,
}

macro_rules! u {
    ($cat:ident, $f:expr, [$($n:expr),+]) => {
        Unit { category: Category::$cat, names: &[$($n),+], factor: $f }
    };
}

pub const UNITS: &[Unit] = &[
    // Length (metre)
    u!(Length, 1e3, ["km", "kilometer", "kilometers", "kilometre", "kilometres"]),
    u!(Length, 1.0, ["m", "meter", "meters", "metre", "metres"]),
    u!(Length, 1e-2, ["cm", "centimeter", "centimeters", "centimetre", "centimetres"]),
    u!(Length, 1e-3, ["mm", "millimeter", "millimeters", "millimetre", "millimetres"]),
    u!(Length, 1e-6, ["um", "µm", "micrometer", "micrometers", "micron", "microns"]),
    u!(Length, 1e-9, ["nm", "nanometer", "nanometers"]),
    u!(Length, 0.0254, ["in", "inch", "inches"]),
    u!(Length, 0.3048, ["ft", "foot", "feet"]),
    u!(Length, 0.9144, ["yd", "yard", "yards"]),
    u!(Length, 1609.344, ["mi", "mile", "miles"]),
    u!(Length, 1852.0, ["nmi", "nauticalmile", "nauticalmiles"]),
    // Area (square metre)
    u!(Area, 1e6, ["km²", "km2", "sqkm"]),
    u!(Area, 1e4, ["ha", "hectare", "hectares"]),
    u!(Area, 1.0, ["m²", "m2", "sqm"]),
    u!(Area, 1e-4, ["cm²", "cm2"]),
    u!(Area, 4046.856_422_4, ["acre", "acres"]),
    u!(Area, 0.092_903_04, ["ft²", "ft2", "sqft"]),
    u!(Area, 0.000_645_16, ["in²", "in2", "sqin"]),
    u!(Area, 2_589_988.110_336, ["mi²", "mi2", "sqmi"]),
    // Volume (litre)
    u!(Volume, 1000.0, ["m³", "m3"]),
    u!(Volume, 1.0, ["l", "L", "litre", "litres", "liter", "liters"]),
    u!(Volume, 1e-3, ["ml", "mL", "millilitre", "milliliter", "cm³", "cm3"]),
    u!(Volume, 3.785_411_784, ["gal", "gallon", "gallons"]),
    u!(Volume, 0.946_352_946, ["qt", "quart", "quarts"]),
    u!(Volume, 0.473_176_473, ["pt", "pint", "pints"]),
    u!(Volume, 0.236_588_236_5, ["cup", "cups"]),
    u!(Volume, 0.029_573_529_562_5, ["floz"]),
    u!(Volume, 0.014_786_764_781_25, ["tbsp", "tablespoon", "tablespoons"]),
    u!(Volume, 0.004_928_921_593_75, ["tsp", "teaspoon", "teaspoons"]),
    // Mass (kilogram)
    u!(Mass, 1000.0, ["t", "tonne", "tonnes"]),
    u!(Mass, 1.0, ["kg", "kilogram", "kilograms"]),
    u!(Mass, 1e-3, ["g", "gram", "grams"]),
    u!(Mass, 1e-6, ["mg", "milligram", "milligrams"]),
    u!(Mass, 0.453_592_37, ["lb", "lbs", "pound", "pounds"]),
    u!(Mass, 0.028_349_523_125, ["oz", "ounce", "ounces"]),
    u!(Mass, 6.350_293_18, ["st", "stone", "stones"]),
    // Time (second)
    u!(Time, 31_557_600.0, ["yr", "year", "years"]),
    u!(Time, 604_800.0, ["wk", "week", "weeks"]),
    u!(Time, 86_400.0, ["d", "day", "days"]),
    u!(Time, 3600.0, ["h", "hr", "hour", "hours"]),
    u!(Time, 60.0, ["min", "minute", "minutes"]),
    u!(Time, 1.0, ["s", "sec", "second", "seconds"]),
    u!(Time, 1e-3, ["ms", "millisecond", "milliseconds"]),
    // Speed (metre per second)
    u!(Speed, 1.0, ["m/s", "mps"]),
    u!(Speed, 1.0 / 3.6, ["km/h", "kph", "kmh"]),
    u!(Speed, 0.447_04, ["mph"]),
    u!(Speed, 0.514_444_444_444_444_5, ["kn", "knot", "knots"]),
    // Data (byte)
    u!(Data, 0.125, ["bit", "bits"]),
    u!(Data, 1.0, ["B", "byte", "bytes"]),
    u!(Data, 1e3, ["kB", "KB", "kilobyte", "kilobytes"]),
    u!(Data, 1e6, ["MB", "megabyte", "megabytes"]),
    u!(Data, 1e9, ["GB", "gigabyte", "gigabytes"]),
    u!(Data, 1e12, ["TB", "terabyte", "terabytes"]),
    u!(Data, 1024.0, ["KiB", "kibibyte", "kibibytes"]),
    u!(Data, 1_048_576.0, ["MiB", "mebibyte", "mebibytes"]),
    u!(Data, 1_073_741_824.0, ["GiB", "gibibyte", "gibibytes"]),
    u!(Data, 1_099_511_627_776.0, ["TiB", "tebibyte", "tebibytes"]),
    // Angle (radian)
    u!(Angle, std::f64::consts::PI / 180.0, ["deg", "degree", "degrees", "°"]),
    u!(Angle, 1.0, ["rad", "radian", "radians"]),
    u!(Angle, std::f64::consts::PI / 200.0, ["grad", "gradian", "gradians"]),
    // Temperature (handled specially)
    u!(Temperature, 0.0, ["degC", "°C", "celsius", "C"]),
    u!(Temperature, 0.0, ["degF", "°F", "fahrenheit", "F"]),
    u!(Temperature, 0.0, ["K", "kelvin"]),
];

pub fn find(name: &str) -> Option<&'static Unit> {
    UNITS
        .iter()
        .find(|u| u.names.contains(&name))
        .or_else(|| UNITS.iter().find(|u| u.names.iter().any(|n| n.eq_ignore_ascii_case(name) && n.len() > 2)))
}

fn to_kelvin(name: &str, v: f64) -> f64 {
    match name {
        "degC" => v + 273.15,
        "degF" => (v - 32.0) * 5.0 / 9.0 + 273.15,
        _ => v,
    }
}

fn from_kelvin(name: &str, k: f64) -> f64 {
    match name {
        "degC" => k - 273.15,
        "degF" => (k - 273.15) * 9.0 / 5.0 + 32.0,
        _ => k,
    }
}

pub fn convert(value: f64, from: &str, to: &str) -> Option<f64> {
    let (f, t) = (find(from)?, find(to)?);
    if f.category != t.category {
        return None;
    }
    if f.category == Category::Temperature {
        return Some(from_kelvin(t.names[0], to_kelvin(f.names[0], value)));
    }
    Some(value * f.factor / t.factor)
}

/// Splits `"<expr> <unit> in|to <unit>"` into `((expr, unit), unit)` when
/// both units are known.
pub fn split_conversion(input: &str) -> Option<((&str, &str), &str)> {
    for kw in [" in ", " to "] {
        // The last keyword occurrence is the separator ("12 in in cm").
        if let Some(idx) = input.rfind(kw) {
            let lhs = input[..idx].trim_end();
            let to = input[idx + kw.len()..].trim();
            let (expr, from) = match lhs.rfind(char::is_whitespace) {
                Some(sp) => (&lhs[..sp], lhs[sp..].trim()),
                None => {
                    // "5km in m": split trailing unit off a number.
                    let split = lhs.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(lhs.len());
                    (&lhs[..split], &lhs[split..])
                }
            };
            if find(from).is_some() && find(to).is_some() {
                return Some(((expr, from), to));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_units() {
        assert_eq!(convert(1.0, "km", "m"), Some(1000.0));
        assert!((convert(0.0, "degC", "degF").unwrap() - 32.0).abs() < 1e-9);
        assert!((convert(0.0, "K", "degC").unwrap() + 273.15).abs() < 1e-9);
        assert!((convert(1.0, "GiB", "MiB").unwrap() - 1024.0).abs() < 1e-9);
        assert_eq!(convert(1.0, "kg", "m"), None);
    }

    #[test]
    fn split() {
        assert_eq!(split_conversion("12 in in cm"), Some((("12", "in"), "cm")));
        assert_eq!(split_conversion("5km to m"), Some((("5", "km"), "m")));
        assert_eq!(split_conversion("sin 30"), None);
    }
}
