//! Process control: signals and priority changes. Processes the current user
//! owns are handled directly; other users' processes (and raising priority,
//! which needs CAP_SYS_NICE) go through the privileged helper run with
//! `pkexec` under the polkit action `org.helix.SystemMonitor.manage-processes`.

use helix_app_common::i18n::tr;
use helix_app_common::trf;
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

    pub fn helper_signal(self) -> crate::helper_args::Signal {
        use crate::helper_args::Signal;
        match self {
            Action::Stop => Signal::Stop,
            Action::Continue => Signal::Cont,
            Action::End => Signal::Term,
            Action::Kill => Signal::Kill,
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
            Error::NoSuchProcess => f.write_str(tr("The process no longer exists")),
            Error::NotOwner => f.write_str(tr("You do not own this process")),
            Error::PermissionDenied => f.write_str(tr("Permission denied")),
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

/// Installed location of the helper; must match the
/// `org.freedesktop.policykit.exec.path` annotation of the polkit action
/// `org.helix.SystemMonitor.manage-processes`
/// (`data/org.helix.SystemMonitor.policy`).
pub const HELPER_PATH: &str = "/usr/libexec/helix-system-monitor/helix-system-monitor-helper";

/// Helper path, overridable with `HELIX_SYSTEM_MONITOR_HELPER` for
/// development installs (pkexec still requires a matching polkit policy).
pub fn helper_path() -> std::path::PathBuf {
    std::env::var_os("HELIX_SYSTEM_MONITOR_HELPER")
        .map(Into::into)
        .unwrap_or_else(|| HELPER_PATH.into())
}

/// Whether a failed direct attempt should be retried through the helper.
pub fn needs_privileges(e: &Error) -> bool {
    matches!(e, Error::NotOwner | Error::PermissionDenied)
}

/// `pkexec` argv for running `command` through the helper.
pub fn pkexec_argv(
    helper: &std::path::Path,
    command: crate::helper_args::Command,
) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = vec!["pkexec".into(), helper.into()];
    v.extend(command.to_args().into_iter().map(Into::into));
    v
}

/// Maps the exit status of `pkexec helper …` to a result.
/// pkexec itself exits 126 when the user dismissed the dialog or was not
/// authorised and 127 when it could not run the helper; the helper's own
/// codes are documented in `src/bin/helper.rs`.
pub fn interpret_exit(code: Option<i32>, stderr: &str) -> Result<(), Error> {
    match code {
        Some(0) => Ok(()),
        Some(126) => Err(Error::Other(
            tr("Authorization was denied or cancelled").into(),
        )),
        Some(127) => Err(Error::Other(
            tr("Not authorized to control this process").into(),
        )),
        Some(3) => Err(Error::NoSuchProcess),
        _ => {
            let msg = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or(tr("The privileged helper failed"));
            Err(Error::Other(
                msg.trim()
                    .trim_start_matches("helix-system-monitor-helper: ")
                    .to_string(),
            ))
        }
    }
}

/// Runs `command` through `pkexec` and the helper. Blocks until the user
/// has answered the authentication dialog, so call it off the main thread.
pub fn run_privileged(command: crate::helper_args::Command) -> Result<(), Error> {
    let helper = helper_path();
    if !helper.exists() {
        return Err(Error::Other(trf!(
            "Administrator privileges are required, but the helper {} is not installed",
            helper.display()
        )));
    }
    let argv = pkexec_argv(&helper, command);
    let out = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Other(trf!("Cannot run pkexec: {e}", e = e)))?;
    interpret_exit(out.status.code(), &String::from_utf8_lossy(&out.stderr))
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
        Guard(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn sleep"),
        )
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
    fn privileged_plumbing() {
        use crate::helper_args::{Command, Signal};
        assert!(needs_privileges(&Error::NotOwner));
        assert!(needs_privileges(&Error::PermissionDenied));
        assert!(!needs_privileges(&Error::NoSuchProcess));
        assert_eq!(Action::End.helper_signal(), Signal::Term);
        assert_eq!(
            Action::Continue.helper_signal().number(),
            Action::Continue.signal()
        );
        let argv = pkexec_argv(
            std::path::Path::new(HELPER_PATH),
            Command::Renice { nice: -5, pid: 42 },
        );
        let argv: Vec<_> = argv.iter().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(argv, ["pkexec", HELPER_PATH, "renice", "-5", "42"]);
        assert_eq!(interpret_exit(Some(0), ""), Ok(()));
        assert_eq!(interpret_exit(Some(3), ""), Err(Error::NoSuchProcess));
        assert!(matches!(
            interpret_exit(Some(126), ""),
            Err(Error::Other(_))
        ));
        assert_eq!(
            interpret_exit(
                Some(4),
                "helix-system-monitor-helper: cannot do it: EPERM\n"
            ),
            Err(Error::Other("cannot do it: EPERM".into()))
        );
    }

    #[test]
    fn action_metadata() {
        assert_eq!(Action::Kill.signal(), libc::SIGKILL);
        assert!(Action::End.needs_confirmation());
        assert!(!Action::Stop.needs_confirmation());
    }
}
