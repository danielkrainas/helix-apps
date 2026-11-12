//! Background journal reading via `journalctl`.

use crate::category::Category;
use crate::journal::{self, Boot, Entry, QueryArgs};
use crate::search::Search;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex};

const JOURNALCTL: &str = "journalctl";

/// Messages from a running query to the UI.
#[derive(Debug)]
pub enum Msg {
    /// Matching entries, in view order.
    Batch(Vec<Entry>),
    /// No more entries.
    Done,
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

pub fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX)
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
        let mut c = Command::new(JOURNALCTL);
        c.args(["--no-pager", "--list-boots"]);
        if json {
            c.args(["-o", "json"]);
        }
        let out = c.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let boots = run(true)
        .and_then(|s| journal::parse_boots_json(&s))
        .filter(|b| !b.is_empty())
        .or_else(|| run(false).map(|s| journal::parse_boots_text(&s)).filter(|b| !b.is_empty()));
    boots.unwrap_or_else(|| {
        vec![Boot { index: 0, id: String::new(), first: None, last: None, range_text: None }]
    })
}

/// Starts a query. The first batch is only produced once requested.
pub fn start(q: Query) -> (Handle, async_channel::Receiver<Msg>) {
    let (more_tx, more_rx) = mpsc::channel::<usize>();
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || run(q, more_rx, tx));
    (Handle { more: more_tx }, rx)
}

fn run(q: Query, more: mpsc::Receiver<usize>, tx: async_channel::Sender<Msg>) {
    let mut child = match Command::new(JOURNALCTL)
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

    let Some(stdout) = child.stdout.take() else { return };
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
                    if tx.send_blocking(Msg::Batch(std::mem::take(&mut batch))).is_err() {
                        break 'outer;
                    }
                }
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(entry) = Entry::parse(line) else { continue };
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
            let err = stderr_text.lock().map(|t| t.trim().to_string()).unwrap_or_default();
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

    /// Runs against the host journal; `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_query() {
        let mut args = QueryArgs { reverse: true, ..Default::default() };
        Category::All.prefilter(&mut args, current_uid());
        let q = Query { args, category: Category::All, search: Search::default(), uid: current_uid() };
        let (h, rx) = start(q);
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
