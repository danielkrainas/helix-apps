//! The music indexer: walks library folders, reads tags with `lofty`,
//! extracts cover art and keeps a JSON index that is rescanned incrementally.

use crate::model::Track;
use crate::playlist::write_atomic;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const INDEX_VERSION: u32 = 1;
const COVER_SIZE: i32 = 256;

pub const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "ogg", "oga", "opus", "m4a", "mp4", "aac", "wav", "wv", "ape", "aif", "aiff", "mpc", "spx"];

const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album"];
const COVER_EXTS: &[&str] = &["jpg", "jpeg", "png"];

#[derive(Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    tracks: Vec<Track>,
}

pub fn load(file: &Path) -> Vec<Track> {
    std::fs::read(file)
        .ok()
        .and_then(|d| serde_json::from_slice::<IndexFile>(&d).ok())
        .filter(|f| f.version == INDEX_VERSION)
        .map(|f| f.tracks)
        .unwrap_or_default()
}

pub fn save(file: &Path, tracks: &[Track]) -> std::io::Result<()> {
    #[derive(Serialize)]
    struct Out<'a> {
        version: u32,
        tracks: &'a [Track],
    }
    let data = serde_json::to_vec(&Out { version: INDEX_VERSION, tracks }).map_err(std::io::Error::other)?;
    write_atomic(file, &data)
}

pub fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Every directory visited (for file monitors).
    pub dirs: Vec<PathBuf>,
}

impl ScanReport {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

struct FileInfo {
    path: PathBuf,
    mtime: i64,
    size: u64,
}

fn mtime_of(md: &std::fs::Metadata) -> i64 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

fn is_hidden(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

fn walk(dir: &Path, files: &mut Vec<FileInfo>, dirs: &mut Vec<PathBuf>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    dirs.push(dir.to_path_buf());
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        if is_hidden(&path) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&path, files, dirs, depth + 1);
        } else if is_audio(&path) {
            // Symlinked files are followed, symlinked directories are not.
            let Ok(md) = std::fs::metadata(&path) else { continue };
            if md.is_file() {
                files.push(FileInfo { mtime: mtime_of(&md), size: md.len(), path });
            }
        }
    }
}

/// Scans `roots`, reusing entries of `previous` whose mtime and size are
/// unchanged. Covers are written to `covers` (if given).
pub fn scan(roots: &[PathBuf], previous: Vec<Track>, covers: Option<&Path>, now: i64) -> (Vec<Track>, ScanReport) {
    let mut files = Vec::new();
    let mut report = ScanReport::default();
    let mut roots: Vec<&PathBuf> = roots.iter().collect();
    roots.sort();
    roots.dedup();
    for (i, root) in roots.iter().enumerate() {
        // Skip roots nested in another root.
        if roots[..i].iter().any(|r| root.starts_with(r)) {
            continue;
        }
        walk(root, &mut files, &mut report.dirs, 0);
    }
    let mut old: HashMap<PathBuf, Track> = previous.into_iter().map(|t| (t.path.clone(), t)).collect();
    let mut folder_covers: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let prev = old.remove(&f.path);
        if let Some(p) = &prev {
            let cover_ok = match (&p.cover, covers) {
                (Some(key), Some(dir)) => cover_path(dir, key).exists(),
                _ => true,
            };
            if p.mtime == f.mtime && p.size == f.size && cover_ok {
                report.unchanged += 1;
                out.push(prev.unwrap());
                continue;
            }
        }
        let mut t = read_track(&f.path, covers, &mut folder_covers);
        t.mtime = f.mtime;
        t.size = f.size;
        match prev {
            Some(p) => {
                t.added = p.added;
                report.updated += 1;
            }
            None => {
                t.added = now;
                report.added += 1;
            }
        }
        out.push(t);
    }
    report.removed = old.len();
    (out, report)
}

fn clean(s: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    s.map(|s| s.trim().trim_matches('\0').to_string()).filter(|s| !s.is_empty())
}

fn parse_number(s: &str) -> Option<u32> {
    s.split('/').next().and_then(|n| n.trim().parse().ok()).filter(|n| *n > 0)
}

/// Reads one file's tags. Unreadable files still produce a track named after
/// the file.
pub fn read_track(path: &Path, covers: Option<&Path>, folder_covers: &mut HashMap<PathBuf, Option<String>>) -> Track {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut t = Track { path: path.to_path_buf(), title: stem, ..Default::default() };
    let mut embedded_cover = None;
    let mut title = None;
    if let Ok(file) = lofty::read_from_path(path) {
        t.duration_ms = file.properties().duration().as_millis() as u64;
        // Prefer the primary tag but fill gaps from the others.
        let mut tags: Vec<&lofty::tag::Tag> = file.primary_tag().into_iter().collect();
        tags.extend(file.tags().iter().filter(|t| Some(t.tag_type()) != file.primary_tag().map(|p| p.tag_type())));
        for tag in tags {
            title = title.or_else(|| clean(tag.title()));
            t.artist = t.artist.take().or_else(|| clean(tag.artist()));
            t.album = t.album.take().or_else(|| clean(tag.album()));
            t.genre = t.genre.take().or_else(|| clean(tag.genre()));
            t.album_artist = t.album_artist.take().or_else(|| clean(tag.get_string(ItemKey::AlbumArtist).map(Into::into)));
            t.track = t.track.or_else(|| tag.track().filter(|n| *n > 0));
            t.disc = t.disc.or_else(|| tag.disk().filter(|n| *n > 0));
            t.year = t.year.or_else(|| tag.date().map(|d| d.year as u32).filter(|y| *y > 0)).or_else(|| {
                tag.get_string(ItemKey::Year).and_then(|y| y.get(..4)).and_then(parse_number)
            });
            if t.track.is_none() {
                t.track = tag.get_string(ItemKey::TrackNumber).and_then(parse_number);
            }
            if let Some(c) = tag.get_string(ItemKey::FlagCompilation) {
                t.compilation |= matches!(c.trim(), "1" | "true" | "True");
            }
            if embedded_cover.is_none() {
                embedded_cover = pick_picture(tag.pictures()).map(|p| p.data().to_vec());
            }
        }
    }
    if let Some(v) = title {
        t.title = v;
    }
    if let Some(dir) = covers {
        t.cover = match embedded_cover {
            Some(data) => store_cover(dir, &data),
            None => {
                let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
                folder_covers.entry(folder.clone()).or_insert_with(|| folder_cover(&folder, dir)).clone()
            }
        };
    }
    t
}

fn pick_picture(pics: &[Picture]) -> Option<&Picture> {
    pics.iter().find(|p| p.pic_type() == PictureType::CoverFront).or_else(|| pics.first())
}

/// Path of a cached cover.
pub fn cover_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.png"))
}

/// FNV-1a: stable across runs and Rust versions.
pub fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Finds a folder image such as `cover.jpg`.
pub fn find_folder_image(folder: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(folder).ok()?;
    let mut found: Vec<(usize, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?.to_ascii_lowercase();
            let ext = p.extension()?.to_str()?.to_ascii_lowercase();
            let rank = COVER_NAMES.iter().position(|n| *n == stem)?;
            COVER_EXTS.contains(&ext.as_str()).then_some((rank, p))
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|(_, p)| p)
}

fn folder_cover(folder: &Path, dir: &Path) -> Option<String> {
    let img = find_folder_image(folder)?;
    let data = std::fs::read(img).ok()?;
    store_cover(dir, &data)
}

/// Scales image bytes to a cover thumbnail in the cache; returns its key.
pub fn store_cover(dir: &Path, data: &[u8]) -> Option<String> {
    let key = format!("{:016x}", fnv1a(data));
    let out = cover_path(dir, &key);
    if out.exists() {
        return Some(key);
    }
    use gtk::gdk_pixbuf::prelude::*;
    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
    loader.write(data).ok()?;
    loader.close().ok()?;
    let pb = loader.pixbuf()?;
    let (w, h) = (pb.width(), pb.height());
    if w <= 0 || h <= 0 {
        return None;
    }
    let scale = (COVER_SIZE as f64 / w.max(h) as f64).min(1.0);
    let (nw, nh) = (((w as f64 * scale).round() as i32).max(1), ((h as f64 * scale).round() as i32).max(1));
    let scaled = if scale < 1.0 { pb.scale_simple(nw, nh, gtk::gdk_pixbuf::InterpType::Bilinear)? } else { pb };
    std::fs::create_dir_all(dir).ok()?;
    let tmp = dir.join(format!("{key}.tmp"));
    scaled.savev(&tmp, "png", &[]).ok()?;
    std::fs::rename(&tmp, &out).ok()?;
    Some(key)
}

#[cfg(test)]
pub(crate) mod testutil {
    use lofty::config::WriteOptions;
    use lofty::prelude::*;
    use lofty::tag::{Tag, TagType};
    use std::path::Path;

    /// Writes a short silent 8 kHz mono WAV file.
    pub fn write_wav(path: &Path, ms: u32) {
        let rate = 8000u32;
        let samples = rate * ms / 1000;
        let data_len = samples * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.resize(b.len() + data_len as usize, 0);
        std::fs::write(path, b).unwrap();
    }

    pub struct Tags<'a> {
        pub title: &'a str,
        pub artist: &'a str,
        pub album: &'a str,
        pub album_artist: Option<&'a str>,
        pub track: u32,
        pub disc: u32,
        pub picture: Option<Vec<u8>>,
    }

    pub fn write_tagged_wav(path: &Path, tags: &Tags) {
        write_wav(path, 1000);
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title(tags.title.into());
        tag.set_artist(tags.artist.into());
        tag.set_album(tags.album.into());
        tag.set_track(tags.track);
        tag.set_disk(tags.disc);
        if let Some(aa) = tags.album_artist {
            tag.insert_text(ItemKey::AlbumArtist, aa.into());
        }
        if let Some(data) = &tags.picture {
            let pic = lofty::picture::Picture::unchecked(data.clone())
                .pic_type(lofty::picture::PictureType::CoverFront)
                .mime_type(lofty::picture::MimeType::Png)
                .build();
            tag.push_picture(pic);
        }
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    }

    /// A tiny solid-colour PNG.
    pub fn png(r: u8) -> Vec<u8> {
        let pb = gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 4, 4).unwrap();
        pb.fill((r as u32) << 24 | 0xff);
        pb.save_to_bufferv("png", &[]).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn tags<'a>(title: &'a str, track: u32) -> Tags<'a> {
        Tags { title, artist: "Artist", album: "Album", album_artist: None, track, disc: 1, picture: None }
    }

    #[test]
    fn reads_tags_and_covers() {
        let root = tempfile::tempdir().unwrap();
        let covers = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        std::fs::create_dir(&a).unwrap();
        write_tagged_wav(
            &a.join("01.wav"),
            &Tags { album_artist: Some("Band"), disc: 2, picture: Some(png(200)), ..tags("First", 3) },
        );
        let b = root.path().join("b");
        std::fs::create_dir(&b).unwrap();
        write_wav(&b.join("Untagged Song.wav"), 500);
        std::fs::write(b.join("Cover.PNG"), png(10)).unwrap();
        std::fs::write(b.join("notes.txt"), "x").unwrap();
        std::fs::create_dir(root.path().join(".hidden")).unwrap();
        write_wav(&root.path().join(".hidden/x.wav"), 100);

        let (tracks, report) = scan(&[root.path().to_path_buf()], Vec::new(), Some(covers.path()), 5);
        assert_eq!(report.added, 2);
        assert_eq!(tracks.len(), 2);
        let t = &tracks[0];
        assert_eq!(t.title, "First");
        assert_eq!(t.artist.as_deref(), Some("Artist"));
        assert_eq!(t.album_artist.as_deref(), Some("Band"));
        assert_eq!((t.disc, t.track), (Some(2), Some(3)));
        assert!((900..=1100).contains(&t.duration_ms), "{}", t.duration_ms);
        assert_eq!(t.added, 5);
        assert!(cover_path(covers.path(), t.cover.as_ref().unwrap()).exists());
        let u = &tracks[1];
        assert_eq!(u.title, "Untagged Song");
        assert_eq!(u.artist, None);
        let key = u.cover.as_ref().expect("folder cover");
        assert_ne!(Some(key), t.cover.as_ref());
    }

    #[test]
    fn incremental_rescan() {
        let root = tempfile::tempdir().unwrap();
        let roots = [root.path().to_path_buf()];
        for (i, name) in ["one", "two", "three"].iter().enumerate() {
            write_tagged_wav(&root.path().join(format!("{name}.wav")), &tags(name, i as u32 + 1));
        }
        let (first, r1) = scan(&roots, Vec::new(), None, 100);
        assert_eq!((r1.added, r1.unchanged), (3, 0));

        // Nothing changed: everything is reused.
        let (second, r2) = scan(&roots, first.clone(), None, 200);
        assert_eq!((r2.added, r2.updated, r2.removed, r2.unchanged), (0, 0, 0, 3));
        assert_eq!(second, first);

        // Retag one file (with a new mtime), delete one, add one.
        let two = root.path().join("two.wav");
        write_tagged_wav(&two, &tags("Two (Remastered)", 2));
        let f = std::fs::File::options().write(true).open(&two).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        std::fs::remove_file(root.path().join("three.wav")).unwrap();
        write_tagged_wav(&root.path().join("four.wav"), &tags("four", 4));
        let (third, r3) = scan(&roots, second, None, 300);
        assert_eq!((r3.added, r3.updated, r3.removed, r3.unchanged), (1, 1, 1, 1));
        let by_name = |n: &str| third.iter().find(|t| t.path.file_name().unwrap() == n).unwrap();
        assert_eq!(by_name("two.wav").title, "Two (Remastered)");
        assert_eq!(by_name("two.wav").added, 100, "date added survives updates");
        assert_eq!(by_name("four.wav").added, 300);

        // Index file round trip.
        let idx = root.path().join("idx/library.json");
        save(&idx, &third).unwrap();
        assert_eq!(load(&idx), third);
    }

    #[test]
    fn nested_roots_are_scanned_once() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        write_wav(&sub.join("x.wav"), 100);
        let (tracks, _) = scan(&[sub.clone(), root.path().to_path_buf(), sub], Vec::new(), None, 0);
        assert_eq!(tracks.len(), 1);
    }

    #[test]
    fn folder_image_priority() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("folder.jpg"), b"x").unwrap();
        std::fs::write(d.path().join("COVER.jpeg"), b"x").unwrap();
        std::fs::write(d.path().join("cover.gif"), b"x").unwrap();
        assert_eq!(find_folder_image(d.path()).unwrap().file_name().unwrap(), "COVER.jpeg");
    }
}
