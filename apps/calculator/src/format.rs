//! Result formatting.

use crate::engine::{to_signed, Value};
use crate::locale::Separators;
use helix_app_common::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberFormat {
    Automatic,
    Fixed,
    Scientific,
    Engineering,
}

impl NumberFormat {
    pub const ALL: [NumberFormat; 4] = [
        NumberFormat::Automatic,
        NumberFormat::Fixed,
        NumberFormat::Scientific,
        NumberFormat::Engineering,
    ];

    pub fn nick(self) -> &'static str {
        match self {
            NumberFormat::Automatic => "automatic",
            NumberFormat::Fixed => "fixed",
            NumberFormat::Scientific => "scientific",
            NumberFormat::Engineering => "engineering",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            NumberFormat::Automatic => tr("Automatic"),
            NumberFormat::Fixed => tr("Fixed"),
            NumberFormat::Scientific => tr("Scientific"),
            NumberFormat::Engineering => tr("Engineering"),
        }
    }
    pub fn from_nick(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|f| f.nick() == s)
            .unwrap_or(NumberFormat::Automatic)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub format: NumberFormat,
    pub accuracy: usize,
    pub thousands: bool,
    pub show_zeroes: bool,
    /// Output base (programming mode).
    pub base: u32,
    pub word_size: u32,
    /// The decimal and group separators shown (base 10).
    pub sep: Separators,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            format: NumberFormat::Automatic,
            accuracy: 9,
            thousands: false,
            show_zeroes: false,
            base: 10,
            word_size: 64,
            sep: Separators::C,
        }
    }
}

const MINUS: char = '−';

pub fn format(v: Value, o: &Options) -> String {
    match v {
        Value::Real(x) => format_real(x, o),
        Value::Int(bits) => format_int(bits, o),
    }
}

pub fn format_int(raw: u64, o: &Options) -> String {
    if o.base == 10 {
        let s = to_signed(raw, o.word_size);
        let digits = group(&s.unsigned_abs().to_string(), o.thousands, o.sep.group, 3);
        return if s < 0 {
            format!("{MINUS}{digits}")
        } else {
            digits
        };
    }
    let digits = match o.base {
        2 => format!("{raw:b}"),
        8 => format!("{raw:o}"),
        _ => format!("{raw:X}"),
    };
    let sub = match o.base {
        2 => "₂",
        8 => "₈",
        _ => "₁₆",
    };
    format!("{}{sub}", group(&digits, o.thousands, ' ', 4))
}

fn group(digits: &str, enable: bool, sep: char, size: usize) -> String {
    if !enable || digits.len() <= size {
        return digits.to_string();
    }
    let mut out = String::new();
    let first = digits.len() % size;
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (i + size - first).is_multiple_of(size) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

fn trim_zeroes(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

fn fixed(x: f64, o: &Options) -> String {
    let s = format!("{:.*}", o.accuracy, x.abs());
    let s = if o.show_zeroes {
        s.as_str()
    } else {
        trim_zeroes(&s)
    };
    let (int, frac) = s.split_once('.').map_or((s, None), |(a, b)| (a, Some(b)));
    let mut out = String::new();
    if x < 0.0 && s.chars().any(|c| c != '0' && c != '.') {
        out.push(MINUS);
    }
    out.push_str(&group(int, o.thousands, o.sep.group, 3));
    if let Some(f) = frac {
        out.push(o.sep.decimal);
        out.push_str(f);
    }
    out
}

fn superscript(n: i32) -> String {
    const SUP: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    let mut s = String::new();
    if n < 0 {
        s.push('⁻');
    }
    for c in n.unsigned_abs().to_string().chars() {
        s.push(SUP[c.to_digit(10).unwrap() as usize]);
    }
    s
}

fn exponential(x: f64, o: &Options, step: i32) -> String {
    if x == 0.0 {
        return fixed(0.0, o);
    }
    let mut exp = x.abs().log10().floor() as i32;
    exp -= exp.rem_euclid(step);
    let mut mantissa = x / 10f64.powi(exp);
    // Rounding may push the mantissa to 10 (or 1000 for engineering).
    let rounded: f64 = format!("{:.*}", o.accuracy, mantissa)
        .parse()
        .unwrap_or(mantissa);
    if rounded.abs() >= 10f64.powi(step) {
        exp += step;
        mantissa = x / 10f64.powi(exp);
    }
    let m = fixed(
        mantissa,
        &Options {
            thousands: false,
            ..*o
        },
    );
    if exp == 0 {
        m
    } else {
        format!("{m}×10{}", superscript(exp))
    }
}

pub fn format_real(x: f64, o: &Options) -> String {
    match o.format {
        NumberFormat::Fixed => fixed(x, o),
        NumberFormat::Scientific => exponential(x, o, 1),
        NumberFormat::Engineering => exponential(x, o, 3),
        NumberFormat::Automatic => {
            let a = x.abs();
            let tiny = a != 0.0 && a < 10f64.powi(-(o.accuracy as i32));
            if a >= 1e15 || tiny {
                exponential(x, o, 1)
            } else {
                fixed(x, o)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(format: NumberFormat) -> Options {
        Options {
            format,
            ..Default::default()
        }
    }

    #[test]
    fn automatic() {
        let o = opts(NumberFormat::Automatic);
        assert_eq!(format_real(0.1 + 0.2, &o), "0.3");
        assert_eq!(format_real(-2.5, &o), "−2.5");
        assert_eq!(format_real(1.0 / 3.0, &o), "0.333333333");
        assert_eq!(format_real(1e20, &o), "1×10²⁰");
        assert_eq!(format_real(1.5e-12, &o), "1.5×10⁻¹²");
        assert_eq!(format_real(-0.0000000000001, &o), "−1×10⁻¹³");
    }

    #[test]
    fn fixed_and_options() {
        let o = Options {
            thousands: true,
            show_zeroes: true,
            accuracy: 2,
            ..opts(NumberFormat::Fixed)
        };
        assert_eq!(format_real(1234567.891, &o), "1,234,567.89");
        assert_eq!(format_real(5.0, &o), "5.00");
        assert_eq!(format_real(-0.001, &o), "0.00");
    }

    #[test]
    fn exponential_forms() {
        assert_eq!(
            format_real(12345.0, &opts(NumberFormat::Scientific)),
            "1.2345×10⁴"
        );
        assert_eq!(
            format_real(12345.0, &opts(NumberFormat::Engineering)),
            "12.345×10³"
        );
        assert_eq!(
            format_real(9.9999999999, &opts(NumberFormat::Scientific)),
            "1×10¹"
        );
    }

    #[test]
    fn ints() {
        let o = Options {
            base: 16,
            word_size: 8,
            ..Default::default()
        };
        assert_eq!(format_int(255, &o), "FF₁₆");
        let o = Options {
            base: 10,
            word_size: 8,
            ..Default::default()
        };
        assert_eq!(format_int(255, &o), "−1");
        let o = Options {
            base: 2,
            thousands: true,
            ..Default::default()
        };
        assert_eq!(format_int(0b1_0110_1001, &o), "1 0110 1001₂");
    }

    #[test]
    fn locale_separators() {
        let de = Separators {
            decimal: ',',
            group: '.',
        };
        let o = Options {
            thousands: true,
            accuracy: 2,
            sep: de,
            ..opts(NumberFormat::Fixed)
        };
        assert_eq!(format_real(1234567.891, &o), "1.234.567,89");
        assert_eq!(format_real(-0.5, &o), "−0,5");
        let o = Options {
            sep: de,
            ..opts(NumberFormat::Scientific)
        };
        assert_eq!(format_real(12345.0, &o), "1,2345×10⁴");
        let o = Options {
            thousands: true,
            sep: de,
            ..Default::default()
        };
        assert_eq!(format_int(1234567, &o), "1.234.567");
        // Comma-locale results read back to the same values.
        let mut ctx = crate::engine::Context {
            sep: de,
            ..Default::default()
        };
        for x in [1234567.891, -0.5, 1.5e-12, 1e20] {
            let o = Options {
                thousands: true,
                sep: de,
                ..opts(NumberFormat::Automatic)
            };
            let s = format_real(x, &o);
            let v = ctx.evaluate(&s).unwrap().as_f64(64);
            assert!((v - x).abs() <= x.abs() * 1e-9, "{s} → {v}");
        }
    }

    #[test]
    fn round_trips_through_engine() {
        let mut ctx = crate::engine::Context::default();
        let s = format_real(1.5e-12, &opts(NumberFormat::Automatic));
        let v = ctx.evaluate(&s).unwrap().as_f64(64);
        assert!((v - 1.5e-12).abs() < 1e-24);
        let s = format_real(-2.5, &opts(NumberFormat::Automatic));
        assert_eq!(ctx.evaluate(&s).unwrap().as_f64(64), -2.5);
    }
}
