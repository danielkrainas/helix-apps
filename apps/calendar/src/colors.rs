//! Calendar colours.

/// Palette offered for new calendars (GNOME HIG palette, "3" shades).
pub const PALETTE: [&str; 8] = [
    "#3584e4", "#33d17a", "#f6d32d", "#ff7800", "#e01b24", "#9141ac", "#986a44", "#77767b",
];

pub const DEFAULT: &str = "#3584e4";

/// Parses `#rgb`, `#rrggbb` or `#rrrrggggbbbb` (as EDS sometimes stores).
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let ch = |i: usize, w: usize| u32::from_str_radix(&h[i * w..i * w + w], 16).ok();
    match h.len() {
        3 => Some((
            (ch(0, 1)? * 17) as u8,
            (ch(1, 1)? * 17) as u8,
            (ch(2, 1)? * 17) as u8,
        )),
        6 => Some((ch(0, 2)? as u8, ch(1, 2)? as u8, ch(2, 2)? as u8)),
        12 => Some((
            (ch(0, 4)? >> 8) as u8,
            (ch(1, 4)? >> 8) as u8,
            (ch(2, 4)? >> 8) as u8,
        )),
        _ => None,
    }
}

pub fn to_hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Normalized `#rrggbb`, or the default colour.
pub fn normalize(s: Option<&str>) -> String {
    s.and_then(parse_hex)
        .map(to_hex)
        .unwrap_or_else(|| DEFAULT.to_string())
}

/// WCAG relative luminance of an sRGB colour (0 black – 1 white).
pub fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio between two colours (1 – 21).
pub fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Text colour (black or white) with the higher contrast on `bg`. One of
/// the two always reaches at least 4.58:1, so chip text meets WCAG AA on
/// any calendar colour.
pub fn contrast_fg(bg: &str) -> &'static str {
    let bg = parse_hex(bg).unwrap_or((53, 132, 228));
    if contrast_ratio(bg, (0, 0, 0)) >= contrast_ratio(bg, (255, 255, 255)) {
        "#000000"
    } else {
        "#ffffff"
    }
}

/// The overlay colour and opacity drawn over a chip when hovered
/// (`pressed` false) or pressed: towards black under white text and
/// towards white under black text, so feedback never lowers contrast.
pub fn overlay(fg: (u8, u8, u8), pressed: bool) -> ((u8, u8, u8), f64) {
    let dark_text = luminance(fg) < 0.5;
    match (dark_text, pressed) {
        (true, false) => ((255, 255, 255), 0.25),
        (true, true) => ((255, 255, 255), 0.45),
        (false, false) => ((0, 0, 0), 0.15),
        (false, true) => ((0, 0, 0), 0.30),
    }
}

/// CSS for one calendar class: background, text colour and the
/// hover / pressed overlays (see [`overlay`]); `.CLASS-dot` for its dot.
pub fn class_css(class: &str, bg: &str) -> String {
    let fg = contrast_fg(bg);
    let rgb = parse_hex(fg).unwrap_or((255, 255, 255));
    let layer = |pressed: bool| {
        let ((r, g, b), a) = overlay(rgb, pressed);
        format!("linear-gradient(rgba({r},{g},{b},{a}), rgba({r},{g},{b},{a}))")
    };
    format!(
        ".{class} {{ background-color: {bg}; color: {fg}; }}\n\
         .{class}:hover {{ background-image: {}; }}\n\
         .{class}:active, .{class}:checked {{ background-image: {}; }}\n\
         .{class}-dot {{ background-color: {bg}; }}\n",
        layer(false),
        layer(true)
    )
}

/// A palette colour not yet used by `existing`, else the first.
pub fn next_free(existing: &[String]) -> &'static str {
    PALETTE
        .iter()
        .find(|c| !existing.iter().any(|e| e.eq_ignore_ascii_case(c)))
        .copied()
        .unwrap_or(PALETTE[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        assert_eq!(parse_hex("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_hex("#3584e4"), Some((0x35, 0x84, 0xe4)));
        assert_eq!(parse_hex("#ffff00000000"), Some((255, 0, 0)));
        assert_eq!(parse_hex("blue"), None);
        assert_eq!(normalize(Some("#ABC")), "#aabbcc");
        assert_eq!(normalize(None), DEFAULT);
    }

    #[test]
    fn contrast() {
        assert_eq!(contrast_fg("#f6d32d"), "#000000");
        assert_eq!(contrast_fg("#1c71d8"), "#ffffff");
        assert_eq!(contrast_fg("#000000"), "#ffffff");
        // Mid-tones that used to get white text below 4.5:1.
        assert_eq!(contrast_fg("#ff7800"), "#000000");
        assert_eq!(contrast_fg("#3584e4"), "#000000");
        assert!((contrast_ratio((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 1e-9);
    }

    fn fg_rgb(bg: &str) -> (u8, u8, u8) {
        parse_hex(contrast_fg(bg)).unwrap()
    }

    #[test]
    fn chip_text_meets_aa_on_the_palette() {
        for c in PALETTE.iter().chain([&DEFAULT]) {
            let bg = parse_hex(c).unwrap();
            let ratio = contrast_ratio(bg, fg_rgb(c));
            assert!(ratio >= 4.5, "{c}: {ratio:.2}:1");
        }
    }

    #[test]
    fn chip_text_meets_aa_on_any_colour() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    let c = to_hex((r as u8, g as u8, b as u8));
                    let ratio = contrast_ratio((r as u8, g as u8, b as u8), fg_rgb(&c));
                    assert!(ratio >= 4.5, "{c}: {ratio:.2}:1");
                }
            }
        }
    }

    #[test]
    fn hover_and_pressed_keep_aa() {
        // The hover / pressed overlays move the background away from the
        // text colour, so contrast only grows.
        for c in PALETTE.iter().chain([&DEFAULT]) {
            let bg = parse_hex(c).unwrap();
            let fg = fg_rgb(c);
            for (over, alpha) in [overlay(fg, false), overlay(fg, true)] {
                let mix =
                    |a: u8, o: u8| (a as f64 * (1.0 - alpha) + o as f64 * alpha).round() as u8;
                let shaded = (mix(bg.0, over.0), mix(bg.1, over.1), mix(bg.2, over.2));
                assert!(
                    contrast_ratio(shaded, fg) >= contrast_ratio(bg, fg) - 1e-9,
                    "{c}"
                );
            }
        }
    }

    #[test]
    fn free_colours() {
        assert_eq!(next_free(&[]), PALETTE[0]);
        assert_eq!(next_free(&["#3584E4".into()]), PALETTE[1]);
    }
}
