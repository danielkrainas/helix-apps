//! `helix-system-monitor-helper`: privileged helper run through `pkexec`
//! (polkit action `org.helixdesktop.SystemMonitor.manage-processes`) to signal or
//! renice processes the calling user does not own.
//!
//! It acts only on the process that started at the given time (see
//! `pidfd.rs`): if that process has exited, nothing that got its PID since
//! is touched.
//!
//! Exit status: 0 success, 2 invalid arguments, 3 no such process,
//! 4 operation failed (including "not running as root").

use helix_app_common::trf;

#[path = "../helper_args.rs"]
mod helper_args;
#[path = "../pidfd.rs"]
mod pidfd;

use helper_args::{Command, Invocation};
use std::process::ExitCode;

const EXIT_USAGE: u8 = 2;
const EXIT_NO_PROCESS: u8 = 3;
const EXIT_FAILED: u8 = 4;

fn fail(code: u8, msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("helix-system-monitor-helper: {msg}");
    ExitCode::from(code)
}

fn execute(process: &pidfd::Process, command: Command) -> Result<(), std::io::Error> {
    match command {
        // The pid was validated to be >= 2 (never init), and the signal
        // goes through the pidfd to that one process.
        Command::Signal { signal, .. } => process.signal(signal.number()),
        // The nice value was validated to be in -20..=19.
        Command::Renice { nice, .. } => process.renice(nice),
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
    let process = match pidfd::Process::open(pid, command.start()) {
        Ok(p) => p,
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
            return fail(EXIT_NO_PROCESS, trf!("no process with ID {pid}", pid = pid))
        }
        Err(e) => return fail(EXIT_FAILED, command.failure(&e)),
    };
    if dry_run {
        println!("dry run: would {}", command.describe());
        return ExitCode::SUCCESS;
    }
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return fail(EXIT_FAILED, "must be run as root (through pkexec)");
    }
    match execute(&process, command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
            fail(EXIT_NO_PROCESS, trf!("no process with ID {pid}", pid = pid))
        }
        Err(e) => fail(EXIT_FAILED, command.failure(&e)),
    }
}
