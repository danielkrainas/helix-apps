//! Character encoding and line-ending detection / conversion.

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use helix_app_common::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub const ALL: [LineEnding; 3] = [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr];

    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    pub fn nick(self) -> &'static str {
        match self {
            LineEnding::Lf => "lf",
            LineEnding::CrLf => "crlf",
            LineEnding::Cr => "cr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => tr("Unix/Linux (LF)"),
            LineEnding::CrLf => tr("Windows (CRLF)"),
            LineEnding::Cr => tr("Classic Mac OS (CR)"),
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }

    pub fn from_nick(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|l| l.nick() == s)
            .unwrap_or_default()
    }
}

/// How a document is stored on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextFormat {
    pub encoding: &'static Encoding,
    pub bom: bool,
    pub line_ending: LineEnding,
}

impl Default for TextFormat {
    fn default() -> Self {
        TextFormat {
            encoding: UTF_8,
            bom: false,
            line_ending: LineEnding::Lf,
        }
    }
}

impl TextFormat {
    pub fn encoding_name(&self) -> &'static str {
        self.encoding.name()
    }

    pub fn encoding_by_name(name: &str) -> &'static Encoding {
        Encoding::for_label(name.as_bytes()).unwrap_or(UTF_8)
    }
}

/// Detects the encoding of `bytes`. Returns the encoding and the length of
/// the byte-order mark, if any.
pub fn detect_encoding(bytes: &[u8]) -> (&'static Encoding, usize) {
    if let Some((enc, len)) = Encoding::for_bom(bytes) {
        return (enc, len);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (UTF_8, 0);
    }
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, true);
    (det.guess(None, true), 0)
}

/// Detects the dominant line ending; `Lf` when the text has no line breaks.
pub fn detect_line_ending(text: &str) -> LineEnding {
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        i += 1;
    }
    if crlf > lf && crlf >= cr {
        LineEnding::CrLf
    } else if cr > lf && cr > crlf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

/// Converts every `\r\n` and lone `\r` into `\n`.
pub fn normalize_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Decodes file contents into `\n`-normalised text plus its on-disk format.
pub fn decode(bytes: &[u8]) -> (String, TextFormat) {
    let (encoding, bom_len) = detect_encoding(bytes);
    let (text, _had_errors) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
    let line_ending = detect_line_ending(&text);
    let text = normalize_newlines(&text);
    (
        text,
        TextFormat {
            encoding,
            bom: bom_len > 0,
            line_ending,
        },
    )
}

#[derive(Debug, PartialEq, Eq)]
pub struct Unrepresentable;

/// Encodes `\n`-separated text in `format`. Fails if a character cannot be
/// represented in the target encoding.
pub fn encode(text: &str, format: &TextFormat) -> Result<Vec<u8>, Unrepresentable> {
    let converted;
    let text = if format.line_ending == LineEnding::Lf {
        text
    } else {
        converted = text.replace('\n', format.line_ending.as_str());
        &converted
    };
    let enc = format.encoding;
    let mut out = Vec::with_capacity(text.len() + 3);
    if enc == UTF_16LE || enc == UTF_16BE {
        let le = enc == UTF_16LE;
        let push = |out: &mut Vec<u8>, u: u16| {
            out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() })
        };
        if format.bom {
            push(&mut out, 0xFEFF);
        }
        for u in text.encode_utf16() {
            push(&mut out, u);
        }
        return Ok(out);
    }
    if enc == UTF_8 {
        if format.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(text.as_bytes());
        return Ok(out);
    }
    let (bytes, _, unmappable) = enc.encode(text);
    if unmappable {
        return Err(Unrepresentable);
    }
    out.extend_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::{SHIFT_JIS, WINDOWS_1252};

    #[test]
    fn utf8_plain() {
        let (t, f) = decode("héllo\nworld\n".as_bytes());
        assert_eq!(t, "héllo\nworld\n");
        assert_eq!(f, TextFormat::default());
    }

    #[test]
    fn utf8_bom_roundtrip() {
        let bytes = b"\xEF\xBB\xBFabc\r\ndef\r\n";
        let (t, f) = decode(bytes);
        assert_eq!(t, "abc\ndef\n");
        assert!(f.bom);
        assert_eq!(f.encoding, UTF_8);
        assert_eq!(f.line_ending, LineEnding::CrLf);
        assert_eq!(encode(&t, &f).unwrap(), bytes);
    }

    #[test]
    fn utf16_roundtrip() {
        for enc in [UTF_16LE, UTF_16BE] {
            let f = TextFormat {
                encoding: enc,
                bom: true,
                line_ending: LineEnding::Lf,
            };
            let bytes = encode("ünï\ncode ✓", &f).unwrap();
            let (t, g) = decode(&bytes);
            assert_eq!(t, "ünï\ncode ✓");
            assert_eq!(g, f);
        }
    }

    #[test]
    fn legacy_fallback() {
        // "café crème brûlée" in windows-1252 is not valid UTF-8.
        let bytes = WINDOWS_1252
            .encode("Le café crème brûlée était délicieux à Paris.")
            .0
            .into_owned();
        assert!(std::str::from_utf8(&bytes).is_err());
        let (t, f) = decode(&bytes);
        assert_eq!(t, "Le café crème brûlée était délicieux à Paris.");
        assert_eq!(f.encoding, WINDOWS_1252);
        assert_eq!(encode(&t, &f).unwrap(), bytes);
    }

    #[test]
    fn shift_jis_detected() {
        let src = "これは日本語のテキストです。文字コードの判定をテストします。";
        let bytes = SHIFT_JIS.encode(src).0.into_owned();
        let (t, f) = decode(&bytes);
        assert_eq!(f.encoding, SHIFT_JIS);
        assert_eq!(t, src);
    }

    #[test]
    fn unrepresentable() {
        let f = TextFormat {
            encoding: WINDOWS_1252,
            bom: false,
            line_ending: LineEnding::Lf,
        };
        assert_eq!(encode("snowman ☃", &f), Err(Unrepresentable));
    }

    #[test]
    fn line_endings() {
        assert_eq!(detect_line_ending("a\nb\n"), LineEnding::Lf);
        assert_eq!(detect_line_ending("a\r\nb\r\n"), LineEnding::CrLf);
        assert_eq!(detect_line_ending("a\rb\r"), LineEnding::Cr);
        assert_eq!(detect_line_ending("no breaks"), LineEnding::Lf);
        assert_eq!(detect_line_ending("a\r\nb\r\nc\n"), LineEnding::CrLf);
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
        let f = TextFormat {
            line_ending: LineEnding::Cr,
            ..Default::default()
        };
        assert_eq!(encode("x\ny\n", &f).unwrap(), b"x\ry\r");
        let (t, g) = decode(b"one\rtwo\r");
        assert_eq!(t, "one\ntwo\n");
        assert_eq!(encode(&t, &g).unwrap(), b"one\rtwo\r");
    }

    #[test]
    fn nicks() {
        for l in LineEnding::ALL {
            assert_eq!(LineEnding::from_nick(l.nick()), l);
        }
        assert_eq!(TextFormat::encoding_by_name("windows-1252"), WINDOWS_1252);
        assert_eq!(TextFormat::encoding_by_name("bogus"), UTF_8);
    }
}
