//! Calendar colours.

/// Palette offered for new calendars (GNOME HIG palette, "3" shades).
pub const PALETTE: [&str; 8] =
    ["#3584e4", "#33d17a", "#f6d32d", "#ff7800", "#e01b24", "#9141ac", "#986a44", "#77767b"];

pub const DEFAULT: &str = "#3584e4";

/// Parses `#rgb`, `#rrggbb` or `#rrrrggggbbbb` (as EDS sometimes stores).
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let ch = |i: usize, w: usize| u32::from_str_radix(&h[i * w..i * w + w], 16).ok();
    match h.len() {
        3 => Some(((ch(0, 1)? * 17) as u8, (ch(1, 1)? * 17) as u8, (ch(2, 1)? * 17) as u8)),
        6 => Some((ch(0, 2)? as u8, ch(1, 2)? as u8, ch(2, 2)? as u8)),
        12 => Some(((ch(0, 4)? >> 8) as u8, (ch(1, 4)? >> 8) as u8, (ch(2, 4)? >> 8) as u8)),
        _ => None,
    }
}

pub fn to_hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Normalized `#rrggbb`, or the default colour.
pub fn normalize(s: Option<&str>) -> String {
    s.and_then(parse_hex).map(to_hex).unwrap_or_else(|| DEFAULT.to_string())
}

/// Readable text colour (black or white) on `bg`.
pub fn contrast_fg(bg: &str) -> &'static str {
    let (r, g, b) = parse_hex(bg).unwrap_or((53, 132, 228));
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    if l > 0.4 {
        "#000000"
    } else {
        "#ffffff"
    }
}

/// A palette colour not yet used by `existing`, else the first.
pub fn next_free(existing: &[String]) -> &'static str {
    PALETTE.iter().find(|c| !existing.iter().any(|e| e.eq_ignore_ascii_case(c))).copied().unwrap_or(PALETTE[0])
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
    }

    #[test]
    fn free_colours() {
        assert_eq!(next_free(&[]), PALETTE[0]);
        assert_eq!(next_free(&["#3584E4".into()]), PALETTE[1]);
    }
}
