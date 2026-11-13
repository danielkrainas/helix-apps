//! `org.gnome.yelp font-adjustment`: an integer offset, in points, added
//! to the base text size (not a zoom factor).

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

fn pt_to_px(pt: i32) -> u32 {
    // CSS: 1pt = 96/72 px.
    ((pt.max(1) as f64) * 96.0 / 72.0).round() as u32
}

/// WebKit `default-font-size` and `default-monospace-font-size` (pixels)
/// for an adjustment.
pub fn font_px(adjust: i32) -> (u32, u32) {
    let a = clamp(adjust);
    (pt_to_px(BASE_PT + a), pt_to_px(BASE_MONO_PT + a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_in_points() {
        assert_eq!(font_px(0), (15, 13));
        assert_eq!(font_px(1), (16, 15));
        assert_eq!(font_px(-3), (11, 9));
        assert_eq!(font_px(-100), font_px(MIN));
        assert_eq!(font_px(10), (28, 27));
        assert_eq!(font_px(99), font_px(MAX));
        assert_eq!(clamp(4), 4);
    }
}
