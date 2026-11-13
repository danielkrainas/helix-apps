//! Scanned pages: 8-bit grey or RGB pixel buffers plus per-page metadata,
//! and the pixel operations the document needs (rotate, crop, assembling
//! SANE frames).

use std::sync::Arc;

/// A rectangle in page pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Rect { x, y, width, height }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    /// 1 (grey) or 3 (RGB) bytes per pixel.
    pub channels: u8,
    pub pixels: Arc<[u8]>,
    /// Scan resolution in dots per inch.
    pub dpi: f64,
    /// Scanned as a text document (saved losslessly in PDFs).
    pub text: bool,
    /// Non-destructive crop applied on export.
    pub crop: Option<Rect>,
}

impl Page {
    pub fn new(width: u32, height: u32, channels: u8, pixels: Vec<u8>, dpi: f64) -> Self {
        assert!(channels == 1 || channels == 3);
        assert_eq!(pixels.len(), (width * height) as usize * channels as usize);
        Page { width, height, channels, pixels: pixels.into(), dpi, text: false, crop: None }
    }

    pub fn is_color(&self) -> bool {
        self.channels == 3
    }

    pub fn stride(&self) -> usize {
        self.width as usize * self.channels as usize
    }

    /// Rotates 90° clockwise (`clockwise = true`) or counter-clockwise.
    pub fn rotate(&self, clockwise: bool) -> Page {
        let (w, h, c) = (self.width as usize, self.height as usize, self.channels as usize);
        let mut out = vec![0u8; self.pixels.len()];
        // New image is h wide and w tall.
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = if clockwise { (h - 1 - y, x) } else { (y, w - 1 - x) };
                let src = (y * w + x) * c;
                let dst = (ny * h + nx) * c;
                out[dst..dst + c].copy_from_slice(&self.pixels[src..src + c]);
            }
        }
        let crop = self.crop.map(|r| rotate_rect(r, self.width, self.height, clockwise));
        Page { width: self.height, height: self.width, pixels: out.into(), crop, ..self.clone() }
    }

    /// Rotates 180°.
    pub fn rotate_half(&self) -> Page {
        let c = self.channels as usize;
        let mut out = Vec::with_capacity(self.pixels.len());
        for px in self.pixels.chunks_exact(c).rev() {
            out.extend_from_slice(px);
        }
        let crop = self.crop.map(|r| Rect {
            x: self.width - r.x - r.width,
            y: self.height - r.y - r.height,
            ..r
        });
        Page { pixels: out.into(), crop, ..self.clone() }
    }

    /// The page with its crop applied (or itself if uncropped).
    pub fn cropped(&self) -> Page {
        let Some(r) = self.crop.map(|r| clamp_rect(r, self.width, self.height)) else {
            return self.clone();
        };
        let c = self.channels as usize;
        let mut out = Vec::with_capacity((r.width * r.height) as usize * c);
        for y in r.y..r.y + r.height {
            let start = (y as usize * self.width as usize + r.x as usize) * c;
            out.extend_from_slice(&self.pixels[start..start + r.width as usize * c]);
        }
        Page {
            width: r.width,
            height: r.height,
            pixels: out.into(),
            crop: None,
            ..self.clone()
        }
    }

    /// Physical size in PDF points (1/72 inch).
    pub fn size_points(&self) -> (f64, f64) {
        let dpi = if self.dpi > 0.0 { self.dpi } else { 72.0 };
        (self.width as f64 * 72.0 / dpi, self.height as f64 * 72.0 / dpi)
    }

    /// Pixels as packed RGB (grey is expanded).
    pub fn to_rgb(&self) -> Vec<u8> {
        if self.channels == 3 {
            return self.pixels.to_vec();
        }
        self.pixels.iter().flat_map(|&g| [g, g, g]).collect()
    }
}

/// Clamps `r` to a `w`×`h` page, keeping at least 1×1.
pub fn clamp_rect(r: Rect, w: u32, h: u32) -> Rect {
    let x = r.x.min(w.saturating_sub(1));
    let y = r.y.min(h.saturating_sub(1));
    let width = r.width.clamp(1, w - x);
    let height = r.height.clamp(1, h - y);
    Rect { x, y, width, height }
}

/// Maps a rectangle on a `w`×`h` page through a 90° rotation.
pub fn rotate_rect(r: Rect, w: u32, h: u32, clockwise: bool) -> Rect {
    if clockwise {
        Rect { x: h - r.y - r.height, y: r.x, width: r.height, height: r.width }
    } else {
        Rect { x: r.y, y: w - r.x - r.width, width: r.height, height: r.width }
    }
}

/// SANE frame formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameFormat {
    Gray,
    Rgb,
    Red,
    Green,
    Blue,
}

/// Parameters of one received frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameParams {
    pub format: FrameFormat,
    pub bytes_per_line: usize,
    pub pixels_per_line: usize,
    pub depth: u32,
}

impl FrameParams {
    /// Number of whole lines contained in `len` bytes.
    pub fn lines_in(&self, len: usize) -> usize {
        len.checked_div(self.bytes_per_line).unwrap_or(0)
    }
}

/// Converts one line of raw SANE samples to 8-bit samples.
/// `samples` is the number of samples on the line (pixels × channels).
fn line_to_8bit(raw: &[u8], depth: u32, samples: usize, out: &mut Vec<u8>) {
    match depth {
        1 => {
            for i in 0..samples {
                let bit = (raw[i / 8] >> (7 - (i % 8))) & 1;
                // In SANE lineart a set bit means black.
                out.push(if bit == 1 { 0 } else { 255 });
            }
        }
        16 => {
            for i in 0..samples {
                let v = u16::from_ne_bytes([raw[2 * i], raw[2 * i + 1]]);
                out.push((v >> 8) as u8);
            }
        }
        _ => out.extend_from_slice(&raw[..samples]),
    }
}

/// Accumulates the frames of one image (one RGB/grey frame, or three
/// single-colour frames) into a [`Page`].
#[derive(Default)]
pub struct PageBuilder {
    width: usize,
    height: usize,
    gray: Option<Vec<u8>>,
    rgb: Option<Vec<u8>>,
    planes: [Option<Vec<u8>>; 3],
}

impl PageBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a complete frame of raw bytes.
    pub fn add_frame(&mut self, p: &FrameParams, raw: &[u8]) -> Result<(), String> {
        let lines = p.lines_in(raw.len());
        let chans = if p.format == FrameFormat::Rgb { 3 } else { 1 };
        if p.pixels_per_line == 0 || lines == 0 {
            return Err("The scanner returned an empty image".into());
        }
        if p.depth != 1 && p.depth != 8 && p.depth != 16 {
            return Err(format!("Unsupported bit depth {}", p.depth));
        }
        let samples = p.pixels_per_line * chans;
        let needed = (samples * p.depth as usize).div_ceil(8);
        if needed > p.bytes_per_line {
            return Err("Invalid scan line length".into());
        }
        let mut out = Vec::with_capacity(samples * lines);
        for l in 0..lines {
            let line = &raw[l * p.bytes_per_line..(l + 1) * p.bytes_per_line];
            line_to_8bit(line, p.depth, samples, &mut out);
        }
        if self.width != 0 && (self.width != p.pixels_per_line) {
            return Err("Frames have different sizes".into());
        }
        self.width = p.pixels_per_line;
        self.height = if self.height == 0 { lines } else { self.height.min(lines) };
        match p.format {
            FrameFormat::Gray => self.gray = Some(out),
            FrameFormat::Rgb => self.rgb = Some(out),
            FrameFormat::Red => self.planes[0] = Some(out),
            FrameFormat::Green => self.planes[1] = Some(out),
            FrameFormat::Blue => self.planes[2] = Some(out),
        }
        Ok(())
    }

    pub fn finish(self, dpi: f64) -> Result<Page, String> {
        let (w, h) = (self.width, self.height);
        if let Some(mut rgb) = self.rgb {
            rgb.truncate(w * h * 3);
            return Ok(Page::new(w as u32, h as u32, 3, rgb, dpi));
        }
        if let Some(mut g) = self.gray {
            g.truncate(w * h);
            return Ok(Page::new(w as u32, h as u32, 1, g, dpi));
        }
        if let [Some(r), Some(g), Some(b)] = &self.planes {
            let mut rgb = Vec::with_capacity(w * h * 3);
            for i in 0..w * h {
                rgb.extend_from_slice(&[r[i], g[i], b[i]]);
            }
            return Ok(Page::new(w as u32, h as u32, 3, rgb, dpi));
        }
        Err("The scanner returned an incomplete image".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(w: u32, h: u32) -> Page {
        Page::new(w, h, 1, (0..w * h).map(|v| v as u8).collect(), 100.0)
    }

    #[test]
    fn rotate_clockwise() {
        // 3x2:  0 1 2
        //       3 4 5
        let p = grey(3, 2).rotate(true);
        assert_eq!((p.width, p.height), (2, 3));
        assert_eq!(&*p.pixels, &[3, 0, 4, 1, 5, 2]);
    }

    #[test]
    fn rotate_counter_clockwise() {
        let p = grey(3, 2).rotate(false);
        assert_eq!(&*p.pixels, &[2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn rotate_round_trip_with_crop() {
        let mut p = grey(7, 5);
        p.crop = Some(Rect::new(1, 2, 3, 2));
        let cropped = p.cropped();
        let r = p.rotate(true);
        assert_eq!(&*r.cropped().rotate(false).pixels, &*cropped.pixels);
        let r = p.rotate(false);
        assert_eq!(&*r.cropped().rotate(true).pixels, &*cropped.pixels);
        let back = p.rotate(true).rotate(true).rotate(true).rotate(true);
        assert_eq!(&*back.pixels, &*p.pixels);
        assert_eq!(back.crop, p.crop);
        let half = p.rotate_half();
        assert_eq!(&*half.pixels, &*p.rotate(true).rotate(true).pixels);
        assert_eq!(half.crop, p.rotate(true).rotate(true).crop);
    }

    #[test]
    fn crop_extracts_region() {
        let mut p = Page::new(3, 2, 3, (0..18).collect(), 100.0);
        p.crop = Some(Rect::new(1, 1, 2, 1));
        let c = p.cropped();
        assert_eq!((c.width, c.height), (2, 1));
        assert_eq!(&*c.pixels, &[12, 13, 14, 15, 16, 17]);
        // Out-of-bounds crops are clamped.
        p.crop = Some(Rect::new(2, 1, 50, 50));
        assert_eq!(p.cropped().width, 1);
    }

    #[test]
    fn size_in_points() {
        let p = Page::new(300, 600, 1, vec![0; 180000], 300.0);
        assert_eq!(p.size_points(), (72.0, 144.0));
    }

    #[test]
    fn builds_rgb_frame() {
        let params = FrameParams {
            format: FrameFormat::Rgb,
            bytes_per_line: 7, // padded
            pixels_per_line: 2,
            depth: 8,
        };
        let raw = [1, 2, 3, 4, 5, 6, 99, 7, 8, 9, 10, 11, 12, 99];
        let mut b = PageBuilder::new();
        b.add_frame(&params, &raw).unwrap();
        let p = b.finish(75.0).unwrap();
        assert_eq!((p.width, p.height, p.channels), (2, 2, 3));
        assert_eq!(&*p.pixels, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn builds_lineart_and_16bit() {
        let params = FrameParams {
            format: FrameFormat::Gray,
            bytes_per_line: 1,
            pixels_per_line: 4,
            depth: 1,
        };
        let mut b = PageBuilder::new();
        b.add_frame(&params, &[0b1010_0000]).unwrap();
        assert_eq!(&*b.finish(1.0).unwrap().pixels, &[0, 255, 0, 255]);

        let params = FrameParams { depth: 16, bytes_per_line: 4, pixels_per_line: 2, ..params };
        let mut raw = Vec::new();
        raw.extend_from_slice(&0xABCDu16.to_ne_bytes());
        raw.extend_from_slice(&0x0102u16.to_ne_bytes());
        let mut b = PageBuilder::new();
        b.add_frame(&params, &raw).unwrap();
        assert_eq!(&*b.finish(1.0).unwrap().pixels, &[0xAB, 0x01]);
    }

    #[test]
    fn builds_three_pass() {
        let mk = |format| FrameParams { format, bytes_per_line: 2, pixels_per_line: 2, depth: 8 };
        let mut b = PageBuilder::new();
        b.add_frame(&mk(FrameFormat::Red), &[1, 2]).unwrap();
        b.add_frame(&mk(FrameFormat::Green), &[3, 4]).unwrap();
        b.add_frame(&mk(FrameFormat::Blue), &[5, 6]).unwrap();
        assert_eq!(&*b.finish(1.0).unwrap().pixels, &[1, 3, 5, 2, 4, 6]);
    }

    #[test]
    fn incomplete_three_pass_is_error() {
        let mut b = PageBuilder::new();
        let p = FrameParams { format: FrameFormat::Red, bytes_per_line: 1, pixels_per_line: 1, depth: 8 };
        b.add_frame(&p, &[1]).unwrap();
        assert!(b.finish(1.0).is_err());
        assert!(PageBuilder::new().add_frame(&p, &[]).is_err());
    }
}
