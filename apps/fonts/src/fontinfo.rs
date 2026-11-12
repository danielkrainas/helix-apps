//! Reading metadata out of a font file (name table, format, coverage).

use std::path::Path;
use ttf_parser::{name_id, Face, PlatformId};

/// Metadata about one face of a font file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FaceInfo {
    pub index: u32,
    pub family: String,
    pub style: String,
    pub full_name: String,
    pub version: String,
    pub copyright: String,
    pub designer: String,
    pub manufacturer: String,
    pub description: String,
    pub license: String,
    pub license_url: String,
    pub format: String,
    pub glyph_count: u16,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parse,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Parse => f.write_str("Not a supported font file"),
        }
    }
}

/// Number of faces in `data` (1 for a plain font, n for a collection,
/// `None` if the data is not a TrueType/OpenType font).
pub fn face_count(data: &[u8]) -> Option<u32> {
    match ttf_parser::fonts_in_collection(data) {
        Some(n) if n > 0 => Some(n),
        Some(_) => None,
        None => Face::parse(data, 0).ok().map(|_| 1),
    }
}

pub fn is_collection(data: &[u8]) -> bool {
    data.starts_with(b"ttcf")
}

/// Reads all faces of the font file at `path`.
pub fn read_file(path: &Path) -> Result<Vec<FaceInfo>, Error> {
    let data = std::fs::read(path).map_err(Error::Io)?;
    read_data(&data)
}

pub fn read_data(data: &[u8]) -> Result<Vec<FaceInfo>, Error> {
    let n = face_count(data).ok_or(Error::Parse)?;
    let faces: Vec<FaceInfo> = (0..n).filter_map(|i| face_info(data, i)).collect();
    if faces.is_empty() {
        Err(Error::Parse)
    } else {
        Ok(faces)
    }
}

/// Reads one face's metadata.
pub fn face_info(data: &[u8], index: u32) -> Option<FaceInfo> {
    let face = Face::parse(data, index).ok()?;
    let get = |id| name_string(&face, id).unwrap_or_default();
    let family = name_string(&face, name_id::TYPOGRAPHIC_FAMILY)
        .or_else(|| name_string(&face, name_id::FAMILY))
        .unwrap_or_default();
    let style = name_string(&face, name_id::TYPOGRAPHIC_SUBFAMILY)
        .or_else(|| name_string(&face, name_id::SUBFAMILY))
        .unwrap_or_else(|| "Regular".into());
    Some(FaceInfo {
        index,
        family,
        style,
        full_name: get(name_id::FULL_NAME),
        version: get(name_id::VERSION),
        copyright: get(name_id::COPYRIGHT_NOTICE),
        designer: get(name_id::DESIGNER),
        manufacturer: get(name_id::MANUFACTURER),
        description: get(name_id::DESCRIPTION),
        license: get(name_id::LICENSE),
        license_url: get(name_id::LICENSE_URL),
        format: format_name(&face, is_collection(data)),
        glyph_count: face.number_of_glyphs(),
    })
}

/// Human-readable outline format.
pub fn format_name(face: &Face, collection: bool) -> String {
    let t = face.tables();
    let base = if t.cff2.is_some() {
        "OpenType (CFF2)"
    } else if t.cff.is_some() {
        "OpenType (CFF)"
    } else if t.glyf.is_some() {
        "TrueType"
    } else {
        "OpenType (bitmap)"
    };
    let variable = if face.is_variable() { "Variable " } else { "" };
    let coll = if collection { " Collection" } else { "" };
    format!("{variable}{base}{coll}")
}

/// Returns the best string for `id`: English Unicode first, then any
/// Unicode record, then Mac Roman.
pub fn name_string(face: &Face, id: u16) -> Option<String> {
    let names: Vec<_> = face.names().into_iter().filter(|n| n.name_id == id).collect();
    let rank = |n: &ttf_parser::name::Name| -> u8 {
        let english = match n.platform_id {
            PlatformId::Windows => n.language_id == 0x0409,
            PlatformId::Macintosh => n.language_id == 0,
            PlatformId::Unicode => true,
            _ => false,
        };
        match (n.is_unicode(), english) {
            (true, true) => 0,
            (false, true) => 1,
            (true, false) => 2,
            (false, false) => 3,
        }
    };
    let mut sorted = names;
    sorted.sort_by_key(rank);
    sorted.iter().find_map(|n| {
        let s = if n.is_unicode() {
            n.to_string()
        } else if n.platform_id == PlatformId::Macintosh && n.encoding_id == 0 {
            Some(mac_roman(n.name))
        } else {
            None
        }?;
        let s = s.trim().trim_matches('\0').to_string();
        (!s.is_empty()).then_some(s)
    })
}

/// Decodes Mac OS Roman.
pub fn mac_roman(bytes: &[u8]) -> String {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø\
¿¡¬√ƒ≈∆«»…\u{A0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{F8FF}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    let high: Vec<char> = HIGH.chars().collect();
    bytes
        .iter()
        .map(|&b| if b < 0x80 { b as char } else { high.get((b - 0x80) as usize).copied().unwrap_or('\u{FFFD}') })
        .collect()
}

/// All Unicode code points mapped by the face's cmap, sorted.
pub fn coverage(face: &Face) -> Vec<char> {
    let mut out = Vec::new();
    if let Some(cmap) = face.tables().cmap {
        for sub in cmap.subtables {
            if !sub.is_unicode() {
                continue;
            }
            sub.codepoints(|cp| {
                if let Some(c) = char::from_u32(cp) {
                    if !c.is_control() && sub.glyph_index(cp).is_some_and(|g| g.0 != 0) {
                        out.push(c);
                    }
                }
            });
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub const PANGRAM: &str = "The quick brown fox jumps over the lazy dog.";
pub const ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789";

/// Whether `face_chars` (sorted) covers at least 90% of the non-space
/// characters of `text`.
pub fn covers(face_chars: &[char], text: &str) -> bool {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.is_empty() {
        return true;
    }
    let hit = chars.iter().filter(|c| face_chars.binary_search(c).is_ok()).count();
    hit * 10 >= chars.len() * 9
}

/// Picks the default preview sentence: the pangram if covered, otherwise
/// a run of characters the font actually has (skipping ASCII punctuation).
pub fn sample_text(face_chars: &[char]) -> String {
    if face_chars.is_empty() || covers(face_chars, PANGRAM) {
        return PANGRAM.to_string();
    }
    let interesting: Vec<char> =
        face_chars.iter().copied().filter(|c| c.is_alphanumeric() || (*c as u32) > 0x2000).collect();
    let pool = if interesting.is_empty() { face_chars.to_vec() } else { interesting };
    pool.iter().take(36).collect()
}

/// Fallback facts about a face when the file cannot be parsed (e.g. Type 1),
/// as reported by fontconfig.
pub struct FaceFacts<'a> {
    pub family: &'a str,
    pub style: &'a str,
    pub format: &'a str,
    pub path: &'a str,
    pub size: Option<u64>,
}

/// Rows for the Info view, in display order, empty values omitted.
pub fn info_rows(info: Option<&FaceInfo>, facts: &FaceFacts, chars: usize) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    match info {
        Some(i) => {
            rows.extend([
                ("Family", i.family.clone()),
                ("Style", i.style.clone()),
                ("Full name", i.full_name.clone()),
                ("Version", i.version.clone()),
                ("Copyright", i.copyright.clone()),
                ("Designer", i.designer.clone()),
                ("Manufacturer", i.manufacturer.clone()),
                ("Description", i.description.clone()),
                ("License", i.license.clone()),
                ("License URL", i.license_url.clone()),
                ("Glyphs", i.glyph_count.to_string()),
                ("Characters", if chars > 0 { chars.to_string() } else { String::new() }),
                ("Format", i.format.clone()),
            ]);
        }
        None => rows.extend([
            ("Family", facts.family.to_string()),
            ("Style", facts.style.to_string()),
            ("Format", facts.format.to_string()),
        ]),
    }
    rows.push(("File", facts.path.to_string()));
    if let Some(sz) = facts.size {
        rows.push(("Size", human_size(sz)));
    }
    rows.retain(|(_, v)| !v.trim().is_empty());
    rows
}

/// Formats a byte count like "1.2 MB" (SI units, as GNOME does).
pub fn human_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let units = ["kB", "MB", "GB", "TB"];
    let mut v = bytes as f64 / 1000.0;
    let mut u = 0;
    while v >= 1000.0 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    format!("{v:.1} {}", units[u])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Finds a font on the system by file name, for read-only tests.
    pub fn system_font(names: &[&str]) -> Option<PathBuf> {
        let roots = ["/usr/share/fonts", "/usr/local/share/fonts"];
        for root in roots {
            for name in names {
                if let Some(p) = find(Path::new(root), name, 4) {
                    return Some(p);
                }
            }
        }
        None
    }

    fn find(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = vec![];
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                subdirs.push(p);
            } else if p.file_name().is_some_and(|f| f == name) {
                return Some(p);
            }
        }
        if depth == 0 {
            return None;
        }
        subdirs.iter().find_map(|d| find(d, name, depth - 1))
    }

    pub fn any_ttf() -> Option<PathBuf> {
        system_font(&["DejaVuSans.ttf", "LiberationSans-Regular.ttf", "NotoSans-Regular.ttf", "Cantarell-Regular.otf"])
    }

    #[test]
    fn reads_name_table_of_system_font() {
        let Some(path) = system_font(&["DejaVuSans.ttf"]) else {
            eprintln!("DejaVuSans.ttf not present; skipping");
            return;
        };
        let faces = read_file(&path).unwrap();
        assert_eq!(faces.len(), 1);
        let f = &faces[0];
        assert_eq!(f.family, "DejaVu Sans");
        assert_eq!(f.style, "Book");
        assert!(f.version.starts_with("Version"), "{}", f.version);
        assert!(f.copyright.contains("Bitstream"), "{}", f.copyright);
        assert_eq!(f.format, "TrueType");
        assert!(f.glyph_count > 1000);
    }

    #[test]
    fn reads_any_font_generic() {
        let Some(path) = any_ttf() else {
            eprintln!("no system font found; skipping");
            return;
        };
        let faces = read_file(&path).unwrap();
        assert!(!faces[0].family.is_empty());
        assert!(!faces[0].style.is_empty());
        let data = std::fs::read(&path).unwrap();
        let face = Face::parse(&data, 0).unwrap();
        let cov = coverage(&face);
        assert!(cov.binary_search(&'A').is_ok());
        assert!(cov.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(sample_text(&cov), PANGRAM);
    }

    #[test]
    fn reads_collection_faces() {
        let Some(path) = system_font(&["NotoSansCJK-Regular.ttc", "NotoSerifCJK-Regular.ttc", "NotoSansCJK.ttc"]) else {
            eprintln!("no TTC font present; skipping");
            return;
        };
        let faces = read_file(&path).unwrap();
        assert!(faces.len() > 1);
        assert!(faces.iter().enumerate().all(|(i, f)| f.index == i as u32));
        assert!(faces[0].format.ends_with("Collection"));
        assert!(faces.iter().all(|f| !f.family.is_empty()));
    }

    #[test]
    fn rejects_garbage() {
        assert!(face_count(b"definitely not a font").is_none());
        assert!(matches!(read_data(b"nope"), Err(Error::Parse)));
        assert!(matches!(read_file(Path::new("/nonexistent/x.ttf")), Err(Error::Io(_))));
    }

    #[test]
    fn info_rows_order_and_fallback() {
        let info = FaceInfo {
            family: "Foo".into(),
            style: "Bold".into(),
            version: "Version 1.0".into(),
            format: "TrueType".into(),
            glyph_count: 42,
            ..Default::default()
        };
        let facts = FaceFacts { family: "x", style: "y", format: "Type 1", path: "/f.ttf", size: Some(1500) };
        let rows = info_rows(Some(&info), &facts, 10);
        let keys: Vec<&str> = rows.iter().map(|r| r.0).collect();
        assert_eq!(keys, ["Family", "Style", "Version", "Glyphs", "Characters", "Format", "File", "Size"]);
        assert_eq!(rows[0].1, "Foo");
        assert_eq!(rows[7].1, "1.5 kB");
        let rows = info_rows(None, &facts, 0);
        let keys: Vec<&str> = rows.iter().map(|r| r.0).collect();
        assert_eq!(keys, ["Family", "Style", "Format", "File", "Size"]);
        assert_eq!(rows[2].1, "Type 1");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(999), "999 bytes");
        assert_eq!(human_size(1000), "1.0 kB");
        assert_eq!(human_size(20_500_000), "20.5 MB");
    }

    #[test]
    fn mac_roman_decoding() {
        assert_eq!(mac_roman(b"Abc"), "Abc");
        assert_eq!(mac_roman(&[0x80, 0xA9, 0xAA]), "Ä©™");
        assert_eq!(mac_roman(&[0xF0]), "\u{F8FF}");
        assert_eq!(mac_roman(&[0xFF]), "ˇ");
        let all: Vec<u8> = (0x80..=0xFF).collect();
        let decoded = mac_roman(&all);
        assert_eq!(decoded.chars().count(), 128);
        assert!(!decoded.contains('\u{FFFD}'));
    }

    #[test]
    fn sample_text_falls_back_to_covered_chars() {
        let cjk: Vec<char> = ('一'..='丿').collect();
        let s = sample_text(&cjk);
        assert_ne!(s, PANGRAM);
        assert!(s.chars().all(|c| cjk.contains(&c)));
        assert_eq!(s.chars().count(), 36);
        assert_eq!(sample_text(&[]), PANGRAM);
        let mut latin: Vec<char> = PANGRAM.chars().collect();
        latin.sort_unstable();
        latin.dedup();
        assert!(covers(&latin, PANGRAM));
        assert_eq!(sample_text(&latin), PANGRAM);
    }
}
