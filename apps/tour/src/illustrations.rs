//! Original flat vector illustrations, drawn with cairo.
//!
//! Every drawing is laid out on a 360×220 canvas and scaled to fit the
//! widget. Two palettes cover light and dark styles.

use gtk::cairo::Context;
use std::f64::consts::PI;

pub const WIDTH: i32 = 360;
pub const HEIGHT: i32 = 220;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Art {
    Welcome,
    Overview,
    Launcher,
    Workspaces,
    Windows,
    Touchpad,
    Help,
    Done,
}

type Rgb = (f64, f64, f64);

const fn hex(v: u32) -> Rgb {
    (((v >> 16) & 0xff) as f64 / 255.0, ((v >> 8) & 0xff) as f64 / 255.0, (v & 0xff) as f64 / 255.0)
}

struct Palette {
    screen: Rgb,
    frame: Rgb,
    panel: Rgb,
    window: Rgb,
    header: Rgb,
    muted: Rgb,
    accent: Rgb,
    teal: Rgb,
    amber: Rgb,
    coral: Rgb,
    violet: Rgb,
    on_accent: Rgb,
}

const LIGHT: Palette = Palette {
    screen: hex(0xeef3f9),
    frame: hex(0xc3ccd9),
    panel: hex(0x283040),
    window: hex(0xffffff),
    header: hex(0xdde3ec),
    muted: hex(0x9ba6b6),
    accent: hex(0x2f80c8),
    teal: hex(0x1fa59c),
    amber: hex(0xf0ae2e),
    coral: hex(0xe5664f),
    violet: hex(0x8a63d2),
    on_accent: hex(0xffffff),
};

const DARK: Palette = Palette {
    screen: hex(0x2b313c),
    frame: hex(0x4c5566),
    panel: hex(0x15181e),
    window: hex(0x3b4250),
    header: hex(0x4b5363),
    muted: hex(0x7e8899),
    accent: hex(0x5aa8ea),
    teal: hex(0x3ccac0),
    amber: hex(0xf5c25a),
    coral: hex(0xf07f6a),
    violet: hex(0xa88af0),
    on_accent: hex(0x10141a),
};

fn set(cr: &Context, c: Rgb) {
    cr.set_source_rgb(c.0, c.1, c.2);
}

fn set_a(cr: &Context, c: Rgb, a: f64) {
    cr.set_source_rgba(c.0, c.1, c.2, a);
}

fn rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

fn fill_rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64, c: Rgb) {
    rounded(cr, x, y, w, h, r);
    set(cr, c);
    let _ = cr.fill();
}

#[allow(clippy::too_many_arguments)]
fn stroke_rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64, c: Rgb, lw: f64) {
    rounded(cr, x, y, w, h, r);
    set(cr, c);
    cr.set_line_width(lw);
    let _ = cr.stroke();
}

fn circle(cr: &Context, x: f64, y: f64, r: f64, c: Rgb) {
    cr.new_sub_path();
    cr.arc(x, y, r, 0.0, 2.0 * PI);
    set(cr, c);
    let _ = cr.fill();
}

fn line(cr: &Context, x1: f64, y1: f64, x2: f64, y2: f64, c: Rgb, lw: f64) {
    cr.move_to(x1, y1);
    cr.line_to(x2, y2);
    set(cr, c);
    cr.set_line_width(lw);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    let _ = cr.stroke();
}

/// A horizontal arrow from `x1` to `x2` at height `y`.
fn arrow(cr: &Context, x1: f64, x2: f64, y: f64, c: Rgb, lw: f64) {
    line(cr, x1, y, x2, y, c, lw);
    let d = if x2 > x1 { -1.0 } else { 1.0 };
    let head = lw * 2.4;
    cr.move_to(x2 + d * head, y - head);
    cr.line_to(x2, y);
    cr.line_to(x2 + d * head, y + head);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    let _ = cr.stroke();
}

/// Monitor frame with a top panel; returns nothing, draws at fixed spot.
fn desktop(cr: &Context, p: &Palette) {
    fill_rounded(cr, 20.0, 8.0, 320.0, 204.0, 12.0, p.screen);
    stroke_rounded(cr, 20.0, 8.0, 320.0, 204.0, 12.0, p.frame, 3.0);
    // Panel: clip to the frame's rounded top.
    let _ = cr.save();
    rounded(cr, 20.0, 8.0, 320.0, 204.0, 12.0);
    cr.clip();
    set(cr, p.panel);
    cr.rectangle(20.0, 8.0, 320.0, 16.0);
    let _ = cr.fill();
    let _ = cr.restore();
    // Clock.
    fill_rounded(cr, 166.0, 13.0, 28.0, 6.0, 3.0, p.muted);
}

fn window(cr: &Context, p: &Palette, x: f64, y: f64, w: f64, h: f64, head: Rgb) {
    fill_rounded(cr, x, y, w, h, 7.0, p.window);
    let _ = cr.save();
    rounded(cr, x, y, w, h, 7.0);
    cr.clip();
    set(cr, head);
    cr.rectangle(x, y, w, 14.0);
    let _ = cr.fill();
    let _ = cr.restore();
    stroke_rounded(cr, x, y, w, h, 7.0, p.frame, 1.5);
    // Content lines.
    let mut ly = y + 24.0;
    let mut i = 0;
    while ly + 4.0 < y + h - 6.0 && i < 5 {
        let lw = (w - 20.0) * [0.9, 0.7, 0.8, 0.5, 0.75][i];
        fill_rounded(cr, x + 10.0, ly, lw, 5.0, 2.5, p.header);
        ly += 12.0;
        i += 1;
    }
}

fn pointer(cr: &Context, p: &Palette, x: f64, y: f64) {
    cr.move_to(x, y);
    cr.line_to(x, y + 20.0);
    cr.line_to(x + 5.0, y + 15.0);
    cr.line_to(x + 9.0, y + 23.0);
    cr.line_to(x + 12.0, y + 21.5);
    cr.line_to(x + 8.0, y + 14.0);
    cr.line_to(x + 15.0, y + 14.0);
    cr.close_path();
    set(cr, p.window);
    let _ = cr.fill_preserve();
    set(cr, p.panel);
    cr.set_line_width(1.5);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    let _ = cr.stroke();
}

fn draw_welcome(cr: &Context, p: &Palette) {
    // Two intertwined strands with rungs: a stylised helix.
    let (x0, x1, cy, amp, period) = (50.0, 310.0, 110.0, 58.0, 130.0);
    let strand = |phase: f64, x: f64| cy + amp * ((x - x0) / period * 2.0 * PI + phase).sin();
    let mut x = x0 + 8.0;
    while x < x1 {
        let (a, b) = (strand(0.0, x), strand(PI, x));
        line(cr, x, a, x, b, p.muted, 4.0);
        x += 16.25;
    }
    for (phase, color) in [(PI, p.teal), (0.0, p.accent)] {
        cr.move_to(x0, strand(phase, x0));
        let mut x = x0;
        while x <= x1 {
            cr.line_to(x, strand(phase, x));
            x += 2.0;
        }
        set(cr, color);
        cr.set_line_width(10.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        let _ = cr.stroke();
    }
    circle(cr, x0, strand(0.0, x0), 9.0, p.amber);
    circle(cr, x1, strand(PI, x1), 9.0, p.coral);
}

fn draw_overview(cr: &Context, p: &Palette) {
    desktop(cr, p);
    // Search field.
    fill_rounded(cr, 120.0, 34.0, 120.0, 16.0, 8.0, p.window);
    stroke_rounded(cr, 120.0, 34.0, 120.0, 16.0, 8.0, p.frame, 1.0);
    cr.new_sub_path();
    cr.arc(132.0, 42.0, 3.5, 0.0, 2.0 * PI);
    set(cr, p.muted);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
    // Workspace thumbnails.
    for (i, x) in [102.0, 156.0, 210.0].into_iter().enumerate() {
        fill_rounded(cr, x, 60.0, 48.0, 32.0, 4.0, p.header);
        if i == 0 {
            fill_rounded(cr, x + 5.0, 65.0, 20.0, 14.0, 2.0, p.window);
            fill_rounded(cr, x + 20.0, 72.0, 22.0, 15.0, 2.0, p.window);
            stroke_rounded(cr, x - 3.0, 57.0, 54.0, 38.0, 6.0, p.accent, 2.5);
        } else if i == 1 {
            fill_rounded(cr, x + 12.0, 68.0, 24.0, 17.0, 2.0, p.window);
        }
    }
    // Window previews.
    window(cr, p, 52.0, 108.0, 120.0, 88.0, p.accent);
    window(cr, p, 188.0, 116.0, 120.0, 80.0, p.teal);
    stroke_rounded(cr, 48.0, 104.0, 128.0, 96.0, 10.0, p.amber, 3.0);
    pointer(cr, p, 140.0, 150.0);
}

fn draw_launcher(cr: &Context, p: &Palette) {
    desktop(cr, p);
    fill_rounded(cr, 26.0, 11.0, 26.0, 10.0, 5.0, p.accent);
    fill_rounded(cr, 110.0, 36.0, 140.0, 18.0, 9.0, p.window);
    stroke_rounded(cr, 110.0, 36.0, 140.0, 18.0, 9.0, p.accent, 1.5);
    fill_rounded(cr, 124.0, 42.0, 34.0, 6.0, 3.0, p.muted);
    line(cr, 160.0, 40.0, 160.0, 50.0, p.accent, 1.5);
    let colors = [p.accent, p.teal, p.amber, p.coral, p.violet];
    for row in 0..3 {
        for col in 0..5 {
            let x = 72.0 + col as f64 * 46.0;
            let y = 68.0 + row as f64 * 46.0;
            let c = colors[(row * 2 + col) % colors.len()];
            fill_rounded(cr, x, y, 32.0, 32.0, 9.0, c);
            // A simple glyph per icon.
            match (row + col) % 3 {
                0 => circle(cr, x + 16.0, y + 16.0, 7.0, p.on_accent),
                1 => fill_rounded(cr, x + 9.0, y + 9.0, 14.0, 14.0, 3.0, p.on_accent),
                _ => {
                    cr.move_to(x + 16.0, y + 8.0);
                    cr.line_to(x + 25.0, y + 24.0);
                    cr.line_to(x + 7.0, y + 24.0);
                    cr.close_path();
                    set(cr, p.on_accent);
                    let _ = cr.fill();
                }
            }
        }
    }
    stroke_rounded(cr, 72.0 + 46.0 - 5.0, 68.0 - 5.0, 42.0, 42.0, 12.0, p.accent, 2.5);
}

fn draw_workspaces(cr: &Context, p: &Palette) {
    let xs = [16.0, 132.0, 248.0];
    for (i, &x) in xs.iter().enumerate() {
        let active = i == 1;
        let (y, w, h) = if active { (40.0, 96.0, 72.0) } else { (52.0, 96.0, 60.0) };
        fill_rounded(cr, x, y, w, h, 8.0, p.screen);
        stroke_rounded(cr, x, y, w, h, 8.0, if active { p.accent } else { p.frame }, if active { 3.0 } else { 2.0 });
        match i {
            0 => window(cr, p, x + 10.0, y + 14.0, 56.0, 38.0, p.teal),
            1 => {
                window(cr, p, x + 10.0, y + 12.0, 52.0, 40.0, p.coral);
                window(cr, p, x + 34.0, y + 24.0, 52.0, 40.0, p.accent);
            }
            _ => {
                // The always-empty workspace at the end.
                cr.set_dash(&[4.0, 4.0], 0.0);
                stroke_rounded(cr, x + 30.0, y + 12.0, 36.0, 36.0, 8.0, p.muted, 2.0);
                cr.set_dash(&[], 0.0);
                line(cr, x + 48.0, y + 22.0, x + 48.0, y + 38.0, p.muted, 2.5);
                line(cr, x + 40.0, y + 30.0, x + 56.0, y + 30.0, p.muted, 2.5);
            }
        }
    }
    arrow(cr, 166.0, 128.0, 140.0, p.muted, 3.0);
    arrow(cr, 194.0, 232.0, 140.0, p.muted, 3.0);
    // Panel indicator dots.
    fill_rounded(cr, 130.0, 172.0, 100.0, 24.0, 12.0, p.panel);
    circle(cr, 158.0, 184.0, 4.0, p.muted);
    fill_rounded(cr, 170.0, 180.0, 20.0, 8.0, 4.0, p.accent);
    circle(cr, 202.0, 184.0, 4.0, p.muted);
}

fn draw_windows(cr: &Context, p: &Palette) {
    desktop(cr, p);
    window(cr, p, 26.0, 30.0, 150.0, 176.0, p.accent);
    // Snap preview on the right half.
    set_a(cr, p.teal, 0.18);
    rounded(cr, 184.0, 30.0, 150.0, 176.0, 7.0);
    let _ = cr.fill();
    cr.set_dash(&[6.0, 4.0], 0.0);
    stroke_rounded(cr, 184.0, 30.0, 150.0, 176.0, 7.0, p.teal, 2.0);
    cr.set_dash(&[], 0.0);
    // The window being dragged toward the edge.
    window(cr, p, 214.0, 64.0, 110.0, 80.0, p.teal);
    arrow(cr, 290.0, 330.0, 104.0, p.amber, 3.0);
    pointer(cr, p, 262.0, 68.0);
}

fn draw_touchpad(cr: &Context, p: &Palette) {
    fill_rounded(cr, 70.0, 24.0, 220.0, 150.0, 20.0, p.window);
    stroke_rounded(cr, 70.0, 24.0, 220.0, 150.0, 20.0, p.frame, 3.0);
    // Motion trails.
    for (cx, cy) in [(140.0, 104.0), (182.0, 90.0), (224.0, 104.0)] {
        for k in 1..3 {
            set_a(cr, p.teal, 0.15 * (3 - k) as f64);
            cr.new_sub_path();
            cr.arc(cx - 10.0 * k as f64, cy, 11.0, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        circle(cr, cx, cy, 13.0, p.teal);
        circle(cr, cx, cy, 5.0, p.on_accent);
    }
    arrow(cr, 160.0, 100.0, 196.0, p.accent, 4.0);
    arrow(cr, 200.0, 260.0, 196.0, p.accent, 4.0);
}

fn draw_help(cr: &Context, p: &Palette) {
    let (cx, cy) = (180.0, 108.0);
    // Ring with alternating bands.
    cr.set_line_width(30.0);
    cr.new_sub_path();
    cr.arc(cx, cy, 68.0, 0.0, 2.0 * PI);
    set(cr, p.window);
    let _ = cr.stroke();
    for q in 0..4 {
        let start = q as f64 * PI / 2.0 + PI / 4.0 - 0.38;
        cr.new_sub_path();
        cr.arc(cx, cy, 68.0, start, start + 0.76);
        set(cr, p.coral);
        let _ = cr.stroke();
    }
    for r in [53.0, 83.0] {
        cr.new_sub_path();
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        set(cr, p.frame);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
    }
    // Question mark in the middle.
    cr.set_line_width(8.0);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.new_sub_path();
    cr.arc(cx, cy - 12.0, 14.0, PI, 2.4 * PI);
    cr.line_to(cx, cy + 10.0);
    set(cr, p.accent);
    let _ = cr.stroke();
    circle(cr, cx, cy + 26.0, 5.0, p.accent);
}

fn draw_done(cr: &Context, p: &Palette) {
    let confetti: [(f64, f64, f64, usize); 14] = [
        (60.0, 40.0, 0.3, 0), (96.0, 170.0, 1.1, 1), (300.0, 50.0, 2.0, 2), (276.0, 176.0, 0.7, 3),
        (40.0, 120.0, 1.6, 4), (322.0, 118.0, 0.2, 0), (120.0, 24.0, 2.6, 2), (244.0, 22.0, 1.3, 1),
        (140.0, 196.0, 0.9, 4), (220.0, 200.0, 2.2, 3), (84.0, 84.0, 0.5, 1), (290.0, 90.0, 1.9, 4),
        (70.0, 196.0, 2.9, 2), (318.0, 190.0, 1.4, 0),
    ];
    let colors = [p.accent, p.teal, p.amber, p.coral, p.violet];
    for (i, &(x, y, rot, c)) in confetti.iter().enumerate() {
        let _ = cr.save();
        cr.translate(x, y);
        cr.rotate(rot);
        if i % 3 == 0 {
            circle(cr, 0.0, 0.0, 5.0, colors[c]);
        } else {
            fill_rounded(cr, -7.0, -3.0, 14.0, 6.0, 2.0, colors[c]);
        }
        let _ = cr.restore();
    }
    circle(cr, 180.0, 110.0, 64.0, p.teal);
    cr.move_to(150.0, 112.0);
    cr.line_to(172.0, 134.0);
    cr.line_to(212.0, 88.0);
    set(cr, p.on_accent);
    cr.set_line_width(14.0);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    let _ = cr.stroke();
}

/// Draws `art` scaled to fit a `width`×`height` area.
pub fn draw(art: Art, cr: &Context, width: i32, height: i32, dark: bool) {
    let p = if dark { &DARK } else { &LIGHT };
    let scale = (width as f64 / WIDTH as f64).min(height as f64 / HEIGHT as f64);
    if scale <= 0.0 {
        return;
    }
    let _ = cr.save();
    cr.translate(
        (width as f64 - WIDTH as f64 * scale) / 2.0,
        (height as f64 - HEIGHT as f64 * scale) / 2.0,
    );
    cr.scale(scale, scale);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    match art {
        Art::Welcome => draw_welcome(cr, p),
        Art::Overview => draw_overview(cr, p),
        Art::Launcher => draw_launcher(cr, p),
        Art::Workspaces => draw_workspaces(cr, p),
        Art::Windows => draw_windows(cr, p),
        Art::Touchpad => draw_touchpad(cr, p),
        Art::Help => draw_help(cr, p),
        Art::Done => draw_done(cr, p),
    }
    let _ = cr.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every illustration renders into an image surface without cairo
    /// errors and leaves visible pixels.
    #[test]
    fn all_illustrations_render() {
        let arts = [
            Art::Welcome, Art::Overview, Art::Launcher, Art::Workspaces,
            Art::Windows, Art::Touchpad, Art::Help, Art::Done,
        ];
        for dark in [false, true] {
            for art in arts {
                let mut surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, WIDTH, HEIGHT).unwrap();
                {
                    let cr = Context::new(&surface).unwrap();
                    draw(art, &cr, WIDTH, HEIGHT, dark);
                    assert_eq!(cr.status(), Ok(()), "{art:?}");
                }
                surface.flush();
                let data = surface.data().unwrap();
                // Raw premultiplied BGRA dumps for visual inspection.
                if let Some(dir) = std::env::var_os("HELIX_TOUR_DUMP_DIR") {
                    let name = format!("{art:?}-{}.bgra", if dark { "dark" } else { "light" });
                    std::fs::write(std::path::Path::new(&dir).join(name), &data[..]).unwrap();
                }
                let opaque = data.chunks(4).filter(|px| px[3] != 0).count();
                assert!(opaque > 1000, "{art:?} drew only {opaque} pixels");
            }
        }
    }
}
