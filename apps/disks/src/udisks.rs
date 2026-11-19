//! UDisks2 over the system bus (zbus, blocking API; call from worker threads).

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{Fd, OwnedFd, OwnedObjectPath, OwnedValue, Value};

use crate::model::iface;
use crate::ops::{Op, OpOutput};
use crate::props::{Interfaces, Prop, Props, Snapshot};
use crate::smart::Attribute;
use helix_app_common::i18n::tr;

pub const SERVICE: &str = "org.freedesktop.UDisks2";
pub const ROOT: &str = "/org/freedesktop/UDisks2";
pub const MANAGER: &str = "/org/freedesktop/UDisks2/Manager";

/// The error as text with its D-Bus name, so the UI can tell the common
/// kinds apart (`helix_app_common::errors`); the name is not shown.
pub fn error_text(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, Some(msg), _) => {
            if name.as_str().ends_with("NotAuthorizedDismissed") {
                tr("Authentication was dismissed.").into()
            } else {
                format!("{name}: {msg}")
            }
        }
        zbus::Error::MethodError(name, None, _) => name.to_string(),
        other => other.to_string(),
    }
}

static CONN: OnceLock<Connection> = OnceLock::new();

/// Shared system bus connection for operations.
pub fn conn() -> Result<Connection, String> {
    if let Some(c) = CONN.get() {
        return Ok(c.clone());
    }
    let c = Connection::system().map_err(|e| error_text(&e))?;
    Ok(CONN.get_or_init(|| c).clone())
}

// ------------------------------------------------------------ conversion

fn convert(v: &Value) -> Prop {
    match v {
        Value::Bool(b) => Prop::Bool(*b),
        Value::U8(x) => Prop::U64(*x as u64),
        Value::U16(x) => Prop::U64(*x as u64),
        Value::U32(x) => Prop::U64(*x as u64),
        Value::U64(x) => Prop::U64(*x),
        Value::I16(x) => Prop::I64(*x as i64),
        Value::I32(x) => Prop::I64(*x as i64),
        Value::I64(x) => Prop::I64(*x),
        Value::F64(x) => Prop::F64(*x),
        Value::Str(s) => Prop::Str(s.to_string()),
        Value::ObjectPath(p) => Prop::Path(p.to_string()),
        Value::Value(inner) => convert(inner),
        Value::Array(a) => {
            let sig = a.element_signature().to_string();
            match sig.as_str() {
                "y" => Prop::Bytes(
                    a.iter()
                        .filter_map(|e| if let Value::U8(b) = e { Some(*b) } else { None })
                        .collect(),
                ),
                "ay" => Prop::ByteStrings(
                    a.iter()
                        .filter_map(|e| match convert(e) {
                            Prop::Bytes(b) => Some(b),
                            _ => None,
                        })
                        .collect(),
                ),
                "o" => Prop::Paths(
                    a.iter()
                        .filter_map(|e| {
                            if let Value::ObjectPath(p) = e {
                                Some(p.to_string())
                            } else {
                                None
                            }
                        })
                        .collect(),
                ),
                "s" => Prop::Strings(
                    a.iter()
                        .filter_map(|e| {
                            if let Value::Str(s) = e {
                                Some(s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect(),
                ),
                _ => Prop::Other,
            }
        }
        _ => Prop::Other,
    }
}

type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn convert_props(p: &HashMap<String, OwnedValue>) -> Props {
    p.iter().map(|(k, v)| (k.clone(), convert(v))).collect()
}

/// Reads all UDisks2 objects (read-only `GetManagedObjects`).
pub fn fetch(conn: &Connection) -> Result<Snapshot, String> {
    let reply = conn
        .call_method(
            Some(SERVICE),
            ROOT,
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .map_err(|e| error_text(&e))?;
    let objs: Managed = reply.body().deserialize().map_err(|e| e.to_string())?;
    Ok(objs
        .iter()
        .map(|(path, ifs)| {
            let ifs: Interfaces = ifs
                .iter()
                .map(|(name, props)| (name.clone(), convert_props(props)))
                .collect();
            (path.to_string(), ifs)
        })
        .collect())
}

pub enum Event {
    Snapshot(Snapshot),
    Error(String),
}

/// Starts background threads that send a snapshot now and after every
/// (debounced) UDisks2 change signal.
pub fn spawn_monitor(tx: async_channel::Sender<Event>) {
    std::thread::spawn(move || {
        let conn = match Connection::system() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send_blocking(Event::Error(error_text(&e)));
                return;
            }
        };
        let (dirty_tx, dirty_rx) = mpsc::channel::<()>();
        // Subscribe before the first fetch so no change is missed.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .path_namespace(ROOT)
            .map(|b| b.build());
        match rule.and_then(|r| MessageIterator::for_match_rule(r, &conn, Some(1024))) {
            Ok(iter) => {
                std::thread::spawn(move || {
                    for msg in iter {
                        if msg.is_err() || dirty_tx.send(()).is_err() {
                            break;
                        }
                    }
                });
            }
            Err(e) => eprintln!(
                "helix-disks: cannot watch UDisks2 signals: {}",
                error_text(&e)
            ),
        }
        let send = |tx: &async_channel::Sender<Event>| match fetch(&conn) {
            Ok(s) => tx.send_blocking(Event::Snapshot(s)).is_ok(),
            Err(e) => tx.send_blocking(Event::Error(e)).is_ok(),
        };
        if !send(&tx) {
            return;
        }
        while dirty_rx.recv().is_ok() {
            // Debounce bursts of signals.
            while dirty_rx.recv_timeout(Duration::from_millis(250)).is_ok() {}
            if !send(&tx) {
                return;
            }
        }
    });
}

// ------------------------------------------------------------ operations

type Opts = HashMap<&'static str, Value<'static>>;

fn opts() -> Opts {
    let mut o = Opts::new();
    // Let udisks ask polkit for authorization interactively.
    o.insert("auth.no_user_interaction", Value::Bool(false));
    o
}

fn call<B>(
    conn: &Connection,
    path: &str,
    iface: &str,
    method: &str,
    body: &B,
) -> Result<zbus::message::Message, String>
where
    B: serde::ser::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(SERVICE), path, Some(iface), method, body)
        .map_err(|e| error_text(&e))
}

fn format_opts(label: &str, erase: bool, passphrase: &Option<String>) -> Opts {
    let mut o = opts();
    if !label.is_empty() {
        o.insert("label", Value::from(label.to_string()));
    }
    if erase {
        o.insert("erase", Value::from("zero".to_string()));
    }
    if let Some(p) = passphrase {
        o.insert("encrypt.passphrase", Value::from(p.clone()));
        o.insert("encrypt.type", Value::from("luks2".to_string()));
    }
    o
}

type RawAttribute = (
    u8,
    String,
    u16,
    i32,
    i32,
    i32,
    i64,
    i32,
    HashMap<String, OwnedValue>,
);

/// Executes one operation. MUTATES SYSTEM STATE — only called from UI
/// actions confirmed by the user, never from tests.
pub fn run(conn: &Connection, op: &Op) -> Result<OpOutput, String> {
    use iface::*;
    let none = |r: Result<zbus::message::Message, String>| r.map(|_| OpOutput::None);
    match op {
        Op::Mount { block } => {
            let r = call(conn, block, FILESYSTEM, "Mount", &(opts(),))?;
            let path: String = r.body().deserialize().map_err(|e| e.to_string())?;
            Ok(OpOutput::Path(path))
        }
        Op::Unmount { block } => none(call(conn, block, FILESYSTEM, "Unmount", &(opts(),))),
        Op::SwapStart { block } => none(call(conn, block, SWAP, "Start", &(opts(),))),
        Op::SwapStop { block } => none(call(conn, block, SWAP, "Stop", &(opts(),))),
        Op::Unlock { block, passphrase } => {
            let r = call(
                conn,
                block,
                ENCRYPTED,
                "Unlock",
                &(passphrase.as_str(), opts()),
            )?;
            let path: OwnedObjectPath = r.body().deserialize().map_err(|e| e.to_string())?;
            Ok(OpOutput::Path(path.to_string()))
        }
        Op::Lock { block } => none(call(conn, block, ENCRYPTED, "Lock", &(opts(),))),
        Op::Format {
            block,
            fs_type,
            label,
            erase,
            passphrase,
            update_partition_type,
        } => {
            let mut o = format_opts(label, *erase, passphrase);
            o.insert("update-partition-type", Value::Bool(*update_partition_type));
            o.insert("tear-down", Value::Bool(true));
            none(call(conn, block, BLOCK, "Format", &(fs_type.as_str(), o)))
        }
        Op::CreatePartition {
            table,
            offset,
            size,
            part_type,
            name,
            fs_type,
            label,
            passphrase,
        } => {
            let fo = format_opts(label, false, passphrase);
            let r = call(
                conn,
                table,
                TABLE,
                "CreatePartitionAndFormat",
                &(
                    *offset,
                    *size,
                    part_type.as_str(),
                    name.as_str(),
                    opts(),
                    fs_type.as_str(),
                    fo,
                ),
            )?;
            let path: OwnedObjectPath = r.body().deserialize().map_err(|e| e.to_string())?;
            Ok(OpOutput::Path(path.to_string()))
        }
        Op::DeletePartition { block } => {
            let mut o = opts();
            o.insert("tear-down", Value::Bool(true));
            none(call(conn, block, PARTITION, "Delete", &(o,)))
        }
        Op::SetPartitionType { block, part_type } => none(call(
            conn,
            block,
            PARTITION,
            "SetType",
            &(part_type.as_str(), opts()),
        )),
        Op::SetPartitionName { block, name } => none(call(
            conn,
            block,
            PARTITION,
            "SetName",
            &(name.as_str(), opts()),
        )),
        Op::SetPartitionFlags { block, flags } => {
            none(call(conn, block, PARTITION, "SetFlags", &(*flags, opts())))
        }
        Op::SetLabel { block, label } => none(call(
            conn,
            block,
            FILESYSTEM,
            "SetLabel",
            &(label.as_str(), opts()),
        )),
        Op::Check { block } | Op::Repair { block } => {
            let method = if matches!(op, Op::Check { .. }) {
                "Check"
            } else {
                "Repair"
            };
            let r = call(conn, block, FILESYSTEM, method, &(opts(),))?;
            let ok: bool = r.body().deserialize().map_err(|e| e.to_string())?;
            Ok(OpOutput::Bool(ok))
        }
        Op::PowerOff { drive } => none(call(conn, drive, DRIVE, "PowerOff", &(opts(),))),
        Op::Eject { drive } => none(call(conn, drive, DRIVE, "Eject", &(opts(),))),
        Op::LoopDetach { block } => none(call(conn, block, LOOP, "Delete", &(opts(),))),
        Op::SmartAttributes { drive } => {
            let r = call(conn, drive, ATA, "SmartGetAttributes", &(opts(),))?;
            let raw: Vec<RawAttribute> = r.body().deserialize().map_err(|e| e.to_string())?;
            Ok(OpOutput::Attributes(
                raw.into_iter()
                    .map(
                        |(id, name, flags, value, worst, threshold, pretty, pretty_unit, _)| {
                            Attribute {
                                id,
                                name,
                                flags,
                                value,
                                worst,
                                threshold,
                                pretty,
                                pretty_unit,
                            }
                        },
                    )
                    .collect(),
            ))
        }
    }
}

/// Attaches `path` as a loop device (`Manager.LoopSetup`). Mutating.
pub fn loop_setup(
    conn: &Connection,
    path: &std::path::Path,
    read_only: bool,
) -> Result<String, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(!read_only)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut o = opts();
    o.insert("read-only", Value::Bool(read_only));
    let r = call(
        conn,
        MANAGER,
        iface::MANAGER,
        "LoopSetup",
        &(Fd::from(file.as_fd()), o),
    )?;
    let p: OwnedObjectPath = r.body().deserialize().map_err(|e| e.to_string())?;
    Ok(p.to_string())
}

/// Opens a block device for reading (`Block.OpenForBackup`).
pub fn open_for_backup(conn: &Connection, block: &str) -> Result<File, String> {
    let r = call(conn, block, iface::BLOCK, "OpenForBackup", &(opts(),))?;
    let fd: OwnedFd = r.body().deserialize().map_err(|e| e.to_string())?;
    Ok(File::from(std::os::fd::OwnedFd::from(fd)))
}

/// Opens a block device for writing (`Block.OpenForRestore`). Mutating.
pub fn open_for_restore(conn: &Connection, block: &str) -> Result<File, String> {
    let r = call(conn, block, iface::BLOCK, "OpenForRestore", &(opts(),))?;
    let fd: OwnedFd = r.body().deserialize().map_err(|e| e.to_string())?;
    Ok(File::from(std::os::fd::OwnedFd::from(fd)))
}

/// Copies up to `total` bytes from `src` to `dst`, calling `progress` with
/// the running byte count. Stops with `ErrorKind::Interrupted` when
/// `cancel` becomes true.
/// Creates (or truncates) the disk image file at `path`, readable and
/// writable by its owner only: an image of a system or home partition
/// holds what only root could read on the disk. An existing file chosen
/// to be replaced gets the same permissions.
pub fn create_image_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

pub fn copy_with_progress<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    total: u64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> io::Result<u64> {
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done = 0u64;
    while done < total {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                tr("Operation was cancelled"),
            ));
        }
        let want = buf.len().min((total - done) as usize);
        let n = match src.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        dst.write_all(&buf[..n])?;
        done += n as u64;
        progress(done);
    }
    dst.flush()?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_images_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("helix-disks-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let new = dir.join("new.img");
        create_image_file(&new).unwrap();
        assert_eq!(mode(&new), 0o600);
        // Replacing a readable file makes it private too, and empties it.
        let old = dir.join("old.img");
        std::fs::write(&old, b"old contents").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        create_image_file(&old).unwrap();
        assert_eq!(mode(&old), 0o600);
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
    use zbus::zvariant::{Array, ObjectPath};

    #[test]
    fn copy_reports_progress_and_stops_at_total() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| i as u8).collect();
        let mut out = Vec::new();
        let mut calls = Vec::new();
        let cancel = AtomicBool::new(false);
        let n = copy_with_progress(&mut &data[..], &mut out, 2_500_000, &cancel, |d| {
            calls.push(d)
        })
        .unwrap();
        assert_eq!(n, 2_500_000);
        assert_eq!(out, data[..2_500_000]);
        assert_eq!(calls, vec![1_048_576, 2_097_152, 2_500_000]);
    }

    #[test]
    fn copy_cancel_and_short_source() {
        let data = [1u8; 10];
        let cancel = AtomicBool::new(true);
        let err =
            copy_with_progress(&mut &data[..], &mut Vec::new(), 10, &cancel, |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        assert_eq!(
            copy_with_progress(&mut &data[..], &mut out, 100, &cancel, |_| {}).unwrap(),
            10
        );
    }

    #[test]
    fn value_conversion() {
        assert_eq!(convert(&Value::U32(7)), Prop::U64(7));
        assert_eq!(convert(&Value::I32(-1)), Prop::I64(-1));
        assert_eq!(convert(&Value::from("x")), Prop::Str("x".into()));
        assert_eq!(
            convert(&Value::from(ObjectPath::try_from("/a").unwrap())),
            Prop::Path("/a".into())
        );
        let bytes = Value::from(vec![b'/', b'd', 0u8]);
        assert_eq!(convert(&bytes), Prop::Bytes(vec![b'/', b'd', 0]));
        let aay = Value::from(vec![vec![b'/', 0u8], vec![b'/', b'm', 0u8]]);
        assert_eq!(
            convert(&aay),
            Prop::ByteStrings(vec![vec![b'/', 0], vec![b'/', b'm', 0]])
        );
        let ao = Value::from(Array::from(vec![ObjectPath::try_from("/x").unwrap()]));
        assert_eq!(convert(&ao), Prop::Paths(vec!["/x".into()]));
        assert_eq!(convert(&Value::new(Value::Bool(true))), Prop::Bool(true));
    }
}
