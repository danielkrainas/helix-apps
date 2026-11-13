//! Exercises the privileged helper's argument validation and dry-run path.
//! Nothing here runs pkexec or sends a signal/renice to any process.

use std::process::{Child, Command, Output};

const HELPER: &str = env!("CARGO_BIN_EXE_helix-system-monitor-helper");

fn run(args: &[&str]) -> Output {
    Command::new(HELPER).args(args).output().expect("run helper")
}

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn invalid_arguments_exit_2() {
    for args in [
        &[][..],
        &["signal", "TERM"],
        &["signal", "HUP", "1234"],
        &["signal", "TERM", "1"],
        &["signal", "TERM", "-1"],
        &["signal", "TERM", "0"],
        &["renice", "-21", "1234"],
        &["renice", "5", "abc"],
        &["--dry-run", "signal", "TERM", "12", "extra"],
        &["exec", "/bin/sh", "1234"],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("Usage:"), "{args:?}");
    }
}

#[test]
fn dry_run_on_own_child_does_nothing() {
    let mut child = Guard(Command::new("sleep").arg("30").spawn().expect("spawn sleep"));
    let pid = child.0.id().to_string();

    let out = run(&["--dry-run", "signal", "KILL", &pid]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("dry run: would send SIGKILL to process {pid}"));

    let out = run(&["--dry-run", "renice", "-10", &pid]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("dry run: would set nice value of process {pid} to -10"));

    // The child is untouched.
    assert!(child.0.try_wait().unwrap().is_none(), "dry run must not signal the process");
}

#[test]
fn dry_run_on_missing_process_exits_3() {
    // Find a PID that does not exist right now.
    let pid = (100_000..4_194_304).rev().find(|p| !std::path::Path::new(&format!("/proc/{p}")).exists()).unwrap();
    let out = run(&["--dry-run", "signal", "TERM", &pid.to_string()]);
    assert_eq!(out.status.code(), Some(3));
}
