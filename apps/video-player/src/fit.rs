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
    let par = if par_n > 0 && par_d > 0 { par_n as f64 / par_d as f64 } else { 1.0 };
    (width as f64 * par, height as f64)
}

/// Largest rectangle with aspect `content_w / content_h` that fits inside a
/// `view_w`×`view_h` area, centred (letterbox / pillarbox).
pub fn fit_contain(content_w: f64, content_h: f64, view_w: f64, view_h: f64) -> Rect {
    if content_w <= 0.0 || content_h <= 0.0 || view_w <= 0.0 || view_h <= 0.0 {
        return Rect { x: 0.0, y: 0.0, width: view_w.max(0.0), height: view_h.max(0.0) };
    }
    let scale = (view_w / content_w).min(view_h / content_h);
    let width = content_w * scale;
    let height = content_h * scale;
    Rect { x: (view_w - width) / 2.0, y: (view_h - height) / 2.0, width, height }
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
        assert!(close(r, Rect { x: 0.0, y: 75.0, width: 800.0, height: 450.0 }));
        // 4:3 into 16:9 → bars left and right.
        let r = fit_contain(640.0, 480.0, 1600.0, 900.0);
        assert!(close(r, Rect { x: 200.0, y: 0.0, width: 1200.0, height: 900.0 }));
        // Same aspect fills exactly, upscaling too.
        let r = fit_contain(320.0, 180.0, 1280.0, 720.0);
        assert!(close(r, Rect { x: 0.0, y: 0.0, width: 1280.0, height: 720.0 }));
    }

    #[test]
    fn degenerate() {
        let r = fit_contain(0.0, 10.0, 100.0, 50.0);
        assert!(close(r, Rect { x: 0.0, y: 0.0, width: 100.0, height: 50.0 }));
        let r = fit_contain(10.0, 10.0, 0.0, 50.0);
        assert_eq!(r.width, 0.0);
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
        assert!(close(r, Rect { x: 0.0, y: 96.0, width: 1024.0, height: 576.0 }));
    }
}
