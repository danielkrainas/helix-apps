//! The bubblewrap sandbox thumbnailers run in, as GNOME's
//! (gnome-desktop's `gnome-desktop-thumbnail-script.c`) does: a thumbnailer
//! reads files chosen by whoever sent them, so a bug in its decoder must
//! not reach the user's files, network or session.
//!
//! Inside: `/usr` (and the `/bin`, `/lib`… links or folders), the dynamic
//! linker cache, `/etc/alternatives` and the fontconfig cache, read-only;
//! fresh `/proc` and `/dev`; a private, empty `/tmp` (a temporary folder
//! outside) that holds the output, with the one input file bound read-only
//! into it. Every namespace is unshared (no network, no other processes),
//! the environment is cleared, the sandbox dies with Files, and a seccomp
//! filter refuses the system calls GNOME's does: new namespaces and mounts
//! (`unshare`, `setns`, `mount`…, `clone` with `CLONE_NEWUSER`, `clone3`),
//! the kernel keyring, `ptrace`, `perf_event_open`, terminal injection
//! (`TIOCSTI`, `TIOCLINUX`), NUMA calls, uncommon socket families, other
//! personalities and other architectures' system calls.
//!
//! GStreamer's plugin registry is kept between runs in
//! `$XDG_CACHE_HOME/helix-files/thumbnailer/gstreamer-1.0` (bound
//! writable), or every video thumbnail would rebuild it.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the output folder appears inside the sandbox.
pub const TMP: &str = "/tmp";

/// bwrap's arguments (without the command) for a sandbox whose `/tmp` is
/// `outdir`, with `input` bound read-only at `inside_input`, reading its
/// seccomp filter from `seccomp_fd`.
pub fn bwrap_args(
    outdir: &Path,
    input: &Path,
    inside_input: &str,
    seccomp_fd: Option<i32>,
    gst_cache: Option<&Path>,
) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    push(&["--ro-bind", "/usr", "/usr"]);
    push(&["--ro-bind-try", "/etc/ld.so.cache", "/etc/ld.so.cache"]);
    for dir in ["bin", "lib64", "lib", "sbin"] {
        let abs = format!("/{dir}");
        let Ok(meta) = std::fs::symlink_metadata(&abs) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            // Merged /usr: /bin -> usr/bin.
            let target = std::fs::read_link(&abs)
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_else(|_| format!("usr/{dir}"));
            push(&["--symlink", &target, &abs]);
        } else {
            push(&["--ro-bind", &abs, &abs]);
        }
    }
    push(&["--ro-bind-try", "/etc/alternatives", "/etc/alternatives"]);
    push(&[
        "--ro-bind-try",
        "/var/cache/fontconfig",
        "/var/cache/fontconfig",
    ]);
    push(&["--proc", "/proc", "--dev", "/dev", "--chdir", "/"]);
    push(&["--unshare-all", "--die-with-parent", "--new-session"]);
    push(&["--clearenv"]);
    push(&["--setenv", "PATH", "/usr/bin:/bin"]);
    push(&["--setenv", "HOME", TMP]);
    push(&["--setenv", "TMPDIR", TMP]);
    push(&["--setenv", "LANG", "C.UTF-8"]);
    push(&["--setenv", "GIO_USE_VFS", "local"]);
    if let Some(dir) = gst_cache {
        let dir = dir.to_string_lossy();
        let registry = format!("{dir}/registry.{}.bin", std::env::consts::ARCH);
        push(&["--bind", &dir, &dir]);
        push(&["--setenv", "GST_REGISTRY_1_0", &registry]);
    }
    let out = outdir.to_string_lossy();
    push(&["--bind", &out, TMP]);
    let input = input.to_string_lossy();
    push(&["--ro-bind", &input, inside_input]);
    if let Some(fd) = seccomp_fd {
        push(&["--seccomp", &fd.to_string()]);
    }
    a
}

/// The GStreamer registry folder (created if needed).
pub fn gst_cache_dir() -> Option<PathBuf> {
    let dir = gtk::glib::user_cache_dir()
        .join("helix-files")
        .join("thumbnailer")
        .join("gstreamer-1.0");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

// ---- seccomp ----------------------------------------------------------------

/// A classic BPF instruction (`struct sock_filter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Insn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

const LD_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JEQ: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const JGE: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
const JSET: u16 = 0x45; // BPF_JMP | BPF_JSET | BPF_K
const RET: u16 = 0x06; // BPF_RET | BPF_K

const RET_ALLOW: u32 = 0x7fff_0000;
const fn ret_errno(e: i32) -> u32 {
    0x0005_0000 | (e as u32 & 0xffff)
}

// `struct seccomp_data` offsets.
const NR: u32 = 0;
const ARCH: u32 = 4;
const fn arg_low(i: u32) -> u32 {
    16 + 8 * i
}

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7;

const fn ld(off: u32) -> Insn {
    Insn {
        code: LD_ABS,
        jt: 0,
        jf: 0,
        k: off,
    }
}
const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Insn {
    Insn { code, jt, jf, k }
}
const fn ret(k: u32) -> Insn {
    Insn {
        code: RET,
        jt: 0,
        jf: 0,
        k,
    }
}

/// System calls refused outright, with the error returned.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn blocked() -> Vec<(i64, i32)> {
    use libc::*;
    let mut v = vec![
        (SYS_syslog, EPERM),
        (SYS_acct, EPERM),
        (SYS_quotactl, EPERM),
        (SYS_add_key, EPERM),
        (SYS_keyctl, EPERM),
        (SYS_request_key, EPERM),
        (SYS_move_pages, EPERM),
        (SYS_mbind, EPERM),
        (SYS_get_mempolicy, EPERM),
        (SYS_set_mempolicy, EPERM),
        (SYS_migrate_pages, EPERM),
        (SYS_unshare, EPERM),
        (SYS_setns, EPERM),
        (SYS_mount, EPERM),
        (SYS_umount2, EPERM),
        (SYS_pivot_root, EPERM),
        (SYS_chroot, EPERM),
        (SYS_clone3, ENOSYS),
        (SYS_open_tree, ENOSYS),
        (SYS_move_mount, ENOSYS),
        (SYS_fsopen, ENOSYS),
        (SYS_fsconfig, ENOSYS),
        (SYS_fsmount, ENOSYS),
        (SYS_fspick, ENOSYS),
        (SYS_mount_setattr, ENOSYS),
        (SYS_perf_event_open, EPERM),
        (SYS_ptrace, EPERM),
    ];
    #[cfg(target_arch = "x86_64")]
    v.extend([(SYS_uselib, EPERM), (SYS_modify_ldt, EPERM)]);
    v
}

/// The filter program, `None` on architectures it is not written for.
pub fn filter() -> Option<Vec<Insn>> {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        let eperm = ret_errno(libc::EPERM);
        let mut p = vec![
            // Only this architecture's system calls.
            ld(ARCH),
            jump(JEQ, AUDIT_ARCH, 1, 0),
            ret(eperm),
            ld(NR),
        ];
        // x32 system calls (bit 30) on x86-64.
        #[cfg(target_arch = "x86_64")]
        p.extend([jump(JGE, 0x4000_0000, 0, 1), ret(eperm)]);
        for (nr, errno) in blocked() {
            p.extend([jump(JEQ, nr as u32, 0, 1), ret(ret_errno(errno))]);
        }
        // clone with CLONE_NEWUSER in its flags (the first argument).
        p.extend([
            jump(JEQ, libc::SYS_clone as u32, 0, 4),
            ld(arg_low(0)),
            jump(JSET, libc::CLONE_NEWUSER as u32, 0, 1),
            ret(eperm),
            ret(RET_ALLOW),
        ]);
        // ioctl(TIOCSTI / TIOCLINUX): faking terminal input.
        p.extend([
            jump(JEQ, libc::SYS_ioctl as u32, 0, 5),
            ld(arg_low(1)),
            jump(JEQ, libc::TIOCSTI as u32, 2, 0),
            jump(JEQ, libc::TIOCLINUX as u32, 1, 0),
            ret(RET_ALLOW),
            ret(eperm),
        ]);
        // personality: only PER_LINUX.
        p.extend([
            jump(JEQ, libc::SYS_personality as u32, 0, 4),
            ld(arg_low(0)),
            jump(JEQ, 0, 0, 1),
            ret(RET_ALLOW),
            ret(eperm),
        ]);
        // socket: AF_UNSPEC, AF_UNIX, AF_INET, AF_INET6 and AF_NETLINK.
        p.extend([
            jump(JEQ, libc::SYS_socket as u32, 0, 8),
            ld(arg_low(0)),
            jump(JEQ, libc::AF_UNSPEC as u32, 5, 0),
            jump(JEQ, libc::AF_UNIX as u32, 4, 0),
            jump(JEQ, libc::AF_INET as u32, 3, 0),
            jump(JEQ, libc::AF_INET6 as u32, 2, 0),
            jump(JEQ, libc::AF_NETLINK as u32, 1, 0),
            ret(ret_errno(libc::EAFNOSUPPORT)),
            ret(RET_ALLOW),
        ]);
        p.push(ret(RET_ALLOW));
        Some(p)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        None
    }
}

/// The filter in a memory file for `bwrap --seccomp` (close-on-exec: the
/// child that should get it clears that, see [`inherit`]).
pub fn filter_fd() -> Option<OwnedFd> {
    let prog = filter()?;
    let bytes: Vec<u8> = prog
        .iter()
        .flat_map(|i| {
            let mut b = Vec::with_capacity(8);
            b.extend(i.code.to_ne_bytes());
            b.push(i.jt);
            b.push(i.jf);
            b.extend(i.k.to_ne_bytes());
            b
        })
        .collect();
    // SAFETY: plain system calls on a file descriptor we own.
    unsafe {
        let fd = libc::memfd_create(c"helix-files-seccomp".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return None;
        }
        let fd = OwnedFd::from_raw_fd(fd);
        let n = libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len());
        if n != bytes.len() as isize || libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_SET) != 0 {
            return None;
        }
        Some(fd)
    }
}

/// Makes `fd` survive into `cmd`'s program (only that child's copy).
pub fn inherit(cmd: &mut std::process::Command, fd: i32) {
    use std::os::unix::process::CommandExt;
    // SAFETY: fcntl is async-signal-safe; it runs in the forked child.
    unsafe {
        cmd.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// The bwrap program, if it is installed and can make a sandbox here
/// (unprivileged user namespaces may be turned off); checked once.
pub fn bwrap() -> Option<&'static Path> {
    static BWRAP: OnceLock<Option<PathBuf>> = OnceLock::new();
    BWRAP
        .get_or_init(|| {
            let path = gtk::glib::find_program_in_path("bwrap")?;
            let ok = probe(&path);
            if !ok {
                eprintln!(
                    "helix-files: bubblewrap cannot make a sandbox here; \
                     not running thumbnailers"
                );
            }
            ok.then_some(path)
        })
        .as_deref()
}

/// Runs `true` in the sandbox.
fn probe(bwrap: &Path) -> bool {
    let Ok(dir) = tempdir() else { return false };
    let input = dir.join("probe");
    let _ = std::fs::write(&input, b"");
    let seccomp = filter_fd();
    let mut cmd = std::process::Command::new(bwrap);
    cmd.args(bwrap_args(
        &dir,
        &input,
        "/tmp/in",
        seccomp.as_ref().map(|f| f.as_raw_fd()),
        None,
    ));
    if let Some(fd) = &seccomp {
        inherit(&mut cmd, fd.as_raw_fd());
    }
    cmd.arg("true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let ok = cmd.status().is_ok_and(|s| s.success());
    remove_tempdir(&dir);
    ok
}

/// A new private folder in the temporary folder.
pub fn tempdir() -> std::io::Result<PathBuf> {
    let template = gtk::glib::tmp_dir().join("helix-files-thumbnailer-XXXXXX");
    gtk::glib::mkdtemp(&template).ok_or_else(|| std::io::Error::other("mkdtemp failed"))
}

/// Removes a folder made by [`tempdir`] and all the sandbox left in it
/// (bwrap's mount point for the input, a thumbnailer's `$HOME/.cache`…).
/// Symbolic links in it are removed, not followed.
pub fn remove_tempdir(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Reads the file the sandbox wrote at `path`: a regular file (not a
/// symbolic link out of the sandbox) of at most `max` bytes.
pub fn read_output(path: &Path, max: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(max + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Every jump of the filter lands inside it.
    #[test]
    fn filter_jumps_stay_inside() {
        let Some(p) = filter() else { return };
        for (i, insn) in p.iter().enumerate() {
            if insn.code & 0x07 == 0x05 {
                assert!(i + 1 + (insn.jt as usize) < p.len(), "jt at {i}");
                assert!(i + 1 + (insn.jf as usize) < p.len(), "jf at {i}");
            }
        }
        assert_eq!(p.last().map(|i| i.k), Some(RET_ALLOW));
    }

    /// Runs `sh -c script` in the sandbox (with `input` bound at
    /// /tmp/in); `None` where bubblewrap cannot make one.
    fn sandboxed(script: &str, input: &Path) -> Option<(bool, String)> {
        let bwrap = bwrap()?;
        let dir = tempdir().unwrap();
        let seccomp = filter_fd();
        let mut cmd = Command::new(bwrap);
        cmd.args(bwrap_args(
            &dir,
            input,
            "/tmp/in",
            seccomp.as_ref().map(|f| f.as_raw_fd()),
            None,
        ));
        if let Some(fd) = &seccomp {
            inherit(&mut cmd, fd.as_raw_fd());
        }
        let out = cmd
            .args(["sh", "-c", script])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        remove_tempdir(&dir);
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        Some((out.status.success(), text))
    }

    #[test]
    fn output_is_read_only_from_a_regular_file() {
        let dir = std::env::temp_dir().join(format!("helix-files-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub/.cache")).unwrap();
        let secret =
            std::env::temp_dir().join(format!("helix-files-secret-{}", std::process::id()));
        std::fs::write(&secret, "secret").unwrap();
        std::fs::write(dir.join("thumbnail.png"), "png").unwrap();
        assert_eq!(
            read_output(&dir.join("thumbnail.png"), 10).as_deref(),
            Some(&b"png"[..])
        );
        assert!(read_output(&dir.join("thumbnail.png"), 2).is_none());
        std::os::unix::fs::symlink(&secret, dir.join("link.png")).unwrap();
        assert!(read_output(&dir.join("link.png"), 100).is_none());
        assert!(read_output(&dir.join("sub"), 100).is_none());
        // Cleaning up removes what the sandbox left, not what links point to.
        std::os::unix::fs::symlink(&secret, dir.join("sub/.cache/link")).unwrap();
        remove_tempdir(&dir);
        assert!(!dir.exists());
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret");
        let _ = std::fs::remove_file(&secret);
    }

    #[test]
    fn the_sandbox_holds() {
        let dir = std::env::temp_dir().join(format!("helix-files-sandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("input.txt");
        std::fs::write(&input, "hello").unwrap();
        let secret = dir.join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        let Some((ok, out)) = sandboxed("cat /tmp/in", &input) else {
            eprintln!("bubblewrap cannot make a sandbox here; skipping");
            return;
        };
        assert!(ok && out == "hello", "{out}");
        // Only the input is there: not its folder, not the home folder.
        let script = format!(
            "cat {} 2>/dev/null && echo LEAK; ls {} 2>/dev/null && echo LEAK; \
             test -e /etc/passwd && echo LEAK; true",
            secret.display(),
            gtk::glib::home_dir().display()
        );
        let (_, out) = sandboxed(&script, &input).unwrap();
        assert!(!out.contains("LEAK"), "{out}");
        // Writable output, read-only input.
        let (ok, _) = sandboxed("echo x > /tmp/out.png", &input).unwrap();
        assert!(ok);
        let (ok, _) = sandboxed("echo x >> /tmp/in", &input).unwrap();
        assert!(!ok);
        assert_eq!(std::fs::read_to_string(&input).unwrap(), "hello");
        // No new namespaces (bubblewrap inside, unshare, …).
        if Path::new("/usr/bin/unshare").exists() {
            let (ok, out) = sandboxed("unshare -U true", &input).unwrap();
            assert!(!ok, "{out}");
        }
        // No network: only loopback, and it is down.
        let (_, out) = sandboxed("cat /proc/net/dev", &input).unwrap();
        let ifaces: Vec<&str> = out
            .lines()
            .skip(2)
            .filter_map(|l| l.split(':').next())
            .map(str::trim)
            .collect();
        assert!(ifaces.iter().all(|i| *i == "lo"), "{ifaces:?}");
        // A clean environment.
        let (_, out) = sandboxed("env", &input).unwrap();
        assert!(!out.contains("DBUS_SESSION_BUS_ADDRESS"), "{out}");
        assert!(!out.contains("WAYLAND_DISPLAY"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
