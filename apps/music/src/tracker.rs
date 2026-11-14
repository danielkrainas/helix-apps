//! Read-only access to the LocalSearch/Tracker 3 index over D-Bus.
//!
//! `org.freedesktop.Tracker3.Endpoint.Query(s sparql, h fd, a{sv} args) → as`
//! answers with the column names and writes the result rows to the passed
//! file descriptor. Each row is: `i32 n_columns`, `i32 types[n]`,
//! `i32 end_offsets[n]`, then the column values as NUL-terminated UTF-8
//! strings (value `i` spans `[i == 0 ? 0 : end[i-1] + 1, end[i])`), all
//! integers in host byte order. Type 0 means "unbound".

use crate::model::Track;
use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;
use zbus::blocking::Connection;
use zbus::zvariant::{Fd, Value};

pub const BUS_NAME: &str = "org.freedesktop.Tracker3.Miner.Files";
pub const OBJECT_PATH: &str = "/org/freedesktop/Tracker3/Endpoint";
pub const INTERFACE: &str = "org.freedesktop.Tracker3.Endpoint";
/// Set to disable Tracker and always use the built-in scanner.
pub const DISABLE_ENV: &str = "HELIX_MUSIC_NO_TRACKER";

/// One result row; `None` for unbound columns.
pub type Row = Vec<Option<String>>;

/// Parses the cursor stream Tracker writes to the query fd.
pub fn parse_cursor(mut data: &[u8]) -> Result<Vec<Row>, String> {
    fn take_i32(d: &mut &[u8]) -> Result<i32, String> {
        let (head, rest) = d.split_first_chunk::<4>().ok_or("truncated cursor")?;
        *d = rest;
        Ok(i32::from_ne_bytes(*head))
    }
    let mut rows = Vec::new();
    while !data.is_empty() {
        let n = take_i32(&mut data)?;
        if !(0..=4096).contains(&n) {
            return Err(format!("bad column count {n}"));
        }
        let n = n as usize;
        let mut types = Vec::with_capacity(n);
        for _ in 0..n {
            types.push(take_i32(&mut data)?);
        }
        let mut ends = Vec::with_capacity(n);
        for _ in 0..n {
            ends.push(take_i32(&mut data)?);
        }
        if n == 0 {
            rows.push(Vec::new());
            continue;
        }
        let len = usize::try_from(ends[n - 1]).map_err(|_| "bad offset")? + 1;
        if data.len() < len {
            return Err("truncated row".into());
        }
        let (block, rest) = data.split_at(len);
        data = rest;
        let mut row = Vec::with_capacity(n);
        let mut start = 0usize;
        for i in 0..n {
            let end = usize::try_from(ends[i]).map_err(|_| "bad offset")?;
            if end < start || end >= len {
                return Err("bad offset".into());
            }
            let s = String::from_utf8_lossy(&block[start..end]).into_owned();
            row.push(if types[i] == 0 { None } else { Some(s) });
            start = end + 1;
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Escapes text for a SPARQL string literal.
pub fn sparql_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// A `file://` URI prefix (with trailing slash) for a folder.
pub fn folder_uri_prefix(dir: &Path) -> String {
    let mut uri = crate::model::path_to_uri(dir);
    if !uri.ends_with('/') {
        uri.push('/');
    }
    uri
}

/// Parses an ISO 8601 date-time (`2023-05-07T17:17:53Z`, optional
/// fraction and `±hh:mm` offset; a bare date is midnight UTC) into seconds
/// since the Unix epoch.
pub fn parse_datetime(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if s.as_bytes().get(4) != Some(&b'-')
        || s.as_bytes().get(7) != Some(&b'-')
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
    {
        return None;
    }
    // Days from civil (proleptic Gregorian).
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut secs = days * 86_400;
    if s.len() > 10 {
        let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
        secs += h * 3600 + mi * 60 + se;
        let tail = s[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
        if let Some(sign) = tail.chars().next().filter(|c| *c == '+' || *c == '-') {
            let t = &tail[1..];
            let oh: i64 = t.get(0..2)?.parse().ok()?;
            let om: i64 = t
                .get(3..5)
                .or_else(|| t.get(2..4))
                .and_then(|m| m.parse().ok())
                .unwrap_or(0);
            let off = oh * 3600 + om * 60;
            secs -= if sign == '+' { off } else { -off };
        }
    }
    Some(secs)
}

pub struct Endpoint {
    conn: Connection,
}

impl Endpoint {
    /// Connects to the session bus and checks that the LocalSearch miner is
    /// running (it is not auto-started).
    pub fn connect() -> Option<Endpoint> {
        if std::env::var_os(DISABLE_ENV).is_some_and(|v| !v.is_empty()) {
            return None;
        }
        let conn = Connection::session().ok()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&conn).ok()?;
        let name = zbus::names::BusName::try_from(BUS_NAME).ok()?;
        if !dbus.name_has_owner(name).ok()? {
            return None;
        }
        let ep = Endpoint { conn };
        ep.query("ASK { }").ok()?;
        Some(ep)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Runs a read-only SPARQL query.
    pub fn query(&self, sparql: &str) -> Result<Vec<Row>, String> {
        let (mut reader, writer) =
            std::os::unix::net::UnixStream::pair().map_err(|e| e.to_string())?;
        reader.set_read_timeout(Some(Duration::from_secs(60))).ok();
        // Read concurrently: large results would otherwise fill the socket
        // buffer and stall the writer.
        let handle = std::thread::spawn(move || {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).map(|_| buf)
        });
        let writer: OwnedFd = writer.into();
        let args: HashMap<&str, Value> = HashMap::new();
        let res = self.conn.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "Query",
            &(sparql, Fd::from(writer.as_fd()), args),
        );
        drop(writer);
        let data = handle
            .join()
            .map_err(|_| "reader panicked")?
            .map_err(|e| e.to_string());
        res.map_err(|e| e.to_string())?;
        parse_cursor(&data?)
    }
}

/// The library as Tracker sees it, restricted to the library folders.
pub struct TrackerLibrary {
    /// Library folders Tracker indexes (files below them are known to it).
    pub covered: Vec<PathBuf>,
    /// Music under the covered folders. `mtime`, `size` and `cover` are
    /// not filled in.
    pub tracks: Vec<Track>,
}

const TRACK_COLUMNS: &str =
    "?url ?title ?artist ?album ?albumArtist ?genre ?date ?track ?disc ?duration ?added";

fn tracks_query(prefix: &str) -> String {
    format!(
        "SELECT {TRACK_COLUMNS} WHERE {{
  ?song a nmm:MusicPiece ; nie:isStoredAs ?file .
  ?file nie:url ?url .
  FILTER (STRSTARTS(?url, \"{p}\"))
  OPTIONAL {{ ?song nie:title ?title }}
  OPTIONAL {{ ?song nmm:artist ?a . ?a nmm:artistName ?artist }}
  OPTIONAL {{ ?song nmm:musicAlbum ?al . ?al nie:title ?album .
             OPTIONAL {{ ?al nmm:albumArtist ?aa . ?aa nmm:artistName ?albumArtist }} }}
  OPTIONAL {{ ?song nfo:genre ?genre }}
  OPTIONAL {{ ?song nie:contentCreated ?date }}
  OPTIONAL {{ ?song nmm:trackNumber ?track }}
  OPTIONAL {{ ?song nmm:musicAlbumDisc ?d . ?d nmm:setNumber ?disc }}
  OPTIONAL {{ ?song nfo:duration ?duration }}
  OPTIONAL {{ ?file nrl:added ?added }}
}}",
        p = sparql_escape(prefix)
    )
}

/// Converts a row of [`TRACK_COLUMNS`] into a track.
pub fn row_to_track(row: &Row) -> Option<Track> {
    let col = |i: usize| {
        row.get(i)
            .cloned()
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let num = |i: usize| {
        col(i)
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n > 0)
    };
    let path = glib::filename_from_uri(&col(0)?).ok()?.0;
    let title = col(1).unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    Some(Track {
        title,
        artist: col(2),
        album: col(3),
        album_artist: col(4),
        genre: col(5),
        year: col(6)
            .and_then(|d| d.get(..4).and_then(|y| y.parse().ok()))
            .filter(|y: &u32| *y > 0),
        track: num(7),
        disc: num(8),
        duration_ms: col(9)
            .and_then(|s| s.parse::<f64>().ok())
            .map_or(0, |s| (s.max(0.0) * 1000.0) as u64),
        added: col(10).and_then(|s| parse_datetime(&s)).unwrap_or(0),
        path,
        ..Default::default()
    })
}

/// Queries the music below `roots`. Roots Tracker knows no files under are
/// left out of `covered` so the caller can scan them itself.
pub fn library(ep: &Endpoint, roots: &[PathBuf]) -> Result<TrackerLibrary, String> {
    let mut out = TrackerLibrary {
        covered: Vec::new(),
        tracks: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        if out.covered.iter().any(|c| root.starts_with(c)) {
            continue;
        }
        let prefix = folder_uri_prefix(root);
        let ask = format!(
            "ASK {{ ?f a nfo:FileDataObject ; nie:url ?u . FILTER (STRSTARTS(?u, \"{}\")) }}",
            sparql_escape(&prefix)
        );
        let known = ep
            .query(&ask)?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .is_some_and(|v| v == "true");
        if !known {
            continue;
        }
        out.covered.retain(|c| !c.starts_with(root));
        out.covered.push(root.clone());
        for row in ep.query(&tracks_query(&prefix))? {
            if let Some(t) = row_to_track(&row) {
                // OPTIONAL joins can repeat a file; keep the first row.
                if seen.insert(t.path.clone()) {
                    out.tracks.push(t);
                }
            }
        }
    }
    out.tracks
        .retain(|t| out.covered.iter().any(|c| t.path.starts_with(c)));
    Ok(out)
}

/// A playlist file indexed by LocalSearch.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedPlaylist {
    /// The playlist file.
    pub file: PathBuf,
    pub title: Option<String>,
    pub entries: Vec<PathBuf>,
}

/// Lists the playlists in the index with their entries in order.
pub fn playlists(ep: &Endpoint) -> Result<Vec<IndexedPlaylist>, String> {
    let rows = ep.query(
        "SELECT ?url ?title ?pos ?entry WHERE {
  ?pl a nmm:Playlist ; nie:isStoredAs ?f ; nfo:hasMediaFileListEntry ?e .
  ?f nie:url ?url .
  ?e nfo:entryUrl ?entry .
  OPTIONAL { ?e nfo:listPosition ?pos }
  OPTIONAL { ?pl nie:title ?title }
} ORDER BY ?url ?pos",
    )?;
    Ok(group_playlist_rows(&rows))
}

/// Groups `?url ?title ?pos ?entry` rows (sorted by url) into playlists.
pub fn group_playlist_rows(rows: &[Row]) -> Vec<IndexedPlaylist> {
    type Positioned = Vec<(f64, usize, PathBuf)>;
    let mut out: Vec<(IndexedPlaylist, Positioned)> = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let col = |i: usize| row.get(i).cloned().flatten();
        let Some(file) = col(0)
            .and_then(|u| glib::filename_from_uri(&u).ok())
            .map(|p| p.0)
        else {
            continue;
        };
        let Some(entry) = col(3)
            .and_then(|u| glib::filename_from_uri(&u).ok())
            .map(|p| p.0)
        else {
            continue;
        };
        let pos = col(2)
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(f64::MAX);
        if out.last().is_none_or(|(p, _)| p.file != file) {
            let title = col(1).filter(|t| !t.trim().is_empty());
            out.push((
                IndexedPlaylist {
                    file,
                    title,
                    entries: Vec::new(),
                },
                Vec::new(),
            ));
        }
        out.last_mut().unwrap().1.push((pos, n, entry));
    }
    out.into_iter()
        .map(|(mut pl, mut entries)| {
            entries.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            pl.entries = entries.into_iter().map(|e| e.2).collect();
            pl
        })
        .collect()
}

/// Waits for `GraphUpdated` signals from the miner on a worker thread and
/// calls `on_change` for each one touching music or playlists.
pub fn watch(ep: Endpoint, on_change: impl Fn() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("tracker-watch".into())
        .spawn(move || {
            let Ok(rule) = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(BUS_NAME)
                .and_then(|b| b.path(OBJECT_PATH))
                .and_then(|b| b.interface(INTERFACE))
                .and_then(|b| b.member("GraphUpdated"))
                .map(|b| b.build())
            else {
                return;
            };
            let Ok(iter) =
                zbus::blocking::MessageIterator::for_match_rule(rule, ep.connection(), Some(16))
            else {
                return;
            };
            for msg in iter.flatten() {
                let body = msg.body();
                let class: String = body
                    .deserialize::<(String, Vec<(i32, i32)>)>()
                    .map(|(c, _)| c)
                    .unwrap_or_default();
                if class.is_empty()
                    || class.contains("MusicPiece")
                    || class.contains("Playlist")
                    || class.contains("Audio")
                {
                    on_change();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(rows: &[Vec<(i32, &str)>]) -> Vec<u8> {
        let mut b = Vec::new();
        for row in rows {
            b.extend_from_slice(&(row.len() as i32).to_ne_bytes());
            for (t, _) in row {
                b.extend_from_slice(&t.to_ne_bytes());
            }
            let mut end = -1i32;
            for (_, v) in row {
                end += v.len() as i32 + 1;
                b.extend_from_slice(&end.to_ne_bytes());
            }
            for (_, v) in row {
                b.extend_from_slice(v.as_bytes());
                b.push(0);
            }
        }
        b
    }

    #[test]
    fn cursor_format() {
        let data = encode(&[
            vec![(1, "file:///m/a%20b.flac"), (0, ""), (3, "152")],
            vec![(1, "file:///m/c.ogg"), (2, "Ünïcode"), (0, "")],
        ]);
        let rows = parse_cursor(&data).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            [
                Some("file:///m/a%20b.flac".into()),
                None,
                Some("152".into())
            ]
        );
        assert_eq!(rows[1][1].as_deref(), Some("Ünïcode"));
        assert!(parse_cursor(&data[..data.len() - 3]).is_err());
        assert_eq!(parse_cursor(&[]).unwrap(), Vec::<Row>::new());
    }

    /// Bytes captured from a real Tracker 3 endpoint (little-endian host).
    #[test]
    fn real_cursor_sample() {
        let data = b"\x01\x00\x00\x00\x03\x00\x00\x00\x03\x00\x00\x00116\x00";
        if cfg!(target_endian = "little") {
            assert_eq!(parse_cursor(data).unwrap(), [vec![Some("116".to_string())]]);
        }
    }

    #[test]
    fn rows_to_tracks() {
        let s = |v: &str| Some(v.to_string());
        let row = vec![
            s("file:///music/x/01%20Song.flac"),
            None,
            s("Artist"),
            s("Album"),
            None,
            s("Rock"),
            s("2001-01-01T00:00:00Z"),
            s("3"),
            s("2"),
            s("152"),
            s("2023-05-07T17:17:53Z"),
        ];
        let t = row_to_track(&row).unwrap();
        assert_eq!(t.path, PathBuf::from("/music/x/01 Song.flac"));
        assert_eq!(t.title, "01 Song");
        assert_eq!(
            (
                t.artist.as_deref(),
                t.album.as_deref(),
                t.album_artist.as_deref()
            ),
            (Some("Artist"), Some("Album"), None)
        );
        assert_eq!(
            (t.year, t.track, t.disc, t.duration_ms),
            (Some(2001), Some(3), Some(2), 152_000)
        );
        assert_eq!(t.added, 1_683_479_873);
        assert!(row_to_track(&vec![s("http://x/y.mp3")]).is_none());
    }

    #[test]
    fn datetimes() {
        assert_eq!(parse_datetime("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_datetime("2000-03-01"), Some(951_868_800));
        assert_eq!(
            parse_datetime("2023-05-07T19:17:53.25+02:00"),
            Some(1_683_479_873)
        );
        assert_eq!(
            parse_datetime("2023-05-07T12:17:53-0500"),
            Some(1_683_479_873)
        );
        assert_eq!(parse_datetime("garbage"), None);
    }

    #[test]
    fn escaping_and_prefixes() {
        assert_eq!(sparql_escape(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(
            folder_uri_prefix(Path::new("/home/u/My Music")),
            "file:///home/u/My%20Music/"
        );
    }

    #[test]
    fn playlist_rows() {
        let s = |v: &str| Some(v.to_string());
        let rows = vec![
            vec![
                s("file:///p/a.m3u"),
                s("Mix"),
                s("2.0"),
                s("file:///m/2.mp3"),
            ],
            vec![
                s("file:///p/a.m3u"),
                s("Mix"),
                s("1.0"),
                s("file:///m/1.mp3"),
            ],
            vec![s("file:///p/b.pls"), None, None, s("file:///m/3.mp3")],
            vec![s("file:///p/b.pls"), None, None, s("http://radio")],
        ];
        let pls = group_playlist_rows(&rows);
        assert_eq!(pls.len(), 2);
        assert_eq!(pls[0].title.as_deref(), Some("Mix"));
        assert_eq!(
            pls[0].entries,
            [PathBuf::from("/m/1.mp3"), PathBuf::from("/m/2.mp3")]
        );
        assert_eq!(pls[1].entries, [PathBuf::from("/m/3.mp3")]);
    }
}
