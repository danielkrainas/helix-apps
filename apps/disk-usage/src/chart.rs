//! Chart drawing (cairo) on top of the pure layout math in `layout`.

use gtk::cairo;

use crate::layout::{self, Rect, RingGeometry, RingHit, Segment, Tile};
use crate::scanner::{Kind, NodeId, ScanTree};

pub const MAX_RINGS: usize = 5;
pub const MIN_RING_ANGLE: f64 = std::f64::consts::PI / 180.0;
pub const TREEMAP_DEPTH: usize = 4;
pub const TREEMAP_MIN_PX: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Rings,
    Treemap,
}

impl ChartKind {
    pub fn nick(self) -> &'static str {
        match self {
            ChartKind::Rings => "rings",
            ChartKind::Treemap => "treemap",
        }
    }
    pub fn from_nick(s: &str) -> Self {
        if s == "treemap" {
            ChartKind::Treemap
        } else {
            ChartKind::Rings
        }
    }
}

pub enum Layout {
    Empty,
    Rings {
        segs: Vec<Segment>,
        geom: RingGeometry,
    },
    Treemap {
        tiles: Vec<Tile>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The chart's root (centre disc of the ring chart).
    Root,
    Node(NodeId),
}

pub fn compute(tree: &ScanTree, root: NodeId, kind: ChartKind, w: f64, h: f64) -> Layout {
    match kind {
        ChartKind::Rings => {
            let segs = layout::rings(tree, root, MAX_RINGS, MIN_RING_ANGLE);
            // Size the rings to the depth actually present (at least 2).
            let depth = segs.iter().map(|s| s.depth).max().unwrap_or(1).max(2);
            Layout::Rings {
                segs,
                geom: RingGeometry::new(w, h, depth),
            }
        }
        ChartKind::Treemap => Layout::Treemap {
            tiles: layout::treemap(
                tree,
                root,
                Rect::new(0.0, 0.0, w, h),
                TREEMAP_DEPTH,
                TREEMAP_MIN_PX,
            ),
        },
    }
}

pub fn hit(l: &Layout, x: f64, y: f64) -> Option<Hit> {
    match l {
        Layout::Empty => None,
        Layout::Rings { segs, geom } => match layout::ring_hit(segs, geom, x, y)? {
            RingHit::Center => Some(Hit::Root),
            RingHit::Segment(i) => Some(Hit::Node(segs[i].node)),
        },
        Layout::Treemap { tiles } => {
            let t = layout::treemap_hit(tiles, x, y)?;
            Some(if t.depth == 0 {
                Hit::Root
            } else {
                Hit::Node(t.node)
            })
        }
    }
}

type Rgb = (f64, f64, f64);

const PALETTE: [Rgb; 8] = [
    (0.208, 0.518, 0.894), // blue
    (0.200, 0.820, 0.478), // green
    (0.965, 0.827, 0.176), // yellow
    (1.000, 0.471, 0.000), // orange
    (0.878, 0.106, 0.141), // red
    (0.569, 0.255, 0.675), // purple
    (0.596, 0.416, 0.267), // brown
    (0.129, 0.565, 0.643), // teal
];

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

pub fn color(branch: usize, depth: usize, kind: Kind, hovered: bool) -> Rgb {
    let base = PALETTE[branch % PALETTE.len()];
    let mut c = mix(
        base,
        (1.0, 1.0, 1.0),
        (0.13 * depth.saturating_sub(1) as f64).min(0.6),
    );
    if kind != Kind::Dir {
        c = mix(c, (0.6, 0.6, 0.6), 0.45);
    }
    if hovered {
        c = mix(c, (0.0, 0.0, 0.0), 0.2);
    }
    c
}

fn set(cr: &cairo::Context, c: Rgb) {
    cr.set_source_rgb(c.0, c.1, c.2);
}

/// Truncates `text` with an ellipsis so it fits in `max_w`.
fn fit_text(cr: &cairo::Context, text: &str, max_w: f64) -> Option<String> {
    let width = |s: &str| {
        cr.text_extents(s)
            .map(|e| e.x_advance())
            .unwrap_or(f64::MAX)
    };
    if width(text) <= max_w {
        return Some(text.to_string());
    }
    let chars: Vec<char> = text.chars().collect();
    for n in (1..chars.len()).rev() {
        let s: String = chars[..n].iter().collect::<String>() + "…";
        if width(&s) <= max_w {
            return Some(s);
        }
    }
    None
}

fn text_color(bg: Rgb) -> Rgb {
    let lum = 0.299 * bg.0 + 0.587 * bg.1 + 0.114 * bg.2;
    if lum > 0.55 {
        (0.1, 0.1, 0.1)
    } else {
        (1.0, 1.0, 1.0)
    }
}

pub fn draw(
    cr: &cairo::Context,
    tree: &ScanTree,
    root: NodeId,
    l: &Layout,
    hover: Option<NodeId>,
    fg: Rgb,
    size_text: &dyn Fn(u64) -> String,
) {
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.0);
    match l {
        Layout::Empty => {}
        Layout::Rings { segs, geom } => {
            let g = geom;
            // Centre disc.
            cr.arc(g.cx, g.cy, g.center_radius, 0.0, std::f64::consts::TAU);
            let center = if hover == Some(root) {
                (0.55, 0.55, 0.6)
            } else {
                (0.68, 0.68, 0.72)
            };
            set(cr, center);
            let _ = cr.fill();
            for s in segs {
                let node = tree.node(s.node);
                let r_in = g.inner_radius(s.depth);
                let r_out = r_in + g.ring_width;
                let (a0, a1) = (
                    RingGeometry::to_cairo(s.start),
                    RingGeometry::to_cairo(s.end),
                );
                cr.new_path();
                cr.arc(g.cx, g.cy, r_out, a0, a1);
                cr.arc_negative(g.cx, g.cy, r_in, a1, a0);
                cr.close_path();
                set(
                    cr,
                    color(s.branch, s.depth, node.kind, hover == Some(s.node)),
                );
                let _ = cr.fill_preserve();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.7);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            }
            // Centre label: name and size.
            let n = tree.node(root);
            set(cr, text_color(center));
            let max_w = g.center_radius * 1.7;
            for (i, line) in [n.name.clone(), size_text(n.size)].iter().enumerate() {
                if let Some(t) = fit_text(cr, line, max_w) {
                    if let Ok(e) = cr.text_extents(&t) {
                        cr.move_to(g.cx - e.x_advance() / 2.0, g.cy - 2.0 + i as f64 * 14.0);
                        let _ = cr.show_text(&t);
                    }
                }
            }
        }
        Layout::Treemap { tiles } => {
            for t in tiles {
                let node = tree.node(t.node);
                let r = t.rect;
                let bg = if t.depth == 0 {
                    mix(fg, (0.5, 0.5, 0.5), 0.8)
                } else {
                    color(t.branch, t.depth, node.kind, hover == Some(t.node))
                };
                cr.rectangle(
                    r.x + 0.5,
                    r.y + 0.5,
                    (r.w - 1.0).max(0.0),
                    (r.h - 1.0).max(0.0),
                );
                set(cr, bg);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
                if t.depth == 0 || r.w < 30.0 || r.h < 14.0 {
                    continue;
                }
                let has_header = node.kind == Kind::Dir && r.h >= layout::TREEMAP_HEADER * 2.5;
                let label = format!("{} ({})", node.name, size_text(node.size));
                if let Some(text) = fit_text(cr, &label, r.w - 6.0) {
                    set(cr, text_color(bg));
                    let y = if has_header || node.kind == Kind::Dir {
                        r.y + 12.0
                    } else {
                        r.y + r.h / 2.0 + 4.0
                    };
                    cr.move_to(r.x + 3.0, y);
                    let _ = cr.show_text(&text);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::ROOT;

    #[test]
    fn chart_kind_nicks() {
        assert_eq!(ChartKind::from_nick("treemap"), ChartKind::Treemap);
        assert_eq!(ChartKind::from_nick("rings"), ChartKind::Rings);
        assert_eq!(ChartKind::from_nick("bogus"), ChartKind::Rings);
        assert_eq!(ChartKind::Treemap.nick(), "treemap");
    }

    #[test]
    fn hit_dispatch() {
        let mut t = ScanTree::synthetic(10);
        let a = t.add_synthetic(ROOT, "a", Kind::File, 10);
        let l = compute(&t, ROOT, ChartKind::Rings, 200.0, 200.0);
        assert_eq!(hit(&l, 100.0, 100.0), Some(Hit::Root));
        let Layout::Rings { geom, .. } = &l else {
            panic!()
        };
        let x = 100.0 + geom.inner_radius(1) + 1.0;
        assert_eq!(hit(&l, x, 100.0), Some(Hit::Node(a)));
        let l = compute(&t, ROOT, ChartKind::Treemap, 200.0, 200.0);
        assert_eq!(hit(&l, 100.0, 100.0), Some(Hit::Node(a)));
        assert_eq!(hit(&l, 0.5, 0.5), Some(Hit::Root));
        assert_eq!(hit(&Layout::Empty, 1.0, 1.0), None);
    }

    /// Renders both charts offscreen; checks that something was drawn.
    /// Set HELIX_DU_PPM_DIR to also write images for visual inspection.
    #[test]
    fn render_offscreen() {
        let mut t = ScanTree::synthetic(1000);
        for (i, sz) in [400u64, 250, 150, 100, 60, 40].into_iter().enumerate() {
            let d = t.add_synthetic(ROOT, &format!("folder{i}"), Kind::Dir, sz);
            t.add_synthetic(d, "big.bin", Kind::File, sz * 6 / 10);
            t.add_synthetic(d, "small.bin", Kind::File, sz * 3 / 10);
        }
        for kind in [ChartKind::Rings, ChartKind::Treemap] {
            let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 300).unwrap();
            {
                let cr = cairo::Context::new(&surf).unwrap();
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.paint().unwrap();
                let l = compute(&t, ROOT, kind, 400.0, 300.0);
                draw(&cr, &t, ROOT, &l, Some(1), (0.0, 0.0, 0.0), &|b| {
                    format!("{b} B")
                });
            }
            surf.flush();
            let stride = surf.stride() as usize;
            let data = surf.take_data().unwrap();
            if let Ok(dir) = std::env::var("HELIX_DU_PPM_DIR") {
                // Binary PPM (convert with any image tool).
                let mut ppm = b"P6\n400 300\n255\n".to_vec();
                for y in 0..300 {
                    for x in 0..400 {
                        let i = y * stride + x * 4;
                        ppm.extend_from_slice(&[data[i + 2], data[i + 1], data[i]]);
                    }
                }
                std::fs::write(format!("{dir}/{}.ppm", kind.nick()), ppm).unwrap();
            }
            let non_white = (0..300)
                .flat_map(|y| (0..400).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let i = y * stride + x * 4;
                    data[i] != 255 || data[i + 1] != 255 || data[i + 2] != 255
                })
                .count();
            assert!(
                non_white > 400 * 300 / 4,
                "{kind:?} drew too little: {non_white}"
            );
        }
    }

    #[test]
    fn colors_lighten_with_depth() {
        let c1 = color(0, 1, Kind::Dir, false);
        let c3 = color(0, 3, Kind::Dir, false);
        assert!(c3.0 > c1.0 && c3.1 > c1.1);
        let h = color(0, 1, Kind::Dir, true);
        assert!(h.2 < c1.2);
    }
}
