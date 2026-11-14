//! Reads the live system through `/proc` and `statvfs(3)` and turns raw
//! counters into snapshots. Runs on a worker thread (see [`spawn`]).

use crate::procfs::{self, MemInfo, Stat};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// One process as shown in the process list.
#[derive(Debug, Clone, Default)]
pub struct ProcessInfo {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub user: String,
    pub uid: u32,
    /// Percent CPU since the previous sample (already adjusted for Solaris
    /// mode).
    pub cpu: f64,
    pub cpu_ticks: u64,
    /// Private resident memory in bytes; `None` for kernel threads.
    pub memory: Option<u64>,
    pub vm_size: Option<u64>,
    pub vm_rss: Option<u64>,
    pub shared: Option<u64>,
    pub nice: i32,
    pub state: char,
    pub threads: i64,
    /// Seconds since the epoch.
    pub start_time: u64,
    pub cmdline: String,
}

#[derive(Debug, Clone, Default)]
pub struct ResourceSample {
    /// Per-CPU busy fraction (0–1).
    pub cpus: Vec<f64>,
    pub mem: MemInfo,
    pub rx_rate: f64,
    pub tx_rate: f64,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub device: String,
    pub dir: String,
    pub fstype: String,
    pub total: u64,
    pub free: u64,
    pub available: u64,
}

impl FsEntry {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
    /// Used fraction as `df` reports it: used / (used + available).
    pub fn used_fraction(&self) -> f64 {
        let denom = self.used() + self.available;
        if denom == 0 {
            0.0
        } else {
            self.used() as f64 / denom as f64
        }
    }
}

pub fn clock_ticks() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 {
        v as u64
    } else {
        100
    }
}

pub fn current_uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Reads one process. Returns `None` if it vanished meanwhile.
pub fn read_process(
    pid: i32,
    users: &HashMap<u32, String>,
    btime: u64,
    hz: u64,
) -> Option<ProcessInfo> {
    let dir = format!("/proc/{pid}");
    let stat = procfs::parse_pid_stat(&read(format!("{dir}/stat"))?)?;
    let status = procfs::parse_pid_status(&read(format!("{dir}/status"))?)?;
    let args = std::fs::read(format!("{dir}/cmdline"))
        .map(|b| procfs::parse_cmdline(&b))
        .unwrap_or_default();
    let comm = if status.name.is_empty() {
        stat.comm.clone()
    } else {
        status.name.clone()
    };
    Some(ProcessInfo {
        pid,
        ppid: stat.ppid,
        name: procfs::display_name(&comm, &args),
        user: users
            .get(&status.uid)
            .cloned()
            .unwrap_or_else(|| status.uid.to_string()),
        uid: status.uid,
        cpu: 0.0,
        cpu_ticks: stat.cpu_ticks(),
        memory: status.private_memory(),
        vm_size: status.vm_size,
        vm_rss: status.vm_rss,
        shared: status.shared_memory(),
        nice: stat.nice,
        state: stat.state,
        threads: stat.num_threads,
        start_time: btime + stat.starttime / hz.max(1),
        cmdline: args.join(" "),
    })
}

pub fn list_pids() -> Vec<i32> {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    rd.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect()
}

pub fn load_users() -> HashMap<u32, String> {
    read("/etc/passwd")
        .map(|t| procfs::parse_passwd(&t))
        .unwrap_or_default()
}

/// Settings the worker reads; written from the UI thread.
#[derive(Debug)]
pub struct Config {
    pub interval_ms: AtomicU32,
    pub disks_interval_ms: AtomicU32,
    pub solaris: AtomicBool,
    pub show_all_fs: AtomicBool,
}

impl Config {
    pub fn new(
        interval_ms: u32,
        disks_interval_ms: u32,
        solaris: bool,
        show_all_fs: bool,
    ) -> Arc<Self> {
        Arc::new(Config {
            interval_ms: AtomicU32::new(clamp_interval(interval_ms)),
            disks_interval_ms: AtomicU32::new(clamp_interval(disks_interval_ms)),
            solaris: AtomicBool::new(solaris),
            show_all_fs: AtomicBool::new(show_all_fs),
        })
    }
}

/// Clamps an update interval to the `<range min="1000" max="100000"/>` of the
/// GNOME schema's `update-interval` and `disks-interval` keys, so values
/// written back are always accepted by GSettings.
pub fn clamp_interval(ms: u32) -> u32 {
    ms.clamp(1000, 100_000)
}

/// Stateful sampler: CPU percentages and rates are deltas between calls.
pub struct Sampler {
    users: HashMap<u32, String>,
    hz: u64,
    prev_stat: Option<Stat>,
    prev_ticks: HashMap<i32, u64>,
    prev_net: Option<(Instant, u64, u64)>,
    missing_uids: std::collections::HashSet<u32>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Sampler {
            users: load_users(),
            hz: clock_ticks(),
            prev_stat: None,
            prev_ticks: HashMap::new(),
            prev_net: None,
            missing_uids: Default::default(),
        }
    }

    pub fn sample(&mut self, solaris: bool) -> (Vec<ProcessInfo>, ResourceSample) {
        let stat = procfs::parse_stat(&read("/proc/stat").unwrap_or_default());
        let ncpus = stat.cpus.len().max(1);
        let elapsed = self
            .prev_stat
            .as_ref()
            .map(|p| procfs::elapsed_ticks(p, &stat))
            .unwrap_or(0.0);

        let mut procs = Vec::new();
        let mut ticks = HashMap::new();
        for pid in list_pids() {
            let Some(mut p) = read_process(pid, &self.users, stat.btime, self.hz) else {
                continue;
            };
            if !self.users.contains_key(&p.uid) && self.missing_uids.insert(p.uid) {
                // Possibly a user created after start-up: reload once per uid.
                self.users = load_users();
                if let Some(name) = self.users.get(&p.uid) {
                    p.user = name.clone();
                }
            }
            if let Some(prev) = self.prev_ticks.get(&pid) {
                let delta = p.cpu_ticks.saturating_sub(*prev);
                p.cpu = procfs::process_cpu_percent(delta, elapsed, ncpus, solaris);
            }
            ticks.insert(pid, p.cpu_ticks);
            procs.push(p);
        }
        self.prev_ticks = ticks;

        let cpus = match &self.prev_stat {
            Some(prev) if prev.cpus.len() == stat.cpus.len() => prev
                .cpus
                .iter()
                .zip(&stat.cpus)
                .map(|(a, b)| procfs::cpu_usage(a, b))
                .collect(),
            _ => vec![0.0; stat.cpus.len()],
        };
        self.prev_stat = Some(stat);

        let mem = procfs::parse_meminfo(&read("/proc/meminfo").unwrap_or_default());
        let (rx, tx) = procfs::net_totals(&procfs::parse_net_dev(
            &read("/proc/net/dev").unwrap_or_default(),
        ));
        let now = Instant::now();
        let (rx_rate, tx_rate) = match self.prev_net {
            Some((t, prx, ptx)) => {
                let dt = now.duration_since(t).as_secs_f64();
                if dt > 0.0 {
                    (
                        rx.saturating_sub(prx) as f64 / dt,
                        tx.saturating_sub(ptx) as f64 / dt,
                    )
                } else {
                    (0.0, 0.0)
                }
            }
            None => (0.0, 0.0),
        };
        self.prev_net = Some((now, rx, tx));

        (
            procs,
            ResourceSample {
                cpus,
                mem,
                rx_rate,
                tx_rate,
                rx_total: rx,
                tx_total: tx,
            },
        )
    }
}

pub fn statvfs(path: &str) -> Option<(u64, u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated string and `s` a valid out pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let f = s.f_frsize as u64;
    Some((
        s.f_blocks as u64 * f,
        s.f_bfree as u64 * f,
        s.f_bavail as u64 * f,
    ))
}

pub fn file_systems(show_all: bool) -> Vec<FsEntry> {
    let mounts = procfs::parse_mounts(&read("/proc/self/mounts").unwrap_or_default());
    procfs::visible_mounts(&mounts, show_all)
        .into_iter()
        .filter_map(|m| {
            let (total, free, available) = statvfs(&m.dir).unwrap_or((0, 0, 0));
            if total == 0 && !show_all {
                return None;
            }
            Some(FsEntry {
                device: m.device,
                dir: m.dir,
                fstype: m.fstype,
                total,
                free,
                available,
            })
        })
        .collect()
}

pub enum Msg {
    Processes(Vec<ProcessInfo>, ResourceSample),
    FileSystems(Vec<FsEntry>),
}

pub enum Control {
    /// Sample everything now.
    Refresh,
}

/// Starts the sampling thread. It stops when the UI drops the receiver.
pub fn spawn(config: Arc<Config>) -> (async_channel::Receiver<Msg>, mpsc::Sender<Control>) {
    let (tx, rx) = async_channel::unbounded();
    let (ctl_tx, ctl_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            let mut sampler = Sampler::new();
            let mut next_proc = Instant::now();
            let mut next_fs = Instant::now();
            loop {
                let now = Instant::now();
                if now >= next_proc {
                    let (p, r) = sampler.sample(config.solaris.load(Ordering::Relaxed));
                    if tx.send_blocking(Msg::Processes(p, r)).is_err() {
                        return;
                    }
                    next_proc = now
                        + Duration::from_millis(config.interval_ms.load(Ordering::Relaxed) as u64);
                }
                if now >= next_fs {
                    let fs = file_systems(config.show_all_fs.load(Ordering::Relaxed));
                    if tx.send_blocking(Msg::FileSystems(fs)).is_err() {
                        return;
                    }
                    next_fs = now
                        + Duration::from_millis(
                            config.disks_interval_ms.load(Ordering::Relaxed) as u64
                        );
                }
                let wait = next_proc
                    .min(next_fs)
                    .saturating_duration_since(Instant::now());
                match ctl_rx.recv_timeout(wait) {
                    Ok(Control::Refresh) => {
                        next_proc = Instant::now();
                        next_fs = Instant::now();
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                if tx.is_closed() {
                    return;
                }
            }
        })
        .expect("spawn sampler thread");
    (rx, ctl_tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_process() {
        let me = std::process::id() as i32;
        let p = read_process(me, &load_users(), 0, clock_ticks()).expect("own process readable");
        assert_eq!(p.pid, me);
        assert_eq!(p.uid, current_uid());
        assert!(p.memory.is_some());
        assert!(list_pids().contains(&me));
    }

    #[test]
    fn sampler_produces_resources() {
        let mut s = Sampler::new();
        let (procs, r) = s.sample(true);
        assert!(!procs.is_empty());
        assert!(!r.cpus.is_empty());
        assert!(r.mem.mem_total > 0);
        let (_, r2) = s.sample(true);
        assert!(r2.cpus.iter().all(|c| (0.0..=1.0).contains(c)));
    }

    #[test]
    fn root_fs_listed() {
        let fs = file_systems(true);
        assert!(fs.iter().any(|f| f.dir == "/"));
        let e = FsEntry {
            device: "d".into(),
            dir: "/".into(),
            fstype: "ext4".into(),
            total: 100,
            free: 40,
            available: 20,
        };
        assert_eq!(e.used(), 60);
        assert!((e.used_fraction() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn interval_clamped() {
        assert_eq!(clamp_interval(0), 1000);
        assert_eq!(clamp_interval(250), 1000);
        assert_eq!(clamp_interval(3000), 3000);
        assert_eq!(clamp_interval(u32::MAX), 100_000);
    }
}
