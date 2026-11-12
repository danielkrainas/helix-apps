//! Enumerating installed fonts.
//!
//! We ask fontconfig itself (via the `fc-list` tool) rather than parsing
//! fonts.conf in Rust: fontconfig's configuration language (includes,
//! `<dir prefix=...>`, `<selectfont>` accept/reject rules, sysroots) is large
//! and a re-implementation would list a different set of fonts than every
//! other application. `fc-list` also uses fontconfig's cache, so it is fast.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Format passed to `fc-list -f`: one tab-separated record per line.
pub const FC_FORMAT: &str =
    "%{file}\\t%{index}\\t%{family[0]}\\t%{style[0]}\\t%{fontformat}\\t%{weight}\\t%{slant}\\t%{namedinstance}\\n";

/// One face as reported by fontconfig.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceEntry {
    pub file: PathBuf,
    pub index: u32,
    pub family: String,
    pub style: String,
    pub format: String,
    pub weight: f64,
    pub slant: f64,
}

/// A family and its faces, sorted by weight then slant.
#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    pub name: String,
    pub faces: Vec<FaceEntry>,
}

impl Family {
    /// Whether all whitespace-separated terms of `query` occur
    /// (case-insensitively) in the family name or one of its style names.
    pub fn matches(&self, query: &str) -> bool {
        let hay = {
            let mut s = self.name.to_lowercase();
            for f in &self.faces {
                s.push(' ');
                s.push_str(&f.style.to_lowercase());
            }
            s
        };
        query.split_whitespace().all(|t| hay.contains(&t.to_lowercase()))
    }
}

/// Parses one number that fontconfig may print as a range `[a b]` or an
/// integer/float.
fn parse_num(s: &str) -> f64 {
    let s = s.trim().trim_start_matches('[');
    s.split(|c: char| c.is_whitespace() || c == ']').next().and_then(|x| x.parse().ok()).unwrap_or(80.0)
}

/// Parses `fc-list -f FC_FORMAT` output. Named instances of variable fonts
/// are skipped (the variable base face is kept).
pub fn parse_fc_list(output: &str) -> Vec<FaceEntry> {
    output
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 4 || f[0].is_empty() || f[2].trim().is_empty() {
                return None;
            }
            if f.get(7).is_some_and(|s| s.trim().eq_ignore_ascii_case("true")) {
                return None;
            }
            let index: u32 = f[1].trim().parse().unwrap_or(0);
            Some(FaceEntry {
                file: PathBuf::from(f[0]),
                index: index & 0xFFFF,
                family: unescape(f[2].trim()),
                style: f.get(3).map(|s| unescape(s.trim())).filter(|s| !s.is_empty()).unwrap_or_else(|| "Regular".into()),
                format: f.get(4).map(|s| s.trim().to_string()).unwrap_or_default(),
                weight: f.get(5).map(|s| parse_num(s)).unwrap_or(80.0),
                slant: f.get(6).map(|s| parse_num(s)).unwrap_or(0.0),
            })
        })
        .collect()
}

/// fontconfig escapes some characters with a backslash in formatted output.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Groups faces by family name, dropping duplicates (same file + index),
/// sorting families case-insensitively and faces by weight/slant/style.
pub fn group_families(faces: Vec<FaceEntry>) -> Vec<Family> {
    let mut map: BTreeMap<String, Family> = BTreeMap::new();
    for face in faces {
        let key = face.family.to_lowercase();
        let fam = map.entry(key).or_insert_with(|| Family { name: face.family.clone(), faces: vec![] });
        if !fam.faces.iter().any(|f| f.file == face.file && f.index == face.index) {
            fam.faces.push(face);
        }
    }
    let mut out: Vec<Family> = map.into_values().collect();
    for fam in &mut out {
        fam.faces.sort_by(|a, b| {
            a.weight
                .total_cmp(&b.weight)
                .then(a.slant.total_cmp(&b.slant))
                .then_with(|| a.style.cmp(&b.style))
                .then_with(|| a.file.cmp(&b.file))
        });
    }
    out
}

/// Index of the face to show first: the one closest to Regular/upright.
pub fn default_face(fam: &Family) -> usize {
    fam.faces
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let score = |f: &FaceEntry| (f.weight - 80.0).abs() + f.slant.abs() * 2.0;
            score(a).total_cmp(&score(b))
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Runs `fc-list`; `None` if it is unavailable or fails.
pub fn run_fc_list() -> Option<Vec<FaceEntry>> {
    let out = Command::new("fc-list").arg("-f").arg(FC_FORMAT).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_fc_list(&String::from_utf8_lossy(&out.stdout)))
}

/// Standard font directories used when fontconfig is unavailable.
pub fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![user_fonts_dir()];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".fonts"));
    }
    dirs.push("/usr/local/share/fonts".into());
    dirs.push("/usr/share/fonts".into());
    dirs
}

/// `$XDG_DATA_HOME/fonts`.
pub fn user_fonts_dir() -> PathBuf {
    gtk::glib::user_data_dir().join("fonts")
}

pub fn is_font_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc"))
}

/// Recursively scans `dirs` and reads faces with ttf-parser.
pub fn scan_dirs(dirs: &[PathBuf]) -> Vec<FaceEntry> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, u32)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 8 {
                    stack.push((p, depth + 1));
                }
            } else if is_font_file(&p) {
                if let Ok(faces) = crate::fontinfo::read_file(&p) {
                    for f in faces {
                        out.push(FaceEntry {
                            file: p.clone(),
                            index: f.index,
                            weight: style_weight(&f.style),
                            slant: if f.style.to_lowercase().contains("italic") || f.style.to_lowercase().contains("oblique") { 100.0 } else { 0.0 },
                            family: f.family,
                            style: f.style,
                            format: f.format,
                        });
                    }
                }
            }
        }
    }
    out
}

/// Approximate fontconfig weight from a style name.
fn style_weight(style: &str) -> f64 {
    let s = style.to_lowercase();
    [
        ("thin", 0.0),
        ("extralight", 40.0),
        ("light", 50.0),
        ("medium", 100.0),
        ("semibold", 180.0),
        ("extrabold", 205.0),
        ("bold", 200.0),
        ("black", 210.0),
    ]
    .iter()
    .find(|(k, _)| s.contains(k))
    .map(|(_, w)| *w)
    .unwrap_or(80.0)
}

/// Lists installed families, preferring fontconfig.
pub fn list_families() -> Vec<Family> {
    let faces = run_fc_list().filter(|f| !f.is_empty()).unwrap_or_else(|| scan_dirs(&fallback_dirs()));
    group_families(faces)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "/usr/share/fonts/truetype/lato/Lato-Medium.ttf\t0\tLato\tMedium\tTrueType\t100\t0\tFalse
/usr/share/fonts/truetype/lato/Lato-Regular.ttf\t0\tLato\tRegular\tTrueType\t80\t0\tFalse
/usr/share/fonts/truetype/lato/Lato-Italic.ttf\t0\tLato\tItalic\tTrueType\t80\t100\tFalse
/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc\t2\tNoto Serif CJK SC\tRegular\tCFF\t80\t0\tFalse
/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc\t3\tNoto Serif CJK TC\tRegular\tCFF\t80\t0\tFalse
/usr/share/fonts/X11/Type1/lmtti10.pfb\t0\tLMMono10\tItalic\tType 1\t80\t100\tFalse
/usr/share/fonts/truetype/var/Foo[wght].ttf\t65537\tFoo\tBold\tTrueType\t200\t0\tTrue
/usr/share/fonts/truetype/var/Foo[wght].ttf\t0\tFoo\tRegular\tTrueType\t[0 210]\t0\tFalse
/usr/share/fonts/truetype/lato/Lato-Regular.ttf\t0\tlato\tRegular\tTrueType\t80\t0\tFalse
\t0\tBroken\t\t\t\t\t
/x.ttf\t0\t\tRegular\t\t\t\t
garbage line
";

    #[test]
    fn parses_fc_list_records() {
        let faces = parse_fc_list(SAMPLE);
        assert_eq!(faces.len(), 8);
        assert_eq!(faces[0].family, "Lato");
        assert_eq!(faces[0].style, "Medium");
        assert_eq!(faces[0].weight, 100.0);
        assert_eq!(faces[3].index, 2);
        assert_eq!(faces[3].format, "CFF");
        // Named instance skipped, variable base kept with range parsed.
        let foo: Vec<_> = faces.iter().filter(|f| f.family == "Foo").collect();
        assert_eq!(foo.len(), 1);
        assert_eq!(foo[0].weight, 0.0);
    }

    #[test]
    fn groups_and_sorts_families() {
        let fams = group_families(parse_fc_list(SAMPLE));
        let names: Vec<&str> = fams.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Foo", "Lato", "LMMono10", "Noto Serif CJK SC", "Noto Serif CJK TC"]);
        let lato = &fams[1];
        // Duplicate (case-different family, same file+index) removed.
        assert_eq!(lato.faces.len(), 3);
        let styles: Vec<&str> = lato.faces.iter().map(|f| f.style.as_str()).collect();
        assert_eq!(styles, ["Regular", "Italic", "Medium"]);
        assert_eq!(default_face(lato), 0);
    }

    #[test]
    fn search_matches_family_and_style() {
        let fams = group_families(parse_fc_list(SAMPLE));
        let lato = &fams[1];
        assert!(lato.matches(""));
        assert!(lato.matches("lat"));
        assert!(lato.matches("LATO italic"));
        assert!(!lato.matches("lato bold"));
        assert!(!lato.matches("noto"));
    }

    #[test]
    fn unescapes_values() {
        assert_eq!(unescape(r"A\-B\\C"), r"A-B\C");
    }

    #[test]
    fn font_file_extensions() {
        assert!(is_font_file(Path::new("a/b.TTF")));
        assert!(is_font_file(Path::new("x.otc")));
        assert!(!is_font_file(Path::new("x.pfb")));
        assert!(!is_font_file(Path::new("ttf")));
    }

    #[test]
    fn scans_directories_with_ttf_parser() {
        let Some(src) = crate::fontinfo::tests::any_ttf() else {
            eprintln!("no system font; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::copy(&src, sub.join("f.ttf")).unwrap();
        std::fs::write(sub.join("bogus.ttf"), b"not a font").unwrap();
        std::fs::write(sub.join("readme.txt"), b"x").unwrap();
        let faces = scan_dirs(&[dir.path().to_path_buf()]);
        assert_eq!(faces.len(), 1);
        assert!(!faces[0].family.is_empty());
    }

    #[test]
    fn live_fc_list_if_available() {
        let Some(faces) = run_fc_list() else {
            eprintln!("fc-list unavailable; skipping");
            return;
        };
        if faces.is_empty() {
            return;
        }
        assert!(faces.iter().all(|f| !f.family.is_empty() && f.file.is_absolute()));
        let fams = group_families(faces);
        assert!(!fams.is_empty());
        assert!(fams.iter().all(|f| !f.faces.is_empty()));
    }
}
