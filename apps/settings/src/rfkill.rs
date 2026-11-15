//! Radio kill switches (`/sys/class/rfkill`, `/dev/rfkill`).
//!
//! Reading is harmless; [`unblock`] writes to the device and is only
//! called when the user turns a radio on.

use std::io::Write;
use std::path::Path;

pub const SYSFS: &str = "/sys/class/rfkill";
pub const DEVICE: &str = "/dev/rfkill";

/// `RFKILL_TYPE_*` of the kernel's `rfkill.h`.
pub const TYPE_BLUETOOTH: u8 = 2;
/// `RFKILL_OP_CHANGE_ALL`.
const OP_CHANGE_ALL: u8 = 3;

/// One switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switch {
    /// `bluetooth`, `wlan`, `wwan`…
    pub kind: String,
    /// Turned off by software (can be undone here).
    pub soft: bool,
    /// Turned off by a hardware switch or the firmware.
    pub hard: bool,
}

/// The switches under `dir` (normally [`SYSFS`]).
pub fn read(dir: &Path) -> Vec<Switch> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Switch)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).ok();
            let kind = read("type")?.trim().to_owned();
            Some((
                e.file_name().to_string_lossy().into_owned(),
                Switch {
                    kind,
                    soft: read("soft").is_some_and(|s| s.trim() == "1"),
                    hard: read("hard").is_some_and(|s| s.trim() == "1"),
                },
            ))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.into_iter().map(|(_, s)| s).collect()
}

/// How the switches of `kind` stand: (any soft block, every one hard
/// blocked). No switch at all is (false, false).
pub fn blocked(switches: &[Switch], kind: &str) -> (bool, bool) {
    let mine: Vec<&Switch> = switches.iter().filter(|s| s.kind == kind).collect();
    (
        mine.iter().any(|s| s.soft),
        !mine.is_empty() && mine.iter().all(|s| s.hard),
    )
}

/// The event that lifts the soft block of every radio of `kind`
/// (`struct rfkill_event`: idx, type, op, soft, hard).
pub fn unblock_event(kind: u8) -> [u8; 8] {
    let mut e = [0u8; 8];
    e[4] = kind;
    e[5] = OP_CHANGE_ALL;
    e[6] = 0;
    e
}

/// Lifts the soft block of every radio of `kind` by writing to `device`
/// (normally [`DEVICE`]).
pub fn unblock(device: &Path, kind: u8) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(device)?;
    f.write_all(&unblock_event(kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_switches() {
        let dir = std::env::temp_dir().join(format!("helix-rfkill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, kind, soft, hard) in [
            ("rfkill1", "bluetooth", "1", "0"),
            ("rfkill0", "wlan", "0", "1"),
        ] {
            let d = dir.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
            std::fs::write(d.join("soft"), soft).unwrap();
            std::fs::write(d.join("hard"), hard).unwrap();
        }
        let s = read(&dir);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, "wlan");
        assert_eq!(blocked(&s, "bluetooth"), (true, false));
        assert_eq!(blocked(&s, "wlan"), (false, true));
        assert_eq!(blocked(&s, "wwan"), (false, false));
        assert!(read(&dir.join("none")).is_empty());

        // Unblocking writes one change-all event (to a plain file here).
        let dev = dir.join("dev");
        std::fs::write(&dev, b"").unwrap();
        unblock(&dev, TYPE_BLUETOOTH).unwrap();
        assert_eq!(std::fs::read(&dev).unwrap(), [0, 0, 0, 0, 2, 3, 0, 0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
