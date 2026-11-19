//! Making thumbnails that are not in the freedesktop thumbnail cache yet
//! (the Thumbnail Managing Standard): the installed thumbnailer for the
//! file's type (`thumbnailers/*.thumbnailer` in the data folders), or
//! gdk-pixbuf for pictures without one. The result goes to the shared
//! cache (`$XDG_CACHE_HOME/thumbnails/large/<md5 of the URI>.png`) with
//! the `Thumb::URI` and `Thumb::MTime` keys, so GIO's `thumbnail::path`
//! finds it and other apps reuse it.
//!
//! No failure is written to the cache's `fail/` folder: a file whose
//! thumbnail could not be made (a thumbnailer that crashed or took too
//! long, say on a slow share) is retried the next time Files runs, or
//! when the file changes. Within a run, `thumbs.rs` remembers failures.
//!
//! Thumbnailers run in a bubblewrap sandbox (`sandbox.rs`), never
//! outside one.
//!
//! Everything here runs on a worker thread.

use super::sandbox;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Size of a "large" thumbnail.
pub const SIZE: i32 = 256;
/// How long a thumbnailer may run (a large video on a network share can
/// take a while).
const TIMEOUT: Duration = Duration::from_secs(30);
/// Largest thumbnail file a thumbnailer may hand back.
const MAX_OUTPUT: u64 = 32 << 20;

/// What a thumbnail is made from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Source {
    /// Local path; `None` for a remote file, whose GVfs FUSE path
    /// [`make`] finds (that asks the GVfs daemon, so not on the main
    /// thread).
    pub path: Option<PathBuf>,
    pub uri: String,
    pub mime: String,
    /// Modification time, seconds since the epoch (`Thumb::MTime`).
    pub mtime: u64,
}

#[derive(Debug, Clone)]
struct Thumbnailer {
    mimes: Vec<String>,
    exec: String,
}

/// Parses a `.thumbnailer` file (a desktop-entry style key file).
fn parse(text: &str) -> Option<(Thumbnailer, Option<String>)> {
    let get = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        })
    };
    let exec = get("Exec")?;
    let mimes = get("MimeType")?
        .split(';')
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect();
    Some((Thumbnailer { mimes, exec }, get("TryExec")))
}

/// The installed thumbnailers, the user's first (one whose `TryExec` is
/// missing is skipped).
fn thumbnailers() -> &'static [Thumbnailer] {
    static LIST: OnceLock<Vec<Thumbnailer>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut dirs = vec![glib::user_data_dir()];
        dirs.extend(glib::system_data_dirs());
        let mut out = Vec::new();
        for dir in dirs {
            let Ok(read) = std::fs::read_dir(dir.join("thumbnailers")) else {
                continue;
            };
            let mut entries: Vec<_> = read.flatten().map(|e| e.path()).collect();
            entries.sort();
            for path in entries {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Some((t, try_exec)) = parse(&text) else {
                    continue;
                };
                if try_exec.is_some_and(|t| {
                    glib::find_program_in_path(&t).is_none() && !Path::new(&t).is_file()
                }) {
                    continue;
                }
                out.push(t);
            }
        }
        out
    })
}

fn thumbnailer_for(mime: &str) -> Option<&'static Thumbnailer> {
    thumbnailers()
        .iter()
        .find(|t| t.mimes.iter().any(|m| m == mime))
}

/// Whether a thumbnail can be made for MIME type `mime`.
pub fn can_make(mime: &str) -> bool {
    thumbnailer_for(mime).is_some() || mime.starts_with("image/")
}

/// The cache file for `uri` (its MD5, `.png`, in `thumbnails/large`).
fn cache_path_in(cache: &Path, uri: &str) -> PathBuf {
    cache.join("thumbnails").join("large").join(cache_name(uri))
}

fn cache_name(uri: &str) -> String {
    let sum = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)
        .map(|s| s.to_string())
        .unwrap_or_default();
    format!("{sum}.png")
}

/// The `tEXt` chunks of a PNG file, as (key, value).
fn png_text(png: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return out;
    }
    let mut at = 8;
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]) as usize;
        let kind = &png[at + 4..at + 8];
        let end = (at + 8).saturating_add(len).min(png.len());
        let data = &png[at + 8..end];
        if kind == b"tEXt" {
            if let Some(nul) = data.iter().position(|&b| b == 0) {
                out.push((
                    String::from_utf8_lossy(&data[..nul]).into_owned(),
                    String::from_utf8_lossy(&data[nul + 1..]).into_owned(),
                ));
            }
        }
        if kind == b"IEND" {
            break;
        }
        at = end + 4;
    }
    out
}

/// Whether `png` is a valid thumbnail of the file `uri` modified at
/// `mtime`: its `Thumb::MTime` matches (and its `Thumb::URI`, if set),
/// as GIO's `thumbnail::is-valid` checks.
fn is_valid(png: &[u8], uri: &str, mtime: u64) -> bool {
    let text = png_text(png);
    let get = |k: &str| {
        text.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    get("Thumb::MTime").and_then(|v| v.parse::<u64>().ok()) == Some(mtime)
        && get("Thumb::URI").is_none_or(|u| u == uri)
}

/// A valid cached thumbnail of `uri` (modified at `mtime`): its path and
/// contents. Looked up when a tile is shown rather than for every file
/// as a folder loads (`thumbnail::path` cost Files more than half of a
/// big folder's loading time).
pub fn lookup(uri: &str, mtime: u64) -> Option<(PathBuf, Vec<u8>)> {
    lookup_in(&glib::user_cache_dir(), uri, mtime)
}

fn lookup_in(cache: &Path, uri: &str, mtime: u64) -> Option<(PathBuf, Vec<u8>)> {
    let name = cache_name(uri);
    for size in ["large", "x-large", "xx-large", "normal"] {
        let path = cache.join("thumbnails").join(size).join(&name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_OUTPUT {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if is_valid(&bytes, uri, mtime) {
            return Some((path, bytes));
        }
    }
    None
}

/// The thumbnailer's command line for `src`, writing to `out`.
fn command(exec: &str, src: &Source, out: &Path) -> Option<Vec<String>> {
    let argv = glib::shell_parse_argv(exec).ok()?;
    let args: Vec<String> = argv
        .iter()
        .map(|a| {
            let a = a.to_string_lossy();
            let mut s = String::new();
            let mut chars = a.chars();
            while let Some(c) = chars.next() {
                if c != '%' {
                    s.push(c);
                    continue;
                }
                match chars.next() {
                    Some('i') => {
                        if let Some(p) = &src.path {
                            s.push_str(&p.to_string_lossy());
                        }
                    }
                    Some('u') => s.push_str(&src.uri),
                    Some('o') => s.push_str(&out.to_string_lossy()),
                    Some('s') => s.push_str(&SIZE.to_string()),
                    Some('%') => s.push('%'),
                    Some(other) => {
                        s.push('%');
                        s.push(other);
                    }
                    None => s.push('%'),
                }
            }
            s
        })
        .collect();
    (!args.is_empty()).then_some(args)
}

/// Runs `cmd` (the sandboxed thumbnailer `program`), killing it after
/// [`TIMEOUT`]; true if it exited fine.
fn run_command(mut cmd: std::process::Command, program: &str) -> bool {
    let Ok(mut child) = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("helix-files: thumbnailer {program} took too long");
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => return false,
        }
    }
}

/// Runs thumbnailer command line `exec` for `src` in the [`sandbox`]: the
/// file appears there as `/tmp/file-to-thumbnail.<ext>` (the extension
/// kept, some thumbnailers go by it) and the thumbnail is written to
/// `/tmp/thumbnail.png`, in a private temporary folder outside. Nothing
/// runs where bubblewrap cannot make a sandbox.
fn run_sandboxed(exec: &str, src: &Source) -> Option<Pixbuf> {
    let bwrap = sandbox::bwrap()?;
    let input = src.path.as_deref()?;
    let ext: String = input
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(16)
        .collect();
    let name = if ext.is_empty() {
        "file-to-thumbnail".to_owned()
    } else {
        format!("file-to-thumbnail.{ext}")
    };
    let inside_input = format!("{}/{name}", sandbox::TMP);
    let inside_output = format!("{}/thumbnail.png", sandbox::TMP);
    let inside = Source {
        path: Some(PathBuf::from(&inside_input)),
        uri: format!("file://{inside_input}"),
        mime: src.mime.clone(),
        mtime: src.mtime,
    };
    let args = command(exec, &inside, Path::new(&inside_output))?;
    let outdir = sandbox::tempdir().ok()?;
    let seccomp = sandbox::filter_fd();
    let gst = sandbox::gst_cache_dir();
    let mut cmd = std::process::Command::new(bwrap);
    cmd.args(sandbox::bwrap_args(
        &outdir,
        input,
        &inside_input,
        seccomp.as_ref().map(|f| f.as_raw_fd()),
        gst.as_deref(),
    ));
    if let Some(fd) = &seccomp {
        sandbox::inherit(&mut cmd, fd.as_raw_fd());
    }
    cmd.args(&args);
    let ok = run_command(cmd, &args[0]);
    let pixbuf = if ok {
        sandbox::read_output(&outdir.join("thumbnail.png"), MAX_OUTPUT).and_then(|bytes| {
            let stream = gtk::gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(bytes));
            Pixbuf::from_stream(&stream, gtk::gio::Cancellable::NONE).ok()
        })
    } else {
        None
    };
    sandbox::remove_tempdir(&outdir);
    pixbuf
}

/// A picture no larger than [`SIZE`] on its longer side.
fn fit(pixbuf: Pixbuf) -> Option<Pixbuf> {
    let (w, h) = (pixbuf.width(), pixbuf.height());
    if w <= SIZE && h <= SIZE {
        return Some(pixbuf);
    }
    let scale = f64::from(SIZE) / f64::from(w.max(h));
    let (nw, nh) = (
        ((f64::from(w) * scale).round() as i32).max(1),
        ((f64::from(h) * scale).round() as i32).max(1),
    );
    pixbuf.scale_simple(nw, nh, gtk::gdk_pixbuf::InterpType::Bilinear)
}

/// A temporary name next to `target`, unique to this process.
fn temp_next_to(target: &Path) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(
        ".{name}.helix-files-{}-{n}.png",
        std::process::id()
    ))
}

/// Makes and caches the thumbnail of `src`; its cache path, or `None` if
/// no thumbnail could be made.
pub fn make(src: &Source) -> Option<PathBuf> {
    let mut src = src.clone();
    if src.path.is_none() {
        src.path = Some(gtk::gio::prelude::FileExt::path(&gtk::gio::File::for_uri(
            &src.uri,
        ))?);
    }
    make_in(&glib::user_cache_dir(), &src)
}

fn make_in(cache: &Path, src: &Source) -> Option<PathBuf> {
    let target = cache_path_in(cache, &src.uri);
    let dir = target.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let pixbuf = match thumbnailer_for(&src.mime) {
        Some(t) => run_sandboxed(&t.exec, src),
        None if src.mime.starts_with("image/") => {
            Pixbuf::from_file_at_scale(src.path.as_deref()?, SIZE, SIZE, true)
                .ok()
                .map(|p| p.apply_embedded_orientation().unwrap_or(p))
        }
        None => None,
    };
    let pixbuf = fit(pixbuf?)?;
    let tmp = temp_next_to(&target);
    let mtime = src.mtime.to_string();
    let saved = pixbuf.savev(
        &tmp,
        "png",
        &[
            ("tEXt::Thumb::URI", src.uri.as_str()),
            ("tEXt::Thumb::MTime", mtime.as_str()),
            ("tEXt::Software", "Helix Files"),
        ],
    );
    if saved.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if std::fs::rename(&tmp, &target).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_valid_thumbnails() {
        let dir = std::env::temp_dir().join(format!("helix-files-lookup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let uri = "file:///photos/a.jpg";
        let save = |size: &str, mtime: &str| {
            let p = dir.join("thumbnails").join(size).join(cache_name(uri));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let pb = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 4, 4).unwrap();
            pb.savev(
                &p,
                "png",
                &[("tEXt::Thumb::URI", uri), ("tEXt::Thumb::MTime", mtime)],
            )
            .unwrap();
            p
        };
        assert!(lookup_in(&dir, uri, 100).is_none());
        // Stale (file changed since): not used.
        save("large", "99");
        assert!(lookup_in(&dir, uri, 100).is_none());
        // A fresh one in another size folder is found.
        let normal = save("normal", "100");
        assert_eq!(lookup_in(&dir, uri, 100).map(|(p, _)| p), Some(normal));
        // Another file has no thumbnail.
        assert!(lookup_in(&dir, "file:///photos/b.jpg", 100).is_none());
        // A thumbnail naming another URI is not this file's.
        let png = std::fs::read(dir.join("thumbnails/normal").join(cache_name(uri))).unwrap();
        assert!(is_valid(&png, uri, 100));
        assert!(!is_valid(&png, "file:///elsewhere.jpg", 100));
        assert!(!is_valid(b"not a png", uri, 100));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_name_follows_the_standard() {
        // The Thumbnail Managing Standard's own example.
        assert_eq!(
            cache_name("file:///home/jens/photos/me.png"),
            "c6ee772d9e49320e97ec29a7eb5b1697.png"
        );
    }

    #[test]
    fn thumbnailer_files_and_command_lines() {
        let (t, try_exec) = parse(
            "[Thumbnailer Entry]\nTryExec=gst-video-thumbnailer\n\
             Exec=gst-video-thumbnailer -s %s %u %o\nMimeType=video/mp4;video/webm;\n",
        )
        .unwrap();
        assert_eq!(try_exec.as_deref(), Some("gst-video-thumbnailer"));
        assert_eq!(t.mimes, ["video/mp4", "video/webm"]);
        let src = Source {
            path: Some("/home/u/My Videos/a b.mp4".into()),
            uri: "file:///home/u/My%20Videos/a%20b.mp4".into(),
            mime: "video/mp4".into(),
            mtime: 1,
        };
        let args = command(&t.exec, &src, Path::new("/tmp/o.png")).unwrap();
        assert_eq!(
            args,
            [
                "gst-video-thumbnailer",
                "-s",
                "256",
                "file:///home/u/My%20Videos/a%20b.mp4",
                "/tmp/o.png"
            ]
        );
        // %i is the path, kept as one argument; %% is a percent sign.
        let args = command("thumb --in %i --q 90%%", &src, Path::new("/o")).unwrap();
        assert_eq!(
            args,
            ["thumb", "--in", "/home/u/My Videos/a b.mp4", "--q", "90%"]
        );
        assert!(parse("[Thumbnailer Entry]\nMimeType=a/b;\n").is_none());
    }

    #[test]
    fn large_pictures_are_scaled_to_fit() {
        let p = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 1000, 500).unwrap();
        let f = fit(p).unwrap();
        assert_eq!((f.width(), f.height()), (256, 128));
        let p = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 100, 50).unwrap();
        let f = fit(p).unwrap();
        assert_eq!((f.width(), f.height()), (100, 50));
    }

    /// A picture without a thumbnailer is thumbnailed through gdk-pixbuf
    /// and saved with the keys GIO checks.
    #[test]
    fn makes_a_cached_thumbnail_with_its_keys() {
        let dir = std::env::temp_dir().join(format!("helix-files-thumb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("pic.bmp");
        let p = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 600, 300).unwrap();
        p.fill(0x3366ccff);
        p.savev(&pic, "bmp", &[]).unwrap();
        // A MIME type no thumbnailer claims, so gdk-pixbuf makes it.
        let src = Source {
            uri: glib::filename_to_uri(&pic, None).unwrap().to_string(),
            path: Some(pic.clone()),
            mime: "image/x-helix-test".into(),
            mtime: 1234,
        };
        let cache = dir.join("cache");
        let target = make_in(&cache, &src).expect("thumbnail made");
        assert_eq!(target, cache_path_in(&cache, &src.uri));
        // No temporary files left behind.
        let left: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        let back = Pixbuf::from_file(&target).unwrap();
        assert_eq!((back.width(), back.height()), (256, 128));
        let text = png_text(&std::fs::read(&target).unwrap());
        assert!(
            text.contains(&("Thumb::URI".into(), src.uri.clone())),
            "{text:?}"
        );
        assert!(
            text.contains(&("Thumb::MTime".into(), "1234".into())),
            "{text:?}"
        );
        // GIO takes it as this file's valid thumbnail only with a matching
        // mtime, which the caller passes from `time::modified`.
        let _ = std::fs::remove_dir_all(&dir);
    }
}
