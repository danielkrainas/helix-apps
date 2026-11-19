//! Acting on one particular process, not on whatever process has its PID
//! by then. The user picks a process, confirms, perhaps authenticates; by
//! the time the signal is sent, the process may have exited and its PID
//! been given to another. So a process is named by PID *and* start time
//! (field 22 of `/proc/<pid>/stat`, clock ticks after boot), opened as a
//! pidfd, checked, and signalled through the pidfd (`pidfd_send_signal`),
//! which reaches that process or none.
//!
//! Shared by the app and the privileged helper (`src/bin/helper.rs`).

// Compiled into both binaries; each uses a different part.
#![allow(dead_code)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// Start time (clock ticks after boot) of `pid`, `None` if there is no
/// such process.
pub fn start_time(pid: i32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_start_time(&text)
}

/// Field 22 of a `/proc/<pid>/stat` line. The command name before it is
/// in parentheses and may contain spaces and parentheses, so fields are
/// counted from the last `)`.
pub fn parse_start_time(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // The first field after the name is field 3.
    rest.split_whitespace().nth(22 - 3)?.parse().ok()
}

fn no_process() -> io::Error {
    io::Error::from_raw_os_error(libc::ESRCH)
}

/// An open process: a pidfd for a process whose start time was checked.
pub struct Process {
    fd: OwnedFd,
    pid: i32,
}

impl Process {
    /// Opens `pid` if it is the process that started at `start` (in
    /// clock ticks after boot); `ESRCH` otherwise.
    pub fn open(pid: i32, start: u64) -> io::Result<Process> {
        if pid <= 0 {
            return Err(no_process());
        }
        // SAFETY: plain system call; a non-negative result is a new fd
        // owned here.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by pidfd_open and is not owned
        // elsewhere.
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        // The pidfd names the process that had `pid` when it was opened;
        // if that PID still belongs to the process started at `start`
        // now, it is the same one (a later process cannot have started
        // earlier).
        if start_time(pid) != Some(start) {
            return Err(no_process());
        }
        Ok(Process { fd, pid })
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Whether the process has exited (its pidfd is readable then).
    pub fn exited(&self) -> bool {
        let mut p = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, no waiting.
        let n = unsafe { libc::poll(&mut p, 1, 0) };
        n > 0 && p.revents & libc::POLLIN != 0
    }

    /// Sends `signal` to this process only.
    pub fn signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: plain system call on an fd owned here; no siginfo.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Sets the nice value. There is no pidfd form of `setpriority`, so
    /// the PID is used and the process checked to be still running
    /// afterwards: a running process keeps its PID, so it was this one.
    pub fn renice(&self, nice: i32) -> io::Result<()> {
        if self.exited() {
            return Err(no_process());
        }
        // SAFETY: plain system call on a single pid > 0.
        let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, self.pid as libc::id_t, nice) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        if self.exited() {
            return Err(no_process());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn parses_start_time() {
        let stat = "4242 (a) b (c)) S 1 4242 4242 0 -1 4194304 100 0 0 0 7 3 0 0 20 0 \
                    1 0 98765 1000000 50 18446744073709551615";
        assert_eq!(parse_start_time(stat), Some(98765));
        assert_eq!(parse_start_time("1 (x) S 0"), None);
        assert_eq!(parse_start_time("garbage"), None);
    }

    #[test]
    fn acts_only_on_the_process_that_was_picked() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let start = start_time(pid).unwrap();
        // Another start time: some other process with this PID.
        assert_eq!(
            Process::open(pid, start + 1)
                .err()
                .and_then(|e| e.raw_os_error()),
            Some(libc::ESRCH)
        );
        let p = Process::open(pid, start).unwrap();
        assert!(!p.exited());
        p.renice(7).unwrap();
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        assert_eq!(
            text[text.rfind(')').unwrap() + 1..]
                .split_whitespace()
                .nth(16),
            Some("7")
        );
        p.signal(libc::SIGKILL).unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
        // Gone (and reaped): the pidfd still names it, and nothing else
        // is reached.
        assert!(p.exited());
        assert!(p.renice(5).is_err());
        assert_eq!(
            p.signal(libc::SIGKILL).err().and_then(|e| e.raw_os_error()),
            Some(libc::ESRCH)
        );
        assert!(Process::open(pid, start).is_err());
    }
}
