//! Background journal reading via `journalctl`.

use crate::category::Category;
use crate::journal::{self, Boot, Entry, QueryArgs};
use crate::search::Search;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex};

/// The journalctl program; tests substitute a fake script.
fn journalctl() -> std::ffi::OsString {
    #[cfg(test)]
    if let Some(p) = std::env::var_os("HELIX_LOGS_TEST_JOURNALCTL") {
        return p;
    }
    "journalctl".into()
}

/// Messages from a running query to the UI.
#[derive(Debug)]
pub enum Msg {
    /// Matching entries, in view order.
    Batch(Vec<Entry>),
    /// No more entries.
    Done,
    /// New entries written since the query started (oldest first); only
    /// sent when the query was started with live following.
    Live(Vec<Entry>),
    /// journalctl said the system journal is not readable.
    LimitedAccess,
    Error(String),
}

pub struct Query {
    pub args: QueryArgs,
    pub category: Category,
    pub search: Search,
    pub uid: u32,
}

/// Owning handle of a running query. Dropping it stops the worker and kills
/// its journalctl process.
pub struct Handle {
    more: mpsc::Sender<usize>,
}

impl Handle {
    /// Asks for `n` more matching entries.
    pub fn request(&self, n: usize) {
        let _ = self.more.send(n);
    }
}

#[derive(Default)]
struct FollowSlot {
    child: Option<Child>,
    cancelled: bool,
}

/// Owning handle of the live-follow `journalctl --follow` process.
/// Dropping it kills the process, which ends the reader thread.
pub struct Follow {
    slot: Arc<Mutex<FollowSlot>>,
}

impl Drop for Follow {
    fn drop(&mut self) {
        if let Ok(mut s) = self.slot.lock() {
            s.cancelled = true;
            if let Some(mut c) = s.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

pub fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX)
}

/// Whether the current user can read the system journal (by group
/// membership).
pub fn system_logs_readable() -> bool {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let groups = std::fs::read_to_string("/etc/group").unwrap_or_default();
    journal::can_read_system_journal(current_uid(), &journal::parse_proc_groups(&status), &groups)
}

/// Lists boots, newest first. Falls back to just the current boot.
pub fn list_boots() -> Vec<Boot> {
    let run = |json: bool| -> Option<String> {
        let mut c = Command::new(journalctl());
        c.args(["--no-pager", "--list-boots"]);
        if json {
            c.args(["-o", "json"]);
        }
        let out = c.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let boots = run(true)
        .and_then(|s| journal::parse_boots_json(&s))
        .filter(|b| !b.is_empty())
        .or_else(|| {
            run(false)
                .map(|s| journal::parse_boots_text(&s))
                .filter(|b| !b.is_empty())
        });
    boots.unwrap_or_else(|| {
        vec![Boot {
            index: 0,
            id: String::new(),
            first: None,
            last: None,
            range_text: None,
        }]
    })
}

/// Starts a query. The first batch is only produced once requested.
/// With `follow`, entries written after the query started are streamed as
/// [`Msg::Live`] until the returned [`Follow`] is dropped.
pub fn start(q: Query, follow: bool) -> (Handle, Option<Follow>, async_channel::Receiver<Msg>) {
    let (more_tx, more_rx) = mpsc::channel::<usize>();
    let (tx, rx) = async_channel::unbounded();
    let slot = follow.then(|| Arc::new(Mutex::new(FollowSlot::default())));
    let follow = slot.clone().map(|slot| Follow { slot });
    std::thread::spawn(move || run(q, slot, more_rx, tx));
    (Handle { more: more_tx }, follow, rx)
}

/// Cursor of the newest entry of the queried boot (unfiltered).
fn tail_cursor(args: &QueryArgs) -> Option<String> {
    let out = Command::new(journalctl())
        .args(args.tail_args())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    Entry::parse(line).ok()?.get("__CURSOR").map(str::to_string)
}

/// Spawns `journalctl --follow` and a thread forwarding matching entries.
fn spawn_follow(
    q: &Query,
    cursor: Option<&str>,
    slot: Arc<Mutex<FollowSlot>>,
    tx: async_channel::Sender<Msg>,
) {
    let Ok(mut guard) = slot.lock() else { return };
    if guard.cancelled {
        return;
    }
    let Ok(mut child) = Command::new(journalctl())
        .args(q.args.follow_args(cursor))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    guard.child = Some(child);
    drop(guard);
    let (category, search, uid) = (q.category, q.search.clone(), q.uid);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::with_capacity(4096);
        let mut batch = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buf);
            if let Ok(entry) = Entry::parse(line.trim()) {
                if category.matches(&entry, uid) && search.matches(&entry) {
                    batch.push(entry);
                }
            }
            // Coalesce bursts: send once the already-read data is consumed.
            if (reader.buffer().is_empty() || batch.len() >= 500)
                && !batch.is_empty()
                && tx
                    .send_blocking(Msg::Live(std::mem::take(&mut batch)))
                    .is_err()
            {
                break;
            }
        }
        // Receiver gone or process ended: make sure the child is reaped.
        if let Ok(mut s) = slot.lock() {
            if let Some(mut c) = s.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    });
}

fn run(
    q: Query,
    follow: Option<Arc<Mutex<FollowSlot>>>,
    more: mpsc::Receiver<usize>,
    tx: async_channel::Sender<Msg>,
) {
    // Learn where "now" is before the paged query starts, so nothing written
    // in between is missed (overlap is de-duplicated by the UI).
    if let Some(slot) = follow {
        let cursor = tail_cursor(&q.args);
        spawn_follow(&q, cursor.as_deref(), slot, tx.clone());
    }
    let mut child = match Command::new(journalctl())
        .args(q.args.to_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send_blocking(Msg::Error(format!("Could not run journalctl: {e}")));
            return;
        }
    };

    let stderr_text = Arc::new(Mutex::new(String::new()));
    let mut err_thread = None;
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        let text = stderr_text.clone();
        err_thread = Some(std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if journal::is_limited_access_hint(&line) {
                    let _ = tx.send_blocking(Msg::LimitedAccess);
                } else if let Ok(mut t) = text.lock() {
                    t.push_str(&line);
                    t.push('\n');
                }
            }
        }));
    }

    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let mut reader = BufReader::with_capacity(256 * 1024, stdout);
    let mut buf = Vec::with_capacity(4096);
    let mut produced = 0usize;
    let mut eof = false;

    'outer: while let Ok(mut want) = more.recv() {
        // Coalesce queued requests.
        while let Ok(n) = more.try_recv() {
            want += n;
        }
        let mut batch = Vec::new();
        let mut scanned = 0usize;
        while batch.len() < want {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => {
                    eof = true;
                    break;
                }
                Ok(_) => {}
            }
            scanned += 1;
            if scanned.is_multiple_of(2000) {
                // Stop promptly when the query was dropped.
                if let Err(TryRecvError::Disconnected) = more.try_recv().map(|n| want += n) {
                    break 'outer;
                }
                // Long searches: show what we have so far.
                if !batch.is_empty() && scanned.is_multiple_of(20_000) {
                    want -= batch.len();
                    produced += batch.len();
                    if tx
                        .send_blocking(Msg::Batch(std::mem::take(&mut batch)))
                        .is_err()
                    {
                        break 'outer;
                    }
                }
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(entry) = Entry::parse(line) else {
                continue;
            };
            if q.category.matches(&entry, q.uid) && q.search.matches(&entry) {
                batch.push(entry);
            }
        }
        produced += batch.len();
        if tx.send_blocking(Msg::Batch(batch)).is_err() {
            break;
        }
        if eof {
            let status = child.wait().ok();
            if let Some(t) = err_thread.take() {
                let _ = t.join();
            }
            let failed = status.is_some_and(|s| !s.success());
            let err = stderr_text
                .lock()
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            if failed && produced == 0 && !err.is_empty() && !err.contains("No entries") {
                let _ = tx.send_blocking(Msg::Error(err));
            }
            let _ = tx.send_blocking(Msg::Done);
            return;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Follow mode against a fake journalctl: the paged query, then the
    /// live stream starting after the tail cursor.
    #[test]
    fn follow_with_fake_journalctl() {
        let dir = std::env::temp_dir().join(format!("helix-logs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("journalctl");
        let line = |c: &str, m: &str| {
            format!(r#"{{"__CURSOR":"{c}","MESSAGE":"{m}","PRIORITY":"6","_TRANSPORT":"journal"}}"#)
        };
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n\
                 *--follow*--after-cursor=c2*) echo '{l3}'; echo '{l2}'; exec sleep 30 ;;\n\
                 *--follow*) exit 9 ;;\n\
                 *--lines=1*) echo '{l2}' ;;\n\
                 *) echo '{l1}'; echo '{l2}' ;;\nesac\n",
                l1 = line("c1", "one"),
                l2 = line("c2", "two"),
                l3 = line("c3", "three"),
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("HELIX_LOGS_TEST_JOURNALCTL", &script);

        let q = Query {
            args: QueryArgs::default(),
            category: Category::All,
            search: Search::default(),
            uid: 1000,
        };
        let (h, follow, rx) = start(q, true);
        h.request(10);
        let (mut loaded, mut live, mut done) = (Vec::new(), Vec::new(), false);
        while live.len() < 2 || !done {
            match rx.recv_blocking().expect("channel open while following") {
                Msg::Batch(v) => loaded.extend(v.into_iter().map(|e| e.message().to_string())),
                Msg::Live(v) => live.extend(v.into_iter().map(|e| e.message().to_string())),
                Msg::Done => done = true,
                m => panic!("{m:?}"),
            }
        }
        assert_eq!(loaded, ["one", "two"]);
        assert_eq!(live, ["three", "two"]); // duplicates are removed by live::Live
        drop(h);
        drop(follow); // kills the fake `sleep`, closing the channel
        while let Ok(m) = rx.recv_blocking() {
            done |= matches!(m, Msg::Done);
        }
        assert!(done);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs against the host journal; `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_query() {
        let mut args = QueryArgs {
            reverse: true,
            ..Default::default()
        };
        Category::All.prefilter(&mut args, current_uid());
        let q = Query {
            args,
            category: Category::All,
            search: Search::default(),
            uid: current_uid(),
        };
        let (h, _follow, rx) = start(q, false);
        h.request(50);
        let mut got = 0;
        while let Ok(m) = rx.recv_blocking() {
            match m {
                Msg::Batch(v) => {
                    got += v.len();
                    break;
                }
                Msg::Error(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(got > 0);
        drop(h);
        assert!(!list_boots().is_empty());
    }
}
