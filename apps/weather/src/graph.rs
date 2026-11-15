//! Cairo drawing for the hourly temperature graph and daily range bars.
//! The geometry helpers are pure and unit-tested.

use gtk::{cairo, pango};

/// Maps `values` linearly onto `[top, bottom]` (larger values are higher,
/// i.e. closer to `top`). A flat series sits in the middle.
pub fn scale_y(values: &[f64], top: f64, bottom: f64) -> Vec<f64> {
    let (min, max) = min_max(values);
    let span = max - min;
    values
        .iter()
        .map(|&v| {
            if span.abs() < f64::EPSILON {
                (top + bottom) / 2.0
            } else {
                bottom - (v - min) / span * (bottom - top)
            }
        })
        .collect()
}

pub fn min_max(values: &[f64]) -> (f64, f64) {
    values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
}

/// Horizontal extent `(x0, x1)` of a `[lo, hi]` range bar inside `width`,
/// relative to the overall `[min, max]`.
pub fn range_extent(lo: f64, hi: f64, min: f64, max: f64, width: f64) -> (f64, f64) {
    let span = max - min;
    if span.abs() < f64::EPSILON {
        return (0.0, width);
    }
    let x0 = ((lo - min) / span).clamp(0.0, 1.0) * width;
    let x1 = ((hi - min) / span).clamp(0.0, 1.0) * width;
    (x0.min(x1), x0.max(x1))
}

#[derive(Clone, Copy)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl From<gtk::gdk::RGBA> for Rgba {
    fn from(c: gtk::gdk::RGBA) -> Self {
        Rgba {
            r: c.red() as f64,
            g: c.green() as f64,
            b: c.blue() as f64,
            a: c.alpha() as f64,
        }
    }
}

/// The colours a graph is drawn in, read from the style when drawing so
/// they follow the dark, light and high-contrast styles.
#[derive(Clone, Copy)]
pub struct Colors {
    /// Text and tracks.
    pub fg: Rgba,
    /// The curve and the range bars.
    pub accent: Rgba,
}

fn set(cr: &cairo::Context, c: Rgba, alpha: f64) {
    cr.set_source_rgba(c.r, c.g, c.b, c.a * alpha);
}

/// Space between a point and the bottom of its label.
const LABEL_GAP: f64 = 6.0;

/// Draws the temperature curve: one point per column of `col_w` pixels,
/// with a filled area, line, dots and a value label above each point.
/// Labels are laid out with `pango` (the widget's context, so they use
/// the interface font and text scaling) in bold.
pub fn draw_temperature(
    cr: &cairo::Context,
    pango: &pango::Context,
    values: &[f64],
    labels: &[String],
    col_w: f64,
    height: f64,
    colors: Colors,
) {
    if values.is_empty() {
        return;
    }
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
    let layouts: Vec<pango::Layout> = labels
        .iter()
        .map(|l| {
            let layout = pango::Layout::new(pango);
            layout.set_text(l);
            layout.set_attributes(Some(&attrs));
            layout
        })
        .collect();
    let label_h = layouts
        .iter()
        .map(|l| l.pixel_size().1 as f64)
        .fold(0.0, f64::max);
    let ys = scale_y(values, label_h + LABEL_GAP + 4.0, height - 12.0);
    let accent = colors.accent;
    let xs: Vec<f64> = (0..values.len())
        .map(|i| col_w * (i as f64 + 0.5))
        .collect();

    // Filled area under the curve.
    cr.move_to(xs[0], height);
    for (x, y) in xs.iter().zip(&ys) {
        cr.line_to(*x, *y);
    }
    cr.line_to(xs[xs.len() - 1], height);
    cr.close_path();
    let grad = cairo::LinearGradient::new(0.0, 0.0, 0.0, height);
    grad.add_color_stop_rgba(0.0, accent.r, accent.g, accent.b, 0.35);
    grad.add_color_stop_rgba(1.0, accent.r, accent.g, accent.b, 0.02);
    let _ = cr.set_source(&grad);
    let _ = cr.fill();

    // Line.
    set(cr, accent, 1.0);
    cr.set_line_width(2.0);
    cr.set_line_join(cairo::LineJoin::Round);
    for (i, (x, y)) in xs.iter().zip(&ys).enumerate() {
        if i == 0 {
            cr.move_to(*x, *y);
        } else {
            cr.line_to(*x, *y);
        }
    }
    let _ = cr.stroke();

    // Dots and labels.
    for (i, (x, y)) in xs.iter().zip(&ys).enumerate() {
        set(cr, accent, 1.0);
        cr.arc(*x, *y, 3.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        if let Some(layout) = layouts.get(i) {
            set(cr, colors.fg, 1.0);
            let (w, h) = layout.pixel_size();
            cr.move_to(x - w as f64 / 2.0, y - LABEL_GAP - h as f64);
            pangocairo::functions::show_layout(cr, layout);
        }
    }
}

/// Draws a rounded min–max bar on a faint track.
pub fn draw_range_bar(
    cr: &cairo::Context,
    (lo, hi): (f64, f64),
    (min, max): (f64, f64),
    width: f64,
    height: f64,
    colors: Colors,
) {
    let h = 6.0;
    let y = (height - h) / 2.0;
    rounded(cr, 0.0, y, width, h);
    set(cr, colors.fg, 0.15);
    let _ = cr.fill();
    let (x0, x1) = range_extent(lo, hi, min, max, width);
    let w = (x1 - x0).max(h);
    let x0 = x0.min(width - w);
    rounded(cr, x0, y, w, h);
    set(cr, colors.accent, 1.0);
    let _ = cr.fill();
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let r = h / 2.0;
    cr.new_sub_path();
    cr.arc(
        x + w - r,
        y + r,
        r,
        -std::f64::consts::FRAC_PI_2,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::FRAC_PI_2,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling() {
        let ys = scale_y(&[0.0, 5.0, 10.0], 10.0, 110.0);
        assert_eq!(ys, vec![110.0, 60.0, 10.0]);
        let flat = scale_y(&[3.0, 3.0], 0.0, 100.0);
        assert_eq!(flat, vec![50.0, 50.0]);
        assert!(scale_y(&[], 0.0, 1.0).is_empty());
        assert_eq!(min_max(&[3.0, -1.0, 7.5]), (-1.0, 7.5));
    }

    #[test]
    fn range_bar_extent() {
        assert_eq!(range_extent(0.0, 10.0, 0.0, 20.0, 100.0), (0.0, 50.0));
        assert_eq!(range_extent(5.0, 20.0, 0.0, 20.0, 100.0), (25.0, 100.0));
        assert_eq!(range_extent(-5.0, 30.0, 0.0, 20.0, 100.0), (0.0, 100.0));
        assert_eq!(range_extent(1.0, 1.0, 1.0, 1.0, 80.0), (0.0, 80.0));
    }

    #[test]
    fn draws_to_image_surface() {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 200, 100).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        let black = Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let colors = Colors {
            fg: black,
            accent: black,
        };
        let pango = pangocairo::functions::create_context(&cr);
        draw_temperature(
            &cr,
            &pango,
            &[1.0, 4.0, 2.0, 3.0],
            &["1°".into(), "4°".into(), "2°".into(), "3°".into()],
            50.0,
            100.0,
            colors,
        );
        draw_temperature(&cr, &pango, &[], &[], 50.0, 100.0, colors);
        draw_range_bar(&cr, (2.0, 5.0), (0.0, 10.0), 100.0, 20.0, colors);
        assert_eq!(cr.status(), Ok(()));
    }
}
