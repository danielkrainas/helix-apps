//! Chart drawing (cairo) on top of the pure layout math in `layout`.

use gtk::{cairo, pango};

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

/// Lays out the chart of `root` in a `w`×`h` area; `header` is the height
/// of a treemap folder label (one line of the widget's font).
pub fn compute(
    tree: &ScanTree,
    root: NodeId,
    kind: ChartKind,
    w: f64,
    h: f64,
    header: f64,
) -> Layout {
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
                header,
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

/// The items shown in the chart, below its root, in drawing order with
/// their depth (ring or nesting level, 1 = the root's children). Items of
/// one depth come in clockwise (rings) or layout (treemap) order.
pub fn visible(l: &Layout) -> Vec<(NodeId, usize)> {
    match l {
        Layout::Empty => Vec::new(),
        Layout::Rings { segs, .. } => segs.iter().map(|s| (s.node, s.depth)).collect(),
        Layout::Treemap { tiles } => tiles
            .iter()
            .filter(|t| t.depth > 0)
            .map(|t| (t.node, t.depth))
            .collect(),
    }
}

/// Where keyboard focus goes from `current` (an item of the chart rooted at
/// `root`, or `root` itself) for a navigation key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// Previous item on the same ring/level.
    Prev,
    /// Next item on the same ring/level.
    Next,
    First,
    Last,
    /// Towards the centre: the parent item, or the root.
    Out,
    /// Away from the centre: the first shown child.
    In,
}

pub fn navigate(
    tree: &ScanTree,
    l: &Layout,
    root: NodeId,
    current: NodeId,
    m: Move,
) -> Option<NodeId> {
    let items = visible(l);
    if current == root {
        // The centre: only "In" leads anywhere.
        return match m {
            Move::In | Move::Next | Move::First => {
                items.iter().find(|(_, d)| *d == 1).map(|(n, _)| *n)
            }
            _ => None,
        };
    }
    let depth = items.iter().find(|(n, _)| *n == current)?.1;
    let level: Vec<NodeId> = items
        .iter()
        .filter(|(_, d)| *d == depth)
        .map(|(n, _)| *n)
        .collect();
    let i = level.iter().position(|&n| n == current)?;
    match m {
        Move::Prev => i.checked_sub(1).map(|j| level[j]),
        Move::Next => level.get(i + 1).copied(),
        Move::First => level.first().copied().filter(|&n| n != current),
        Move::Last => level.last().copied().filter(|&n| n != current),
        Move::Out => tree.node(current).parent,
        Move::In => items
            .iter()
            .find(|(n, _)| tree.node(*n).parent == Some(current))
            .map(|(n, _)| *n),
    }
}

/// A box around `node` (an item of the chart, or the root), for pointing a
/// popover at it.
pub fn anchor(l: &Layout, root: NodeId, node: NodeId) -> Option<Rect> {
    match l {
        Layout::Empty => None,
        Layout::Rings { segs, geom } => {
            if node == root {
                let r = geom.center_radius;
                return Some(Rect::new(geom.cx - r, geom.cy - r, 2.0 * r, 2.0 * r));
            }
            let s = segs.iter().find(|s| s.node == node)?;
            let radius = geom.inner_radius(s.depth) + geom.ring_width / 2.0;
            let angle = RingGeometry::to_cairo((s.start + s.end) / 2.0);
            let (x, y) = (
                geom.cx + radius * angle.cos(),
                geom.cy + radius * angle.sin(),
            );
            Some(Rect::new(x - 1.0, y - 1.0, 2.0, 2.0))
        }
        Layout::Treemap { tiles } => tiles.iter().find(|t| t.node == node).map(|t| t.rect),
    }
}

/// Outlines the keyboard-focused item: a thick line in the text colour
/// with a thin line of the opposite colour inside, visible on any fill.
pub fn draw_focus(cr: &cairo::Context, l: &Layout, root: NodeId, node: NodeId, theme: Theme) {
    match l {
        Layout::Empty => return,
        Layout::Rings { segs, geom } => {
            let g = geom;
            cr.new_path();
            if node == root {
                cr.arc(
                    g.cx,
                    g.cy,
                    g.center_radius - 2.0,
                    0.0,
                    std::f64::consts::TAU,
                );
            } else {
                let Some(s) = segs.iter().find(|s| s.node == node) else {
                    return;
                };
                let r_in = g.inner_radius(s.depth) + 2.0;
                let r_out = g.inner_radius(s.depth) + g.ring_width - 2.0;
                let (a0, a1) = (
                    RingGeometry::to_cairo(s.start),
                    RingGeometry::to_cairo(s.end),
                );
                cr.arc(g.cx, g.cy, r_out, a0, a1);
                cr.arc_negative(g.cx, g.cy, r_in.min(r_out), a1, a0);
                cr.close_path();
            }
        }
        Layout::Treemap { tiles } => {
            let Some(t) = tiles.iter().find(|t| t.node == node) else {
                return;
            };
            let r = t.rect;
            cr.rectangle(
                r.x + 2.0,
                r.y + 2.0,
                (r.w - 4.0).max(0.0),
                (r.h - 4.0).max(0.0),
            );
        }
    }
    set(cr, theme.fg);
    cr.set_line_width(3.0);
    let _ = cr.stroke_preserve();
    set(cr, text_color(theme.fg));
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

pub type Rgb = (f64, f64, f64);

/// The desktop style the chart is drawn for, read at draw time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub dark: bool,
    pub high_contrast: bool,
}

/// Colours taken from the widget's style when drawing.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// The widget's foreground (text) colour.
    pub fg: Rgb,
    pub style: Style,
}

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

/// Fully saturated, clearly separated hues for high contrast; depth fades
/// them only a little so neighbouring rings stay distinct.
const PALETTE_HC: [Rgb; 8] = [
    (0.000, 0.329, 0.800), // blue
    (0.000, 0.600, 0.200), // green
    (1.000, 0.850, 0.000), // yellow
    (1.000, 0.400, 0.000), // orange
    (0.800, 0.000, 0.000), // red
    (0.502, 0.000, 0.702), // purple
    (0.400, 0.200, 0.000), // brown
    (0.000, 0.502, 0.502), // teal
];

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

const WHITE: Rgb = (1.0, 1.0, 1.0);
const BLACK: Rgb = (0.0, 0.0, 0.0);

/// Segment fill for the `branch`-th top-level item at `depth`.
pub fn color(branch: usize, depth: usize, kind: Kind, hovered: bool, style: Style) -> Rgb {
    let d = depth.saturating_sub(1) as f64;
    let mut c = if style.high_contrast {
        let base = PALETTE_HC[branch % PALETTE_HC.len()];
        let toward = if style.dark { BLACK } else { WHITE };
        mix(base, toward, (0.08 * d).min(0.3))
    } else if style.dark {
        // Deeper rings sink towards the dark background instead of
        // glowing towards white.
        let base = mix(PALETTE[branch % PALETTE.len()], BLACK, 0.15);
        mix(base, (0.2, 0.2, 0.2), (0.12 * d).min(0.5))
    } else {
        mix(PALETTE[branch % PALETTE.len()], WHITE, (0.13 * d).min(0.6))
    };
    if kind != Kind::Dir {
        let grey = if style.dark {
            (0.45, 0.45, 0.45)
        } else {
            (0.6, 0.6, 0.6)
        };
        c = mix(c, grey, if style.high_contrast { 0.3 } else { 0.45 });
    }
    if hovered {
        c = mix(c, if style.dark { WHITE } else { BLACK }, 0.2);
    }
    c
}

fn set(cr: &cairo::Context, c: Rgb) {
    cr.set_source_rgb(c.0, c.1, c.2);
}

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: Rgb) -> f64 {
    let lin = |v: f64| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

/// WCAG contrast ratio between two colours (1 to 21).
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Black or white, whichever contrasts more with `bg`. One of the two is
/// always at least 4.58:1 against any colour.
pub fn text_color(bg: Rgb) -> Rgb {
    if contrast(bg, BLACK) >= contrast(bg, WHITE) {
        BLACK
    } else {
        WHITE
    }
}

/// A label laid out with the widget's font (family, size and text
/// scaling), shaped and with font fallback by Pango, ellipsized to
/// `max_w`. `None` when not even a few characters fit.
fn label(pango: &pango::Context, text: &str, max_w: f64, bold: bool) -> Option<pango::Layout> {
    let layout = pango::Layout::new(pango);
    layout.set_text(text);
    if bold {
        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        layout.set_attributes(Some(&attrs));
    }
    let (w, h) = layout.pixel_size();
    if w as f64 <= max_w {
        return Some(layout);
    }
    // Less room than about two characters plus the ellipsis: no label.
    if max_w < 2.0 * h as f64 {
        return None;
    }
    layout.set_width((max_w * pango::SCALE as f64) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    Some(layout)
}

/// Height of one line of chart text, for sizing treemap folder headers.
pub fn line_height(pango: &pango::Context) -> f64 {
    let layout = pango::Layout::new(pango);
    layout.set_text("Ag");
    layout.pixel_size().1 as f64
}

fn show(cr: &cairo::Context, layout: &pango::Layout, x: f64, y: f64) {
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, layout);
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    cr: &cairo::Context,
    pango: &pango::Context,
    tree: &ScanTree,
    root: NodeId,
    l: &Layout,
    hover: Option<NodeId>,
    theme: Theme,
    size_text: &dyn Fn(u64) -> String,
) {
    let style = theme.style;
    // Borders separate neighbouring segments; in high contrast they are
    // drawn in the text colour and thicker.
    let (border, border_w) = if style.high_contrast {
        (theme.fg, 1.5)
    } else if style.dark {
        ((0.14, 0.14, 0.14), 1.0)
    } else {
        (WHITE, 1.0)
    };
    match l {
        Layout::Empty => {}
        Layout::Rings { segs, geom } => {
            let g = geom;
            // Centre disc.
            cr.arc(g.cx, g.cy, g.center_radius, 0.0, std::f64::consts::TAU);
            let center = match (style.dark, hover == Some(root)) {
                (false, false) => (0.68, 0.68, 0.72),
                (false, true) => (0.55, 0.55, 0.6),
                (true, false) => (0.32, 0.32, 0.36),
                (true, true) => (0.42, 0.42, 0.46),
            };
            set(cr, center);
            let _ = cr.fill_preserve();
            set(cr, border);
            cr.set_line_width(border_w);
            let _ = cr.stroke();
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
                    color(s.branch, s.depth, node.kind, hover == Some(s.node), style),
                );
                let _ = cr.fill_preserve();
                set(cr, border);
                cr.set_line_width(border_w);
                let _ = cr.stroke();
            }
            // Centre label: name (bold) over size.
            let n = tree.node(root);
            set(cr, text_color(center));
            let max_w = g.center_radius * 1.7;
            let lines: Vec<pango::Layout> = [(n.name.clone(), true), (size_text(n.size), false)]
                .iter()
                .filter_map(|(t, bold)| label(pango, t, max_w, *bold))
                .collect();
            let total: f64 = lines.iter().map(|l| l.pixel_size().1 as f64).sum();
            let mut y = g.cy - total / 2.0;
            for line in &lines {
                let (w, h) = line.pixel_size();
                show(cr, line, g.cx - w as f64 / 2.0, y);
                y += h as f64;
            }
        }
        Layout::Treemap { tiles } => {
            let line_h = line_height(pango);
            for t in tiles {
                let node = tree.node(t.node);
                let r = t.rect;
                let bg = if t.depth == 0 {
                    mix(theme.fg, (0.5, 0.5, 0.5), 0.8)
                } else {
                    color(t.branch, t.depth, node.kind, hover == Some(t.node), style)
                };
                cr.rectangle(
                    r.x + 0.5,
                    r.y + 0.5,
                    (r.w - 1.0).max(0.0),
                    (r.h - 1.0).max(0.0),
                );
                set(cr, bg);
                let _ = cr.fill_preserve();
                if style.high_contrast {
                    set(cr, theme.fg);
                } else {
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
                }
                cr.set_line_width(1.0);
                let _ = cr.stroke();
                if t.depth == 0 || r.w < 30.0 || r.h < line_h + 2.0 {
                    continue;
                }
                let text = format!("{} ({})", node.name, size_text(node.size));
                if let Some(layout) = label(pango, &text, r.w - 6.0, false) {
                    set(cr, text_color(bg));
                    let y = if node.kind == Kind::Dir {
                        r.y + 1.0
                    } else {
                        r.y + (r.h - layout.pixel_size().1 as f64) / 2.0
                    };
                    show(cr, &layout, r.x + 3.0, y);
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
        let l = compute(&t, ROOT, ChartKind::Rings, 200.0, 200.0, 14.0);
        assert_eq!(hit(&l, 100.0, 100.0), Some(Hit::Root));
        let Layout::Rings { geom, .. } = &l else {
            panic!()
        };
        let x = 100.0 + geom.inner_radius(1) + 1.0;
        assert_eq!(hit(&l, x, 100.0), Some(Hit::Node(a)));
        let l = compute(&t, ROOT, ChartKind::Treemap, 200.0, 200.0, 14.0);
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
            // A non-Latin name exercises Pango's shaping and fallback.
            let name = if i == 1 {
                "日本語の写真".to_string()
            } else {
                format!("folder{i}")
            };
            let d = t.add_synthetic(ROOT, &name, Kind::Dir, sz);
            t.add_synthetic(d, "big.bin", Kind::File, sz * 6 / 10);
            t.add_synthetic(d, "small.bin", Kind::File, sz * 3 / 10);
        }
        for kind in [ChartKind::Rings, ChartKind::Treemap] {
            let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 300).unwrap();
            {
                let cr = cairo::Context::new(&surf).unwrap();
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.paint().unwrap();
                let pango = pangocairo::functions::create_context(&cr);
                let l = compute(&t, ROOT, kind, 400.0, 300.0, line_height(&pango));
                let theme = Theme {
                    fg: BLACK,
                    style: Style::default(),
                };
                draw(&cr, &pango, &t, ROOT, &l, Some(1), theme, &|b| {
                    format!("{b} B")
                });
                draw_focus(&cr, &l, ROOT, 2, theme);
                assert_eq!(cr.status(), Ok(()));
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
    fn keyboard_navigation() {
        let mut t = ScanTree::synthetic(100);
        let a = t.add_synthetic(ROOT, "a", Kind::Dir, 60);
        let a1 = t.add_synthetic(a, "a1", Kind::File, 40);
        let a2 = t.add_synthetic(a, "a2", Kind::File, 20);
        let b = t.add_synthetic(ROOT, "b", Kind::Dir, 30);
        let b1 = t.add_synthetic(b, "b1", Kind::File, 30);
        for kind in [ChartKind::Rings, ChartKind::Treemap] {
            let l = compute(&t, ROOT, kind, 400.0, 400.0, 14.0);
            let nav = |cur, m| navigate(&t, &l, ROOT, cur, m);
            assert_eq!(nav(ROOT, Move::In), Some(a), "{kind:?}");
            assert_eq!(nav(ROOT, Move::Out), None);
            assert_eq!(nav(a, Move::Next), Some(b));
            assert_eq!(nav(b, Move::Next), None);
            assert_eq!(nav(b, Move::Prev), Some(a));
            assert_eq!(nav(a, Move::In), Some(a1));
            assert_eq!(nav(a1, Move::Next), Some(a2));
            // The same ring continues into the next folder's children.
            assert_eq!(nav(a2, Move::Next), Some(b1));
            assert_eq!(nav(b1, Move::First), Some(a1));
            assert_eq!(nav(a1, Move::Last), Some(b1));
            assert_eq!(nav(b1, Move::Out), Some(b));
            assert_eq!(nav(a, Move::Out), Some(ROOT));
            assert_eq!(nav(a1, Move::In), None);
            assert!(anchor(&l, ROOT, a1).is_some());
            assert!(anchor(&l, ROOT, ROOT).is_some() || kind == ChartKind::Treemap);
        }
    }

    #[test]
    fn colors_lighten_with_depth() {
        let light = Style::default();
        let c1 = color(0, 1, Kind::Dir, false, light);
        let c3 = color(0, 3, Kind::Dir, false, light);
        assert!(c3.0 > c1.0 && c3.1 > c1.1);
        let h = color(0, 1, Kind::Dir, true, light);
        assert!(h.2 < c1.2);
    }

    #[test]
    fn styles_differ() {
        let c = |dark, high_contrast| {
            color(
                2,
                2,
                Kind::Dir,
                false,
                Style {
                    dark,
                    high_contrast,
                },
            )
        };
        assert_ne!(c(false, false), c(true, false));
        assert_ne!(c(false, false), c(false, true));
        assert_ne!(c(true, false), c(true, true));
    }

    /// Every segment label meets WCAG AA (4.5:1) against its segment in the
    /// light, dark and high-contrast styles, hovered or not.
    #[test]
    fn labels_meet_contrast() {
        for dark in [false, true] {
            for high_contrast in [false, true] {
                let style = Style {
                    dark,
                    high_contrast,
                };
                for branch in 0..8 {
                    for depth in 1..=MAX_RINGS {
                        for kind in [Kind::Dir, Kind::File] {
                            for hovered in [false, true] {
                                let bg = color(branch, depth, kind, hovered, style);
                                let ratio = contrast(bg, text_color(bg));
                                assert!(ratio >= 4.5, "{style:?} {branch} {depth}: {ratio}");
                            }
                        }
                    }
                }
            }
        }
        // The worst case for black-or-white text.
        let mid = (0.46, 0.46, 0.46);
        assert!(contrast(mid, text_color(mid)) >= 4.5);
        assert!((contrast(BLACK, WHITE) - 21.0).abs() < 1e-9);
    }
}
