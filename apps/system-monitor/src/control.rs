//! Process control: signals and priority changes, limited to processes the
//! current user owns. Privileged actions (polkit) are not implemented.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stop,
    Continue,
    End,
    Kill,
}

impl Action {
    pub fn signal(self) -> libc::c_int {
        match self {
            Action::Stop => libc::SIGSTOP,
            Action::Continue => libc::SIGCONT,
            Action::End => libc::SIGTERM,
            Action::Kill => libc::SIGKILL,
        }
    }

    pub fn needs_confirmation(self) -> bool {
        matches!(self, Action::End | Action::Kill)
    }

    pub fn verb(self) -> &'static str {
        match self {
            Action::Stop => "Stop",
            Action::Continue => "Continue",
            Action::End => "End",
            Action::Kill => "Kill",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    NoSuchProcess,
    NotOwner,
    PermissionDenied,
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoSuchProcess => write!(f, "The process no longer exists"),
            Error::NotOwner => write!(f, "You do not own this process"),
            Error::PermissionDenied => write!(f, "Permission denied"),
            Error::Other(s) => write!(f, "{s}"),
        }
    }
}

fn last_error() -> Error {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ESRCH) => Error::NoSuchProcess,
        Some(libc::EPERM) | Some(libc::EACCES) => Error::PermissionDenied,
        _ => Error::Other(e.to_string()),
    }
}

/// Real UID owning `pid`, from `/proc/<pid>/status`.
pub fn owner(pid: i32) -> Option<u32> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    crate::procfs::parse_pid_status(&text).map(|s| s.uid)
}

/// Succeeds only if `pid` exists and is owned by the current user.
pub fn check_owner(pid: i32) -> Result<(), Error> {
    if pid <= 0 {
        return Err(Error::NoSuchProcess);
    }
    match owner(pid) {
        None => Err(Error::NoSuchProcess),
        Some(uid) if uid == crate::sampler::current_uid() => Ok(()),
        Some(_) => Err(Error::NotOwner),
    }
}

pub fn send(pid: i32, action: Action) -> Result<(), Error> {
    check_owner(pid)?;
    // SAFETY: plain syscall; pid > 0 is checked so no process group is hit.
    if unsafe { libc::kill(pid, action.signal()) } == 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

/// Sets the nice value. Lowering it below the current value needs
/// privileges the app does not request, so the kernel refuses with EPERM/EACCES.
pub fn renice(pid: i32, nice: i32) -> Result<(), Error> {
    check_owner(pid)?;
    let nice = nice.clamp(-20, 19);
    // SAFETY: plain syscall on a single, checked pid.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice) } == 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    fn state_of(pid: i32) -> Option<char> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        crate::procfs::parse_pid_stat(&text).map(|s| s.state)
    }

    fn wait_state(pid: i32, f: impl Fn(char) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if state_of(pid).is_some_and(&f) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    struct Guard(Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn_sleep() -> Guard {
        Guard(Command::new("sleep").arg("30").spawn().expect("spawn sleep"))
    }

    #[test]
    fn stop_continue_end_own_child() {
        let mut child = spawn_sleep();
        let pid = child.0.id() as i32;
        assert_eq!(check_owner(pid), Ok(()));

        send(pid, Action::Stop).unwrap();
        assert!(wait_state(pid, |c| c == 'T'), "child should be stopped");
        send(pid, Action::Continue).unwrap();
        assert!(wait_state(pid, |c| c != 'T'), "child should be continued");

        send(pid, Action::End).unwrap();
        let status = child.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn kill_own_child() {
        let mut child = spawn_sleep();
        let pid = child.0.id() as i32;
        send(pid, Action::Kill).unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(child.0.wait().unwrap().signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn renice_own_child() {
        let child = spawn_sleep();
        let pid = child.0.id() as i32;
        renice(pid, 10).unwrap();
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        assert_eq!(crate::procfs::parse_pid_stat(&text).unwrap().nice, 10);
    }

    #[test]
    fn ownership_checks() {
        // Only the check runs here; no signal is sent to foreign processes.
        assert_eq!(check_owner(0), Err(Error::NoSuchProcess));
        assert_eq!(check_owner(-1), Err(Error::NoSuchProcess));
        assert_eq!(check_owner(i32::MAX), Err(Error::NoSuchProcess));
        if crate::sampler::current_uid() != 0 && owner(1) == Some(0) {
            assert_eq!(check_owner(1), Err(Error::NotOwner));
        }
    }

    #[test]
    fn action_metadata() {
        assert_eq!(Action::Kill.signal(), libc::SIGKILL);
        assert!(Action::End.needs_confirmation());
        assert!(!Action::Stop.needs_confirmation());
    }
}
