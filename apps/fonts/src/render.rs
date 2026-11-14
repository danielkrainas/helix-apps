//! Drawing text straight from a font file: shaping with rustybuzz and
//! outlines from ttf-parser rendered as cairo paths. This shows exactly the
//! selected face of the file, whether or not fontconfig knows about it.

use gtk::cairo;
use rustybuzz::ttf_parser::{self, GlyphId, OutlineBuilder};
use std::sync::Arc;

/// A loaded font file plus the face index to use.
#[derive(Clone)]
pub struct FontData {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
}

impl FontData {
    /// Face with shaping tables, for running text.
    pub fn face(&self) -> Option<rustybuzz::Face<'_>> {
        rustybuzz::Face::from_slice(&self.data, self.index)
    }

    /// Plain parsed face, cheaper; for single glyphs.
    pub fn raw_face(&self) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.data, self.index).ok()
    }
}

/// A positioned glyph in font units.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub glyph: GlyphId,
    pub x: i32,
    pub y: i32,
}

/// Shapes `text`, returning glyphs and the total advance (font units).
pub fn shape(face: &rustybuzz::Face, text: &str) -> (Vec<Placed>, i32) {
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(text);
    buf.guess_segment_properties();
    let out = rustybuzz::shape(face, &[], buf);
    let mut pen = 0;
    let mut placed = Vec::with_capacity(out.len());
    for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
        placed.push(Placed {
            glyph: GlyphId(info.glyph_id as u16),
            x: pen + pos.x_offset,
            y: pos.y_offset,
        });
        pen += pos.x_advance;
    }
    (placed, pen)
}

/// Vertical metrics in font units: (ascender, descender (negative), units per em).
pub fn metrics(face: &ttf_parser::Face) -> (f64, f64, f64) {
    let upem = face.units_per_em().max(1) as f64;
    let mut asc = face.ascender() as f64;
    let mut desc = face.descender() as f64;
    if asc <= 0.0 && desc >= 0.0 {
        asc = upem * 0.8;
        desc = -upem * 0.2;
    }
    (asc, desc, upem)
}

struct CairoPath<'a> {
    cr: &'a cairo::Context,
    ox: f64,
    oy: f64,
    scale: f64,
    last: (f64, f64),
}

impl CairoPath<'_> {
    fn pt(&self, x: f32, y: f32) -> (f64, f64) {
        (
            self.ox + x as f64 * self.scale,
            self.oy - y as f64 * self.scale,
        )
    }
}

impl OutlineBuilder for CairoPath<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.cr.move_to(p.0, p.1);
        self.last = p;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.cr.line_to(p.0, p.1);
        self.last = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.pt(x1, y1), self.pt(x, y));
        let l = self.last;
        let c1 = (l.0 + 2.0 / 3.0 * (c.0 - l.0), l.1 + 2.0 / 3.0 * (c.1 - l.1));
        let c2 = (p.0 + 2.0 / 3.0 * (c.0 - p.0), p.1 + 2.0 / 3.0 * (c.1 - p.1));
        self.cr.curve_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1);
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, p) = (self.pt(x1, y1), self.pt(x2, y2), self.pt(x, y));
        self.cr.curve_to(a.0, a.1, b.0, b.1, p.0, p.1);
        self.last = p;
    }
    fn close(&mut self) {
        self.cr.close_path();
    }
}

/// Appends the outline of `glyph` to the current path with its origin at
/// (`x`, `baseline`) and `px` pixels per em. Returns false if the glyph
/// has no outline.
pub fn glyph_path(
    cr: &cairo::Context,
    face: &ttf_parser::Face,
    glyph: GlyphId,
    x: f64,
    baseline: f64,
    px: f64,
) -> bool {
    let (_, _, upem) = metrics(face);
    let mut b = CairoPath {
        cr,
        ox: x,
        oy: baseline,
        scale: px / upem,
        last: (x, baseline),
    };
    face.outline_glyph(glyph, &mut b).is_some()
}

/// Draws `text` at `px` pixels per em with the baseline at `baseline`.
/// Returns the width drawn in pixels.
pub fn draw_text(
    cr: &cairo::Context,
    face: &rustybuzz::Face,
    text: &str,
    x: f64,
    baseline: f64,
    px: f64,
) -> f64 {
    let (_, _, upem) = metrics(face);
    let scale = px / upem;
    let (glyphs, advance) = shape(face, text);
    cr.new_path();
    for g in &glyphs {
        glyph_path(
            cr,
            face,
            g.glyph,
            x + g.x as f64 * scale,
            baseline - g.y as f64 * scale,
            px,
        );
    }
    cr.set_fill_rule(cairo::FillRule::Winding);
    let _ = cr.fill();
    advance as f64 * scale
}

/// Width and height in pixels of `text` at `px` pixels per em.
pub fn measure(face: &rustybuzz::Face, text: &str, px: f64) -> (f64, f64) {
    let (asc, desc, upem) = metrics(face);
    let (_, advance) = shape(face, text);
    (advance as f64 * px / upem, (asc - desc) * px / upem)
}

/// Draws the glyph for `c` centred in a `w`×`h` cell. Returns false if the
/// face has no glyph for it.
pub fn draw_char_centered(
    cr: &cairo::Context,
    face: &ttf_parser::Face,
    c: char,
    w: f64,
    h: f64,
    px: f64,
) -> bool {
    let Some(gid) = face.glyph_index(c) else {
        return false;
    };
    let (asc, desc, upem) = metrics(face);
    let scale = px / upem;
    let adv = face.glyph_hor_advance(gid).unwrap_or(0) as f64 * scale;
    let x = (w - adv) / 2.0;
    let baseline = (h - (asc - desc) * scale) / 2.0 + asc * scale;
    cr.new_path();
    let ok = glyph_path(cr, face, gid, x, baseline, px);
    let _ = cr.fill();
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load() -> Option<FontData> {
        let p = crate::fontinfo::tests::any_ttf()?;
        Some(FontData {
            data: Arc::new(std::fs::read(p).ok()?),
            index: 0,
        })
    }

    fn inked(surface: &mut cairo::ImageSurface) -> usize {
        surface.flush();
        let data = surface.data().unwrap();
        data.chunks(4).filter(|px| px[3] != 0).count()
    }

    #[test]
    fn shapes_and_measures() {
        let Some(fd) = load() else { return };
        let face = fd.face().unwrap();
        let (glyphs, adv) = shape(&face, "Aa");
        assert_eq!(glyphs.len(), 2);
        assert!(glyphs.iter().all(|g| g.glyph.0 != 0));
        assert!(adv > 0);
        let (w1, h1) = measure(&face, "Hello", 10.0);
        let (w2, h2) = measure(&face, "Hello", 20.0);
        assert!((w2 - 2.0 * w1).abs() < 0.01 && (h2 - 2.0 * h1).abs() < 0.01);
        assert_eq!(measure(&face, "", 10.0).0, 0.0);
    }

    #[test]
    fn draws_ink() {
        let Some(fd) = load() else { return };
        let face = fd.face().unwrap();
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 200, 60).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            let w = draw_text(&cr, &face, "Aa Bb", 2.0, 45.0, 40.0);
            assert!(w > 40.0 && w < 198.0, "{w}");
        }
        assert!(inked(&mut surface) > 100);

        let mut cell = cairo::ImageSurface::create(cairo::Format::ARgb32, 40, 40).unwrap();
        {
            let cr = cairo::Context::new(&cell).unwrap();
            assert!(draw_char_centered(
                &cr,
                &fd.raw_face().unwrap(),
                'g',
                40.0,
                40.0,
                24.0
            ));
            assert!(!draw_char_centered(
                &cr,
                &fd.raw_face().unwrap(),
                '\u{10FFFD}',
                40.0,
                40.0,
                24.0
            ));
        }
        assert!(inked(&mut cell) > 20);
    }
}
