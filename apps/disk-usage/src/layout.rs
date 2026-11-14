//! Chart geometry: squarified treemap and ring chart layout, plus hit
//! testing. Pure math, no drawing.

use crate::scanner::{NodeId, ScanTree};
use std::f64::consts::{FRAC_PI_2, TAU};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn inset(&self, l: f64, t: f64, r: f64, b: f64) -> Rect {
        Rect::new(
            self.x + l,
            self.y + t,
            (self.w - l - r).max(0.0),
            (self.h - t - b).max(0.0),
        )
    }
}

/// Worst aspect ratio of a row with total area `sum`, extreme areas
/// `min`/`max`, laid along a side of length `side`.
fn worst(sum: f64, min: f64, max: f64, side: f64) -> f64 {
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

/// Squarified treemap (Bruls, Huizing, van Wijk). Returns one rectangle per
/// weight, in input order. Rectangle areas are proportional to the weights
/// and tile `rect` exactly. Zero / negative weights get empty rectangles.
pub fn squarify(weights: &[f64], rect: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::new(rect.x, rect.y, 0.0, 0.0); weights.len()];
    let total: f64 = weights.iter().filter(|w| **w > 0.0).sum();
    if total <= 0.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return out;
    }
    let scale = rect.area() / total;
    let mut order: Vec<usize> = (0..weights.len()).filter(|&i| weights[i] > 0.0).collect();
    order.sort_by(|&a, &b| weights[b].total_cmp(&weights[a]));
    let areas: Vec<f64> = order.iter().map(|&i| weights[i] * scale).collect();

    let mut r = rect;
    let mut start = 0;
    while start < order.len() {
        let side = r.w.min(r.h);
        let mut end = start + 1;
        let mut sum = areas[start];
        let (mut min, mut max) = (areas[start], areas[start]);
        while end < order.len() {
            let a = areas[end];
            let (nmin, nmax, nsum) = (min.min(a), max.max(a), sum + a);
            if worst(nsum, nmin, nmax, side) > worst(sum, min, max, side) {
                break;
            }
            sum = nsum;
            min = nmin;
            max = nmax;
            end += 1;
        }
        let last_row = end == order.len();
        if r.w >= r.h {
            // Column on the left.
            let w = if last_row { r.w } else { sum / r.h };
            let mut y = r.y;
            for k in start..end {
                let h = if k + 1 == end {
                    r.y + r.h - y
                } else {
                    areas[k] / w
                };
                out[order[k]] = Rect::new(r.x, y, w, h);
                y += h;
            }
            r = Rect::new(r.x + w, r.y, r.w - w, r.h);
        } else {
            // Row on top.
            let h = if last_row { r.h } else { sum / r.w };
            let mut x = r.x;
            for k in start..end {
                let w = if k + 1 == end {
                    r.x + r.w - x
                } else {
                    areas[k] / h
                };
                out[order[k]] = Rect::new(x, r.y, w, h);
                x += w;
            }
            r = Rect::new(r.x, r.y + h, r.w, r.h - h);
        }
        start = end;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub node: NodeId,
    pub rect: Rect,
    pub depth: usize,
    /// Index of the top-level child this tile descends from (for colour).
    pub branch: usize,
}

pub const TREEMAP_PAD: f64 = 2.0;
pub const TREEMAP_HEADER: f64 = 14.0;

fn visible_children(tree: &ScanTree, id: NodeId) -> Vec<NodeId> {
    tree.node(id)
        .children
        .iter()
        .copied()
        .filter(|&c| !tree.node(c).removed && tree.node(c).size > 0)
        .collect()
}

/// Nested treemap of `root` inside `rect`. Parents come before their
/// children in the result, so the last tile containing a point is the
/// deepest one.
pub fn treemap(
    tree: &ScanTree,
    root: NodeId,
    rect: Rect,
    max_depth: usize,
    min_px: f64,
) -> Vec<Tile> {
    let mut out = vec![Tile {
        node: root,
        rect,
        depth: 0,
        branch: 0,
    }];
    #[allow(clippy::too_many_arguments)]
    fn recurse(
        tree: &ScanTree,
        out: &mut Vec<Tile>,
        id: NodeId,
        rect: Rect,
        depth: usize,
        branch: Option<usize>,
        max_depth: usize,
        min_px: f64,
    ) {
        if depth >= max_depth {
            return;
        }
        let header = if rect.h >= TREEMAP_HEADER * 2.5 && depth > 0 {
            TREEMAP_HEADER
        } else {
            0.0
        };
        let inner = rect.inset(TREEMAP_PAD, TREEMAP_PAD + header, TREEMAP_PAD, TREEMAP_PAD);
        if inner.w < min_px || inner.h < min_px {
            return;
        }
        let kids = visible_children(tree, id);
        let weights: Vec<f64> = kids.iter().map(|&c| tree.node(c).size as f64).collect();
        for (i, (&kid, r)) in kids.iter().zip(squarify(&weights, inner)).enumerate() {
            if r.w < min_px || r.h < min_px {
                continue;
            }
            let b = branch.unwrap_or(i);
            out.push(Tile {
                node: kid,
                rect: r,
                depth: depth + 1,
                branch: b,
            });
            recurse(tree, out, kid, r, depth + 1, Some(b), max_depth, min_px);
        }
    }
    recurse(tree, &mut out, root, rect, 0, None, max_depth, min_px);
    out
}

pub fn treemap_hit(tiles: &[Tile], x: f64, y: f64) -> Option<&Tile> {
    tiles.iter().rev().find(|t| t.rect.contains(x, y))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub node: NodeId,
    /// Ring index, 1 = innermost ring around the centre disc.
    pub depth: usize,
    /// Angles in radians, clockwise from 12 o'clock, within [0, 2π].
    pub start: f64,
    pub end: f64,
    pub branch: usize,
}

/// Ring chart segments for `root` up to `max_depth` rings. A child's span is
/// its share of the parent's size times the parent's span. Segments
/// narrower than `min_angle` are dropped (with their descendants).
pub fn rings(tree: &ScanTree, root: NodeId, max_depth: usize, min_angle: f64) -> Vec<Segment> {
    let mut out = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn recurse(
        tree: &ScanTree,
        out: &mut Vec<Segment>,
        id: NodeId,
        start: f64,
        span: f64,
        depth: usize,
        branch: Option<usize>,
        max_depth: usize,
        min_angle: f64,
    ) {
        if depth > max_depth {
            return;
        }
        let total = tree.node(id).size as f64;
        if total <= 0.0 {
            return;
        }
        let mut a = start;
        for (i, kid) in visible_children(tree, id).into_iter().enumerate() {
            let s = span * tree.node(kid).size as f64 / total;
            if s >= min_angle {
                let b = branch.unwrap_or(i);
                out.push(Segment {
                    node: kid,
                    depth,
                    start: a,
                    end: a + s,
                    branch: b,
                });
                recurse(
                    tree,
                    out,
                    kid,
                    a,
                    s,
                    depth + 1,
                    Some(b),
                    max_depth,
                    min_angle,
                );
            }
            a += s;
        }
    }
    recurse(
        tree, &mut out, root, 0.0, TAU, 1, None, max_depth, min_angle,
    );
    out
}

/// Ring chart geometry for a drawing area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingGeometry {
    pub cx: f64,
    pub cy: f64,
    pub center_radius: f64,
    pub ring_width: f64,
}

impl RingGeometry {
    pub fn new(width: f64, height: f64, rings: usize) -> Self {
        let radius = (width.min(height) / 2.0 - 8.0).max(10.0);
        // Centre disc is as wide as 1.5 rings.
        let ring_width = radius / (rings as f64 + 1.5);
        RingGeometry {
            cx: width / 2.0,
            cy: height / 2.0,
            center_radius: ring_width * 1.5,
            ring_width,
        }
    }

    pub fn inner_radius(&self, depth: usize) -> f64 {
        self.center_radius + (depth - 1) as f64 * self.ring_width
    }

    /// Converts a layout angle (clockwise from 12 o'clock) to a cairo angle.
    pub fn to_cairo(angle: f64) -> f64 {
        angle - FRAC_PI_2
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RingHit {
    Center,
    Segment(usize),
}

/// Hit test; returns the index into `segs` (or the centre disc).
pub fn ring_hit(segs: &[Segment], g: &RingGeometry, x: f64, y: f64) -> Option<RingHit> {
    let (dx, dy) = (x - g.cx, y - g.cy);
    let d = (dx * dx + dy * dy).sqrt();
    if d < g.center_radius {
        return Some(RingHit::Center);
    }
    let depth = ((d - g.center_radius) / g.ring_width).floor() as usize + 1;
    let mut a = dy.atan2(dx) + FRAC_PI_2;
    if a < 0.0 {
        a += TAU;
    }
    segs.iter()
        .position(|s| s.depth == depth && a >= s.start && a < s.end)
        .map(RingHit::Segment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{Kind, ROOT};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        let e = 1e-9;
        a.x + e < b.x + b.w && b.x + e < a.x + a.w && a.y + e < b.y + b.h && b.y + e < a.y + a.h
    }

    #[test]
    fn squarify_areas_bounds_no_overlap() {
        let weights = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rect = Rect::new(10.0, 20.0, 600.0, 400.0);
        let rs = squarify(&weights, rect);
        let total: f64 = weights.iter().sum();
        for (w, r) in weights.iter().zip(&rs) {
            assert!(close(r.area(), rect.area() * w / total), "{r:?} for {w}");
            assert!(r.x >= rect.x - 1e-9 && r.y >= rect.y - 1e-9);
            assert!(r.x + r.w <= rect.x + rect.w + 1e-6 && r.y + r.h <= rect.y + rect.h + 1e-6);
        }
        for i in 0..rs.len() {
            for j in i + 1..rs.len() {
                assert!(
                    !overlaps(&rs[i], &rs[j]),
                    "{:?} overlaps {:?}",
                    rs[i],
                    rs[j]
                );
            }
        }
        let sum: f64 = rs.iter().map(Rect::area).sum();
        assert!(close(sum, rect.area()));
    }

    #[test]
    fn squarify_classic_example_aspect_ratios() {
        // The paper's example: 6,6,4,3,2,2,1 in a 6x4 rectangle.
        let rs = squarify(
            &[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0],
            Rect::new(0.0, 0.0, 6.0, 4.0),
        );
        // First two items stacked in a column of width 3.
        assert!(close(rs[0].w, 3.0) && close(rs[0].h, 2.0));
        assert!(close(rs[1].w, 3.0) && close(rs[1].h, 2.0));
        for r in &rs {
            let ratio = (r.w / r.h).max(r.h / r.w);
            assert!(ratio <= 3.0, "aspect ratio too poor: {r:?}");
        }
    }

    #[test]
    fn squarify_edge_cases() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
        let rs = squarify(&[0.0, 5.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(rs[0].area(), 0.0);
        assert!(close(rs[1].area(), 100.0));
        let rs = squarify(&[1.0, 1.0], Rect::new(0.0, 0.0, 0.0, 10.0));
        assert!(rs.iter().all(|r| r.area() == 0.0));
        // Input order is preserved even when weights are unsorted.
        let rs = squarify(&[1.0, 3.0], Rect::new(0.0, 0.0, 4.0, 1.0));
        assert!(close(rs[0].area(), 1.0) && close(rs[1].area(), 3.0));
    }

    fn sample() -> ScanTree {
        // root 100: a(dir 60: a1 40, a2 20), b 30, c 10
        let mut t = ScanTree::synthetic(100);
        let a = t.add_synthetic(ROOT, "a", Kind::Dir, 60);
        t.add_synthetic(a, "a1", Kind::File, 40);
        t.add_synthetic(a, "a2", Kind::File, 20);
        t.add_synthetic(ROOT, "b", Kind::File, 30);
        t.add_synthetic(ROOT, "c", Kind::File, 10);
        t
    }

    #[test]
    fn rings_spans_proportional() {
        let t = sample();
        let segs = rings(&t, ROOT, 5, 0.0);
        assert_eq!(segs.len(), 5);
        let a = segs.iter().find(|s| t.node(s.node).name == "a").unwrap();
        assert_eq!(a.depth, 1);
        assert!(close(a.start, 0.0) && close(a.end, TAU * 0.6));
        let b = segs.iter().find(|s| t.node(s.node).name == "b").unwrap();
        assert!(close(b.start, TAU * 0.6) && close(b.end, TAU * 0.9));
        let a2 = segs.iter().find(|s| t.node(s.node).name == "a2").unwrap();
        assert_eq!(a2.depth, 2);
        assert!(close(a2.start, TAU * 0.4) && close(a2.end, TAU * 0.6));
        assert_eq!(a2.branch, a.branch);
        // Depth limit and min-angle filtering.
        assert_eq!(rings(&t, ROOT, 1, 0.0).len(), 3);
        assert_eq!(rings(&t, ROOT, 5, TAU * 0.15).len(), 4); // drops c (0.1)
    }

    #[test]
    fn ring_hit_test() {
        let t = sample();
        let segs = rings(&t, ROOT, 3, 0.0);
        let g = RingGeometry::new(400.0, 400.0, 3);
        assert_eq!(ring_hit(&segs, &g, 200.0, 200.0), Some(RingHit::Center));
        // Straight right (3 o'clock = 90°) in ring 1 → "a" (0..216°).
        let r1 = g.inner_radius(1) + g.ring_width / 2.0;
        let hit = ring_hit(&segs, &g, 200.0 + r1, 200.0).unwrap();
        let RingHit::Segment(i) = hit else { panic!() };
        assert_eq!(t.node(segs[i].node).name, "a");
        // Straight left (270°) in ring 1 → "b" (216°..324°).
        let RingHit::Segment(i) = ring_hit(&segs, &g, 200.0 - r1, 200.0).unwrap() else {
            panic!()
        };
        assert_eq!(t.node(segs[i].node).name, "b");
        // Ring 2 at 3 o'clock → a1 (0..144°).
        let r2 = g.inner_radius(2) + g.ring_width / 2.0;
        let RingHit::Segment(i) = ring_hit(&segs, &g, 200.0 + r2, 200.0).unwrap() else {
            panic!()
        };
        assert_eq!(t.node(segs[i].node).name, "a1");
        // Ring 2 at 9 o'clock: b has no children → nothing.
        assert_eq!(ring_hit(&segs, &g, 200.0 - r2, 200.0), None);
    }

    #[test]
    fn treemap_nesting_and_hit() {
        let t = sample();
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let tiles = treemap(&t, ROOT, area, 4, 1.0);
        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles[0].node, ROOT);
        let a = tiles.iter().find(|x| t.node(x.node).name == "a").unwrap();
        let a1 = tiles.iter().find(|x| t.node(x.node).name == "a1").unwrap();
        // Children lie within their parent.
        assert!(a1.rect.x >= a.rect.x && a1.rect.x + a1.rect.w <= a.rect.x + a.rect.w + 1e-6);
        assert!(a1.rect.y >= a.rect.y && a1.rect.y + a1.rect.h <= a.rect.y + a.rect.h + 1e-6);
        assert_eq!(a1.depth, 2);
        // Top-level areas proportional to sizes (within the padded inner rect).
        let b = tiles.iter().find(|x| t.node(x.node).name == "b").unwrap();
        assert!(close(a.rect.area() / b.rect.area(), 2.0));
        // Deepest tile wins hit test.
        let (cx, cy) = (a1.rect.x + a1.rect.w / 2.0, a1.rect.y + a1.rect.h / 2.0);
        assert_eq!(treemap_hit(&tiles, cx, cy).unwrap().node, a1.node);
        // Depth limit.
        assert_eq!(treemap(&t, ROOT, area, 1, 1.0).len(), 4);
        // Tiny area: nothing below the root.
        assert_eq!(
            treemap(&t, ROOT, Rect::new(0.0, 0.0, 6.0, 6.0), 4, 4.0).len(),
            1
        );
    }
}
