//! Pure logic for mapping user choices onto SANE option values.

use crate::sane::{Constraint, Word};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Color,
    Gray,
    Lineart,
}

impl ColorMode {
    fn candidates(self) -> &'static [&'static str] {
        match self {
            ColorMode::Color => &[
                "color",
                "colour",
                "rgb",
                "24bit color",
                "color24",
                "true color",
            ],
            ColorMode::Gray => &[
                "gray",
                "grey",
                "grayscale",
                "greyscale",
                "8bit gray",
                "true gray",
                "gray8",
            ],
            ColorMode::Lineart => &[
                "lineart",
                "binary",
                "black & white",
                "black and white",
                "bw",
                "monochrome",
            ],
        }
    }

    fn fallback(self) -> Option<ColorMode> {
        match self {
            ColorMode::Lineart => Some(ColorMode::Gray),
            ColorMode::Gray => Some(ColorMode::Color),
            ColorMode::Color => None,
        }
    }
}

/// Picks the device's `mode` value for `want`, falling back lineart → gray
/// → colour. Returns `None` if nothing matches.
pub fn pick_mode(available: &[String], want: ColorMode) -> Option<String> {
    let mut mode = Some(want);
    while let Some(m) = mode {
        for cand in m.candidates() {
            if let Some(s) = available.iter().find(|a| a.to_ascii_lowercase() == *cand) {
                return Some(s.clone());
            }
        }
        // Looser match: e.g. "Color 48 bit" contains "color".
        if let Some(s) = available.iter().find(|a| {
            let a = a.to_ascii_lowercase();
            m.candidates().iter().any(|c| a.contains(c))
        }) {
            return Some(s.clone());
        }
        mode = m.fallback();
    }
    None
}

/// Kind of source wanted for a scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Flatbed,
    Adf,
    AdfBack,
    AdfDuplex,
}

fn is_adf(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("adf") || s.contains("feeder") || s.contains("duplex")
}

/// Picks a `source` value of the requested kind (best effort).
pub fn pick_source(available: &[String], kind: SourceKind) -> Option<String> {
    let lower: Vec<String> = available.iter().map(|s| s.to_ascii_lowercase()).collect();
    let find = |pred: &dyn Fn(&str) -> bool| {
        lower
            .iter()
            .position(|s| pred(s))
            .map(|i| available[i].clone())
    };
    match kind {
        SourceKind::Flatbed => {
            find(&|s| s.contains("flatbed") || s.contains("platen") || s.contains("normal"))
                .or_else(|| find(&|s| !is_adf(s)))
        }
        SourceKind::AdfDuplex => find(&|s| s.contains("duplex"))
            .or_else(|| find(&|s| is_adf(s) && s.contains("back")))
            .or_else(|| pick_source(available, SourceKind::Adf)),
        SourceKind::AdfBack => find(&|s| is_adf(s) && s.contains("back"))
            .or_else(|| pick_source(available, SourceKind::Adf)),
        SourceKind::Adf => find(&|s| is_adf(s) && s.contains("front"))
            .or_else(|| find(&|s| is_adf(s) && !s.contains("duplex") && !s.contains("back")))
            .or_else(|| find(&|s| is_adf(s))),
    }
}

const COMMON_DPI: [i32; 8] = [75, 100, 150, 200, 300, 400, 600, 1200];

/// Resolutions to offer, from a resolution option's constraint. `fixed`
/// says the values are SANE_Fixed.
pub fn resolution_choices(c: &Constraint, fixed: bool) -> Vec<i32> {
    let to_dpi = |w: Word| {
        if fixed {
            (w as f64 / 65536.0).round() as i32
        } else {
            w
        }
    };
    let mut v: Vec<i32> = match c {
        Constraint::Words(list) => list.iter().map(|&w| to_dpi(w)).filter(|&d| d > 0).collect(),
        Constraint::Range { min, max, .. } => {
            let (lo, hi) = (to_dpi(*min).max(1), to_dpi(*max));
            let mut v: Vec<i32> = COMMON_DPI
                .iter()
                .copied()
                .filter(|d| (lo..=hi).contains(d))
                .collect();
            if v.is_empty() {
                v = vec![lo, hi];
            }
            v
        }
        _ => COMMON_DPI.to_vec(),
    };
    v.sort_unstable();
    v.dedup();
    v
}

/// The supported resolution nearest to `want` (prefers the higher of two
/// equally near values).
pub fn nearest_resolution(choices: &[i32], want: i32) -> i32 {
    choices
        .iter()
        .copied()
        .min_by_key(|&d| ((d - want).abs(), -d))
        .unwrap_or(want)
}

/// Clamps a word into a range constraint, honouring quantisation.
pub fn clamp_to_range(v: Word, min: Word, max: Word, quant: Word) -> Word {
    let mut v = v.clamp(min, max);
    if quant > 0 {
        v = min + ((v - min) as i64 / quant as i64 * quant as i64) as Word;
    }
    v
}

/// Maps a −100…100 adjustment onto a range around its midpoint.
pub fn scale_adjustment(value: i32, min: Word, max: Word) -> Word {
    let mid = (min as f64 + max as f64) / 2.0;
    let half = (max as f64 - min as f64) / 2.0;
    let v = mid + value.clamp(-100, 100) as f64 / 100.0 * half;
    v.round() as Word
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn modes() {
        let test = s(&["Gray", "Color"]);
        assert_eq!(pick_mode(&test, ColorMode::Color).as_deref(), Some("Color"));
        assert_eq!(
            pick_mode(&test, ColorMode::Lineart).as_deref(),
            Some("Gray")
        );
        let other = s(&["Black & White", "Grayscale", "24bit Color"]);
        assert_eq!(
            pick_mode(&other, ColorMode::Lineart).as_deref(),
            Some("Black & White")
        );
        assert_eq!(
            pick_mode(&other, ColorMode::Gray).as_deref(),
            Some("Grayscale")
        );
        assert_eq!(
            pick_mode(&other, ColorMode::Color).as_deref(),
            Some("24bit Color")
        );
        assert_eq!(
            pick_mode(&s(&["Color 48 bit"]), ColorMode::Gray).as_deref(),
            Some("Color 48 bit")
        );
        assert_eq!(pick_mode(&s(&["Halftone"]), ColorMode::Color), None);
    }

    #[test]
    fn sources() {
        let test = s(&["Flatbed", "Automatic Document Feeder"]);
        assert_eq!(
            pick_source(&test, SourceKind::Flatbed).as_deref(),
            Some("Flatbed")
        );
        assert_eq!(
            pick_source(&test, SourceKind::Adf).as_deref(),
            Some("Automatic Document Feeder")
        );
        assert_eq!(
            pick_source(&test, SourceKind::AdfDuplex).as_deref(),
            Some("Automatic Document Feeder")
        );
        let duplex = s(&["Flatbed", "ADF Front", "ADF Back", "ADF Duplex"]);
        assert_eq!(
            pick_source(&duplex, SourceKind::Adf).as_deref(),
            Some("ADF Front")
        );
        assert_eq!(
            pick_source(&duplex, SourceKind::AdfBack).as_deref(),
            Some("ADF Back")
        );
        assert_eq!(
            pick_source(&duplex, SourceKind::AdfDuplex).as_deref(),
            Some("ADF Duplex")
        );
        assert_eq!(pick_source(&s(&["ADF"]), SourceKind::Flatbed), None);
    }

    #[test]
    fn resolutions() {
        let r = resolution_choices(
            &Constraint::Range {
                min: 65536,
                max: 1200 * 65536,
                quant: 65536,
            },
            true,
        );
        assert_eq!(r, COMMON_DPI.to_vec());
        let r = resolution_choices(&Constraint::Words(vec![600, 75, 300, 150]), false);
        assert_eq!(r, vec![75, 150, 300, 600]);
        assert_eq!(
            resolution_choices(
                &Constraint::Range {
                    min: 30,
                    max: 50,
                    quant: 0
                },
                false
            ),
            vec![30, 50]
        );
        assert_eq!(nearest_resolution(&r, 200), 150);
        assert_eq!(nearest_resolution(&r, 225), 300);
        assert_eq!(nearest_resolution(&r, 5000), 600);
        assert_eq!(nearest_resolution(&[], 5000), 5000);
    }

    #[test]
    fn ranges_and_adjustments() {
        assert_eq!(clamp_to_range(17, 0, 100, 5), 15);
        assert_eq!(clamp_to_range(-3, 0, 100, 0), 0);
        assert_eq!(scale_adjustment(0, -100, 100), 0);
        assert_eq!(scale_adjustment(50, 0, 200), 150);
        assert_eq!(scale_adjustment(-100, 0, 255), 0);
    }
}
