//! `helix-system-monitor-helper`: privileged helper run through `pkexec`
//! (polkit action `org.helix.SystemMonitor.manage-processes`) to signal or
//! renice processes the calling user does not own.
//!
//! Exit status: 0 success, 2 invalid arguments, 3 no such process,
//! 4 operation failed (including "not running as root").

#[path = "../helper_args.rs"]
mod helper_args;

use helper_args::{Command, Invocation};
use std::process::ExitCode;

const EXIT_USAGE: u8 = 2;
const EXIT_NO_PROCESS: u8 = 3;
const EXIT_FAILED: u8 = 4;

fn fail(code: u8, msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("helix-system-monitor-helper: {msg}");
    ExitCode::from(code)
}

fn process_exists(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).is_dir()
}

fn execute(command: Command) -> Result<(), std::io::Error> {
    let rc = match command {
        // SAFETY: plain syscall; the pid was validated to be >= 2, so it never
        // addresses a process group, every process (-1) or init.
        Command::Signal { signal, pid } => unsafe { libc::kill(pid, signal.number()) },
        // SAFETY: plain syscall on a single validated pid with a nice value
        // validated to be in -20..=19.
        Command::Renice { nice, pid } => unsafe {
            libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice)
        },
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn main() -> ExitCode {
    let mut args = Vec::new();
    for a in std::env::args_os().skip(1) {
        match a.into_string() {
            Ok(s) => args.push(s),
            Err(_) => {
                return fail(
                    EXIT_USAGE,
                    format!("non-UTF-8 argument\n{}", helper_args::USAGE),
                )
            }
        }
    }
    let Invocation { command, dry_run } = match helper_args::parse(&args) {
        Ok(inv) => inv,
        Err(e) => return fail(EXIT_USAGE, format!("{e}\n{}", helper_args::USAGE)),
    };
    let pid = command.pid();
    if pid == std::process::id() as i32 {
        return fail(EXIT_USAGE, "refusing to act on the helper itself");
    }
    if !process_exists(pid) {
        return fail(EXIT_NO_PROCESS, format!("no process with ID {pid}"));
    }
    if dry_run {
        println!("dry run: would {}", command.describe());
        return ExitCode::SUCCESS;
    }
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return fail(EXIT_FAILED, "must be run as root (through pkexec)");
    }
    match execute(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
            fail(EXIT_NO_PROCESS, format!("no process with ID {pid}"))
        }
        Err(e) => fail(EXIT_FAILED, format!("cannot {}: {e}", command.describe())),
    }
}
