//! Paper sizes, crop presets and interactive crop-rectangle editing.

use crate::page::{clamp_rect, Rect};

/// A named paper size in tenths of a millimetre (the unit of the
/// `paper-width` / `paper-height` GSettings keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paper {
    pub id: &'static str,
    pub label: &'static str,
    pub width: i32,
    pub height: i32,
}

pub const PAPERS: &[Paper] = &[
    Paper { id: "a6", label: "A6", width: 1050, height: 1480 },
    Paper { id: "a5", label: "A5", width: 1480, height: 2100 },
    Paper { id: "a4", label: "A4", width: 2100, height: 2970 },
    Paper { id: "letter", label: "US Letter", width: 2159, height: 2794 },
    Paper { id: "legal", label: "US Legal", width: 2159, height: 3556 },
    Paper { id: "4x6", label: "4×6", width: 1016, height: 1524 },
];

pub fn paper(id: &str) -> Option<&'static Paper> {
    PAPERS.iter().find(|p| p.id == id)
}

/// Index into [`PAPERS`] matching a GSettings width/height pair (either
/// orientation); `None` means automatic or a custom size.
pub fn paper_for_size(width: i32, height: i32) -> Option<usize> {
    PAPERS
        .iter()
        .position(|p| (p.width, p.height) == (width, height) || (p.height, p.width) == (width, height))
}

/// Converts tenths of millimetres to pixels at `dpi`.
pub fn tenth_mm_to_px(v: i32, dpi: f64) -> u32 {
    (v as f64 / 254.0 * dpi).round().max(1.0) as u32
}

/// The crop rectangle for a paper preset on a `w`×`h` page at `dpi`,
/// oriented like the page and centred.
pub fn preset_rect(p: &Paper, w: u32, h: u32, dpi: f64) -> Rect {
    let (mut pw, mut ph) = (tenth_mm_to_px(p.width, dpi), tenth_mm_to_px(p.height, dpi));
    if (w > h) != (pw > ph) {
        std::mem::swap(&mut pw, &mut ph);
    }
    centred(pw, ph, w, h)
}

/// A "custom" starting rectangle: the page inset by 10% on every side.
pub fn custom_rect(w: u32, h: u32) -> Rect {
    let (mx, my) = (w / 10, h / 10);
    clamp_rect(Rect::new(mx, my, w - 2 * mx, h - 2 * my), w, h)
}

/// Swaps the rectangle's orientation about its centre, clamped to the page.
pub fn rotate_crop(r: Rect, w: u32, h: u32) -> Rect {
    let cx = r.x as i64 + r.width as i64 / 2;
    let cy = r.y as i64 + r.height as i64 / 2;
    let (nw, nh) = (r.height.min(w), r.width.min(h));
    let x = (cx - nw as i64 / 2).clamp(0, (w - nw) as i64) as u32;
    let y = (cy - nh as i64 / 2).clamp(0, (h - nh) as i64) as u32;
    Rect::new(x, y, nw, nh)
}

fn centred(cw: u32, ch: u32, w: u32, h: u32) -> Rect {
    let (cw, ch) = (cw.min(w), ch.min(h));
    Rect::new((w - cw) / 2, (h - ch) / 2, cw, ch)
}

/// Which part of the crop rectangle is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Handle {
    /// CSS cursor name for this handle.
    pub fn cursor(self) -> &'static str {
        match self {
            Handle::Move => "move",
            Handle::Left | Handle::Right => "ew-resize",
            Handle::Top | Handle::Bottom => "ns-resize",
            Handle::TopLeft | Handle::BottomRight => "nwse-resize",
            Handle::TopRight | Handle::BottomLeft => "nesw-resize",
        }
    }
}

/// Finds the handle under point (`x`, `y`) (image coordinates); `tol` is
/// the grab distance in image pixels.
pub fn hit_test(r: Rect, x: f64, y: f64, tol: f64) -> Option<Handle> {
    let (l, t) = (r.x as f64, r.y as f64);
    let (rt, b) = (l + r.width as f64, t + r.height as f64);
    if x < l - tol || x > rt + tol || y < t - tol || y > b + tol {
        return None;
    }
    let near_l = (x - l).abs() <= tol;
    let near_r = (x - rt).abs() <= tol;
    let near_t = (y - t).abs() <= tol;
    let near_b = (y - b).abs() <= tol;
    Some(match (near_l, near_r, near_t, near_b) {
        (true, _, true, _) => Handle::TopLeft,
        (_, true, true, _) => Handle::TopRight,
        (true, _, _, true) => Handle::BottomLeft,
        (_, true, _, true) => Handle::BottomRight,
        (true, ..) => Handle::Left,
        (_, true, ..) => Handle::Right,
        (_, _, true, _) => Handle::Top,
        (.., true) => Handle::Bottom,
        _ if x > l && x < rt && y > t && y < b => Handle::Move,
        _ => return None,
    })
}

/// Applies a drag of (`dx`, `dy`) image pixels to `start`, keeping the
/// rectangle inside a `w`×`h` page and at least `min` pixels in size.
pub fn drag(start: Rect, handle: Handle, dx: f64, dy: f64, w: u32, h: u32, min: u32) -> Rect {
    let (dx, dy) = (dx.round() as i64, dy.round() as i64);
    let (w, h) = (w as i64, h as i64);
    let min_w = (min as i64).min(w);
    let min_h = (min as i64).min(h);
    let mut l = start.x as i64;
    let mut t = start.y as i64;
    let mut r = l + start.width as i64;
    let mut b = t + start.height as i64;
    if handle == Handle::Move {
        let nl = (l + dx).clamp(0, w - (r - l));
        let nt = (t + dy).clamp(0, h - (b - t));
        return Rect::new(nl as u32, nt as u32, start.width, start.height);
    }
    let (left, right, top, bottom) = match handle {
        Handle::Left => (true, false, false, false),
        Handle::Right => (false, true, false, false),
        Handle::Top => (false, false, true, false),
        Handle::Bottom => (false, false, false, true),
        Handle::TopLeft => (true, false, true, false),
        Handle::TopRight => (false, true, true, false),
        Handle::BottomLeft => (true, false, false, true),
        Handle::BottomRight => (false, true, false, true),
        Handle::Move => unreachable!(),
    };
    if left {
        l = (l + dx).clamp(0, r - min_w);
    }
    if right {
        r = (r + dx).clamp(l + min_w, w);
    }
    if top {
        t = (t + dy).clamp(0, b - min_h);
    }
    if bottom {
        b = (b + dy).clamp(t + min_h, h);
    }
    Rect::new(l as u32, t as u32, (r - l) as u32, (b - t) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_lookup() {
        assert_eq!(paper_for_size(2100, 2970), Some(2));
        assert_eq!(paper_for_size(2970, 2100), Some(2));
        assert_eq!(paper_for_size(0, 0), None);
        assert_eq!(paper("letter").unwrap().width, 2159);
    }

    #[test]
    fn preset_is_centred_and_oriented() {
        // A6 at 100 dpi ≈ 413 × 583 px.
        let r = preset_rect(paper("a6").unwrap(), 850, 1100, 100.0);
        assert_eq!((r.width, r.height), (413, 583));
        assert_eq!((r.x, r.y), ((850 - 413) / 2, (1100 - 583) / 2));
        // Landscape page → landscape crop.
        let r = preset_rect(paper("a6").unwrap(), 1100, 850, 100.0);
        assert_eq!((r.width, r.height), (583, 413));
        // Larger than the page → clamped.
        let r = preset_rect(paper("legal").unwrap(), 100, 120, 100.0);
        assert_eq!(r, Rect::new(0, 0, 100, 120));
    }

    #[test]
    fn rotate_crop_about_centre() {
        let r = rotate_crop(Rect::new(10, 20, 40, 20), 100, 100);
        assert_eq!(r, Rect::new(20, 10, 20, 40));
        // Clamped at the page edge.
        let r = rotate_crop(Rect::new(0, 0, 80, 10), 100, 50);
        assert_eq!(r, Rect::new(35, 0, 10, 50));
    }

    #[test]
    fn hit_testing() {
        let r = Rect::new(10, 10, 100, 50);
        assert_eq!(hit_test(r, 10.0, 10.0, 4.0), Some(Handle::TopLeft));
        assert_eq!(hit_test(r, 111.0, 61.0, 4.0), Some(Handle::BottomRight));
        assert_eq!(hit_test(r, 60.0, 8.0, 4.0), Some(Handle::Top));
        assert_eq!(hit_test(r, 108.0, 30.0, 4.0), Some(Handle::Right));
        assert_eq!(hit_test(r, 60.0, 30.0, 4.0), Some(Handle::Move));
        assert_eq!(hit_test(r, 200.0, 30.0, 4.0), None);
    }

    #[test]
    fn dragging() {
        let r = Rect::new(10, 10, 50, 50);
        assert_eq!(drag(r, Handle::Move, 100.0, -100.0, 100, 100, 5), Rect::new(50, 0, 50, 50));
        assert_eq!(drag(r, Handle::BottomRight, 10.0, 5.0, 100, 100, 5), Rect::new(10, 10, 60, 55));
        // Cannot shrink below the minimum or flip.
        assert_eq!(drag(r, Handle::Left, 100.0, 0.0, 100, 100, 5), Rect::new(55, 10, 5, 50));
        assert_eq!(drag(r, Handle::TopLeft, -50.0, -50.0, 100, 100, 5), Rect::new(0, 0, 60, 60));
        assert_eq!(drag(r, Handle::Bottom, 0.0, 500.0, 100, 100, 5), Rect::new(10, 10, 50, 90));
    }

    #[test]
    fn custom_inset() {
        assert_eq!(custom_rect(100, 200), Rect::new(10, 20, 80, 160));
        assert_eq!(custom_rect(3, 3), Rect::new(0, 0, 3, 3));
    }
}
