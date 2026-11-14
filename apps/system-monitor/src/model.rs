//! Pure process-list logic: filtering and column ordering.

use crate::sampler::{FsEntry, ProcessInfo};
use std::cmp::Ordering;

/// `show-whose-processes` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    All,
    User,
    Active,
}

impl Whose {
    pub fn from_nick(s: &str) -> Self {
        match s {
            "all" => Whose::All,
            "active" => Whose::Active,
            _ => Whose::User,
        }
    }
    pub fn nick(self) -> &'static str {
        match self {
            Whose::All => "all",
            Whose::User => "user",
            Whose::Active => "active",
        }
    }
}

/// Whether a process passes the Show filter and the search text.
/// `search` must already be lower-case.
pub fn matches(p: &ProcessInfo, whose: Whose, my_uid: u32, search: &str) -> bool {
    let shown = match whose {
        Whose::All => true,
        Whose::User => p.uid == my_uid,
        Whose::Active => p.state == 'R' || p.cpu > 0.0,
    };
    if !shown {
        return false;
    }
    let search = search.trim();
    if search.is_empty() {
        return true;
    }
    if search.chars().all(|c| c.is_ascii_digit()) && p.pid.to_string().starts_with(search) {
        return true;
    }
    p.name.to_lowercase().contains(search)
        || p.user.to_lowercase().contains(search)
        || p.cmdline.to_lowercase().contains(search)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcColumn {
    Name,
    User,
    Cpu,
    Memory,
    Pid,
    Priority,
    State,
    Command,
}

/// Ascending order for a column; ties broken by PID so order is stable.
pub fn compare(a: &ProcessInfo, b: &ProcessInfo, col: ProcColumn) -> Ordering {
    let primary = match col {
        ProcColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        ProcColumn::User => a.user.cmp(&b.user),
        ProcColumn::Cpu => a.cpu.total_cmp(&b.cpu),
        ProcColumn::Memory => a.memory.unwrap_or(0).cmp(&b.memory.unwrap_or(0)),
        ProcColumn::Pid => Ordering::Equal,
        // Higher priority = lower nice value.
        ProcColumn::Priority => b.nice.cmp(&a.nice),
        ProcColumn::State => crate::format::state(a.state).cmp(crate::format::state(b.state)),
        ProcColumn::Command => a.cmdline.cmp(&b.cmdline),
    };
    primary.then(a.pid.cmp(&b.pid))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsColumn {
    Device,
    Directory,
    Type,
    Total,
    Free,
    Available,
    Used,
}

pub fn compare_fs(a: &FsEntry, b: &FsEntry, col: FsColumn) -> Ordering {
    let primary = match col {
        FsColumn::Device => a.device.cmp(&b.device),
        FsColumn::Directory => Ordering::Equal,
        FsColumn::Type => a.fstype.cmp(&b.fstype),
        FsColumn::Total => a.total.cmp(&b.total),
        FsColumn::Free => a.free.cmp(&b.free),
        FsColumn::Available => a.available.cmp(&b.available),
        FsColumn::Used => a.used_fraction().total_cmp(&b.used_fraction()),
    };
    primary.then_with(|| a.dir.cmp(&b.dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: i32, name: &str, uid: u32, cpu: f64, state: char) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.into(),
            user: if uid == 0 { "root".into() } else { "me".into() },
            uid,
            cpu,
            state,
            cmdline: format!("/usr/bin/{name} --flag"),
            ..Default::default()
        }
    }

    #[test]
    fn whose_filter() {
        let mine = p(10, "bash", 1000, 0.0, 'S');
        let root = p(1, "systemd", 0, 0.0, 'S');
        let busy = p(20, "make", 0, 3.0, 'S');
        let running = p(21, "cc1", 0, 0.0, 'R');
        assert!(matches(&mine, Whose::User, 1000, ""));
        assert!(!matches(&root, Whose::User, 1000, ""));
        assert!(matches(&root, Whose::All, 1000, ""));
        assert!(!matches(&mine, Whose::Active, 1000, ""));
        assert!(matches(&busy, Whose::Active, 1000, ""));
        assert!(matches(&running, Whose::Active, 1000, ""));
        assert_eq!(Whose::from_nick("bogus"), Whose::User);
        assert_eq!(Whose::from_nick(Whose::Active.nick()), Whose::Active);
    }

    #[test]
    fn search() {
        let x = p(4321, "Firefox", 1000, 0.0, 'S');
        assert!(matches(&x, Whose::All, 1000, "fire"));
        assert!(matches(&x, Whose::All, 1000, "432"));
        assert!(matches(&x, Whose::All, 1000, "--flag"));
        assert!(matches(&x, Whose::All, 1000, "me"));
        assert!(!matches(&x, Whose::All, 1000, "chrome"));
        assert!(!matches(&x, Whose::All, 1000, "999"));
    }

    #[test]
    fn ordering() {
        let a = p(2, "alpha", 0, 5.0, 'S');
        let mut b = p(1, "Beta", 1000, 1.0, 'R');
        b.nice = 10;
        b.memory = Some(10);
        assert_eq!(compare(&a, &b, ProcColumn::Name), Ordering::Less);
        assert_eq!(compare(&a, &b, ProcColumn::Cpu), Ordering::Greater);
        assert_eq!(compare(&a, &b, ProcColumn::Pid), Ordering::Greater);
        assert_eq!(compare(&a, &b, ProcColumn::Memory), Ordering::Less);
        // nice 0 is a higher priority than nice 10.
        assert_eq!(compare(&a, &b, ProcColumn::Priority), Ordering::Greater);
        assert_eq!(compare(&a, &a.clone(), ProcColumn::User), Ordering::Equal);
    }

    #[test]
    fn fs_ordering() {
        let a = FsEntry {
            device: "/dev/a".into(),
            dir: "/".into(),
            fstype: "ext4".into(),
            total: 100,
            free: 50,
            available: 50,
        };
        let b = FsEntry {
            device: "/dev/b".into(),
            dir: "/home".into(),
            fstype: "btrfs".into(),
            total: 10,
            free: 1,
            available: 1,
        };
        assert_eq!(compare_fs(&a, &b, FsColumn::Directory), Ordering::Less);
        assert_eq!(compare_fs(&a, &b, FsColumn::Type), Ordering::Greater);
        assert_eq!(compare_fs(&a, &b, FsColumn::Used), Ordering::Less);
        assert_eq!(compare_fs(&a, &b, FsColumn::Total), Ordering::Greater);
    }
}
