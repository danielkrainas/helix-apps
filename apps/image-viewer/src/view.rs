//! Zoom, pan and rotation math for the image canvas (no GTK).
//!
//! Coordinates: the pan `offset` is the displacement, in view pixels, of the
//! image centre from the view centre. Anchor points are given relative to the
//! view's top-left corner.

pub const MAX_ZOOM: f64 = 20.0;
pub const ZOOM_STEP: f64 = 1.25;
const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    /// Image size in pixels (already EXIF-oriented).
    pub image: (f64, f64),
    /// View (widget) size in pixels.
    pub viewport: (f64, f64),
    /// Clockwise view rotation, one of 0, 90, 180, 270.
    pub rotation: u32,
    /// `None` = best fit (follows viewport changes).
    zoom: Option<f64>,
    pub offset: (f64, f64),
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            image: (0.0, 0.0),
            viewport: (0.0, 0.0),
            rotation: 0,
            zoom: None,
            offset: (0.0, 0.0),
        }
    }
}

impl ViewState {
    pub fn new(image_w: f64, image_h: f64) -> Self {
        ViewState {
            image: (image_w, image_h),
            ..Default::default()
        }
    }

    /// Image size after rotation.
    pub fn rotated_image(&self) -> (f64, f64) {
        if self.rotation % 180 == 90 {
            (self.image.1, self.image.0)
        } else {
            self.image
        }
    }

    /// Scale at which the rotated image fits the viewport, capped at 1.
    pub fn fit_zoom(&self) -> f64 {
        let (iw, ih) = self.rotated_image();
        let (vw, vh) = self.viewport;
        if iw <= 0.0 || ih <= 0.0 || vw <= 0.0 || vh <= 0.0 {
            return 1.0;
        }
        (vw / iw).min(vh / ih).min(1.0)
    }

    pub fn min_zoom(&self) -> f64 {
        self.fit_zoom().min(1.0)
    }

    pub fn is_best_fit(&self) -> bool {
        self.zoom.is_none()
    }

    pub fn zoom(&self) -> f64 {
        self.zoom.unwrap_or_else(|| self.fit_zoom())
    }

    /// Size of the rotated image on screen.
    pub fn scaled_size(&self) -> (f64, f64) {
        let (iw, ih) = self.rotated_image();
        let z = self.zoom();
        (iw * z, ih * z)
    }

    /// Whether the image overflows the view (so it can be panned).
    pub fn can_pan(&self) -> bool {
        let (sw, sh) = self.scaled_size();
        sw > self.viewport.0 + 0.5 || sh > self.viewport.1 + 0.5
    }

    pub fn set_viewport(&mut self, w: f64, h: f64) {
        self.viewport = (w, h);
        self.clamp_offset();
    }

    fn clamp_offset(&mut self) {
        let (sw, sh) = self.scaled_size();
        let clamp = |off: f64, scaled: f64, view: f64| {
            let max = ((scaled - view) / 2.0).max(0.0);
            off.clamp(-max, max)
        };
        self.offset = (
            clamp(self.offset.0, sw, self.viewport.0),
            clamp(self.offset.1, sh, self.viewport.1),
        );
    }

    pub fn pan_to(&mut self, offset: (f64, f64)) {
        self.offset = offset;
        self.clamp_offset();
    }

    pub fn pan_by(&mut self, dx: f64, dy: f64) {
        self.pan_to((self.offset.0 + dx, self.offset.1 + dy));
    }

    pub fn best_fit(&mut self) {
        self.zoom = None;
        self.offset = (0.0, 0.0);
    }

    fn center(&self) -> (f64, f64) {
        (self.viewport.0 / 2.0, self.viewport.1 / 2.0)
    }

    /// Sets the zoom, keeping the image point under `anchor` (view coords;
    /// `None` = view centre) fixed on screen. Zoom is clamped to
    /// `[min_zoom, MAX_ZOOM]`; reaching the fit zoom returns to best-fit mode.
    pub fn zoom_to(&mut self, zoom: f64, anchor: Option<(f64, f64)>) {
        let fit = self.fit_zoom();
        let new = zoom.clamp(self.min_zoom(), MAX_ZOOM);
        let old = self.zoom();
        let (cx, cy) = self.center();
        let (ax, ay) = anchor.unwrap_or((cx, cy));
        // Anchor relative to the image centre, in unscaled image units.
        let px = (ax - cx - self.offset.0) / old;
        let py = (ay - cy - self.offset.1) / old;
        self.zoom = if (new - fit).abs() < EPS {
            None
        } else {
            Some(new)
        };
        self.offset = (ax - cx - px * new, ay - cy - py * new);
        self.clamp_offset();
    }

    /// Next zoom level when zooming in by one step, snapping to 100 % and to
    /// the fit level when they are crossed.
    pub fn step_in(&self) -> f64 {
        let cur = self.zoom();
        let next = cur * ZOOM_STEP;
        snap_levels(cur, next, &[self.fit_zoom(), 1.0])
            .fold(next, f64::min)
            .min(MAX_ZOOM)
    }

    pub fn step_out(&self) -> f64 {
        let cur = self.zoom();
        let next = cur / ZOOM_STEP;
        snap_levels(next, cur, &[self.fit_zoom(), 1.0])
            .fold(next, f64::max)
            .max(self.min_zoom())
    }

    pub fn zoom_in(&mut self, anchor: Option<(f64, f64)>) {
        let z = self.step_in();
        self.zoom_to(z, anchor);
    }

    pub fn zoom_out(&mut self, anchor: Option<(f64, f64)>) {
        let z = self.step_out();
        self.zoom_to(z, anchor);
    }

    /// Rotates by `degrees` (a multiple of 90, may be negative) and returns to
    /// best fit.
    pub fn rotate(&mut self, degrees: i32) {
        self.rotation = (self.rotation as i32 + degrees).rem_euclid(360) as u32;
        self.best_fit();
    }

    /// Image rectangle on screen (x, y, w, h) of the *rotated* image.
    pub fn image_rect(&self) -> (f64, f64, f64, f64) {
        let (sw, sh) = self.scaled_size();
        let (cx, cy) = self.center();
        (
            cx + self.offset.0 - sw / 2.0,
            cy + self.offset.1 - sh / 2.0,
            sw,
            sh,
        )
    }
}

/// Snap levels strictly inside `(lo, hi)`. Zooming in takes the lowest of
/// them (the first one crossed), zooming out the highest.
fn snap_levels(lo: f64, hi: f64, levels: &[f64]) -> impl Iterator<Item = f64> + '_ {
    levels
        .iter()
        .copied()
        .filter(move |l| *l > lo + EPS && *l < hi - EPS)
}

/// Zoom as a percentage label, e.g. `"150 %"`.
pub fn zoom_label(zoom: f64) -> String {
    format!("{:.0} %", zoom * 100.0)
}

/// The image canvas's accessible description: position in the folder,
/// pixel size and zoom, as far as known — "3 of 4, 3840×2160, 100 %".
pub fn canvas_description(
    position: Option<(usize, usize)>,
    size: Option<(i32, i32)>,
    zoom: Option<f64>,
) -> String {
    let mut parts = Vec::new();
    if let Some((i, n)) = position.filter(|(_, n)| *n > 1) {
        parts.push(format!("{} of {n}", i + 1));
    }
    if let Some((w, h)) = size {
        parts.push(format!("{w}×{h}"));
    }
    if let Some(z) = zoom {
        parts.push(zoom_label(z));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_is_described() {
        assert_eq!(
            canvas_description(Some((2, 4)), Some((3840, 2160)), Some(1.0)),
            "3 of 4, 3840×2160, 100 %"
        );
        assert_eq!(
            canvas_description(Some((0, 1)), Some((10, 20)), None),
            "10×20"
        );
        assert_eq!(canvas_description(None, None, None), "");
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn view(iw: f64, ih: f64, vw: f64, vh: f64) -> ViewState {
        let mut v = ViewState::new(iw, ih);
        v.set_viewport(vw, vh);
        v
    }

    #[test]
    fn best_fit_never_upscales() {
        let v = view(4000.0, 3000.0, 800.0, 600.0);
        assert!(approx(v.fit_zoom(), 0.2));
        assert!(v.is_best_fit());
        let v = view(100.0, 50.0, 800.0, 600.0);
        assert!(approx(v.zoom(), 1.0));
        // Limited by height.
        let v = view(1000.0, 2000.0, 800.0, 600.0);
        assert!(approx(v.zoom(), 0.3));
        // Degenerate sizes don't produce NaN.
        let v = view(0.0, 0.0, 0.0, 0.0);
        assert!(approx(v.zoom(), 1.0));
    }

    #[test]
    fn best_fit_follows_viewport() {
        let mut v = view(1000.0, 1000.0, 500.0, 500.0);
        assert!(approx(v.zoom(), 0.5));
        v.set_viewport(250.0, 400.0);
        assert!(approx(v.zoom(), 0.25));
    }

    #[test]
    fn rotation_swaps_dimensions() {
        let mut v = view(2000.0, 1000.0, 1000.0, 1000.0);
        assert!(approx(v.zoom(), 0.5));
        v.rotate(90);
        assert_eq!(v.rotated_image(), (1000.0, 2000.0));
        assert!(approx(v.zoom(), 0.5));
        v.rotate(-180);
        assert_eq!(v.rotation, 270);
        v.rotate(90);
        assert_eq!(v.rotation, 0);
        let mut v = view(1600.0, 900.0, 1600.0, 900.0);
        v.rotate(-90);
        assert!(approx(v.zoom(), 0.5625));
    }

    #[test]
    fn zoom_steps_snap_to_fit_and_100() {
        let mut v = view(1000.0, 1000.0, 900.0, 900.0); // fit = 0.9
        assert!(approx(v.step_in(), 1.0)); // 0.9*1.25 crosses 1.0 → snap
        v.zoom_in(None);
        assert!(approx(v.zoom(), 1.0));
        assert!(!v.is_best_fit());
        v.zoom_in(None);
        assert!(approx(v.zoom(), 1.25));
        v.zoom_out(None);
        assert!(approx(v.zoom(), 1.0));
        v.zoom_out(None);
        assert!(approx(v.zoom(), 0.9));
        assert!(v.is_best_fit());
        // Cannot zoom out below fit.
        v.zoom_out(None);
        assert!(approx(v.zoom(), 0.9));
        // Zooming out from just above 100 % stops at 100 % before fit.
        v.zoom_to(1.1, None);
        v.zoom_out(None);
        assert!(approx(v.zoom(), 1.0));
        v.zoom_to(1000.0, None);
        assert!(approx(v.zoom(), MAX_ZOOM));
        assert!(approx(v.step_in(), MAX_ZOOM));
    }

    #[test]
    fn small_images_zoom_out_stops_at_100() {
        let mut v = view(100.0, 100.0, 800.0, 600.0);
        assert!(approx(v.min_zoom(), 1.0));
        v.zoom_to(0.1, None);
        assert!(approx(v.zoom(), 1.0));
        assert!(v.is_best_fit());
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut v = view(1000.0, 1000.0, 500.0, 500.0); // fit 0.5
                                                        // Image point under (400, 100) at fit: image rect is (0,0,500,500).
        let anchor = (400.0, 100.0);
        v.zoom_to(2.0, Some(anchor));
        // That point was image pixel (800, 200); it must still be under the anchor.
        let (x, y, _, _) = v.image_rect();
        assert!(approx(x + 800.0 * 2.0, 400.0));
        assert!(approx(y + 200.0 * 2.0, 100.0));
        // Zooming back out recentres to best fit.
        v.zoom_to(0.1, Some(anchor));
        assert!(v.is_best_fit());
        assert_eq!(v.offset, (0.0, 0.0));
    }

    #[test]
    fn anchor_near_edge_is_clamped() {
        let mut v = view(1000.0, 1000.0, 500.0, 500.0);
        v.zoom_to(1.0, Some((0.0, 0.0)));
        // Top-left corner anchored: offset is max positive (image's left edge at 0).
        let (x, y, w, h) = v.image_rect();
        assert!(approx(x, 0.0) && approx(y, 0.0));
        assert!(approx(w, 1000.0) && approx(h, 1000.0));
    }

    #[test]
    fn pan_is_clamped() {
        let mut v = view(1000.0, 400.0, 500.0, 500.0);
        v.zoom_to(1.0, None);
        assert!(v.can_pan());
        v.pan_by(10_000.0, 10_000.0);
        // Horizontal overflow 500 → max offset 250; vertical fits → 0.
        assert_eq!(v.offset, (250.0, 0.0));
        v.pan_by(-600.0, 0.0);
        assert_eq!(v.offset, (-250.0, 0.0));
        v.best_fit();
        assert!(!v.can_pan());
        v.pan_by(50.0, 50.0);
        assert_eq!(v.offset, (0.0, 0.0));
    }

    #[test]
    fn shrinking_viewport_reclamps_offset() {
        let mut v = view(1000.0, 1000.0, 400.0, 400.0);
        v.zoom_to(1.0, None);
        v.pan_by(300.0, 300.0);
        assert_eq!(v.offset, (300.0, 300.0));
        v.set_viewport(900.0, 900.0);
        assert_eq!(v.offset, (50.0, 50.0));
    }

    #[test]
    fn labels() {
        assert_eq!(zoom_label(1.0), "100 %");
        assert_eq!(zoom_label(0.333), "33 %");
    }
}
