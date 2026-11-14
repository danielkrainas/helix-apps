//! Library model: tracks and their grouping into albums and artists.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const UNKNOWN_ARTIST: &str = "Unknown Artist";
pub const UNKNOWN_ALBUM: &str = "Unknown Album";
pub const VARIOUS_ARTISTS: &str = "Various Artists";

/// One indexed audio file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub path: PathBuf,
    /// Modification time (seconds since the epoch) when the tags were read.
    pub mtime: i64,
    pub size: u64,
    pub title: String,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub year: Option<u32>,
    #[serde(default)]
    pub track: Option<u32>,
    #[serde(default)]
    pub disc: Option<u32>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub compilation: bool,
    /// Key of the cached cover image (see `index::cover_path`).
    #[serde(default)]
    pub cover: Option<String>,
    /// When the file first appeared in the index (seconds since the epoch).
    #[serde(default)]
    pub added: i64,
    /// Metadata came from the LocalSearch/Tracker index rather than `lofty`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_tracker: bool,
}

impl Track {
    pub fn artist_name(&self) -> &str {
        self.artist.as_deref().unwrap_or(UNKNOWN_ARTIST)
    }

    pub fn album_name(&self) -> &str {
        self.album.as_deref().unwrap_or(UNKNOWN_ALBUM)
    }

    pub fn uri(&self) -> String {
        path_to_uri(&self.path)
    }
}

pub fn path_to_uri(path: &Path) -> String {
    glib::filename_to_uri(path, None)
        .map(|s| s.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    pub title: String,
    pub artist: String,
    /// True when the artist is the synthetic "Various Artists".
    pub various: bool,
    /// Whether the artist came from an album-artist tag.
    pub has_album_artist: bool,
    pub year: Option<u32>,
    /// Track indices, sorted by disc / track number / title.
    pub tracks: Vec<usize>,
    pub cover: Option<String>,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Artist {
    pub name: String,
    /// Albums that contain this artist, sorted by year then title.
    pub albums: Vec<usize>,
}

/// The whole library grouped for display.
#[derive(Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    by_path: HashMap<PathBuf, usize>,
    track_album: Vec<usize>,
    folded: Vec<String>,
}

/// Lower-cases and strips diacritics for searching and comparisons.
pub fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Sort key for titles and names: folded, leading "the " removed.
pub fn sort_key(s: &str) -> String {
    let f = fold(s.trim());
    match f.strip_prefix("the ") {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => f,
    }
}

fn track_order(a: &Track, b: &Track) -> std::cmp::Ordering {
    a.disc
        .unwrap_or(1)
        .cmp(&b.disc.unwrap_or(1))
        .then(
            a.track
                .unwrap_or(u32::MAX)
                .cmp(&b.track.unwrap_or(u32::MAX)),
        )
        .then_with(|| sort_key(&a.title).cmp(&sort_key(&b.title)))
        .then_with(|| a.path.cmp(&b.path))
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    AlbumArtist(String, String),
    Folder(String, PathBuf),
}

impl Library {
    pub fn new(tracks: Vec<Track>) -> Self {
        let mut groups: BTreeMap<GroupKey, Vec<usize>> = BTreeMap::new();
        for (i, t) in tracks.iter().enumerate() {
            let album = fold(nonempty(&t.album).unwrap_or(""));
            let key = match nonempty(&t.album_artist) {
                Some(aa) => GroupKey::AlbumArtist(fold(aa), album),
                None if t.album.is_none() => {
                    // Untitled albums: group per artist so unrelated loose files stay apart.
                    GroupKey::AlbumArtist(fold(t.artist_name()), String::new())
                }
                None => GroupKey::Folder(
                    album,
                    t.path.parent().map(Path::to_path_buf).unwrap_or_default(),
                ),
            };
            groups.entry(key).or_default().push(i);
        }

        let mut albums: Vec<Album> = groups
            .into_values()
            .map(|mut idx| {
                idx.sort_by(|&a, &b| track_order(&tracks[a], &tracks[b]));
                let first = &tracks[idx[0]];
                let album_artist = idx.iter().find_map(|&i| nonempty(&tracks[i].album_artist));
                let mut artists: Vec<String> =
                    idx.iter().map(|&i| fold(tracks[i].artist_name())).collect();
                artists.sort();
                artists.dedup();
                let compilation = idx.iter().any(|&i| tracks[i].compilation);
                let (artist, various, has_aa) = match album_artist {
                    Some(aa) => (aa.to_string(), fold(aa) == fold(VARIOUS_ARTISTS), true),
                    None if artists.len() > 1 || compilation => {
                        (VARIOUS_ARTISTS.to_string(), true, false)
                    }
                    None => (first.artist_name().to_string(), false, false),
                };
                let mut years: HashMap<u32, usize> = HashMap::new();
                for &i in &idx {
                    if let Some(y) = tracks[i].year.filter(|y| *y > 0) {
                        *years.entry(y).or_default() += 1;
                    }
                }
                let year = years
                    .into_iter()
                    .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                    .map(|(y, _)| y);
                Album {
                    title: nonempty(&first.album).unwrap_or(UNKNOWN_ALBUM).to_string(),
                    artist,
                    various,
                    has_album_artist: has_aa,
                    year,
                    cover: idx.iter().find_map(|&i| tracks[i].cover.clone()),
                    duration_ms: idx.iter().map(|&i| tracks[i].duration_ms).sum(),
                    tracks: idx,
                }
            })
            .collect();
        albums.sort_by(|a, b| {
            sort_key(&a.title)
                .cmp(&sort_key(&b.title))
                .then_with(|| sort_key(&a.artist).cmp(&sort_key(&b.artist)))
        });

        let mut track_album = vec![0; tracks.len()];
        for (ai, a) in albums.iter().enumerate() {
            for &t in &a.tracks {
                track_album[t] = ai;
            }
        }

        // Artists: album artists plus every track artist.
        let mut artist_map: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
        for (ai, a) in albums.iter().enumerate() {
            let mut names: Vec<&str> = Vec::new();
            if !a.various {
                names.push(&a.artist);
            }
            for &t in &a.tracks {
                names.push(tracks[t].artist_name());
            }
            for name in names {
                let entry = artist_map
                    .entry(fold(name))
                    .or_insert_with(|| (name.to_string(), Vec::new()));
                if entry.1.last() != Some(&ai) && !entry.1.contains(&ai) {
                    entry.1.push(ai);
                }
            }
        }
        let mut artists: Vec<Artist> = artist_map
            .into_values()
            .filter(|(name, _)| fold(name) != fold(VARIOUS_ARTISTS))
            .map(|(name, mut al)| {
                al.sort_by(|&a, &b| {
                    albums[a]
                        .year
                        .unwrap_or(u32::MAX)
                        .cmp(&albums[b].year.unwrap_or(u32::MAX))
                        .then_with(|| sort_key(&albums[a].title).cmp(&sort_key(&albums[b].title)))
                });
                Artist { name, albums: al }
            })
            .collect();
        artists.sort_by_key(|a| sort_key(&a.name));

        let by_path = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.path.clone(), i))
            .collect();
        let folded = tracks
            .iter()
            .map(|t| {
                let mut s = String::new();
                for part in [
                    Some(t.title.as_str()),
                    t.artist.as_deref(),
                    t.album_artist.as_deref(),
                    t.album.as_deref(),
                    t.genre.as_deref(),
                ]
                .into_iter()
                .flatten()
                {
                    s.push_str(&fold(part));
                    s.push('\u{1}');
                }
                s
            })
            .collect();
        Library {
            tracks,
            albums,
            artists,
            by_path,
            track_album,
            folded,
        }
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    pub fn track_by_path(&self, path: &Path) -> Option<&Track> {
        self.index_of(path).map(|i| &self.tracks[i])
    }

    pub fn album_of(&self, track: usize) -> Option<usize> {
        self.track_album
            .get(track)
            .copied()
            .filter(|_| !self.albums.is_empty())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn artist_index(&self, name: &str) -> Option<usize> {
        let f = fold(name);
        self.artists.iter().position(|a| fold(&a.name) == f)
    }

    /// Tracks of `album` that belong on `artist`'s page: the whole album when
    /// the artist is the album artist, otherwise only the artist's own tracks.
    pub fn artist_tracks_in_album(&self, artist: &str, album: usize) -> Vec<usize> {
        let a = &self.albums[album];
        let f = fold(artist);
        if !a.various && fold(&a.artist) == f {
            return a.tracks.clone();
        }
        a.tracks
            .iter()
            .copied()
            .filter(|&t| fold(self.tracks[t].artist_name()) == f)
            .collect()
    }

    /// All track indices in album order (album title, then disc/track).
    pub fn all_tracks_by_album(&self) -> Vec<usize> {
        self.albums
            .iter()
            .flat_map(|a| a.tracks.iter().copied())
            .collect()
    }

    pub fn search(&self, query: &str) -> SearchResults {
        let terms: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
        if terms.is_empty() {
            return SearchResults::default();
        }
        let matches = |hay: &str| terms.iter().all(|t| hay.contains(t.as_str()));
        let tracks: Vec<usize> = self
            .all_tracks_by_album()
            .into_iter()
            .filter(|&i| matches(&self.folded[i]))
            .collect();
        let albums = (0..self.albums.len())
            .filter(|&ai| {
                let a = &self.albums[ai];
                matches(&format!("{}\u{1}{}", fold(&a.title), fold(&a.artist)))
            })
            .collect();
        let artists = (0..self.artists.len())
            .filter(|&i| matches(&fold(&self.artists[i].name)))
            .collect();
        SearchResults {
            albums,
            artists,
            tracks,
        }
    }
}

#[derive(Default, Debug, PartialEq)]
pub struct SearchResults {
    pub albums: Vec<usize>,
    pub artists: Vec<usize>,
    pub tracks: Vec<usize>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.albums.is_empty() && self.artists.is_empty() && self.tracks.is_empty()
    }
}

/// Formats milliseconds as `m:ss` or `h:mm:ss`.
pub fn format_duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Value text of the seek slider: "1:23 of 4:56".
pub fn position_text(pos_ms: u64, dur_ms: u64) -> String {
    format!(
        "{} of {}",
        format_duration(pos_ms.min(dur_ms)),
        format_duration(dur_ms)
    )
}

/// Value text of the volume slider (0.0–1.0): "75%".
pub fn volume_text(level: f64) -> String {
    format!("{:.0}%", level.clamp(0.0, 1.0) * 100.0)
}

/// Human-friendly total length, e.g. "42 minutes" or "1 hour 5 minutes".
pub fn format_total(ms: u64) -> String {
    let mins = ms.div_ceil(60_000);
    let (h, m) = (mins / 60, mins % 60);
    let plural = |n: u64, w: &str| {
        if n == 1 {
            format!("1 {w}")
        } else {
            format!("{n} {w}s")
        }
    };
    match (h, m) {
        (0, m) => plural(m, "minute"),
        (h, 0) => plural(h, "hour"),
        (h, m) => format!("{} {}", plural(h, "hour"), plural(m, "minute")),
    }
}

#[cfg(test)]
pub(crate) fn track(path: &str, title: &str, artist: Option<&str>, album: Option<&str>) -> Track {
    Track {
        path: PathBuf::from(path),
        title: title.into(),
        artist: artist.map(Into::into),
        album: album.map(Into::into),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(mut t: Track, f: impl FnOnce(&mut Track)) -> Track {
        f(&mut t);
        t
    }

    #[test]
    fn groups_by_album_artist() {
        let lib = Library::new(vec![
            with(
                track("/m/a/1.flac", "One", Some("X feat. Y"), Some("Album")),
                |t| t.album_artist = Some("X".into()),
            ),
            with(track("/m/b/2.flac", "Two", Some("X"), Some("album")), |t| {
                t.album_artist = Some("x".into())
            }),
        ]);
        assert_eq!(
            lib.albums.len(),
            1,
            "album artist + title (case-insensitive) groups across folders"
        );
        assert_eq!(lib.albums[0].artist, "X");
        assert!(!lib.albums[0].various);
        let names: Vec<_> = lib.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["X", "X feat. Y"]);
    }

    #[test]
    fn various_artists_without_album_artist() {
        let lib = Library::new(vec![
            track("/m/comp/1.mp3", "A", Some("Alice"), Some("Hits")),
            track("/m/comp/2.mp3", "B", Some("Bob"), Some("Hits")),
            // Same album title in another folder by one artist stays separate.
            track("/m/other/1.mp3", "C", Some("Carol"), Some("Hits")),
        ]);
        assert_eq!(lib.albums.len(), 2);
        let comp = lib.albums.iter().find(|a| a.tracks.len() == 2).unwrap();
        assert_eq!(comp.artist, VARIOUS_ARTISTS);
        assert!(comp.various);
        let solo = lib.albums.iter().find(|a| a.tracks.len() == 1).unwrap();
        assert_eq!(solo.artist, "Carol");
        // "Various Artists" is not listed as an artist; Alice appears with her one track.
        assert!(lib.artist_index(VARIOUS_ARTISTS).is_none());
        let alice = lib.artist_index("alice").unwrap();
        let al = lib.artists[alice].albums[0];
        assert_eq!(lib.artist_tracks_in_album("Alice", al).len(), 1);
    }

    #[test]
    fn compilation_flag_makes_various() {
        let lib = Library::new(vec![with(
            track("/m/c/1.mp3", "A", Some("Alice"), Some("Mix")),
            |t| t.compilation = true,
        )]);
        assert_eq!(lib.albums[0].artist, VARIOUS_ARTISTS);
    }

    #[test]
    fn disc_and_track_sorting() {
        let mk = |p: &str, disc: Option<u32>, n: Option<u32>| {
            with(track(p, p, Some("A"), Some("Al")), |t| {
                t.disc = disc;
                t.track = n;
            })
        };
        let lib = Library::new(vec![
            mk("/m/x/d2t1", Some(2), Some(1)),
            mk("/m/x/none", None, None),
            mk("/m/x/d1t2", Some(1), Some(2)),
            mk("/m/x/nodisc-t1", None, Some(1)),
            mk("/m/x/d1t10", Some(1), Some(10)),
        ]);
        let order: Vec<_> = lib.albums[0]
            .tracks
            .iter()
            .map(|&i| lib.tracks[i].title.as_str())
            .collect();
        assert_eq!(
            order,
            [
                "/m/x/nodisc-t1",
                "/m/x/d1t2",
                "/m/x/d1t10",
                "/m/x/none",
                "/m/x/d2t1"
            ]
        );
    }

    #[test]
    fn unknown_album_and_artist() {
        let lib = Library::new(vec![
            track("/m/1.ogg", "Loose", None, None),
            track("/n/2.ogg", "Loose 2", None, None),
        ]);
        assert_eq!(lib.albums.len(), 1);
        assert_eq!(lib.albums[0].title, UNKNOWN_ALBUM);
        assert_eq!(lib.albums[0].artist, UNKNOWN_ARTIST);
    }

    #[test]
    fn album_year_and_sorting() {
        let lib = Library::new(vec![
            with(track("/m/z/1", "a", Some("A"), Some("The Zoo")), |t| {
                t.year = Some(1999)
            }),
            with(track("/m/z/2", "b", Some("A"), Some("The Zoo")), |t| {
                t.year = Some(2001)
            }),
            with(track("/m/z/3", "c", Some("A"), Some("The Zoo")), |t| {
                t.year = Some(2001)
            }),
            track("/m/b/1", "x", Some("A"), Some("Beta")),
        ]);
        assert_eq!(lib.albums[0].title, "Beta");
        assert_eq!(lib.albums[1].year, Some(2001));
        let a = &lib.artists[lib.artist_index("A").unwrap()];
        // Albums of an artist sort by year, undated last.
        assert_eq!(lib.albums[a.albums[0]].title, "The Zoo");
    }

    #[test]
    fn search_is_accent_and_case_insensitive() {
        let lib = Library::new(vec![
            with(
                track("/m/1", "Café del Mar", Some("Björk"), Some("Post")),
                |t| t.genre = Some("Electronic".into()),
            ),
            track("/m/2", "Other", Some("Someone"), Some("Else")),
        ]);
        let r = lib.search("bjork cafe");
        assert_eq!(r.tracks.len(), 1);
        assert!(r.artists.is_empty(), "artists must match every term");
        assert_eq!(lib.search("BJÖRK").artists.len(), 1);
        assert!(r.albums.is_empty());
        assert_eq!(lib.search("electronic").tracks.len(), 1);
        assert!(lib.search("  ").is_empty());
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(65_000), "1:05");
        assert_eq!(position_text(83_000, 296_000), "1:23 of 4:56");
        assert_eq!(position_text(400_000, 296_000), "4:56 of 4:56");
        assert_eq!(volume_text(0.754), "75%");
        assert_eq!(volume_text(1.2), "100%");
        assert_eq!(format_duration(3_725_000), "1:02:05");
        assert_eq!(format_total(60_000), "1 minute");
        assert_eq!(format_total(3_900_000), "1 hour 5 minutes");
        assert_eq!(format_total(7_200_000), "2 hours");
    }
}
