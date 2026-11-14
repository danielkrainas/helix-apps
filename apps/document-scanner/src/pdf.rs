//! A minimal PDF 1.4 writer for image-only documents.
//!
//! Each page holds one image XObject drawn over the full media box.

use std::io::Write;

/// Image data for one PDF page.
pub struct PdfImage {
    pub width: u32,
    pub height: u32,
    pub color: bool,
    /// Either baseline JPEG bytes or zlib-compressed raw samples.
    pub data: Vec<u8>,
    pub filter: Filter,
    /// Page size in points.
    pub page_width: f64,
    pub page_height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Dct,
    Flate,
}

impl Filter {
    fn name(self) -> &'static str {
        match self {
            Filter::Dct => "DCTDecode",
            Filter::Flate => "FlateDecode",
        }
    }
}

struct Writer {
    buf: Vec<u8>,
    offsets: Vec<usize>,
}

impl Writer {
    /// Starts object number `offsets.len() + 1`.
    fn begin(&mut self, id: usize) {
        debug_assert_eq!(id, self.offsets.len() + 1);
        self.offsets.push(self.buf.len());
        let _ = writeln!(self.buf, "{id} 0 obj");
    }

    fn end(&mut self) {
        self.buf.extend_from_slice(b"endobj\n");
    }

    fn object(&mut self, id: usize, body: &str) {
        self.begin(id);
        self.buf.extend_from_slice(body.as_bytes());
        self.buf.push(b'\n');
        self.end();
    }

    fn stream(&mut self, id: usize, dict: &str, data: &[u8]) {
        self.begin(id);
        let _ = write!(self.buf, "<< {dict} /Length {} >>\nstream\n", data.len());
        self.buf.extend_from_slice(data);
        self.buf.extend_from_slice(b"\nendstream\n");
        self.end();
    }
}

fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.into()
    }
}

/// Escapes a PDF literal string.
fn text(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

/// Builds a complete PDF file. `date` is a PDF date string body such as
/// `20240131120000Z` (or `None`).
pub fn build(images: &[PdfImage], producer: &str, date: Option<&str>) -> Vec<u8> {
    let mut w = Writer {
        buf: Vec::new(),
        offsets: Vec::new(),
    };
    w.buf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    // 1: catalog, 2: pages, 3: info, then 3 objects per page.
    let page_id = |i: usize| 4 + i * 3;
    w.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..images.len())
        .map(|i| format!("{} 0 R", page_id(i)))
        .collect();
    w.object(
        2,
        &format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            images.len()
        ),
    );
    let mut info = format!("<< /Producer {}", text(producer));
    if let Some(d) = date {
        info.push_str(&format!(" /CreationDate {}", text(&format!("D:{d}"))));
    }
    info.push_str(" >>");
    w.object(3, &info);
    for (i, img) in images.iter().enumerate() {
        let (pid, cid, iid) = (page_id(i), page_id(i) + 1, page_id(i) + 2);
        let (pw, ph) = (num(img.page_width), num(img.page_height));
        w.object(
            pid,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw} {ph}] \
                 /Resources << /XObject << /Im0 {iid} 0 R >> >> /Contents {cid} 0 R >>"
            ),
        );
        let content = format!("q\n{pw} 0 0 {ph} 0 0 cm\n/Im0 Do\nQ\n");
        w.stream(cid, "", content.as_bytes());
        let cs = if img.color {
            "/DeviceRGB"
        } else {
            "/DeviceGray"
        };
        w.stream(
            iid,
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {cs} \
                 /BitsPerComponent 8 /Filter /{}",
                img.width,
                img.height,
                img.filter.name()
            ),
            &img.data,
        );
    }
    let xref = w.buf.len();
    let n = w.offsets.len() + 1;
    let _ = write!(w.buf, "xref\n0 {n}\n0000000000 65535 f \n");
    for off in &w.offsets {
        let _ = writeln!(w.buf, "{off:010} 00000 n ");
    }
    let _ = write!(
        w.buf,
        "trailer\n<< /Size {n} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    );
    w.buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn numbers() {
        assert_eq!(num(595.0), "595");
        assert_eq!(num(841.889), "841.889");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(text("a(b)\\é"), "(a\\(b\\)\\\\?)");
    }

    #[test]
    fn structure_and_xref() {
        let img = |color| PdfImage {
            width: 2,
            height: 2,
            color,
            data: vec![1, 2, 3, 4],
            filter: Filter::Flate,
            page_width: 144.0,
            page_height: 72.0,
        };
        let pdf = build(&[img(false), img(true)], "Helix", Some("20240101000000Z"));
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        assert!(find(&pdf, b"/Count 2").is_some());
        assert!(find(&pdf, b"/MediaBox [0 0 144 72]").is_some());
        assert!(find(&pdf, b"/DeviceRGB").is_some());
        assert!(find(&pdf, b"/CreationDate (D:20240101000000Z)").is_some());
        // startxref points at the xref table, and every entry at its object.
        // One char per byte so string offsets equal byte offsets.
        let s: String = pdf
            .iter()
            .map(|&b| if b.is_ascii() { b as char } else { '?' })
            .collect();
        let sx = s.rfind("startxref\n").unwrap();
        let xref: usize = s[sx + 10..].lines().next().unwrap().parse().unwrap();
        assert!(s[xref..].starts_with("xref\n0 10\n"));
        let entries: Vec<usize> = s[xref..]
            .lines()
            .skip(3)
            .take(9)
            .map(|l| l[..10].parse().unwrap())
            .collect();
        for (i, off) in entries.iter().enumerate() {
            assert!(
                s[*off..].starts_with(&format!("{} 0 obj", i + 1)),
                "object {}",
                i + 1
            );
        }
    }
}
