//! Command line of the privileged helper (`helix-system-monitor-helper`).
//!
//! Shared by the helper binary (parsing) and the app (building the argv
//! passed to `pkexec`). The grammar is deliberately tiny and strict because
//! the helper runs as root:
//!
//! ```text
//! helix-system-monitor-helper [--dry-run] signal <STOP|CONT|TERM|KILL> <PID> <START>
//! helix-system-monitor-helper [--dry-run] renice <NICE> <PID> <START>
//! ```
//!
//! * `PID`: decimal, no sign, no leading zeros, 2 ..= 4194304 (the kernel's
//!   `PID_MAX_LIMIT`). PID 1 (init) is always refused.
//! * `START`: the process's start time, field 22 of `/proc/<PID>/stat`
//!   (clock ticks after boot), decimal, no sign, no leading zeros. The
//!   helper acts only if the PID still belongs to the process that
//!   started then (see `pidfd.rs`), not to one that got the PID while
//!   the user was authenticating.
//! * `NICE`: decimal, optional leading `-`, no leading zeros, −20 ..= 19.
//! * Nothing else is accepted: no extra arguments, no `--`, no signal numbers.

// Compiled into both binaries; each uses a different half.
#![allow(dead_code)]

use helix_app_common::trf;
use std::fmt;

/// Kernel upper bound for PIDs (`PID_MAX_LIMIT` on 64-bit).
pub const PID_MAX_LIMIT: i32 = 4_194_304;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Stop,
    Cont,
    Term,
    Kill,
}

impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Signal::Stop => "STOP",
            Signal::Cont => "CONT",
            Signal::Term => "TERM",
            Signal::Kill => "KILL",
        }
    }

    pub fn number(self) -> i32 {
        match self {
            Signal::Stop => libc::SIGSTOP,
            Signal::Cont => libc::SIGCONT,
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "STOP" => Signal::Stop,
            "CONT" => Signal::Cont,
            "TERM" => Signal::Term,
            "KILL" => Signal::Kill,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Signal {
        signal: Signal,
        pid: i32,
        start: u64,
    },
    Renice {
        nice: i32,
        pid: i32,
        start: u64,
    },
}

impl Command {
    pub fn pid(self) -> i32 {
        match self {
            Command::Signal { pid, .. } | Command::Renice { pid, .. } => pid,
        }
    }

    /// Start time of the process meant (clock ticks after boot).
    pub fn start(self) -> u64 {
        match self {
            Command::Signal { start, .. } | Command::Renice { start, .. } => start,
        }
    }

    /// Argument vector (without the program name) for this command.
    pub fn to_args(self) -> Vec<String> {
        match self {
            Command::Signal { signal, pid, start } => vec![
                "signal".into(),
                signal.name().into(),
                pid.to_string(),
                start.to_string(),
            ],
            Command::Renice { nice, pid, start } => vec![
                "renice".into(),
                nice.to_string(),
                pid.to_string(),
                start.to_string(),
            ],
        }
    }

    /// Human-readable description, used for dry runs.
    pub fn describe(self) -> String {
        match self {
            Command::Signal { signal, pid, .. } => {
                trf!(
                    "send SIG{name} to process {pid}",
                    name = signal.name(),
                    pid = pid
                )
            }
            Command::Renice { nice, pid, .. } => trf!(
                "set nice value of process {pid} to {nice}",
                pid = pid,
                nice = nice
            ),
        }
    }
}

impl Command {
    /// The error message when the command failed with `error`.
    pub fn failure(self, error: &dyn std::fmt::Display) -> String {
        match self {
            Command::Signal { signal, pid, .. } => trf!(
                "cannot send SIG{name} to process {pid}: {error}",
                name = signal.name(),
                pid = pid,
                error = error
            ),
            Command::Renice { nice, pid, .. } => trf!(
                "cannot set nice value of process {pid} to {nice}: {error}",
                pid = pid,
                nice = nice,
                error = error
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    Usage,
    UnknownCommand(String),
    BadSignal(String),
    BadPid(String),
    BadNice(String),
    BadStart(String),
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgError::Usage => write!(f, "wrong number of arguments"),
            ArgError::UnknownCommand(s) => write!(f, "unknown command “{s}”"),
            ArgError::BadSignal(s) => write!(
                f,
                "invalid signal “{s}” (expected STOP, CONT, TERM or KILL)"
            ),
            ArgError::BadPid(s) => write!(f, "invalid process ID “{s}”"),
            ArgError::BadNice(s) => write!(f, "invalid nice value “{s}” (expected -20..19)"),
            ArgError::BadStart(s) => write!(f, "invalid process start time “{s}”"),
        }
    }
}

pub const USAGE: &str = "Usage: helix-system-monitor-helper [--dry-run] signal <STOP|CONT|TERM|KILL> <PID> <START>\n       helix-system-monitor-helper [--dry-run] renice <NICE> <PID> <START>";

/// Strict decimal parser: optional `-` (when `signed`), digits only, no
/// leading zeros, at most 7 digits (so it never overflows `i32`).
fn parse_decimal(s: &str, signed: bool) -> Option<i32> {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) if signed => (true, rest),
        Some(_) => return None,
        None => (false, s),
    };
    if digits.is_empty() || digits.len() > 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    if neg && digits == "0" {
        return None;
    }
    let v: i32 = digits.parse().ok()?;
    Some(if neg { -v } else { v })
}

pub fn parse_pid(s: &str) -> Result<i32, ArgError> {
    match parse_decimal(s, false) {
        Some(p) if (2..=PID_MAX_LIMIT).contains(&p) => Ok(p),
        _ => Err(ArgError::BadPid(s.to_string())),
    }
}

pub fn parse_nice(s: &str) -> Result<i32, ArgError> {
    match parse_decimal(s, true) {
        Some(n) if (-20..=19).contains(&n) => Ok(n),
        _ => Err(ArgError::BadNice(s.to_string())),
    }
}

/// Start time: decimal digits, no sign, no leading zeros, fits `u64`.
pub fn parse_start(s: &str) -> Result<u64, ArgError> {
    let ok =
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    ok.then(|| s.parse().ok())
        .flatten()
        .ok_or_else(|| ArgError::BadStart(s.to_string()))
}

/// Parses the helper's arguments (without the program name).
pub fn parse<S: AsRef<str>>(args: &[S]) -> Result<Invocation, ArgError> {
    let mut args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let dry_run = args.first() == Some(&"--dry-run");
    if dry_run {
        args.remove(0);
    }
    let [cmd, value, pid, start] = args[..] else {
        return Err(ArgError::Usage);
    };
    let command = match cmd {
        "signal" => {
            let signal =
                Signal::parse(value).ok_or_else(|| ArgError::BadSignal(value.to_string()))?;
            Command::Signal {
                signal,
                pid: parse_pid(pid)?,
                start: parse_start(start)?,
            }
        }
        "renice" => Command::Renice {
            nice: parse_nice(value)?,
            pid: parse_pid(pid)?,
            start: parse_start(start)?,
        },
        other => return Err(ArgError::UnknownCommand(other.to_string())),
    };
    Ok(Invocation { command, dry_run })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(args: &[&str]) -> Invocation {
        parse(args).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn valid_signal_commands() {
        assert_eq!(
            ok(&["signal", "TERM", "1234", "5678"]),
            Invocation {
                command: Command::Signal {
                    signal: Signal::Term,
                    pid: 1234,
                    start: 5678
                },
                dry_run: false
            }
        );
        for (name, sig) in [
            ("STOP", Signal::Stop),
            ("CONT", Signal::Cont),
            ("KILL", Signal::Kill),
        ] {
            assert_eq!(
                ok(&["signal", name, "2", "0"]).command,
                Command::Signal {
                    signal: sig,
                    pid: 2,
                    start: 0
                }
            );
        }
        assert!(
            ok(&[
                "--dry-run",
                "signal",
                "KILL",
                "4194304",
                "18446744073709551615"
            ])
            .dry_run
        );
    }

    #[test]
    fn valid_renice_commands() {
        assert_eq!(
            ok(&["renice", "-20", "99", "7"]).command,
            Command::Renice {
                nice: -20,
                pid: 99,
                start: 7
            }
        );
        assert_eq!(
            ok(&["renice", "19", "99", "7"]).command,
            Command::Renice {
                nice: 19,
                pid: 99,
                start: 7
            }
        );
        assert_eq!(
            ok(&["renice", "0", "99", "7"]).command,
            Command::Renice {
                nice: 0,
                pid: 99,
                start: 7
            }
        );
        assert_eq!(
            ok(&["--dry-run", "renice", "-5", "7", "8"]).command,
            Command::Renice {
                nice: -5,
                pid: 7,
                start: 8
            }
        );
    }

    #[test]
    fn rejects_wrong_arity_and_order() {
        assert_eq!(parse::<&str>(&[]), Err(ArgError::Usage));
        assert_eq!(parse(&["signal", "TERM"]), Err(ArgError::Usage));
        // The start time is required.
        assert_eq!(parse(&["signal", "TERM", "12"]), Err(ArgError::Usage));
        assert_eq!(parse(&["renice", "5", "12"]), Err(ArgError::Usage));
        assert_eq!(
            parse(&["signal", "TERM", "12", "13", "14"]),
            Err(ArgError::Usage)
        );
        assert_eq!(parse(&["--dry-run"]), Err(ArgError::Usage));
        // --dry-run only as the first argument, only once.
        assert_eq!(
            parse(&["signal", "TERM", "12", "1", "--dry-run"]),
            Err(ArgError::Usage)
        );
        assert!(parse(&["--dry-run", "--dry-run", "signal", "TERM"]).is_err());
        assert_eq!(
            parse(&["--", "signal", "TERM", "12", "1"]),
            Err(ArgError::Usage)
        );
        assert_eq!(
            parse(&["kill", "TERM", "12", "1"]),
            Err(ArgError::UnknownCommand("kill".into()))
        );
        assert_eq!(
            parse(&["SIGNAL", "TERM", "12", "1"]),
            Err(ArgError::UnknownCommand("SIGNAL".into()))
        );
    }

    #[test]
    fn rejects_bad_signals() {
        for s in [
            "9", "SIGKILL", "kill", "HUP", "USR1", "", " TERM", "TERM ", "-9",
        ] {
            assert_eq!(
                parse(&["signal", s, "12", "1"]),
                Err(ArgError::BadSignal(s.into())),
                "{s:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_pids() {
        for p in [
            "0",
            "1",
            "-1",
            "-12",
            "+12",
            "012",
            "00",
            "",
            " 12",
            "12 ",
            "1e3",
            "0x10",
            "12.0",
            "4194305",
            "99999999",
            "2147483648",
            "١٢",
            "12\0",
        ] {
            assert_eq!(
                parse(&["signal", "TERM", p, "1"]),
                Err(ArgError::BadPid(p.into())),
                "{p:?}"
            );
            assert_eq!(
                parse(&["renice", "5", p, "1"]),
                Err(ArgError::BadPid(p.into())),
                "{p:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_nice_values() {
        for n in [
            "-21", "20", "100", "+5", "-0", "05", "-05", "", "-", "5.0", " 5", "high", "--5",
        ] {
            assert_eq!(
                parse(&["renice", n, "12", "1"]),
                Err(ArgError::BadNice(n.into())),
                "{n:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_start_times() {
        for st in [
            "",
            "-1",
            "+1",
            "01",
            "1.5",
            " 1",
            "1 ",
            "0x10",
            "18446744073709551616",
            "١",
        ] {
            assert_eq!(
                parse(&["signal", "KILL", "12", st]),
                Err(ArgError::BadStart(st.into())),
                "{st:?}"
            );
        }
        assert_eq!(parse_start("0"), Ok(0));
    }

    #[test]
    fn to_args_round_trips() {
        let cmds = [
            Command::Signal {
                signal: Signal::Stop,
                pid: 2,
                start: 0,
            },
            Command::Signal {
                signal: Signal::Kill,
                pid: PID_MAX_LIMIT,
                start: u64::MAX,
            },
            Command::Renice {
                nice: -20,
                pid: 300,
                start: 1,
            },
            Command::Renice {
                nice: 19,
                pid: 301,
                start: 12345,
            },
            Command::Renice {
                nice: 0,
                pid: 302,
                start: 9,
            },
        ];
        for c in cmds {
            assert_eq!(
                parse(&c.to_args()).unwrap(),
                Invocation {
                    command: c,
                    dry_run: false
                }
            );
        }
    }

    #[test]
    fn describe_is_readable() {
        assert_eq!(
            Command::Signal {
                signal: Signal::Term,
                pid: 5,
                start: 1
            }
            .describe(),
            "send SIGTERM to process 5"
        );
        assert_eq!(
            Command::Renice {
                nice: -3,
                pid: 5,
                start: 1
            }
            .describe(),
            "set nice value of process 5 to -3"
        );
        assert_eq!(Signal::Kill.number(), libc::SIGKILL);
        let denied = std::io::Error::from_raw_os_error(libc::EPERM);
        assert_eq!(
            Command::Signal {
                signal: Signal::Kill,
                pid: 5,
                start: 1
            }
            .failure(&denied),
            format!("cannot send SIGKILL to process 5: {denied}")
        );
        assert_eq!(
            Command::Renice {
                nice: 4,
                pid: 5,
                start: 1
            }
            .failure(&"busy"),
            "cannot set nice value of process 5 to 4: busy"
        );
    }
}
