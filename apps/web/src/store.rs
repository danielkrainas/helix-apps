//! Small JSON persistence helpers.

use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;

/// Loads `path` as JSON, returning `T::default()` when missing or invalid.
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Serialises `value` to a string (cheap, done on the caller's thread).
pub fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// Atomically writes `contents` to `path` (temp file + rename).
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

enum Job {
    Write(std::path::PathBuf, String),
    Flush(std::sync::mpsc::Sender<()>),
}

fn writer() -> &'static std::sync::Mutex<std::sync::mpsc::Sender<Job>> {
    static WRITER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Job>>> =
        std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::spawn(move || {
            for job in rx {
                match job {
                    Job::Write(path, contents) => {
                        if let Err(e) = write_atomic(&path, &contents) {
                            eprintln!("helix-web: could not write {}: {e}", path.display());
                        }
                    }
                    Job::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        std::sync::Mutex::new(tx)
    })
}

/// Writes on a single background writer thread (in submission order) so the
/// UI never blocks on disk I/O.
pub fn write_in_background(path: std::path::PathBuf, contents: String) {
    if let Ok(tx) = writer().lock() {
        let _ = tx.send(Job::Write(path, contents));
    }
}

/// Blocks until every queued write has hit the disk (used at quit).
pub fn flush() {
    let (tx, rx) = std::sync::mpsc::channel();
    if let Ok(w) = writer().lock() {
        if w.send(Job::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
        }
    }
}

/// Current Unix time in seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub fn temp_dir(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helix-web-test-{}-{tag}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_missing() {
        let dir = temp_dir("store");
        let p = dir.join("x.json");
        let v: Vec<u32> = load(&p);
        assert!(v.is_empty());
        write_atomic(&p, &to_json(&vec![1u32, 2])).unwrap();
        let v: Vec<u32> = load(&p);
        assert_eq!(v, vec![1, 2]);
        std::fs::write(&p, "garbage").unwrap();
        let v: Vec<u32> = load(&p);
        assert!(v.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
