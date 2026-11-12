//! EXIF orientation (tag 0x0112) and its application to pixel buffers.
//!
//! | Value | Correction to display upright           |
//! |-------|-----------------------------------------|
//! | 1     | none                                    |
//! | 2     | mirror horizontally                     |
//! | 3     | rotate 180°                             |
//! | 4     | mirror vertically                       |
//! | 5     | mirror horizontally, rotate 270° CW (transpose) |
//! | 6     | rotate 90° CW                           |
//! | 7     | mirror horizontally, rotate 90° CW (transverse) |
//! | 8     | rotate 270° CW                          |

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orientation(u8);

impl Default for Orientation {
    fn default() -> Self {
        Orientation(1)
    }
}

impl Orientation {
    pub const NORMAL: Orientation = Orientation(1);

    /// From the raw tag value; out-of-range values mean "normal".
    pub fn from_exif(v: u32) -> Self {
        if (1..=8).contains(&v) {
            Orientation(v as u8)
        } else {
            Orientation::NORMAL
        }
    }

    #[cfg(test)]
    pub fn value(self) -> u8 {
        self.0
    }

    pub fn is_normal(self) -> bool {
        self.0 == 1
    }

    /// Whether width and height are exchanged by the correction.
    pub fn swaps_dimensions(self) -> bool {
        self.0 >= 5
    }

    /// The correction as (mirror horizontally first?, then clockwise degrees).
    #[cfg(test)]
    pub fn correction(self) -> (bool, u32) {
        match self.0 {
            2 => (true, 0),
            3 => (false, 180),
            4 => (true, 180),
            5 => (true, 270),
            6 => (false, 90),
            7 => (true, 90),
            8 => (false, 270),
            _ => (false, 0),
        }
    }

    /// Display size for a stored image of `w`×`h`.
    pub fn display_size(self, w: u32, h: u32) -> (u32, u32) {
        if self.swaps_dimensions() {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// For output pixel (x, y) of the corrected image, the source pixel in the
    /// stored `w`×`h` image.
    pub fn source_pixel(self, x: u32, y: u32, w: u32, h: u32) -> (u32, u32) {
        match self.0 {
            2 => (w - 1 - x, y),
            3 => (w - 1 - x, h - 1 - y),
            4 => (x, h - 1 - y),
            5 => (y, x),
            6 => (y, h - 1 - x),
            7 => (w - 1 - y, h - 1 - x),
            8 => (w - 1 - y, x),
            _ => (x, y),
        }
    }
}

/// Applies `o` to a pixel buffer with `bpp` bytes per pixel and the given row
/// stride. Returns the packed corrected buffer and its size.
pub fn apply(data: &[u8], w: u32, h: u32, stride: usize, bpp: usize, o: Orientation) -> (Vec<u8>, u32, u32) {
    let (ow, oh) = o.display_size(w, h);
    let mut out = vec![0u8; ow as usize * oh as usize * bpp];
    for y in 0..oh {
        let row = y as usize * ow as usize * bpp;
        for x in 0..ow {
            let (sx, sy) = o.source_pixel(x, y, w, h);
            let src = sy as usize * stride + sx as usize * bpp;
            let dst = row + x as usize * bpp;
            out[dst..dst + bpp].copy_from_slice(&data[src..src + bpp]);
        }
    }
    (out, ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3×2 single-byte image:
    /// ```text
    /// 1 2 3
    /// 4 5 6
    /// ```
    const IMG: [u8; 6] = [1, 2, 3, 4, 5, 6];

    fn run(v: u32) -> (Vec<u8>, u32, u32) {
        apply(&IMG, 3, 2, 3, 1, Orientation::from_exif(v))
    }

    #[test]
    fn all_eight_orientations() {
        assert_eq!(run(1), (vec![1, 2, 3, 4, 5, 6], 3, 2));
        assert_eq!(run(2), (vec![3, 2, 1, 6, 5, 4], 3, 2));
        assert_eq!(run(3), (vec![6, 5, 4, 3, 2, 1], 3, 2));
        assert_eq!(run(4), (vec![4, 5, 6, 1, 2, 3], 3, 2));
        // Transpose.
        assert_eq!(run(5), (vec![1, 4, 2, 5, 3, 6], 2, 3));
        // 90° clockwise: left column becomes the top row, read bottom-up.
        assert_eq!(run(6), (vec![4, 1, 5, 2, 6, 3], 2, 3));
        // Transverse.
        assert_eq!(run(7), (vec![6, 3, 5, 2, 4, 1], 2, 3));
        // 90° counter-clockwise.
        assert_eq!(run(8), (vec![3, 6, 2, 5, 1, 4], 2, 3));
    }

    #[test]
    fn invalid_values_are_normal() {
        assert!(Orientation::from_exif(0).is_normal());
        assert!(Orientation::from_exif(9).is_normal());
        assert_eq!(run(42), run(1));
    }

    /// Reference implementation built from `correction()` (mirror, then
    /// rotate clockwise in 90° steps) must agree with `source_pixel`.
    #[test]
    fn source_pixel_matches_correction_table() {
        fn mirror(d: &[u8], w: u32, h: u32) -> Vec<u8> {
            (0..h).flat_map(|y| (0..w).rev().map(move |x| d[(y * w + x) as usize])).collect()
        }
        fn rot_cw(d: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
            // New size h×w; out(x, y) = in(y, h-1-x).
            let out = (0..w).flat_map(|y| (0..h).map(move |x| d[((h - 1 - x) * w + y) as usize])).collect();
            (out, h, w)
        }
        for v in 1..=8 {
            let o = Orientation::from_exif(v);
            let (m, deg) = o.correction();
            let mut d = IMG.to_vec();
            let (mut w, mut h) = (3, 2);
            if m {
                d = mirror(&d, w, h);
            }
            for _ in 0..deg / 90 {
                let r = rot_cw(&d, w, h);
                d = r.0;
                w = r.1;
                h = r.2;
            }
            assert_eq!(run(v), (d, w, h), "orientation {v}");
            assert_eq!(o.swaps_dimensions(), deg % 180 == 90);
        }
    }

    #[test]
    fn multi_byte_pixels_and_stride() {
        // 2×1 RGBA with 4 bytes of row padding.
        let data = [1, 1, 1, 1, 2, 2, 2, 2, 0, 0, 0, 0];
        let (out, w, h) = apply(&data, 2, 1, 12, 4, Orientation::from_exif(6));
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![1, 1, 1, 1, 2, 2, 2, 2]);
        let (out, _, _) = apply(&data, 2, 1, 12, 4, Orientation::from_exif(2));
        assert_eq!(out, vec![2, 2, 2, 2, 1, 1, 1, 1]);
    }
}
