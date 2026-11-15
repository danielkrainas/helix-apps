//! `org.gnome.yelp font-adjustment`: an integer offset, in points, added
//! to the base text size (not a zoom factor), and the desktop's text
//! scaling (Large Text), which multiplies the result.

/// Base body text size in points at adjustment 0.
pub const BASE_PT: i32 = 11;
/// Base monospace size in points at adjustment 0.
pub const BASE_MONO_PT: i32 = 10;
/// Range given in the published schema's (commented-out) `<range>`.
pub const MIN: i32 = -3;
pub const MAX: i32 = 10;

pub fn clamp(adjust: i32) -> i32 {
    adjust.clamp(MIN, MAX)
}

fn pt_to_px(pt: i32, scale: f64) -> u32 {
    // CSS: 1pt = 96/72 px.
    ((pt.max(1) as f64) * 96.0 / 72.0 * scale).round() as u32
}

/// The desktop text scaling factor from GTK's font resolution
/// (`gtk-xft-dpi`, 1024 × DPI; 96 DPI is 1.0). GTK derives it from
/// `org.gnome.desktop.interface text-scaling-factor`. Unset (≤ 0, e.g. on
/// Broadway) means 1.0.
pub fn scale_from_xft_dpi(xft_dpi: i32) -> f64 {
    if xft_dpi <= 0 {
        1.0
    } else {
        (xft_dpi as f64 / 1024.0 / 96.0).clamp(0.5, 3.0)
    }
}

/// WebKit `default-font-size` and `default-monospace-font-size` (pixels)
/// for an adjustment and a text scaling factor.
pub fn font_px(adjust: i32, scale: f64) -> (u32, u32) {
    let a = clamp(adjust);
    (
        pt_to_px(BASE_PT + a, scale),
        pt_to_px(BASE_MONO_PT + a, scale),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_in_points() {
        assert_eq!(font_px(0, 1.0), (15, 13));
        assert_eq!(font_px(1, 1.0), (16, 15));
        assert_eq!(font_px(-3, 1.0), (11, 9));
        assert_eq!(font_px(-100, 1.0), font_px(MIN, 1.0));
        assert_eq!(font_px(10, 1.0), (28, 27));
        assert_eq!(font_px(99, 1.0), font_px(MAX, 1.0));
        assert_eq!(clamp(4), 4);
    }

    #[test]
    fn large_text_scales() {
        // Large Text is a factor of 1.25: Xft DPI 120.
        let large = scale_from_xft_dpi(120 * 1024);
        assert!((large - 1.25).abs() < 1e-9);
        assert_eq!(font_px(0, large), (18, 17));
        assert_eq!(scale_from_xft_dpi(96 * 1024), 1.0);
        assert_eq!(scale_from_xft_dpi(-1), 1.0);
    }
}
