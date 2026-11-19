//! The film strip drawn over a video's thumbnail in the grid: a dark band
//! down each side with sprocket holes, as if the picture were a frame on
//! film. Drawn when the thumbnail is shown, not saved into the thumbnail
//! cache (other apps read that cache and expect the plain frame).

/// Dark band colour (RGB).
const BAND: [u8; 3] = [0x1e, 0x1e, 0x1e];

/// Geometry of the strip for a `w`×`h` picture: band width, hole width,
/// hole height and the vertical pitch between holes (pixels).
fn geometry(w: usize, h: usize) -> (usize, usize, usize, usize) {
    let band = (w.min(h) / 6).clamp(4, 40);
    let hole_w = (band * 5 / 9).max(1);
    let hole_h = (band * 4 / 9).max(1);
    let pitch = (hole_h * 2).max(2);
    (band, hole_w, hole_h, pitch)
}

/// Whether (x, y) inside a `hw`×`hh` hole falls in its rounded corners'
/// cut-off (corner radius a third of the shorter side).
fn outside_rounding(x: usize, y: usize, hw: usize, hh: usize) -> bool {
    let r = (hw.min(hh) / 3) as f64;
    if r < 1.0 {
        return false;
    }
    let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
    let cx = if fx < r {
        r
    } else if fx > hw as f64 - r {
        hw as f64 - r
    } else {
        return false;
    };
    let cy = if fy < r {
        r
    } else if fy > hh as f64 - r {
        hh as f64 - r
    } else {
        return false;
    };
    (fx - cx).powi(2) + (fy - cy).powi(2) > r * r
}

/// Draws the strip over RGBA pixels (`stride` bytes per row): opaque dark
/// bands, and holes cleared to transparent so the view's background shows
/// through, whatever the theme.
pub fn draw(pixels: &mut [u8], w: usize, h: usize, stride: usize) {
    if w < 8 || h < 8 || stride < w * 4 || pixels.len() < stride * (h - 1) + w * 4 {
        return;
    }
    let (band, hole_w, hole_h, pitch) = geometry(w, h);
    let hole_x = (band - hole_w) / 2;
    // Centre the run of holes vertically.
    let count = (h + pitch - hole_h) / pitch;
    let used = count.saturating_sub(1) * pitch + hole_h;
    let top = h.saturating_sub(used) / 2;
    for y in 0..h {
        let row = &mut pixels[y * stride..y * stride + w * 4];
        // Which hole row (if any) this line crosses.
        let hole_y = (y >= top)
            .then(|| (y - top) % pitch)
            .filter(|&dy| dy < hole_h && (y - top) / pitch < count);
        for side in [0, w - band] {
            for bx in 0..band {
                let px = &mut row[(side + bx) * 4..(side + bx) * 4 + 4];
                let in_hole = hole_y.is_some_and(|dy| {
                    bx >= hole_x
                        && bx < hole_x + hole_w
                        && !outside_rounding(bx - hole_x, dy, hole_w, hole_h)
                });
                if in_hole {
                    px.copy_from_slice(&[0, 0, 0, 0]);
                } else {
                    px[..3].copy_from_slice(&BAND);
                    px[3] = 255;
                }
            }
        }
    }
}

/// Whether a MIME type is a video's.
pub fn is_video(mime: &str) -> bool {
    mime.starts_with("video/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize) -> Vec<u8> {
        [10u8, 200, 30, 255].repeat(w * h)
    }

    fn at(p: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [p[i], p[i + 1], p[i + 2], p[i + 3]]
    }

    #[test]
    fn bands_holes_and_the_picture() {
        let (w, h) = (256, 144);
        let mut p = picture(w, h);
        draw(&mut p, w, h, w * 4);
        let (band, hole_w, hole_h, pitch) = geometry(w, h);
        assert!(band >= 10, "{band}");
        // The middle of the picture is untouched.
        assert_eq!(at(&p, w, w / 2, h / 2), [10, 200, 30, 255]);
        assert_eq!(at(&p, w, band, h / 2), [10, 200, 30, 255]);
        // The band's edge is dark and opaque on both sides.
        assert_eq!(at(&p, w, 0, 0), [0x1e, 0x1e, 0x1e, 255]);
        assert_eq!(at(&p, w, w - 1, h - 1), [0x1e, 0x1e, 0x1e, 255]);
        // Holes: transparent at their centres, on both sides, repeating.
        let count = (h + pitch - hole_h) / pitch;
        let top = (h - ((count - 1) * pitch + hole_h)) / 2;
        let cx = (band - hole_w) / 2 + hole_w / 2;
        for k in 0..count {
            let cy = top + k * pitch + hole_h / 2;
            assert_eq!(at(&p, w, cx, cy)[3], 0, "hole {k} left");
            assert_eq!(at(&p, w, w - band + cx, cy)[3], 0, "hole {k} right");
        }
        // Between two holes the band is solid.
        let between = top + hole_h + (pitch - hole_h) / 2;
        assert_eq!(at(&p, w, cx, between)[3], 255);
    }

    #[test]
    fn tall_and_tiny_pictures() {
        // A phone's vertical video: bands sized by the narrow side.
        let (w, h) = (144, 256);
        let mut p = picture(w, h);
        draw(&mut p, w, h, w * 4);
        assert_eq!(at(&p, w, w / 2, h / 2), [10, 200, 30, 255]);
        assert_eq!(at(&p, w, 0, 1)[..3], BAND);
        // Too small to draw on: left alone.
        let mut p = picture(6, 6);
        let before = p.clone();
        draw(&mut p, 6, 6, 24);
        assert_eq!(p, before);
    }

    #[test]
    fn row_padding_is_left_alone() {
        let (w, h, stride) = (64, 36, 64 * 4 + 12);
        let mut p = vec![7u8; stride * h];
        draw(&mut p, w, h, stride);
        for y in 0..h {
            assert!(p[y * stride + w * 4..(y + 1) * stride]
                .iter()
                .all(|&b| b == 7));
        }
    }

    #[test]
    fn video_types() {
        assert!(is_video("video/mp4"));
        assert!(is_video("video/x-matroska"));
        assert!(!is_video("image/png"));
        assert!(!is_video("audio/ogg"));
    }
}
