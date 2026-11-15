//! Mounted file systems (`/proc/self/mounts` + `statvfs`).

use helix_app_common::i18n::tr;
use std::collections::HashSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub device: String,
    pub mount_point: PathBuf,
    pub fs_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub total: u64,
    pub free: u64,
    pub available: u64,
}

impl Usage {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
    /// Fraction used, 0..=1.
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.used() as f64 / self.total as f64
        }
    }
}

#[derive(Debug, Clone)]
pub struct Location {
    pub name: String,
    pub path: PathBuf,
    pub detail: String,
    pub usage: Option<Usage>,
    pub is_home: bool,
}

/// File system types that are virtual / not interesting for disk usage.
const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "ramfs",
    "securityfs",
    "cgroup",
    "cgroup2",
    "pstore",
    "bpf",
    "debugfs",
    "tracefs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "efivarfs",
    "rpc_pipefs",
    "nsfs",
    "overlay",
    "squashfs",
    "selinuxfs",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "fuse.snapfuse",
    "nfsd",
    "devfs",
    "none",
    "ecryptfs",
    "zram",
    "rootfs",
];

pub fn is_pseudo(fs_type: &str) -> bool {
    PSEUDO_FS.contains(&fs_type)
}

/// Decodes the octal escapes (`\040` etc.) used in `/proc/self/mounts`.
pub fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') as u32 * 64
                + (b[i + 2] - b'0') as u32 * 8
                + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses mount table text, dropping pseudo file systems and duplicate
/// mount points (the last mount on a point wins, like the kernel).
pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    let mut entries: Vec<MountEntry> = Vec::new();
    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(dev), Some(mp), Some(ty)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if is_pseudo(ty) {
            continue;
        }
        let mount_point = PathBuf::from(unescape(mp));
        entries.retain(|e| e.mount_point != mount_point);
        entries.push(MountEntry {
            device: unescape(dev),
            mount_point,
            fs_type: ty.to_string(),
        });
    }
    entries
}

pub fn statvfs(path: &Path) -> Option<Usage> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a valid NUL-terminated string and st a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = if st.f_frsize > 0 {
        st.f_frsize
    } else {
        st.f_bsize
    } as u64;
    Some(Usage {
        total: st.f_blocks as u64 * frsize,
        free: st.f_bfree as u64 * frsize,
        available: st.f_bavail as u64 * frsize,
    })
}

fn mount_name(mp: &Path) -> String {
    if mp == Path::new("/") {
        tr("Computer").into()
    } else {
        mp.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| mp.display().to_string())
    }
}

/// Home folder plus every real mounted file system. May block on network
/// mounts, so call it from a worker thread.
pub fn locations(home: &Path) -> Vec<Location> {
    let mut out = vec![Location {
        name: tr("Home Folder").into(),
        path: home.to_path_buf(),
        detail: home.display().to_string(),
        usage: statvfs(home),
        is_home: true,
    }];
    let text = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut seen_devices = HashSet::new();
    for m in parse_mounts(&text) {
        let Some(usage) = statvfs(&m.mount_point) else {
            continue;
        };
        if usage.total == 0 {
            continue;
        }
        // Bind mounts of the same device: list the first only.
        if m.device.starts_with('/') && !seen_devices.insert(m.device.clone()) {
            continue;
        }
        out.push(Location {
            name: mount_name(&m.mount_point),
            detail: format!("{} · {} ({})", m.mount_point.display(), m.device, m.fs_type),
            path: m.mount_point,
            usage: Some(usage),
            is_home: false,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_octal() {
        assert_eq!(unescape(r"/media/My\040Disk"), "/media/My Disk");
        assert_eq!(unescape(r"a\011b\134c"), "a\tb\\c");
        assert_eq!(unescape(r"trailing\04"), r"trailing\04");
        assert_eq!(unescape("plain"), "plain");
    }

    #[test]
    fn parse_skips_pseudo_and_dedups() {
        let text = "\
sysfs /sys sysfs rw,nosuid 0 0
proc /proc proc rw 0 0
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
tmpfs /run tmpfs rw 0 0
/dev/nvme0n1p1 /boot/efi vfat rw 0 0
cgroup2 /sys/fs/cgroup cgroup2 rw 0 0
/dev/sdb1 /media/My\\040Disk ext4 rw 0 0
/dev/sdc1 /mnt btrfs rw 0 0
/dev/sdd1 /mnt xfs rw 0 0
";
        let m = parse_mounts(text);
        let points: Vec<_> = m
            .iter()
            .map(|e| e.mount_point.to_string_lossy().into_owned())
            .collect();
        assert_eq!(points, ["/", "/boot/efi", "/media/My Disk", "/mnt"]);
        assert_eq!(m[3].fs_type, "xfs");
    }

    #[test]
    fn usage_math() {
        let u = Usage {
            total: 1000,
            free: 250,
            available: 200,
        };
        assert_eq!(u.used(), 750);
        assert!((u.fraction() - 0.75).abs() < 1e-9);
        assert_eq!(Usage::default().fraction(), 0.0);
    }

    #[test]
    fn statvfs_root_works() {
        let u = statvfs(Path::new("/")).unwrap();
        assert!(u.total > 0);
        assert!(statvfs(Path::new("/definitely/not/here")).is_none());
    }

    #[test]
    fn names() {
        assert_eq!(mount_name(Path::new("/")), "Computer");
        assert_eq!(mount_name(Path::new("/media/usb")), "usb");
    }
}
