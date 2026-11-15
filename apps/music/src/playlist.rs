//! User playlists (extended M3U files; XSPF read/write, PLS read), play
//! statistics and smart playlists.

use crate::model::Library;
use helix_app_common::i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SMART_LIMIT: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub name: String,
    pub entries: Vec<PathBuf>,
    /// File backing this playlist.
    pub file: PathBuf,
}

/// Extra information written into `#EXTINF` lines.
pub struct EntryInfo {
    pub seconds: u64,
    pub artist: String,
    pub title: String,
}

pub fn to_m3u(
    name: &str,
    entries: &[PathBuf],
    info: impl Fn(&Path) -> Option<EntryInfo>,
) -> String {
    let mut out = String::from("#EXTM3U\n");
    out.push_str(&format!("#PLAYLIST:{}\n", one_line(name)));
    for p in entries {
        if let Some(i) = info(p) {
            out.push_str(&format!(
                "#EXTINF:{},{} - {}\n",
                i.seconds,
                one_line(&i.artist),
                one_line(&i.title)
            ));
        }
        out.push_str(&p.to_string_lossy());
        out.push('\n');
    }
    out
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Parses M3U/M3U8 text. Relative entries resolve against `base`, `file://`
/// URIs are converted to paths, other URIs are skipped.
pub fn parse_m3u(text: &str, base: &Path) -> (Option<String>, Vec<PathBuf>) {
    let mut name = None;
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        if let Some(n) = line.strip_prefix("#PLAYLIST:") {
            name = Some(n.trim().to_string()).filter(|n| !n.is_empty());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("file://") {
            if let Ok((p, _)) = glib::filename_from_uri(line) {
                entries.push(p);
            }
        } else if line.contains("://") {
            continue;
        } else {
            let p = PathBuf::from(line);
            entries.push(if p.is_absolute() { p } else { base.join(p) });
        }
    }
    (name, entries)
}

/// Playlist file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    M3u,
    Xspf,
    /// Read-only (import).
    Pls,
}

impl Format {
    pub fn of(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "m3u" | "m3u8" => Some(Format::M3u),
            "xspf" => Some(Format::Xspf),
            "pls" => Some(Format::Pls),
            _ => None,
        }
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Characters XML 1.0 cannot carry.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Writes an XSPF playlist (`file://` locations, title/creator/duration).
pub fn to_xspf(
    name: &str,
    entries: &[PathBuf],
    info: impl Fn(&Path) -> Option<EntryInfo>,
) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\">\n");
    out.push_str(&format!(
        "  <title>{}</title>\n  <trackList>\n",
        xml_escape(name)
    ));
    for p in entries {
        out.push_str("    <track>\n");
        out.push_str(&format!(
            "      <location>{}</location>\n",
            xml_escape(&crate::model::path_to_uri(p))
        ));
        if let Some(i) = info(p) {
            out.push_str(&format!("      <title>{}</title>\n", xml_escape(&i.title)));
            out.push_str(&format!(
                "      <creator>{}</creator>\n",
                xml_escape(&i.artist)
            ));
            out.push_str(&format!(
                "      <duration>{}</duration>\n",
                i.seconds * 1000
            ));
        }
        out.push_str("    </track>\n");
    }
    out.push_str("  </trackList>\n</playlist>\n");
    out
}

fn xml_child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Parses XSPF. Relative locations resolve against `base`; non-file
/// locations are skipped.
pub fn parse_xspf(text: &str, base: &Path) -> Result<(Option<String>, Vec<PathBuf>), String> {
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "playlist" {
        return Err("not an XSPF playlist".into());
    }
    let name = xml_child(root, "title")
        .and_then(|t| t.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let mut entries = Vec::new();
    if let Some(list) = xml_child(root, "trackList") {
        for track in list
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "track")
        {
            // A track may list several alternative locations; use the first usable one.
            for loc in track
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "location")
            {
                if let Some(p) = location_to_path(loc.text().unwrap_or("").trim(), base) {
                    entries.push(p);
                    break;
                }
            }
        }
    }
    Ok((name, entries))
}

/// Resolves a playlist entry (path, relative path or `file://` URI).
fn location_to_path(loc: &str, base: &Path) -> Option<PathBuf> {
    if loc.is_empty() {
        return None;
    }
    if loc.starts_with("file:") {
        return glib::filename_from_uri(loc).ok().map(|(p, _)| p);
    }
    if loc.contains("://") {
        return None;
    }
    // Relative URI references are percent-encoded.
    let decoded = glib::Uri::unescape_string(loc, None::<&str>)
        .map(|s| s.to_string())
        .unwrap_or_else(|| loc.to_string());
    let p = PathBuf::from(decoded);
    Some(if p.is_absolute() { p } else { base.join(p) })
}

/// Parses a PLS playlist (`FileN=` entries, in N order).
pub fn parse_pls(text: &str, base: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<(u32, PathBuf)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(n) = key
            .trim()
            .strip_prefix("File")
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let value = value.trim();
        let path = if value.starts_with("file:") || value.contains("://") {
            location_to_path(value, base)
        } else {
            let p = PathBuf::from(value);
            Some(if p.is_absolute() { p } else { base.join(p) })
        };
        if let Some(p) = path {
            entries.push((n, p));
        }
    }
    entries.sort_by_key(|e| e.0);
    entries.into_iter().map(|e| e.1).collect()
}

/// Reads a playlist file of any supported format: (name, entries).
pub fn read_any(file: &Path) -> std::io::Result<(Option<String>, Vec<PathBuf>)> {
    let text = std::fs::read_to_string(file)?;
    let base = file.parent().unwrap_or(Path::new("/"));
    match Format::of(file) {
        Some(Format::Xspf) => parse_xspf(&text, base).map_err(std::io::Error::other),
        Some(Format::Pls) => Ok((None, parse_pls(&text, base))),
        _ => Ok(parse_m3u(&text, base)),
    }
}

fn stem_name(file: &Path) -> String {
    file.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Copies a foreign playlist into `dir` as a new M3U playlist, keeping only
/// entries `keep` accepts. Returns `None` when nothing is left.
pub fn import(
    name: Option<String>,
    source: &Path,
    entries: Vec<PathBuf>,
    dir: &Path,
    keep: impl Fn(&Path) -> bool,
) -> Option<Playlist> {
    let entries: Vec<PathBuf> = entries.into_iter().filter(|p| keep(p)).collect();
    if entries.is_empty() {
        return None;
    }
    let name = name.unwrap_or_else(|| stem_name(source));
    Some(Playlist {
        file: new_file_for(dir, &name),
        name,
        entries,
    })
}

/// Writes `pl`'s entries to `dest` in the format its extension names
/// (XSPF for `.xspf`, M3U otherwise).
pub fn export(
    pl: &Playlist,
    dest: &Path,
    info: impl Fn(&Path) -> Option<EntryInfo>,
) -> std::io::Result<()> {
    let data = match Format::of(dest) {
        Some(Format::Xspf) => to_xspf(&pl.name, &pl.entries, info),
        _ => to_m3u(&pl.name, &pl.entries, info),
    };
    write_atomic(dest, data.as_bytes())
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .chars()
        .take(100)
        .collect();
    if s.is_empty() {
        tr("Playlist").into()
    } else {
        s
    }
}

/// A file name in `dir` for a new playlist called `name`.
pub fn new_file_for(dir: &Path, name: &str) -> PathBuf {
    let base = sanitize(name);
    let mut candidate = dir.join(format!("{base}.m3u"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} {n}.m3u"));
        n += 1;
    }
    candidate
}

pub fn load(file: &Path) -> std::io::Result<Playlist> {
    let (name, entries) = read_any(file)?;
    let name = name.unwrap_or_else(|| stem_name(file));
    Ok(Playlist {
        name,
        entries,
        file: file.to_path_buf(),
    })
}

pub fn load_all(dir: &Path) -> Vec<Playlist> {
    let mut out: Vec<Playlist> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(Format::of(p), Some(Format::M3u | Format::Xspf)))
        .filter_map(|p| load(&p).ok())
        .collect();
    out.sort_by_key(|p| crate::model::sort_key(&p.name));
    out
}

/// Saves `pl` to its file, keeping the file's format (M3U or XSPF).
pub fn save(pl: &Playlist, info: impl Fn(&Path) -> Option<EntryInfo>) -> std::io::Result<()> {
    export(pl, &pl.file, info)
}

pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Per-track play statistics and favourites.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stat {
    #[serde(default)]
    pub plays: u32,
    #[serde(default)]
    pub last_played: Option<i64>,
    #[serde(default)]
    pub favorite: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub tracks: HashMap<PathBuf, Stat>,
}

impl Stats {
    pub fn load(file: &Path) -> Self {
        std::fs::read(file)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        let data = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        write_atomic(file, &data)
    }

    pub fn get(&self, path: &Path) -> Option<&Stat> {
        self.tracks.get(path)
    }

    pub fn plays(&self, path: &Path) -> u32 {
        self.get(path).map_or(0, |s| s.plays)
    }

    pub fn is_favorite(&self, path: &Path) -> bool {
        self.get(path).is_some_and(|s| s.favorite)
    }

    pub fn set_favorite(&mut self, path: &Path, on: bool) {
        self.tracks.entry(path.to_path_buf()).or_default().favorite = on;
        self.tracks.retain(|_, s| *s != Stat::default());
    }

    pub fn record_play(&mut self, path: &Path, now: i64) {
        let s = self.tracks.entry(path.to_path_buf()).or_default();
        s.plays += 1;
        s.last_played = Some(now);
    }
}

/// Whether a playback has lasted long enough to count as a play:
/// half the track or four minutes, whichever comes first.
pub fn counts_as_play(played_ms: u64, duration_ms: u64) -> bool {
    if duration_ms == 0 {
        return played_ms >= 30_000;
    }
    played_ms >= (duration_ms / 2).min(240_000)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Smart {
    Favorites,
    MostPlayed,
    RecentlyPlayed,
    RecentlyAdded,
}

impl Smart {
    pub const ALL: [Smart; 4] = [
        Smart::Favorites,
        Smart::MostPlayed,
        Smart::RecentlyPlayed,
        Smart::RecentlyAdded,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Smart::Favorites => tr("Favorite Songs"),
            Smart::MostPlayed => tr("Most Played"),
            Smart::RecentlyPlayed => tr("Recently Played"),
            Smart::RecentlyAdded => tr("Recently Added"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Smart::Favorites => "starred-symbolic",
            Smart::MostPlayed => "view-continuous-symbolic",
            Smart::RecentlyPlayed => "document-open-recent-symbolic",
            Smart::RecentlyAdded => "list-add-symbolic",
        }
    }

    /// Track indices of `lib` in this smart playlist.
    pub fn tracks(self, lib: &Library, stats: &Stats) -> Vec<usize> {
        let stat = |i: usize| stats.get(&lib.tracks[i].path);
        match self {
            Smart::Favorites => lib
                .all_tracks_by_album()
                .into_iter()
                .filter(|&i| stat(i).is_some_and(|s| s.favorite))
                .collect(),
            Smart::MostPlayed => {
                let mut v: Vec<usize> = (0..lib.tracks.len())
                    .filter(|&i| stat(i).is_some_and(|s| s.plays > 0))
                    .collect();
                v.sort_by_key(|&i| {
                    (
                        std::cmp::Reverse(stat(i).map_or(0, |s| s.plays)),
                        std::cmp::Reverse(stat(i).and_then(|s| s.last_played)),
                    )
                });
                v.truncate(SMART_LIMIT);
                v
            }
            Smart::RecentlyPlayed => {
                let mut v: Vec<usize> = (0..lib.tracks.len())
                    .filter(|&i| stat(i).and_then(|s| s.last_played).is_some())
                    .collect();
                v.sort_by_key(|&i| std::cmp::Reverse(stat(i).and_then(|s| s.last_played)));
                v.truncate(SMART_LIMIT);
                v
            }
            Smart::RecentlyAdded => {
                let mut v = lib.all_tracks_by_album();
                // Stable sort keeps album order among files added together.
                v.sort_by_key(|&i| std::cmp::Reverse(lib.tracks[i].added));
                v.truncate(SMART_LIMIT);
                v
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{track, Library};

    #[test]
    fn m3u_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![
            PathBuf::from("/music/a b/01 – Ünïcode.flac"),
            PathBuf::from("/music/x.mp3"),
        ];
        let pl = Playlist {
            name: "Road\ntrip".into(),
            entries: entries.clone(),
            file: new_file_for(dir.path(), "Road/trip"),
        };
        assert_eq!(pl.file.file_name().unwrap(), "Road_trip.m3u");
        save(&pl, |p| {
            Some(EntryInfo {
                seconds: 61,
                artist: "A".into(),
                title: p.to_string_lossy().into_owned(),
            })
        })
        .unwrap();
        let text = std::fs::read_to_string(&pl.file).unwrap();
        assert!(text.starts_with("#EXTM3U\n#PLAYLIST:Road trip\n#EXTINF:61,A - /music/a b/01"));
        let back = load(&pl.file).unwrap();
        assert_eq!(back.name, "Road trip");
        assert_eq!(back.entries, entries);
        assert_eq!(load_all(dir.path()), vec![back]);
        // A second playlist with the same name gets a distinct file.
        assert_eq!(
            new_file_for(dir.path(), "Road/trip").file_name().unwrap(),
            "Road_trip 2.m3u"
        );
    }

    #[test]
    fn parses_foreign_m3u() {
        let text = "\u{feff}#EXTM3U\r\n#EXTINF:10,x\r\nsub/song.ogg\r\nfile:///abs/s%20p.mp3\r\nhttp://radio/stream\r\n\r\n/abs/b.flac\r\n";
        let (name, entries) = parse_m3u(text, Path::new("/lists"));
        assert_eq!(name, None);
        assert_eq!(
            entries,
            [
                PathBuf::from("/lists/sub/song.ogg"),
                PathBuf::from("/abs/s p.mp3"),
                PathBuf::from("/abs/b.flac")
            ]
        );
    }

    #[test]
    fn m3u_edge_cases() {
        // Names survive, CR/LF in titles are flattened, EXTINF is optional.
        let entries = vec![PathBuf::from("/m/a.ogg")];
        let text = to_m3u("Mix", &entries, |_| None);
        assert_eq!(text, "#EXTM3U\n#PLAYLIST:Mix\n/m/a.ogg\n");
        let text = to_m3u("Mix", &entries, |_| {
            Some(EntryInfo {
                seconds: 5,
                artist: "A\r\nB".into(),
                title: "T".into(),
            })
        });
        assert!(text.contains("#EXTINF:5,A  B - T\n"));
        // A plain (non-extended) M3U with relative paths.
        let (name, e) = parse_m3u("a.mp3\n../b.mp3\n", Path::new("/x/y"));
        assert_eq!(name, None);
        assert_eq!(
            e,
            [PathBuf::from("/x/y/a.mp3"), PathBuf::from("/x/y/../b.mp3")]
        );
        assert_eq!(Format::of(Path::new("a.M3U8")), Some(Format::M3u));
        assert_eq!(Format::of(Path::new("a.txt")), None);
    }

    #[test]
    fn xspf_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![
            PathBuf::from("/music/R&B <live>/01 \"x\".flac"),
            PathBuf::from("/music/ü.mp3"),
        ];
        let pl = Playlist {
            name: "Rock & Roll".into(),
            entries: entries.clone(),
            file: dir.path().join("rr.xspf"),
        };
        save(&pl, |_| {
            Some(EntryInfo {
                seconds: 2,
                artist: "A<B".into(),
                title: "T".into(),
            })
        })
        .unwrap();
        let text = std::fs::read_to_string(&pl.file).unwrap();
        assert!(text.contains("<title>Rock &amp; Roll</title>"));
        assert!(text.contains("<duration>2000</duration>"));
        let back = load(&pl.file).unwrap();
        assert_eq!(
            (back.name.as_str(), back.entries.clone()),
            ("Rock & Roll", entries)
        );
        assert_eq!(load_all(dir.path()).len(), 1);
    }

    #[test]
    fn parses_foreign_xspf_and_pls() {
        let xspf = r#"<?xml version="1.0"?>
<playlist version="1" xmlns="http://xspf.org/ns/0/">
  <trackList>
    <track><location>http://radio/x</location><location>file:///abs/a%20b.ogg</location></track>
    <track><location>sub/c%20d.mp3</location></track>
    <track><title>no location</title></track>
  </trackList>
</playlist>"#;
        let (name, e) = parse_xspf(xspf, Path::new("/lists")).unwrap();
        assert_eq!(name, None);
        assert_eq!(
            e,
            [
                PathBuf::from("/abs/a b.ogg"),
                PathBuf::from("/lists/sub/c d.mp3")
            ]
        );
        assert!(parse_xspf("<html/>", Path::new("/")).is_err());

        let pls = "[playlist]\nFile2=/m/two.mp3\nTitle2=Two\nFile1=one.mp3\nFile3=http://radio\nNumberOfEntries=3\nVersion=2\n";
        assert_eq!(
            parse_pls(pls, Path::new("/m")),
            [PathBuf::from("/m/one.mp3"), PathBuf::from("/m/two.mp3")]
        );
    }

    #[test]
    fn import_and_export() {
        let src = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        let f = src.path().join("Party.pls");
        std::fs::write(&f, "[playlist]\nFile1=/m/a.mp3\nFile2=/elsewhere/z.mp4\n").unwrap();
        let (name, entries) = read_any(&f).unwrap();
        let pl = import(name, &f, entries, lib.path(), |p| p.starts_with("/m")).unwrap();
        assert_eq!(
            (pl.name.as_str(), pl.entries.clone()),
            ("Party", vec![PathBuf::from("/m/a.mp3")])
        );
        assert_eq!(pl.file, lib.path().join("Party.m3u"));
        save(&pl, |_| None).unwrap();
        assert!(import(None, &f, vec![PathBuf::from("/x")], lib.path(), |_| false).is_none());

        let out = src.path().join("out.xspf");
        export(&pl, &out, |_| None).unwrap();
        assert_eq!(
            read_any(&out).unwrap(),
            (Some("Party".into()), vec![PathBuf::from("/m/a.mp3")])
        );
        let out = src.path().join("out.m3u8");
        export(&pl, &out, |_| None).unwrap();
        assert_eq!(
            read_any(&out).unwrap(),
            (Some("Party".into()), vec![PathBuf::from("/m/a.mp3")])
        );
    }

    #[test]
    fn stats_and_smart_playlists() {
        let mut tracks = vec![
            track("/m/1", "one", Some("A"), Some("X")),
            track("/m/2", "two", Some("A"), Some("X")),
            track("/m/3", "three", Some("A"), Some("X")),
        ];
        tracks[0].added = 10;
        tracks[1].added = 30;
        tracks[2].added = 20;
        let lib = Library::new(tracks);
        let mut st = Stats::default();
        st.record_play(Path::new("/m/2"), 100);
        st.record_play(Path::new("/m/2"), 200);
        st.record_play(Path::new("/m/3"), 300);
        st.set_favorite(Path::new("/m/1"), true);
        let titles = |v: Vec<usize>| {
            v.into_iter()
                .map(|i| lib.tracks[i].title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(Smart::MostPlayed.tracks(&lib, &st)),
            ["two", "three"]
        );
        assert_eq!(
            titles(Smart::RecentlyPlayed.tracks(&lib, &st)),
            ["three", "two"]
        );
        assert_eq!(titles(Smart::Favorites.tracks(&lib, &st)), ["one"]);
        assert_eq!(
            titles(Smart::RecentlyAdded.tracks(&lib, &st)),
            ["two", "three", "one"]
        );

        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("stats.json");
        st.save(&f).unwrap();
        let back = Stats::load(&f);
        assert_eq!(back.plays(Path::new("/m/2")), 2);
        assert!(back.is_favorite(Path::new("/m/1")));
        st.set_favorite(Path::new("/m/1"), false);
        assert!(
            st.get(Path::new("/m/1")).is_none(),
            "empty stats are pruned"
        );
    }

    #[test]
    fn play_counting_threshold() {
        assert!(!counts_as_play(59_000, 120_000));
        assert!(counts_as_play(60_000, 120_000));
        assert!(counts_as_play(240_000, 1_200_000));
        assert!(!counts_as_play(10_000, 0));
    }
}
