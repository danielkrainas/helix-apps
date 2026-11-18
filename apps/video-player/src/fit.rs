//! Aspect-ratio fitting of video frames into the view.

/// A rectangle in view coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Display size of a frame of `width`×`height` storage pixels with the given
/// pixel aspect ratio (`par_n / par_d`). Non-square pixels widen or narrow
/// the picture horizontally; the height is kept.
pub fn display_size(width: u32, height: u32, par_n: i32, par_d: i32) -> (f64, f64) {
    let par = if par_n > 0 && par_d > 0 {
        par_n as f64 / par_d as f64
    } else {
        1.0
    };
    (width as f64 * par, height as f64)
}

/// Largest rectangle with aspect `content_w / content_h` that fits inside a
/// `view_w`×`view_h` area, centred (letterbox / pillarbox).
pub fn fit_contain(content_w: f64, content_h: f64, view_w: f64, view_h: f64) -> Rect {
    if content_w <= 0.0 || content_h <= 0.0 || view_w <= 0.0 || view_h <= 0.0 {
        return Rect {
            x: 0.0,
            y: 0.0,
            width: view_w.max(0.0),
            height: view_h.max(0.0),
        };
    }
    let scale = (view_w / content_w).min(view_h / content_h);
    let width = content_w * scale;
    let height = content_h * scale;
    Rect {
        x: (view_w - width) / 2.0,
        y: (view_h - height) / 2.0,
        width,
        height,
    }
}

/// Window size (logical px) that shows a `video` (display w, h) with no
/// bars, outside fullscreen: the video's aspect at about the area of the
/// `current` window, so sizes stay steady from one video to the next; made
/// to fit `bound` (the room on the monitor), then at least `min` (the
/// window's smallest size), keeping the aspect while both allow it. Past
/// both limits (a very tall or wide video on a small screen) `bound` wins
/// and the video is letterboxed there.
pub fn window_size(
    video: (f64, f64),
    current: (i32, i32),
    bound: (i32, i32),
    min: (i32, i32),
) -> Option<(i32, i32)> {
    let (vw, vh) = video;
    let (cw, ch) = (f64::from(current.0.max(1)), f64::from(current.1.max(1)));
    if vw <= 0.0 || vh <= 0.0 {
        return None;
    }
    let aspect = vw / vh;
    let area = cw * ch;
    let (mut w, mut h) = ((area * aspect).sqrt(), (area / aspect).sqrt());
    let (bw, bh) = (f64::from(bound.0), f64::from(bound.1));
    if bw > 0.0 && bh > 0.0 {
        let down = (bw / w).min(bh / h).min(1.0);
        w *= down;
        h *= down;
    }
    let (mw, mh) = (f64::from(min.0), f64::from(min.1));
    let up = (mw / w).max(mh / h).max(1.0);
    w *= up;
    h *= up;
    let (mut w, mut h) = (w.round() as i32, h.round() as i32);
    if bw > 0.0 && bh > 0.0 {
        w = w.min(bound.0.max(min.0));
        h = h.min(bound.1.max(min.1));
    }
    Some((w.max(min.0), h.max(min.1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rect, b: Rect) -> bool {
        (a.x - b.x).abs() < 1e-9
            && (a.y - b.y).abs() < 1e-9
            && (a.width - b.width).abs() < 1e-9
            && (a.height - b.height).abs() < 1e-9
    }

    #[test]
    fn letterbox_and_pillarbox() {
        // 16:9 into 4:3 → bars top and bottom.
        let r = fit_contain(1920.0, 1080.0, 800.0, 600.0);
        assert!(close(
            r,
            Rect {
                x: 0.0,
                y: 75.0,
                width: 800.0,
                height: 450.0
            }
        ));
        // 4:3 into 16:9 → bars left and right.
        let r = fit_contain(640.0, 480.0, 1600.0, 900.0);
        assert!(close(
            r,
            Rect {
                x: 200.0,
                y: 0.0,
                width: 1200.0,
                height: 900.0
            }
        ));
        // Same aspect fills exactly, upscaling too.
        let r = fit_contain(320.0, 180.0, 1280.0, 720.0);
        assert!(close(
            r,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1280.0,
                height: 720.0
            }
        ));
    }

    #[test]
    fn degenerate() {
        let r = fit_contain(0.0, 10.0, 100.0, 50.0);
        assert!(close(
            r,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 50.0
            }
        ));
        let r = fit_contain(10.0, 10.0, 0.0, 50.0);
        assert_eq!(r.width, 0.0);
    }

    /// The window takes the video's aspect at about the same area: no
    /// bars for wide or tall videos, within the monitor and the window's
    /// smallest size.
    #[test]
    fn window_fits_the_video() {
        let bound = (1728, 918);
        let min = (360, 294);
        let aspect = |(w, h): (i32, i32)| f64::from(w) / f64::from(h);
        // 16:9 in the default 960×600: about the same area.
        let (w, h) = window_size((1920.0, 1080.0), (960, 600), bound, min).unwrap();
        assert!((aspect((w, h)) - 16.0 / 9.0).abs() < 0.01, "{w}×{h}");
        assert!((f64::from(w * h) / 576_000.0 - 1.0).abs() < 0.01, "{w}×{h}");
        // A phone's vertical video: tall and narrow, no side bars.
        let (w, h) = window_size((1080.0, 1920.0), (960, 600), bound, min).unwrap();
        assert!((aspect((w, h)) - 9.0 / 16.0).abs() < 0.01, "{w}×{h}");
        assert!(h <= 918);
        // Too tall for the monitor: scaled down to fit it, aspect kept.
        let (w, h) = window_size((1080.0, 1920.0), (1600, 1000), (1600, 800), min).unwrap();
        assert_eq!(h, 800);
        assert!((aspect((w, h)) - 9.0 / 16.0).abs() < 0.01, "{w}×{h}");
        // Tiny window: grown to the smallest size, aspect kept.
        let (w, h) = window_size((1920.0, 1080.0), (100, 100), bound, min).unwrap();
        assert!(w >= 360 && h >= 294, "{w}×{h}");
        assert!((aspect((w, h)) - 16.0 / 9.0).abs() < 0.01, "{w}×{h}");
        // The same video again keeps the window as it is.
        assert_eq!(
            window_size((1920.0, 1080.0), (w, h), bound, min),
            Some((w, h))
        );
        // No video size yet: nothing to do.
        assert_eq!(window_size((0.0, 0.0), (960, 600), bound, min), None);
    }

    #[test]
    fn pixel_aspect_ratio() {
        // DVD PAL anamorphic: 720x576 with PAR 64/45 → 1024x576.
        assert_eq!(display_size(720, 576, 64, 45), (1024.0, 576.0));
        assert_eq!(display_size(640, 480, 1, 1), (640.0, 480.0));
        // Invalid PAR is treated as square.
        assert_eq!(display_size(640, 480, 0, 1), (640.0, 480.0));
        let (w, h) = display_size(720, 576, 64, 45);
        let r = fit_contain(w, h, 1024.0, 768.0);
        assert!(close(
            r,
            Rect {
                x: 0.0,
                y: 96.0,
                width: 1024.0,
                height: 576.0
            }
        ));
    }
}
