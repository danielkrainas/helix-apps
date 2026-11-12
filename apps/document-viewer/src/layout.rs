//! Pure page-layout and zoom math for the continuous / dual-page view.
//!
//! Coordinates:
//! - *page space*: PDF points, origin top-left of the unrotated page.
//! - *content space*: logical pixels of the whole scrollable document.

/// Logical pixels per PDF point at 100 % zoom (96 dpi / 72 pt per inch).
pub const PX_PER_PT: f64 = 96.0 / 72.0;
/// Margin around the document, logical px.
pub const MARGIN: f64 = 12.0;
/// Vertical gap between rows and horizontal gap between dual pages.
pub const GAP: f64 = 12.0;
pub const MIN_ZOOM: f64 = 0.10;
pub const MAX_ZOOM: f64 = 16.0;
pub const ZOOM_PRESETS: &[f64] = &[
    0.10, 0.25, 0.333, 0.50, 0.667, 0.80, 1.0, 1.25, 1.50, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0,
];
/// Side length of a render tile, device pixels.
pub const TILE: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }
    /// From two corners in any order.
    pub fn from_corners(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Rect::new(x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs())
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        (r > x && b > y).then(|| Rect::new(x, y, r - x, b - y))
    }
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}

/// How the zoom level is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ZoomMode {
    FitWidth,
    FitPage,
    #[default]
    Automatic,
    Free,
}

impl ZoomMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ZoomMode::FitWidth => "fit-width",
            ZoomMode::FitPage => "fit-page",
            ZoomMode::Automatic => "automatic",
            ZoomMode::Free => "free",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "fit-width" => ZoomMode::FitWidth,
            "fit-page" => ZoomMode::FitPage,
            "automatic" => ZoomMode::Automatic,
            "free" => ZoomMode::Free,
            _ => return None,
        })
    }
}

/// Normalises a rotation to 0, 90, 180 or 270 degrees (clockwise).
pub fn norm_rotation(deg: i32) -> u32 {
    (deg.rem_euclid(360) / 90 * 90) as u32
}

/// Size of a page after rotation, in the units of `(w, h)`.
pub fn rotated_size(w: f64, h: f64, rotation: u32) -> (f64, f64) {
    if rotation % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    }
}

/// Maps an unrotated page point to rotated page space (same units).
pub fn rotate_point(x: f64, y: f64, w: f64, h: f64, rotation: u32) -> (f64, f64) {
    match rotation {
        90 => (h - y, x),
        180 => (w - x, h - y),
        270 => (y, w - x),
        _ => (x, y),
    }
}

/// Inverse of [`rotate_point`].
pub fn unrotate_point(rx: f64, ry: f64, w: f64, h: f64, rotation: u32) -> (f64, f64) {
    match rotation {
        90 => (ry, h - rx),
        180 => (w - rx, h - ry),
        270 => (w - ry, rx),
        _ => (rx, ry),
    }
}

/// Maps a rectangle in unrotated page space (points) to content space given
/// the page's content rectangle and scale (px per pt).
pub fn page_rect_to_content(
    r: &Rect,
    page: &Rect,
    pw: f64,
    ph: f64,
    rotation: u32,
    scale: f64,
) -> Rect {
    let (x1, y1) = rotate_point(r.x, r.y, pw, ph, rotation);
    let (x2, y2) = rotate_point(r.right(), r.bottom(), pw, ph, rotation);
    let rr = Rect::from_corners(x1, y1, x2, y2);
    Rect::new(
        page.x + rr.x * scale,
        page.y + rr.y * scale,
        rr.w * scale,
        rr.h * scale,
    )
}

/// Converts the zoom factor to px per pt.
pub fn zoom_to_scale(zoom: f64) -> f64 {
    zoom * PX_PER_PT
}

pub fn clamp_zoom(z: f64) -> f64 {
    if z.is_finite() {
        z.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

pub fn zoom_in_step(z: f64) -> f64 {
    ZOOM_PRESETS
        .iter()
        .copied()
        .find(|p| *p > z * 1.001)
        .unwrap_or(MAX_ZOOM)
}

pub fn zoom_out_step(z: f64) -> f64 {
    ZOOM_PRESETS
        .iter()
        .rev()
        .copied()
        .find(|p| *p < z / 1.001)
        .unwrap_or(MIN_ZOOM)
}

/// Parameters of a layout.
#[derive(Clone, Debug)]
pub struct LayoutParams<'a> {
    /// Unrotated page sizes in points.
    pub pages: &'a [(f64, f64)],
    pub rotation: u32,
    pub dual: bool,
    /// px per pt
    pub scale: f64,
    /// Viewport width used to centre narrow documents.
    pub viewport_w: f64,
}

/// Indices of the pages in each row.
pub fn rows(n: usize, dual: bool) -> Vec<Vec<usize>> {
    if dual {
        (0..n)
            .step_by(2)
            .map(|i| (i..(i + 2).min(n)).collect())
            .collect()
    } else {
        (0..n).map(|i| vec![i]).collect()
    }
}

/// Row widths and heights in points (rotated, without margins), the
/// horizontal dual gap counted as `GAP / scale` is not included here.
fn row_extents(pages: &[(f64, f64)], rotation: u32, dual: bool) -> Vec<(f64, f64, usize)> {
    rows(pages.len(), dual)
        .into_iter()
        .map(|row| {
            let mut w = 0.0;
            let mut h: f64 = 0.0;
            for &i in &row {
                let (rw, rh) = rotated_size(pages[i].0, pages[i].1, rotation);
                w += rw;
                h = h.max(rh);
            }
            (w, h, row.len())
        })
        .collect()
}

/// Zoom factor (not scale) for a fit mode given the viewport size.
pub fn fit_zoom(
    mode: ZoomMode,
    pages: &[(f64, f64)],
    rotation: u32,
    dual: bool,
    viewport_w: f64,
    viewport_h: f64,
    current: f64,
) -> f64 {
    if pages.is_empty() || mode == ZoomMode::Free {
        return current;
    }
    let ext = row_extents(pages, rotation, dual);
    // scale = (vw - 2*margin - gaps) / row_w_pts, minimised over rows.
    let mut fit_w = f64::INFINITY;
    let mut fit_h = f64::INFINITY;
    for (w, h, n) in ext {
        let gaps = GAP * (n.saturating_sub(1)) as f64;
        if w > 0.0 {
            fit_w = fit_w.min((viewport_w - 2.0 * MARGIN - gaps).max(1.0) / w);
        }
        if h > 0.0 {
            fit_h = fit_h.min((viewport_h - 2.0 * MARGIN).max(1.0) / h);
        }
    }
    let scale = match mode {
        ZoomMode::FitWidth => fit_w,
        ZoomMode::FitPage => fit_w.min(fit_h),
        ZoomMode::Automatic => fit_w.min(PX_PER_PT),
        ZoomMode::Free => unreachable!(),
    };
    if !scale.is_finite() {
        return current;
    }
    clamp_zoom(scale / PX_PER_PT)
}

/// Result of laying out the document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// Content-space rectangle of each page (rotated size).
    pub pages: Vec<Rect>,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub rotation: u32,
}

impl Layout {
    pub fn compute(p: &LayoutParams) -> Layout {
        let ext = row_extents(p.pages, p.rotation, p.dual);
        let max_row_w = ext
            .iter()
            .map(|(w, _, n)| w * p.scale + GAP * n.saturating_sub(1) as f64)
            .fold(0.0, f64::max);
        let width = (max_row_w + 2.0 * MARGIN).max(p.viewport_w);
        let mut rects = vec![Rect::default(); p.pages.len()];
        let mut y = MARGIN;
        for (row, (rw, rh, n)) in rows(p.pages.len(), p.dual).into_iter().zip(ext) {
            let row_w = rw * p.scale + GAP * n.saturating_sub(1) as f64;
            let row_h = rh * p.scale;
            let mut x = ((width - row_w) / 2.0).floor();
            for i in row {
                let (w, h) = rotated_size(p.pages[i].0, p.pages[i].1, p.rotation);
                let (w, h) = (w * p.scale, h * p.scale);
                rects[i] = Rect::new(x, (y + (row_h - h) / 2.0).floor(), w, h);
                x += w + GAP;
            }
            y += row_h + GAP;
        }
        let height = if p.pages.is_empty() {
            2.0 * MARGIN
        } else {
            y - GAP + MARGIN
        };
        Layout {
            pages: rects,
            width,
            height,
            scale: p.scale,
            rotation: p.rotation,
        }
    }

    /// Page under a content-space point.
    pub fn page_at(&self, x: f64, y: f64) -> Option<usize> {
        self.pages.iter().position(|r| r.contains(x, y))
    }

    /// Nearest page to a content point (for drags outside pages).
    pub fn nearest_page(&self, x: f64, y: f64) -> Option<usize> {
        self.pages
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let dx = (r.x - x).max(0.0).max(x - r.right());
                let dy = (r.y - y).max(0.0).max(y - r.bottom());
                (i, dx * dx + dy * dy)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Converts a content point to unrotated page points of page `i`
    /// (clamped to the page).
    pub fn content_to_page(&self, i: usize, x: f64, y: f64, pw: f64, ph: f64) -> (f64, f64) {
        let r = &self.pages[i];
        let rx = ((x - r.x) / self.scale).clamp(0.0, r.w / self.scale);
        let ry = ((y - r.y) / self.scale).clamp(0.0, r.h / self.scale);
        unrotate_point(rx, ry, pw, ph, self.rotation)
    }

    /// Like [`Layout::content_to_page`] but without clamping (points
    /// outside the page map to coordinates outside `0..w`, `0..h`).
    pub fn content_to_page_unclamped(
        &self,
        i: usize,
        x: f64,
        y: f64,
        pw: f64,
        ph: f64,
    ) -> (f64, f64) {
        let r = &self.pages[i];
        unrotate_point(
            (x - r.x) / self.scale,
            (y - r.y) / self.scale,
            pw,
            ph,
            self.rotation,
        )
    }

    /// Content-space point of an unrotated page point.
    pub fn page_to_content(&self, i: usize, x: f64, y: f64, pw: f64, ph: f64) -> (f64, f64) {
        let r = &self.pages[i];
        let (rx, ry) = rotate_point(x, y, pw, ph, self.rotation);
        (r.x + rx * self.scale, r.y + ry * self.scale)
    }

    /// Pages intersecting the viewport, in order.
    pub fn visible(&self, view: &Rect) -> Vec<usize> {
        self.pages
            .iter()
            .enumerate()
            .filter(|(_, r)| r.intersect(view).is_some())
            .map(|(i, _)| i)
            .collect()
    }

    /// The page with the largest visible area (first one on ties).
    pub fn current_page(&self, view: &Rect) -> usize {
        let mut best = (0usize, -1.0f64);
        for (i, r) in self.pages.iter().enumerate() {
            let a = r.intersect(view).map(|x| x.area()).unwrap_or(0.0);
            if a > best.1 + 0.5 {
                best = (i, a);
            }
        }
        if best.1 <= 0.0 {
            // Nothing visible: pick the page closest to the viewport centre.
            return self
                .nearest_page(view.x + view.w / 2.0, view.y + view.h / 2.0)
                .unwrap_or(0);
        }
        best.0
    }
}

/// Tiles (tx, ty) of a page rendered at `w_px`×`h_px` device pixels that
/// intersect `vis`, a rectangle in the *unrotated* page's device pixels.
pub fn tiles_for(w_px: u32, h_px: u32, vis: &Rect) -> Vec<(u32, u32)> {
    if w_px == 0 || h_px == 0 {
        return Vec::new();
    }
    let page = Rect::new(0.0, 0.0, w_px as f64, h_px as f64);
    let Some(v) = page.intersect(vis) else {
        return Vec::new();
    };
    let t = TILE as f64;
    let tx0 = (v.x / t).floor() as u32;
    let ty0 = (v.y / t).floor() as u32;
    let tx1 = ((v.right() / t).ceil() as u32).min(w_px.div_ceil(TILE));
    let ty1 = ((v.bottom() / t).ceil() as u32).min(h_px.div_ceil(TILE));
    let mut out = Vec::new();
    for ty in ty0..ty1 {
        for tx in tx0..tx1 {
            out.push((tx, ty));
        }
    }
    out
}

/// Device-pixel size of a page at render scale `s` (device px per pt).
pub fn page_px(pw: f64, ph: f64, s: f64) -> (u32, u32) {
    (
        ((pw * s).ceil() as u32).max(1),
        ((ph * s).ceil() as u32).max(1),
    )
}

/// Pixel rectangle covered by tile `(tx, ty)` of a `w_px`×`h_px` page.
pub fn tile_rect(w_px: u32, h_px: u32, tx: u32, ty: u32) -> (u32, u32, u32, u32) {
    let x = tx * TILE;
    let y = ty * TILE;
    (
        x,
        y,
        TILE.min(w_px.saturating_sub(x)),
        TILE.min(h_px.saturating_sub(y)),
    )
}

/// Visible part of page `i` in unrotated page *points* (or `None`).
pub fn visible_page_area(layout: &Layout, i: usize, view: &Rect, pw: f64, ph: f64) -> Option<Rect> {
    let r = layout.pages.get(i)?;
    let v = r.intersect(view)?;
    let s = layout.scale;
    let (x1, y1) = unrotate_point((v.x - r.x) / s, (v.y - r.y) / s, pw, ph, layout.rotation);
    let (x2, y2) = unrotate_point(
        (v.right() - r.x) / s,
        (v.bottom() - r.y) / s,
        pw,
        ph,
        layout.rotation,
    );
    Some(Rect::from_corners(x1, y1, x2, y2))
}

/// Selection rectangle (page points) on page `i` for a selection running
/// from `(a_page, a)` to `(b_page, b)`. Returns `None` for pages outside.
pub fn selection_rect_for_page(
    i: usize,
    a_page: usize,
    a: (f64, f64),
    b_page: usize,
    b: (f64, f64),
    size: (f64, f64),
) -> Option<Rect> {
    let forward = a_page < b_page || (a_page == b_page && (a.1, a.0) <= (b.1, b.0));
    let ((sp, s), (ep, e)) = if forward {
        ((a_page, a), (b_page, b))
    } else {
        ((b_page, b), (a_page, a))
    };
    if i < sp || i > ep {
        return None;
    }
    // Poppler selections are "from point to point" in reading order; we
    // pass the two points as x1,y1 → x2,y2 unchanged within one page.
    let (x1, y1) = if i == sp { s } else { (0.0, 0.0) };
    let (x2, y2) = if i == ep { e } else { size };
    Some(Rect {
        x: x1,
        y: y1,
        w: x2 - x1,
        h: y2 - y1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: (f64, f64) = (595.0, 842.0);

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn rotation_roundtrip() {
        for rot in [0, 90, 180, 270] {
            let (rx, ry) = rotate_point(10.0, 20.0, 100.0, 200.0, rot);
            let (x, y) = unrotate_point(rx, ry, 100.0, 200.0, rot);
            assert!(approx(x, 10.0) && approx(y, 20.0), "rot {rot}");
            let (w, h) = rotated_size(100.0, 200.0, rot);
            assert!(rx >= 0.0 && rx <= w && ry >= 0.0 && ry <= h);
        }
        assert_eq!(rotate_point(0.0, 0.0, 100.0, 200.0, 90), (200.0, 0.0));
        assert_eq!(norm_rotation(-90), 270);
        assert_eq!(norm_rotation(450), 90);
    }

    #[test]
    fn continuous_layout() {
        let pages = [A4, A4, (842.0, 595.0)];
        let l = Layout::compute(&LayoutParams {
            pages: &pages,
            rotation: 0,
            dual: false,
            scale: 1.0,
            viewport_w: 0.0,
        });
        assert_eq!(l.width, 842.0 + 2.0 * MARGIN);
        assert_eq!(l.pages[0].y, MARGIN);
        assert_eq!(l.pages[1].y, MARGIN + 842.0 + GAP);
        assert_eq!(l.pages[2].y, MARGIN + 2.0 * (842.0 + GAP));
        assert_eq!(l.height, MARGIN + 2.0 * 842.0 + 595.0 + 2.0 * GAP + MARGIN);
        // narrower page centred
        assert_eq!(l.pages[0].x, ((l.width - 595.0) / 2.0).floor());
        assert_eq!(l.pages[2].x, MARGIN);
        // Wider viewport centres everything.
        let l2 = Layout::compute(&LayoutParams {
            pages: &pages,
            rotation: 0,
            dual: false,
            scale: 1.0,
            viewport_w: 2000.0,
        });
        assert_eq!(l2.width, 2000.0);
        assert_eq!(l2.pages[2].x, ((2000.0 - 842.0) / 2.0_f64).floor());
    }

    #[test]
    fn dual_layout() {
        let pages = [A4, A4, A4];
        let l = Layout::compute(&LayoutParams {
            pages: &pages,
            rotation: 0,
            dual: true,
            scale: 0.5,
            viewport_w: 0.0,
        });
        let pw = 595.0 * 0.5;
        assert_eq!(l.width, 2.0 * pw + GAP + 2.0 * MARGIN);
        assert_eq!(l.pages[0].y, l.pages[1].y);
        assert_eq!(l.pages[0].x, MARGIN);
        assert_eq!(l.pages[1].x, MARGIN + pw + GAP);
        assert_eq!(l.pages[2].y, MARGIN + 421.0 + GAP);
        assert_eq!(rows(5, true), vec![vec![0, 1], vec![2, 3], vec![4]]);
    }

    #[test]
    fn rotated_layout_and_hit_test() {
        let pages = [A4];
        let l = Layout::compute(&LayoutParams {
            pages: &pages,
            rotation: 90,
            dual: false,
            scale: 2.0,
            viewport_w: 0.0,
        });
        assert_eq!(l.pages[0].w, 842.0 * 2.0);
        assert_eq!(l.pages[0].h, 595.0 * 2.0);
        // Top-left of the unrotated page lands at the top-right corner.
        let (cx, cy) = l.page_to_content(0, 0.0, 0.0, A4.0, A4.1);
        assert!(approx(cx, l.pages[0].right()) && approx(cy, l.pages[0].y));
        let (px, py) = l.content_to_page(0, cx - 1.0, cy + 1.0, A4.0, A4.1);
        assert!(approx(px, 0.5) && approx(py, 0.5));
        assert_eq!(l.page_at(cx - 1.0, cy + 1.0), Some(0));
        assert_eq!(l.page_at(0.0, 0.0), None);
        let (ux, uy) = l.content_to_page_unclamped(0, cx + 4.0, cy - 2.0, A4.0, A4.1);
        assert!(approx(ux, -1.0) && approx(uy, -2.0), "{ux} {uy}");
        let (bx, by) = l.page_to_content(0, ux, uy, A4.0, A4.1);
        assert!(approx(bx, cx + 4.0) && approx(by, cy - 2.0));
        let r = page_rect_to_content(
            &Rect::new(0.0, 0.0, 10.0, 20.0),
            &l.pages[0],
            A4.0,
            A4.1,
            90,
            2.0,
        );
        assert!(approx(r.right(), l.pages[0].right()) && approx(r.w, 40.0) && approx(r.h, 20.0));
    }

    #[test]
    fn fit_modes() {
        let pages = [A4, A4];
        let zw = fit_zoom(ZoomMode::FitWidth, &pages, 0, false, 619.0, 500.0, 1.0);
        assert!(approx(zw * PX_PER_PT, 1.0));
        let zp = fit_zoom(ZoomMode::FitPage, &pages, 0, false, 619.0, 445.0, 1.0);
        assert!(approx(zp * PX_PER_PT, 0.5));
        // Automatic caps at 100 %.
        let za = fit_zoom(ZoomMode::Automatic, &pages, 0, false, 5000.0, 500.0, 1.0);
        assert!(approx(za, 1.0));
        // Dual: two pages + gap.
        let zd = fit_zoom(
            ZoomMode::FitWidth,
            &pages,
            0,
            true,
            2.0 * 595.0 + GAP + 2.0 * MARGIN,
            500.0,
            1.0,
        );
        assert!(approx(zd * PX_PER_PT, 1.0));
        // Rotated: width is the page height.
        let zr = fit_zoom(
            ZoomMode::FitWidth,
            &pages,
            90,
            false,
            842.0 + 2.0 * MARGIN,
            500.0,
            1.0,
        );
        assert!(approx(zr * PX_PER_PT, 1.0));
        assert_eq!(
            fit_zoom(ZoomMode::Free, &pages, 0, false, 10.0, 10.0, 3.0),
            3.0
        );
    }

    #[test]
    fn zoom_steps() {
        assert_eq!(zoom_in_step(1.0), 1.25);
        assert_eq!(zoom_out_step(1.0), 0.8);
        assert_eq!(zoom_in_step(1.1), 1.25);
        assert_eq!(zoom_out_step(1.1), 1.0);
        assert_eq!(zoom_in_step(16.0), MAX_ZOOM);
        assert_eq!(zoom_out_step(0.1), MIN_ZOOM);
        assert_eq!(clamp_zoom(100.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(f64::NAN), 1.0);
    }

    #[test]
    fn current_page_and_visibility() {
        let pages = [A4, A4, A4];
        let l = Layout::compute(&LayoutParams {
            pages: &pages,
            rotation: 0,
            dual: false,
            scale: 1.0,
            viewport_w: 0.0,
        });
        let view = Rect::new(0.0, 800.0, 600.0, 400.0);
        assert_eq!(l.visible(&view), vec![0, 1]);
        assert_eq!(l.current_page(&view), 1);
        assert_eq!(l.current_page(&Rect::new(0.0, 0.0, 600.0, 400.0)), 0);
    }

    #[test]
    fn tiles() {
        assert_eq!(
            tiles_for(2000, 1000, &Rect::new(0.0, 0.0, 2000.0, 1000.0)),
            vec![(0, 0), (1, 0)]
        );
        assert_eq!(
            tiles_for(3000, 3000, &Rect::new(1500.0, 1500.0, 10.0, 10.0)),
            vec![(1, 1)]
        );
        assert!(tiles_for(3000, 3000, &Rect::new(4000.0, 0.0, 10.0, 10.0)).is_empty());
        assert_eq!(tile_rect(1500, 900, 1, 0), (1024, 0, 476, 900));
        assert_eq!(page_px(595.0, 842.0, 2.0), (1190, 1684));
    }

    #[test]
    fn visible_area_rotated() {
        let pages = [(100.0, 200.0)];
        let l = Layout::compute(&LayoutParams {
            pages: &pages,
            rotation: 90,
            dual: false,
            scale: 1.0,
            viewport_w: 0.0,
        });
        let r = l.pages[0];
        // Viewport covering the right half of the rotated page = top half of unrotated.
        let view = Rect::new(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h);
        let a = visible_page_area(&l, 0, &view, 100.0, 200.0).unwrap();
        assert!(approx(a.x, 0.0) && approx(a.y, 0.0) && approx(a.w, 100.0) && approx(a.h, 100.0));
    }

    #[test]
    fn selection_rects() {
        let size = (100.0, 200.0);
        // Single page.
        let r = selection_rect_for_page(0, 0, (10.0, 20.0), 0, (50.0, 60.0), size).unwrap();
        assert_eq!((r.x, r.y, r.right(), r.bottom()), (10.0, 20.0, 50.0, 60.0));
        // Multi page, reversed drag direction.
        let first = selection_rect_for_page(1, 3, (5.0, 5.0), 1, (10.0, 20.0), size).unwrap();
        assert_eq!(
            (first.x, first.y, first.right(), first.bottom()),
            (10.0, 20.0, 100.0, 200.0)
        );
        let mid = selection_rect_for_page(2, 3, (5.0, 5.0), 1, (10.0, 20.0), size).unwrap();
        assert_eq!((mid.x, mid.y, mid.w, mid.h), (0.0, 0.0, 100.0, 200.0));
        let last = selection_rect_for_page(3, 3, (5.0, 5.0), 1, (10.0, 20.0), size).unwrap();
        assert_eq!(
            (last.x, last.y, last.right(), last.bottom()),
            (0.0, 0.0, 5.0, 5.0)
        );
        assert!(selection_rect_for_page(0, 3, (5.0, 5.0), 1, (10.0, 20.0), size).is_none());
    }
}
