//! Sizing the window to the image (no GTK).

/// Smallest window size, in logical pixels, whatever the image. (The
/// window is also never smaller than its header bar needs once it has shed
/// what it can, see `Viewer::compact_for`.)
pub const MIN_WINDOW: (i32, i32) = (100, 100);

/// Size of the image area that shows a (rotated) `image` of that many
/// pixels with no bars around it: its own size, scaled down to fit `room`
/// (the most the image area may take on the monitor), keeping its aspect.
/// `None` without an image or room.
pub fn image_area(image: (f64, f64), room: (i32, i32)) -> Option<(i32, i32)> {
    let (iw, ih) = image;
    let (rw, rh) = (f64::from(room.0), f64::from(room.1));
    if iw <= 0.0 || ih <= 0.0 || rw <= 0.0 || rh <= 0.0 {
        return None;
    }
    let scale = (rw / iw).min(rh / ih).min(1.0);
    let w = (iw * scale).round().max(1.0) as i32;
    let h = (ih * scale).round().max(1.0) as i32;
    Some((w, h))
}

/// Window size for an image `area` plus the window's own `chrome` (header
/// bar, thumbnail strip, sidebar), at least [`MIN_WINDOW`].
pub fn window_size(area: (i32, i32), chrome: (i32, i32)) -> (i32, i32) {
    (
        (area.0 + chrome.0).max(MIN_WINDOW.0),
        (area.1 + chrome.1).max(MIN_WINDOW.1),
    )
}

/// Width an `AdwOverlaySplitView` sidebar takes beside an image area
/// `area` px wide: `fraction` of the whole width (area + sidebar), kept
/// between `min` and `max`.
pub fn sidebar_width(area: i32, fraction: f64, min: f64, max: f64) -> i32 {
    // s = fraction × (area + s)  ⇒  s = area × fraction / (1 − fraction)
    let s = if fraction > 0.0 && fraction < 1.0 {
        f64::from(area) * fraction / (1.0 - fraction)
    } else {
        min
    };
    s.clamp(min, max.max(min)).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_beside_the_image() {
        // libadwaita's default: a quarter of the width, 260 to 340 px here.
        assert_eq!(sidebar_width(640, 0.25, 260.0, 340.0), 260);
        assert_eq!(sidebar_width(900, 0.25, 260.0, 340.0), 300);
        assert_eq!(sidebar_width(1400, 0.25, 260.0, 340.0), 340);
    }

    #[test]
    fn small_images_keep_their_size() {
        assert_eq!(image_area((640.0, 480.0), (1700, 900)), Some((640, 480)));
        // A tiny icon: the image area is the icon; the window floor applies.
        assert_eq!(image_area((16.0, 16.0), (1700, 900)), Some((16, 16)));
        assert_eq!(window_size((16, 16), (0, 47)), (100, 100));
    }

    #[test]
    fn large_images_scale_down_keeping_aspect() {
        // A 6000×4000 photo into 1700×850: limited by height.
        let (w, h) = image_area((6000.0, 4000.0), (1700, 850)).unwrap();
        assert_eq!(h, 850);
        assert!((f64::from(w) / f64::from(h) - 1.5).abs() < 0.01, "{w}×{h}");
        // A phone screenshot: tall and narrow.
        let (w, h) = image_area((1080.0, 2400.0), (1700, 850)).unwrap();
        assert_eq!(h, 850);
        assert!(w < 400, "{w}×{h}");
        // A panorama: limited by width.
        let (w, h) = image_area((12000.0, 1000.0), (1700, 850)).unwrap();
        assert_eq!(w, 1700);
        assert!((141..=142).contains(&h), "{w}×{h}");
    }

    #[test]
    fn chrome_is_added() {
        assert_eq!(window_size((640, 480), (0, 47)), (640, 527));
        assert_eq!(window_size((640, 480), (300, 131)), (940, 611));
        // A 1 px tall strip still gets a usable window.
        assert_eq!(window_size((1700, 1), (0, 47)), (1700, 100));
    }

    #[test]
    fn nothing_to_fit() {
        assert_eq!(image_area((0.0, 0.0), (1700, 900)), None);
        assert_eq!(image_area((640.0, 480.0), (0, 0)), None);
    }
}
