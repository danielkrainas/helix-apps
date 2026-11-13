//! One-time import of GNOME Music's own favourites and playlists.
//!
//! GNOME Music keeps user data (favourite flags, user playlists) in a
//! private Tracker 3 database with the Nepomuk ontology, separate from the
//! LocalSearch miner. Its location (general knowledge, not verified against
//! source): `$XDG_CACHE_HOME/gnome-music/db` for a native install and
//! `~/.var/app/org.gnome.Music/cache/gnome-music/db` for the Flatpak.
//!
//! tinysparql is not linked, so the database is read with the `tracker3`
//! CLI (`tracker3 sparql --database DIR --query …`). To be strictly
//! read-only towards GNOME Music's data, the database files are first
//! copied into a private temporary directory and only the copy is opened.
//! Queries are `SELECT`s only, run on a worker thread with a timeout.
//!
//! The CLI prints rows as `  v1, v2` lines, which is ambiguous for values
//! with commas, so each query returns a single column built with
//! `CONCAT("@", ENCODE_FOR_URI(a), "|", ENCODE_FOR_URI(b), …)`: every field
//! is percent-encoded and can therefore contain neither `|` nor a newline.

use crate::tracker;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Overrides the database directory (tests and debugging).
pub const DB_ENV: &str = "HELIX_MUSIC_GNOME_DB";
/// Seconds one `tracker3` query may take.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(20);

/// Files making up a Tracker 3 database directory.
const DB_FILES: &[&str] = &["meta.db", "meta.db-wal", "meta.db-shm", "ontologies.gvdb"];

/// Candidate locations of GNOME Music's database, most likely first.
pub fn candidate_dirs(cache: &Path, home: &Path) -> Vec<PathBuf> {
    if let Some(p) = std::env::var_os(DB_ENV).filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(p)];
    }
    vec![
        cache.join("gnome-music").join("db"),
        home.join(".var/app/org.gnome.Music/cache/gnome-music/db"),
    ]
}

/// The first candidate holding a database (existence check only).
pub fn find_db() -> Option<PathBuf> {
    candidate_dirs(&glib::user_cache_dir(), &glib::home_dir())
        .into_iter()
        .find(|d| d.join("meta.db").is_file())
}

/// What GNOME Music's database contains, with file references as URIs.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GnomeMusicData {
    /// Favourite songs: file URI, or the song's resource IRI when the
    /// database has no file URL for it.
    pub favorites: Vec<String>,
    pub playlists: Vec<GnomePlaylist>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct GnomePlaylist {
    pub id: String,
    pub title: String,
    /// Entry URLs in list order.
    pub entries: Vec<String>,
}

pub const FAVORITES_QUERY: &str = r#"SELECT (CONCAT("@", ENCODE_FOR_URI(STR(?s)), "|", ENCODE_FOR_URI(COALESCE(?url, ?direct, STR(?f), ""))) AS ?r) WHERE {
  ?s nao:hasTag nao:predefined-tag-favorite .
  OPTIONAL { ?s nie:isStoredAs ?f . OPTIONAL { ?f nie:url ?url } }
  OPTIONAL { ?s nie:url ?direct }
}"#;

pub const PLAYLISTS_QUERY: &str = r#"SELECT (CONCAT("@", ENCODE_FOR_URI(STR(?pl)), "|", ENCODE_FOR_URI(COALESCE(?title, "")), "|", COALESCE(STR(?pos), ""), "|", ENCODE_FOR_URI(?entry)) AS ?r) WHERE {
  ?pl a nmm:Playlist ; nfo:hasMediaFileListEntry ?e .
  ?e nfo:entryUrl ?entry .
  OPTIONAL { ?e nfo:listPosition ?pos }
  OPTIONAL { ?pl nie:title ?title }
} ORDER BY ?pl ?pos"#;

/// Decodes `%XX` escapes (invalid escapes are kept literally).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extracts the encoded rows (`@a|b|…`) from `tracker3 sparql` output.
pub fn parse_rows(output: &str) -> Vec<Vec<String>> {
    output
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix('@'))
        .map(|l| l.split('|').map(percent_decode).collect())
        .collect()
}

pub fn parse_favorites(output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for row in parse_rows(output) {
        let subject = row.first().cloned().unwrap_or_default();
        let url = row.get(1).cloned().unwrap_or_default();
        let r = if url.is_empty() { subject } else { url };
        if !r.is_empty() && !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// Groups playlist rows (sorted by playlist, position) into playlists.
pub fn parse_playlists(output: &str) -> Vec<GnomePlaylist> {
    type Positioned = Vec<(f64, usize, String)>;
    let mut out: Vec<(GnomePlaylist, Positioned)> = Vec::new();
    for (n, row) in parse_rows(output).into_iter().enumerate() {
        if row.len() < 4 || row[3].is_empty() {
            continue;
        }
        let pos = row[2].parse::<f64>().unwrap_or(f64::MAX);
        if out.last().is_none_or(|(p, _)| p.id != row[0]) {
            let title = if row[1].trim().is_empty() {
                "Playlist".to_string()
            } else {
                row[1].clone()
            };
            out.push((
                GnomePlaylist {
                    id: row[0].clone(),
                    title,
                    entries: Vec::new(),
                },
                Vec::new(),
            ));
        }
        out.last_mut().unwrap().1.push((pos, n, row[3].clone()));
    }
    out.into_iter()
        .map(|(mut pl, mut e)| {
            e.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            pl.entries = e.into_iter().map(|x| x.2).collect();
            pl
        })
        .collect()
}

/// Runs `tracker3 sparql --database DIR --query Q` with a timeout and
/// returns its standard output. No shell is involved.
pub fn run_query(db: &Path, query: &str, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("tracker3")
        .arg("sparql")
        .arg("--database")
        .arg(db)
        .arg("--query")
        .arg(query)
        .env("LC_ALL", "C.UTF-8")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run tracker3: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("tracker3 timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("tracker3 failed: {}", err.trim()));
    }
    Ok(out)
}

/// Copies the database files into a fresh private directory.
fn snapshot(db: &Path) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = glib::user_runtime_dir();
    let base = if base.is_dir() {
        base
    } else {
        std::env::temp_dir()
    };
    let dir = base.join(format!(
        "helix-music-gnome-db-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    for f in DB_FILES {
        let src = db.join(f);
        if src.is_file() {
            if let Err(e) = std::fs::copy(&src, dir.join(f)) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("cannot copy {}: {e}", src.display()));
            }
        }
    }
    Ok(dir)
}

/// Reads favourites and playlists from a copy of the database at `db`.
pub fn read(db: &Path) -> Result<GnomeMusicData, String> {
    let copy = snapshot(db)?;
    let res = (|| {
        let favorites = parse_favorites(&run_query(&copy, FAVORITES_QUERY, QUERY_TIMEOUT)?);
        let playlists = parse_playlists(&run_query(&copy, PLAYLISTS_QUERY, QUERY_TIMEOUT)?);
        Ok(GnomeMusicData {
            favorites,
            playlists,
        })
    })();
    let _ = std::fs::remove_dir_all(&copy);
    res
}

/// Whether `iri` can be embedded as `<iri>` in a SPARQL query.
pub fn safe_iri(iri: &str) -> bool {
    !iri.is_empty()
        && !iri
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"{}|^`\\".contains(c))
}

/// Converts a reference to a local path: `file://` URIs directly, other
/// IRIs through `resolved` (IRI → file URI looked up in LocalSearch).
pub fn to_path(r: &str, resolved: &std::collections::HashMap<String, String>) -> Option<PathBuf> {
    let uri = if r.starts_with("file://") {
        r
    } else {
        resolved.get(r)?.as_str()
    };
    glib::filename_from_uri(uri).ok().map(|p| p.0)
}

/// Song references that are resource IRIs rather than file URIs.
pub fn unresolved(data: &GnomeMusicData) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in data
        .favorites
        .iter()
        .chain(data.playlists.iter().flat_map(|p| p.entries.iter()))
    {
        if !r.starts_with("file://") && safe_iri(r) && !out.contains(r) {
            out.push(r.clone());
        }
    }
    out
}

/// Looks up the file URLs of song resources in the LocalSearch index
/// (read-only). GNOME Music may refer to songs by the miner's IRIs.
pub fn resolve_in_index(
    ep: &tracker::Endpoint,
    iris: &[String],
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for chunk in iris.chunks(200) {
        let values: Vec<String> = chunk
            .iter()
            .filter(|i| safe_iri(i))
            .map(|i| format!("<{i}>"))
            .collect();
        if values.is_empty() {
            continue;
        }
        let q = format!(
            "SELECT ?s ?url WHERE {{ VALUES ?s {{ {} }} ?s nie:isStoredAs ?f . ?f nie:url ?url }}",
            values.join(" ")
        );
        match ep.query(&q) {
            Ok(rows) => {
                for row in rows {
                    if let (Some(Some(s)), Some(Some(u))) = (row.first(), row.get(1)) {
                        map.insert(s.clone(), u.clone());
                    }
                }
            }
            Err(e) => eprintln!("helix-music: cannot resolve GNOME Music songs: {e}"),
        }
    }
    map
}

/// GNOME Music's data with references mapped to local paths.
#[derive(Debug, Default)]
pub struct Found {
    pub favorites: Vec<PathBuf>,
    /// (title, entries) in GNOME Music's order.
    pub playlists: Vec<(String, Vec<PathBuf>)>,
}

/// Toast text after an import.
pub fn summary(favorites: usize, playlists: usize) -> String {
    let f = match favorites {
        0 => None,
        1 => Some("1 favorite song".to_string()),
        n => Some(format!("{n} favorite songs")),
    };
    let p = match playlists {
        0 => None,
        1 => Some("1 playlist".to_string()),
        n => Some(format!("{n} playlists")),
    };
    match (f, p) {
        (Some(f), Some(p)) => format!("Imported {f} and {p} from GNOME Music"),
        (Some(x), None) | (None, Some(x)) => format!("Imported {x} from GNOME Music"),
        (None, None) => "Nothing to import from GNOME Music".to_string(),
    }
}

/// Reads GNOME Music's data and maps every reference to a local path
/// (worker thread). `Ok(None)` when no database exists.
pub fn load() -> Result<Option<Found>, String> {
    let Some(db) = find_db() else { return Ok(None) };
    load_from(&db).map(Some)
}

pub fn load_from(db: &Path) -> Result<Found, String> {
    let data = read(db)?;
    let pending = unresolved(&data);
    let resolved = if pending.is_empty() {
        Default::default()
    } else {
        tracker::Endpoint::connect()
            .map(|ep| resolve_in_index(&ep, &pending))
            .unwrap_or_default()
    };
    let favorites = data
        .favorites
        .iter()
        .filter_map(|r| to_path(r, &resolved))
        .collect();
    let playlists = data
        .playlists
        .into_iter()
        .map(|p| {
            (
                p.title,
                p.entries
                    .iter()
                    .filter_map(|r| to_path(r, &resolved))
                    .collect(),
            )
        })
        .collect();
    Ok(Found {
        favorites,
        playlists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const FAV_OUT: &str = "Results:\n  @urn%3Asong1|file%3A%2F%2F%2Fmusic%2Fa%2520b%2Fone.mp3\n  @urn%3Asong2|\n  @urn%3Asong1|file%3A%2F%2F%2Fmusic%2Fa%2520b%2Fone.mp3\n\n";
    const PL_OUT: &str = "Results:\n  @urn%3Apl1|Road%20Trip%7C%2C|1.0|file%3A%2F%2F%2Fmusic%2Ftwo.flac\n  @urn%3Apl1|Road%20Trip%7C%2C|0.0|file%3A%2F%2F%2Fmusic%2Fa%2520b%2Fone.mp3\n  @urn%3Apl2||0|urn%3Asong9\n\n";

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("a%20b%7C%2C%C3%A9"), "a b|,é");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
    }

    #[test]
    fn favorites() {
        assert_eq!(
            parse_favorites(FAV_OUT),
            vec![
                "file:///music/a%20b/one.mp3".to_string(),
                "urn:song2".to_string()
            ]
        );
        assert!(parse_favorites("Results:\n\n").is_empty());
    }

    #[test]
    fn playlists() {
        let p = parse_playlists(PL_OUT);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].title, "Road Trip|,");
        assert_eq!(
            p[0].entries,
            vec!["file:///music/a%20b/one.mp3", "file:///music/two.flac"]
        );
        assert_eq!(p[1].title, "Playlist");
        assert_eq!(p[1].entries, vec!["urn:song9"]);
    }

    #[test]
    fn paths_and_resolution() {
        let data = GnomeMusicData {
            favorites: vec![
                "file:///music/a%20b/one.mp3".into(),
                "urn:song2".into(),
                "urn:bad iri".into(),
            ],
            playlists: vec![GnomePlaylist {
                id: "x".into(),
                title: "t".into(),
                entries: vec!["urn:song2".into(), "urn:song3".into()],
            }],
        };
        assert_eq!(
            unresolved(&data),
            vec!["urn:song2".to_string(), "urn:song3".to_string()]
        );
        let mut map = HashMap::new();
        map.insert(
            "urn:song2".to_string(),
            "file:///music/two.flac".to_string(),
        );
        assert_eq!(
            to_path("file:///music/a%20b/one.mp3", &map),
            Some(PathBuf::from("/music/a b/one.mp3"))
        );
        assert_eq!(
            to_path("urn:song2", &map),
            Some(PathBuf::from("/music/two.flac"))
        );
        assert_eq!(to_path("urn:song3", &map), None);
        assert!(!safe_iri("urn:x> } DELETE"));
    }

    #[test]
    fn candidates() {
        if std::env::var_os(DB_ENV).is_some() {
            return;
        }
        let c = candidate_dirs(Path::new("/c"), Path::new("/h"));
        assert_eq!(c[0], PathBuf::from("/c/gnome-music/db"));
        assert_eq!(
            c[1],
            PathBuf::from("/h/.var/app/org.gnome.Music/cache/gnome-music/db")
        );
    }

    /// Creates a scratch Nepomuk database with `tracker3` (when installed)
    /// and reads it back through the real code path.
    #[test]
    fn scratch_database() {
        if Command::new("tracker3").arg("--version").output().is_err() {
            eprintln!("tracker3 not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("db");
        if !create_scratch_db(&db) {
            eprintln!("cannot create a scratch database; skipping");
            return;
        }
        let before = std::fs::read(db.join("meta.db")).unwrap();
        let data = read(&db).unwrap();
        assert_eq!(
            std::fs::read(db.join("meta.db")).unwrap(),
            before,
            "original database must not change"
        );
        assert_eq!(
            data.favorites,
            vec!["file:///music/a%20b/one.mp3".to_string()]
        );
        assert_eq!(data.playlists.len(), 1);
        assert_eq!(data.playlists[0].title, "Road, Trip");
        assert_eq!(
            data.playlists[0].entries,
            vec!["file:///music/a%20b/one.mp3", "file:///music/two.flac"]
        );
    }

    /// Runs `tracker3 endpoint -o nepomuk` on a private `dbus-run-session`
    /// bus to create the database and insert sample data.
    fn create_scratch_db(db: &Path) -> bool {
        let script = format!(
            r#"tracker3 endpoint -b org.helix.MusicTest -o nepomuk -d '{db}' --session >/dev/null 2>&1 &
P=$!
for i in $(seq 50); do tracker3 sparql -b org.helix.MusicTest -q 'ASK {{}}' >/dev/null 2>&1 && break; sleep 0.1; done
tracker3 sparql -b org.helix.MusicTest -u -q '
INSERT DATA {{
 <urn:song1> a nmm:MusicPiece, nfo:Audio ; nao:hasTag nao:predefined-tag-favorite ; nie:isStoredAs <file:///music/a%20b/one.mp3> .
 <file:///music/a%20b/one.mp3> a nfo:FileDataObject ; nie:url "file:///music/a%20b/one.mp3" .
 <urn:pl1> a nmm:Playlist ; nie:title "Road, Trip" ;
   nfo:hasMediaFileListEntry [ a nfo:MediaFileListEntry ; nfo:entryUrl "file:///music/two.flac" ; nfo:listPosition 1 ] ,
   [ a nfo:MediaFileListEntry ; nfo:entryUrl "file:///music/a%20b/one.mp3" ; nfo:listPosition 0 ] .
}}'
R=$?
kill $P; wait $P
exit $R"#,
            db = db.display()
        );
        Command::new("/usr/bin/dbus-run-session")
            .args(["--", "sh", "-c", &script])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && db.join("meta.db").is_file()
    }
}
