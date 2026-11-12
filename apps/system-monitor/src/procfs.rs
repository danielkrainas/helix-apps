//! Pure parsers for the `/proc` text formats the monitor reads, plus CPU
//! usage arithmetic. Nothing in this module touches the file system, so it
//! is unit-tested with fixture text.

use std::collections::HashMap;

/// Fields of `/proc/<pid>/stat` that the monitor uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PidStat {
    pub pid: i32,
    pub comm: String,
    pub state: char,
    pub ppid: i32,
    /// User + system time in clock ticks.
    pub utime: u64,
    pub stime: u64,
    pub nice: i32,
    pub num_threads: i64,
    /// Start time in clock ticks after boot.
    pub starttime: u64,
    /// Virtual memory size in bytes.
    pub vsize: u64,
    /// Resident set size in pages.
    pub rss_pages: i64,
}

impl PidStat {
    pub fn cpu_ticks(&self) -> u64 {
        self.utime + self.stime
    }
}

/// Parses `/proc/<pid>/stat`. The command name is in parentheses and may
/// itself contain spaces and parentheses, so the *last* `)` ends it.
pub fn parse_pid_stat(text: &str) -> Option<PidStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let pid = text[..open].trim().parse().ok()?;
    let comm = text[open + 1..close].to_string();
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // rest[0] is field 3 (state); field N is rest[N - 3].
    let f = |n: usize| rest.get(n - 3).copied();
    let num = |n: usize| f(n).and_then(|s| s.parse::<i64>().ok());
    Some(PidStat {
        pid,
        comm,
        state: f(3)?.chars().next()?,
        ppid: num(4)? as i32,
        utime: num(14)? as u64,
        stime: num(15)? as u64,
        nice: num(19)? as i32,
        num_threads: num(20)?,
        starttime: f(22)?.parse().ok()?,
        vsize: f(23)?.parse().ok()?,
        rss_pages: num(24)?,
    })
}

/// Fields of `/proc/<pid>/status` that the monitor uses. Memory is in bytes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PidStatus {
    pub name: String,
    pub uid: u32,
    pub vm_size: Option<u64>,
    pub vm_rss: Option<u64>,
    pub rss_anon: Option<u64>,
    pub rss_file: Option<u64>,
    pub rss_shmem: Option<u64>,
}

impl PidStatus {
    /// Private resident memory: `RssAnon`, falling back to `VmRSS`.
    /// `None` for kernel threads, which have no user address space.
    pub fn private_memory(&self) -> Option<u64> {
        self.rss_anon.or(self.vm_rss)
    }

    /// Shared resident memory (file-backed + shmem).
    pub fn shared_memory(&self) -> Option<u64> {
        match (self.rss_file, self.rss_shmem) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        }
    }
}

fn kb_value(v: &str) -> Option<u64> {
    let mut it = v.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    match it.next() {
        Some("kB") | Some("KB") | Some("kb") => Some(n * 1024),
        None => Some(n),
        Some(_) => Some(n),
    }
}

pub fn parse_pid_status(text: &str) -> Option<PidStatus> {
    let mut st = PidStatus::default();
    let mut have_uid = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match key {
            "Name" => st.name = unescape_status_name(value),
            "Uid" => {
                st.uid = value.split_whitespace().next()?.parse().ok()?;
                have_uid = true;
            }
            "VmSize" => st.vm_size = kb_value(value),
            "VmRSS" => st.vm_rss = kb_value(value),
            "RssAnon" => st.rss_anon = kb_value(value),
            "RssFile" => st.rss_file = kb_value(value),
            "RssShmem" => st.rss_shmem = kb_value(value),
            _ => {}
        }
    }
    have_uid.then_some(st)
}

/// The kernel escapes `\n` and `\\` in the status `Name:` field.
fn unescape_status_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits `/proc/<pid>/cmdline` (NUL-separated) into arguments.
pub fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

/// The name shown for a process: the kernel's (truncated, 15 char) comm,
/// unless argv[0]'s basename starts with it, then the longer basename.
pub fn display_name(comm: &str, args: &[String]) -> String {
    if let Some(first) = args.first() {
        let base = first.rsplit('/').next().unwrap_or(first);
        // Some programs put their whole command line in argv[0].
        let base = base.split(' ').next().unwrap_or(base);
        if base.len() > comm.len() && base.starts_with(comm) {
            return base.to_string();
        }
    }
    comm.to_string()
}

/// Cumulative CPU time counters from one `cpu` line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    pub fn total(&self) -> u64 {
        self.busy() + self.idle_all()
    }
    pub fn idle_all(&self) -> u64 {
        self.idle + self.iowait
    }
    pub fn busy(&self) -> u64 {
        self.user + self.nice + self.system + self.irq + self.softirq + self.steal
    }
}

/// Parsed `/proc/stat`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stat {
    pub total: CpuTimes,
    pub cpus: Vec<CpuTimes>,
    /// Boot time, seconds since the epoch.
    pub btime: u64,
}

pub fn parse_stat(text: &str) -> Stat {
    let mut stat = Stat::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        if let Some(rest) = key.strip_prefix("cpu") {
            let v: Vec<u64> = it.map(|s| s.parse().unwrap_or(0)).collect();
            let g = |i: usize| v.get(i).copied().unwrap_or(0);
            let t = CpuTimes {
                user: g(0),
                nice: g(1),
                system: g(2),
                idle: g(3),
                iowait: g(4),
                irq: g(5),
                softirq: g(6),
                steal: g(7),
            };
            if rest.is_empty() {
                stat.total = t;
            } else if rest.parse::<usize>().is_ok() {
                stat.cpus.push(t);
            }
        } else if key == "btime" {
            stat.btime = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        }
    }
    stat
}

/// Fraction (0.0–1.0) of time a CPU was busy between two samples.
pub fn cpu_usage(prev: &CpuTimes, cur: &CpuTimes) -> f64 {
    let total = cur.total().saturating_sub(prev.total());
    if total == 0 {
        return 0.0;
    }
    let busy = cur.busy().saturating_sub(prev.busy());
    (busy as f64 / total as f64).clamp(0.0, 1.0)
}

/// Percent CPU used by a process between two samples.
///
/// `proc_ticks` is the process's tick delta; `elapsed_ticks` is the wall
/// time between the samples, in clock ticks (one CPU's worth). Without
/// Solaris mode, a process using every core of an N-CPU machine shows
/// N×100 %; in Solaris mode the result is divided by `ncpus`.
pub fn process_cpu_percent(proc_ticks: u64, elapsed_ticks: f64, ncpus: usize, solaris: bool) -> f64 {
    if elapsed_ticks <= 0.0 {
        return 0.0;
    }
    let ncpus = ncpus.max(1) as f64;
    let pct = (proc_ticks as f64 / elapsed_ticks * 100.0).clamp(0.0, 100.0 * ncpus);
    if solaris {
        pct / ncpus
    } else {
        pct
    }
}

/// Wall time between two `/proc/stat` samples in one CPU's ticks: the total
/// tick delta across all CPUs divided by the CPU count.
pub fn elapsed_ticks(prev: &Stat, cur: &Stat) -> f64 {
    let n = cur.cpus.len().max(1) as f64;
    cur.total.total().saturating_sub(prev.total.total()) as f64 / n
}

/// Parsed `/proc/meminfo`, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemInfo {
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl MemInfo {
    pub fn mem_used(&self) -> u64 {
        self.mem_total.saturating_sub(self.mem_available)
    }
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

pub fn parse_meminfo(text: &str) -> MemInfo {
    let mut map: HashMap<&str, u64> = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if let Some(n) = kb_value(v.trim()) {
                map.insert(k.trim(), n);
            }
        }
    }
    let g = |k: &str| map.get(k).copied().unwrap_or(0);
    let available = map
        .get("MemAvailable")
        .copied()
        .unwrap_or_else(|| g("MemFree") + g("Buffers") + g("Cached"));
    MemInfo {
        mem_total: g("MemTotal"),
        mem_available: available.min(g("MemTotal")),
        swap_total: g("SwapTotal"),
        swap_free: g("SwapFree").min(g("SwapTotal")),
    }
}

/// Byte counters of one interface in `/proc/net/dev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetDev {
    pub iface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

pub fn parse_net_dev(text: &str) -> Vec<NetDev> {
    text.lines()
        .filter_map(|line| {
            let (iface, rest) = line.split_once(':')?;
            let iface = iface.trim();
            if iface.is_empty() || iface.contains('|') {
                return None;
            }
            let v: Vec<u64> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if v.len() < 16 {
                return None;
            }
            Some(NetDev { iface: iface.to_string(), rx_bytes: v[0], tx_bytes: v[8] })
        })
        .collect()
}

/// Total (rx, tx) bytes over all interfaces except loopback.
pub fn net_totals(devs: &[NetDev]) -> (u64, u64) {
    devs.iter()
        .filter(|d| d.iface != "lo")
        .fold((0, 0), |(r, t), d| (r + d.rx_bytes, t + d.tx_bytes))
}

/// One line of `/proc/self/mounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub device: String,
    pub dir: String,
    pub fstype: String,
    pub options: String,
}

/// Decodes the kernel's octal escapes (`\040` for space, `\011`, `\012`,
/// `\134`) in mount fields.
pub fn unescape_mount_field(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            Some(Mount {
                device: unescape_mount_field(it.next()?),
                dir: unescape_mount_field(it.next()?),
                fstype: it.next()?.to_string(),
                options: it.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// File system types that do not represent storage.
const PSEUDO_FS: &[&str] = &[
    "autofs", "binfmt_misc", "bpf", "cgroup", "cgroup2", "configfs", "debugfs", "devpts", "devtmpfs",
    "efivarfs", "fusectl", "hugetlbfs", "mqueue", "nsfs", "overlay", "proc", "pstore", "ramfs",
    "rpc_pipefs", "securityfs", "selinuxfs", "squashfs", "sysfs", "tmpfs", "tracefs", "nfsd",
    "fuse.gvfsd-fuse", "fuse.portal", "fuse.snapfuse", "fuse.lxcfs", "ecryptfs", "none",
];

pub fn is_pseudo_fs(fstype: &str) -> bool {
    PSEUDO_FS.contains(&fstype)
}

/// Selects the mounts to show. Without `show_all`, pseudo file systems are
/// dropped and each device is listed only once (its first mount point).
pub fn visible_mounts(mounts: &[Mount], show_all: bool) -> Vec<Mount> {
    if show_all {
        return mounts.to_vec();
    }
    let mut seen = std::collections::HashSet::new();
    mounts
        .iter()
        .filter(|m| !is_pseudo_fs(&m.fstype))
        .filter(|m| seen.insert(m.device.clone()))
        .cloned()
        .collect()
}

/// Parses `/etc/passwd` into a uid → login name map.
pub fn parse_passwd(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 3 {
            if let Ok(uid) = f[2].parse() {
                map.entry(uid).or_insert_with(|| f[0].to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT_FIXTURE: &str = "1234 (Web Content (x)) S 1000 1234 1000 0 -1 4194560 52145 0 12 0 1500 250 0 0 20 5 27 0 98765 3405438976 61234 18446744073709551615 1 1 0 0 0 0 0 16781312 1260 0 0 0 17 3 0 0 0 0 0\n";

    #[test]
    fn pid_stat() {
        let s = parse_pid_stat(STAT_FIXTURE).unwrap();
        assert_eq!(s.pid, 1234);
        assert_eq!(s.comm, "Web Content (x)");
        assert_eq!(s.state, 'S');
        assert_eq!(s.ppid, 1000);
        assert_eq!(s.utime, 1500);
        assert_eq!(s.stime, 250);
        assert_eq!(s.cpu_ticks(), 1750);
        assert_eq!(s.nice, 5);
        assert_eq!(s.num_threads, 27);
        assert_eq!(s.starttime, 98765);
        assert_eq!(s.vsize, 3405438976);
        assert_eq!(s.rss_pages, 61234);
    }

    #[test]
    fn pid_stat_negative_nice_and_garbage() {
        let s = parse_pid_stat("7 (kworker/0:1H) I 2 0 0 0 -1 69238880 0 0 0 0 0 3 0 0 0 -20 1 0 40 0 0 18446744073709551615 0 0 0 0 0 0 0 2147483647 0 0 0 0 17 0 0 0 0 0 0").unwrap();
        assert_eq!(s.nice, -20);
        assert_eq!(s.state, 'I');
        assert_eq!(s.comm, "kworker/0:1H");
        assert!(parse_pid_stat("").is_none());
        assert!(parse_pid_stat("12 (x) S 1").is_none());
    }

    const STATUS_FIXTURE: &str = "Name:\tfirefox\nUmask:\t0022\nState:\tS (sleeping)\nTgid:\t1234\nPid:\t1234\nPPid:\t1000\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\nVmPeak:\t 3500000 kB\nVmSize:\t 3325624 kB\nVmRSS:\t  244936 kB\nRssAnon:\t  180000 kB\nRssFile:\t   60000 kB\nRssShmem:\t    4936 kB\nThreads:\t27\n";

    #[test]
    fn pid_status() {
        let s = parse_pid_status(STATUS_FIXTURE).unwrap();
        assert_eq!(s.name, "firefox");
        assert_eq!(s.uid, 1000);
        assert_eq!(s.vm_size, Some(3325624 * 1024));
        assert_eq!(s.vm_rss, Some(244936 * 1024));
        assert_eq!(s.private_memory(), Some(180000 * 1024));
        assert_eq!(s.shared_memory(), Some(64936 * 1024));
    }

    #[test]
    fn pid_status_kernel_thread() {
        let s = parse_pid_status("Name:\tkthreadd\nState:\tS (sleeping)\nUid:\t0\t0\t0\t0\nThreads:\t1\n").unwrap();
        assert_eq!(s.uid, 0);
        assert_eq!(s.private_memory(), None);
        assert_eq!(s.shared_memory(), None);
        assert!(parse_pid_status("Name:\tx\n").is_none());
        let s = parse_pid_status("Name:\ta\\nb\\\\c\nUid:\t5 5 5 5\nVmRSS:\t10 kB\n").unwrap();
        assert_eq!(s.name, "a\nb\\c");
        assert_eq!(s.private_memory(), Some(10240));
    }

    #[test]
    fn cmdline_and_name() {
        let args = parse_cmdline(b"/usr/lib/firefox/firefox-bin\0-contentproc\0\0-isForBrowser\0");
        assert_eq!(args, vec!["/usr/lib/firefox/firefox-bin", "-contentproc", "-isForBrowser"]);
        assert!(parse_cmdline(b"").is_empty());
        assert_eq!(display_name("firefox-bin", &args), "firefox-bin");
        let long = vec!["/usr/bin/gnome-session-binary".to_string()];
        assert_eq!(display_name("gnome-session-b", &long), "gnome-session-binary");
        assert_eq!(display_name("Web Content", &["/usr/lib/firefox/firefox".to_string()]), "Web Content");
        assert_eq!(display_name("kworker/0:1", &[]), "kworker/0:1");
    }

    const PROC_STAT_FIXTURE: &str = "cpu  1000 10 500 8000 100 0 20 0 0 0\ncpu0 600 5 250 3900 50 0 10 0 0 0\ncpu1 400 5 250 4100 50 0 10 0 0 0\nintr 12345 0 0\nctxt 999\nbtime 1700000000\nprocesses 4242\nprocs_running 2\n";

    #[test]
    fn proc_stat() {
        let s = parse_stat(PROC_STAT_FIXTURE);
        assert_eq!(s.cpus.len(), 2);
        assert_eq!(s.btime, 1700000000);
        assert_eq!(s.total.user, 1000);
        assert_eq!(s.total.busy(), 1530);
        assert_eq!(s.total.idle_all(), 8100);
        assert_eq!(s.total.total(), 9630);
        assert_eq!(s.cpus[1].idle, 4100);
    }

    #[test]
    fn cpu_usage_between_samples() {
        let a = CpuTimes { user: 100, idle: 900, ..Default::default() };
        let b = CpuTimes { user: 175, system: 25, idle: 1000, ..Default::default() };
        assert!((cpu_usage(&a, &b) - 0.5).abs() < 1e-9);
        assert_eq!(cpu_usage(&a, &a), 0.0);
        // Counter going backwards (CPU hot-unplug) must not panic.
        assert_eq!(cpu_usage(&b, &a), 0.0);
    }

    #[test]
    fn process_cpu() {
        // 2 CPUs, 300 ticks of wall time elapsed, process used 150 ticks.
        assert!((process_cpu_percent(150, 300.0, 2, false) - 50.0).abs() < 1e-9);
        assert!((process_cpu_percent(150, 300.0, 2, true) - 25.0).abs() < 1e-9);
        // Using both cores fully: 200 % in Irix mode, 100 % in Solaris mode.
        assert!((process_cpu_percent(600, 300.0, 2, false) - 200.0).abs() < 1e-9);
        assert!((process_cpu_percent(600, 300.0, 2, true) - 100.0).abs() < 1e-9);
        // Clamped and safe on zero elapsed.
        assert!((process_cpu_percent(9999, 300.0, 2, false) - 200.0).abs() < 1e-9);
        assert_eq!(process_cpu_percent(10, 0.0, 2, false), 0.0);

        let prev = parse_stat(PROC_STAT_FIXTURE);
        let mut cur = prev.clone();
        cur.total.idle += 600; // 600 ticks across 2 CPUs = 300 ticks of wall time
        assert!((elapsed_ticks(&prev, &cur) - 300.0).abs() < 1e-9);
    }

    const MEMINFO_FIXTURE: &str = "MemTotal:       16303428 kB\nMemFree:         1234567 kB\nMemAvailable:    8151714 kB\nBuffers:          123456 kB\nCached:          4567890 kB\nSwapCached:            0 kB\nSwapTotal:       2097148 kB\nSwapFree:        1048574 kB\nHugePages_Total:       0\n";

    #[test]
    fn meminfo() {
        let m = parse_meminfo(MEMINFO_FIXTURE);
        assert_eq!(m.mem_total, 16303428 * 1024);
        assert_eq!(m.mem_available, 8151714 * 1024);
        assert_eq!(m.mem_used(), (16303428 - 8151714) * 1024);
        assert_eq!(m.swap_used(), 1048574 * 1024);
        // Old kernels without MemAvailable.
        let old = parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 250 kB\n");
        assert_eq!(old.mem_available, 400 * 1024);
        assert_eq!(old.swap_used(), 0);
    }

    const NET_DEV_FIXTURE: &str = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 5000000   40000    0    0    0     0          0         0  5000000   40000    0    0    0     0       0          0\n  eth0:123456789  98765    0    0    0     0          0       100  9876543   54321    0    0    0     0       0          0\nwlan0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n";

    #[test]
    fn net_dev() {
        let d = parse_net_dev(NET_DEV_FIXTURE);
        assert_eq!(d.len(), 3);
        assert_eq!(d[1], NetDev { iface: "eth0".into(), rx_bytes: 123456789, tx_bytes: 9876543 });
        assert_eq!(net_totals(&d), (123456789 + 1000, 9876543 + 2000));
    }

    const MOUNTS_FIXTURE: &str = "sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0\nproc /proc proc rw,nosuid,nodev,noexec,relatime 0 0\n/dev/nvme0n1p2 / ext4 rw,relatime 0 0\ntmpfs /run tmpfs rw,nosuid,nodev,size=1631540k,mode=755 0 0\n/dev/nvme0n1p1 /boot/efi vfat rw,relatime,fmask=0077 0 0\n/dev/sdb1 /media/user/My\\040Disk ext4 rw,nosuid,nodev 0 0\n/dev/nvme0n1p2 /var/snap ext4 rw,relatime 0 0\n/dev/loop3 /snap/core/1 squashfs ro,nodev,relatime 0 0\n";

    #[test]
    fn mounts() {
        let m = parse_mounts(MOUNTS_FIXTURE);
        assert_eq!(m.len(), 8);
        assert_eq!(m[5].dir, "/media/user/My Disk");
        assert_eq!(m[5].fstype, "ext4");
        assert_eq!(m[4].options, "rw,relatime,fmask=0077");
        let v = visible_mounts(&m, false);
        let dirs: Vec<&str> = v.iter().map(|m| m.dir.as_str()).collect();
        assert_eq!(dirs, vec!["/", "/boot/efi", "/media/user/My Disk"]);
        assert_eq!(visible_mounts(&m, true).len(), 8);
        assert_eq!(unescape_mount_field("a\\011b\\134c\\012"), "a\tb\\c\n");
        assert_eq!(unescape_mount_field("trailing\\04"), "trailing\\04");
    }

    #[test]
    fn passwd() {
        let p = parse_passwd("# comment\nroot:x:0:0:root:/root:/bin/bash\ndanny:x:1000:1000:Danny,,,:/home/danny:/bin/bash\nbroken\ndup:x:0:0::/:/bin/sh\n");
        assert_eq!(p.get(&0).map(String::as_str), Some("root"));
        assert_eq!(p.get(&1000).map(String::as_str), Some("danny"));
        assert_eq!(p.len(), 2);
    }
}
