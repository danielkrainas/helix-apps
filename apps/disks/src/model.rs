//! Drive/volume model built from a UDisks2 object snapshot.

use crate::format;
use crate::layout::{self, Span, MIB};
use crate::parttypes;
use crate::props::{Props, PropsExt, Snapshot};
use crate::smart::Smart;

pub mod iface {
    pub const MANAGER: &str = "org.freedesktop.UDisks2.Manager";
    pub const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
    pub const ATA: &str = "org.freedesktop.UDisks2.Drive.Ata";
    pub const BLOCK: &str = "org.freedesktop.UDisks2.Block";
    pub const PARTITION: &str = "org.freedesktop.UDisks2.Partition";
    pub const TABLE: &str = "org.freedesktop.UDisks2.PartitionTable";
    pub const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
    pub const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
    pub const LOOP: &str = "org.freedesktop.UDisks2.Loop";
    pub const SWAP: &str = "org.freedesktop.UDisks2.Swapspace";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Disk,
    Loop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drive {
    /// Stable identity: drive object path, or block path for loop devices.
    pub key: String,
    pub kind: DriveKind,
    pub drive_path: Option<String>,
    /// Whole-disk block object.
    pub block_path: Option<String>,
    pub device: String,
    pub vendor: String,
    pub model: String,
    pub revision: String,
    pub serial: String,
    pub size: u64,
    pub removable: bool,
    pub media_removable: bool,
    pub media_available: bool,
    pub ejectable: bool,
    pub can_power_off: bool,
    pub optical: bool,
    pub rotation_rate: i64,
    pub connection_bus: String,
    pub media: String,
    pub sort_key: String,
    pub read_only: bool,
    pub smart: Option<Smart>,
    /// Partition table type (`dos`, `gpt`, …) or `None`.
    pub table_type: Option<String>,
    pub backing_file: Option<String>,
    pub autoclear: bool,
    pub volumes: Vec<Volume>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VolumeKind {
    Block,
    /// Unallocated space in the partition table of block `table`.
    Free { table: String, logical: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartitionInfo {
    pub number: u32,
    pub type_id: String,
    pub name: String,
    pub uuid: String,
    pub flags: u64,
    pub is_container: bool,
    pub is_contained: bool,
    pub table_path: String,
    pub table_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub kind: VolumeKind,
    pub block_path: Option<String>,
    pub device: String,
    pub offset: u64,
    pub size: u64,
    pub id_usage: String,
    pub id_type: String,
    pub id_version: String,
    pub label: String,
    pub uuid: String,
    pub read_only: bool,
    pub partition: Option<PartitionInfo>,
    /// `Some(mount points)` when the block has the Filesystem interface.
    pub filesystem: Option<Vec<String>>,
    /// `Some(cleartext block)` when the block has the Encrypted interface.
    pub encrypted: Option<Option<String>>,
    /// `Some(active)` when the block has the Swapspace interface.
    pub swap_active: Option<bool>,
    pub children: Vec<Volume>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub drives: Vec<Drive>,
}

/// Human name of a filesystem / content type (`IdType`).
pub fn fs_name(id_type: &str, id_version: &str) -> String {
    match id_type {
        "ext2" => "Ext2".into(),
        "ext3" => "Ext3".into(),
        "ext4" => "Ext4".into(),
        "vfat" => match id_version {
            "FAT12" => "FAT (12-bit version)".into(),
            "FAT16" => "FAT (16-bit version)".into(),
            "FAT32" => "FAT (32-bit version)".into(),
            _ => "FAT".into(),
        },
        "exfat" => "exFAT".into(),
        "ntfs" => "NTFS".into(),
        "btrfs" => "Btrfs".into(),
        "xfs" => "XFS".into(),
        "f2fs" => "F2FS".into(),
        "iso9660" => "ISO 9660".into(),
        "udf" => "UDF".into(),
        "squashfs" => "SquashFS".into(),
        "hfsplus" => "HFS+".into(),
        "apfs" => "APFS".into(),
        "crypto_LUKS" => "LUKS Encryption".into(),
        "BitLocker" => "BitLocker Encryption".into(),
        "swap" => "Swap".into(),
        "LVM2_member" => "LVM2 Physical Volume".into(),
        "linux_raid_member" => "Linux RAID Member".into(),
        "zfs_member" => "ZFS Device".into(),
        "" => "Unknown".into(),
        other => other.to_string(),
    }
}

impl Volume {
    /// Stable identity used for selection.
    pub fn key(&self) -> String {
        match (&self.kind, &self.block_path) {
            (VolumeKind::Free { table, .. }, _) => format!("free:{table}:{}", self.offset),
            (_, Some(p)) => p.clone(),
            _ => String::new(),
        }
    }

    pub fn is_free(&self) -> bool {
        matches!(self.kind, VolumeKind::Free { .. })
    }

    pub fn is_container(&self) -> bool {
        self.partition.as_ref().is_some_and(|p| p.is_container)
    }

    pub fn mount_points(&self) -> &[String] {
        self.filesystem.as_deref().unwrap_or(&[])
    }

    pub fn is_mounted(&self) -> bool {
        !self.mount_points().is_empty()
    }

    pub fn is_unlocked(&self) -> bool {
        matches!(self.encrypted, Some(Some(_)))
    }

    fn type_name(&self) -> String {
        fs_name(&self.id_type, &self.id_version)
    }

    /// Content description, e.g. `Ext4 (version 1.0) — Mounted at /home`.
    pub fn contents(&self) -> String {
        if self.is_free() {
            return "Unallocated Space".into();
        }
        if self.is_container() {
            return "Extended Partition".into();
        }
        let mut name = self.type_name();
        if !self.id_version.is_empty() && self.id_type != "vfat" {
            name = format!("{name} (version {})", self.id_version);
        }
        if let Some(mps) = &self.filesystem {
            return match mps.first() {
                Some(mp) => format!("{name} — Mounted at {mp}"),
                None => format!("{name} — Not Mounted"),
            };
        }
        if let Some(clear) = &self.encrypted {
            return format!("{name} — {}", if clear.is_some() { "Unlocked" } else { "Locked" });
        }
        if let Some(active) = self.swap_active {
            return format!("{name} — {}", if active { "Active" } else { "Not Active" });
        }
        if self.id_usage.is_empty() && self.id_type.is_empty() {
            return "Unknown".into();
        }
        name
    }

    /// Short name for the volume bar.
    pub fn short_name(&self) -> String {
        if self.is_free() {
            return "Free Space".into();
        }
        if !self.label.is_empty() {
            return self.label.clone();
        }
        if let Some(p) = &self.partition {
            if p.is_container {
                return "Extended Partition".into();
            }
            if !p.name.is_empty() {
                return p.name.clone();
            }
        }
        if self.id_type.is_empty() {
            match &self.partition {
                Some(p) => format!("Partition {}", p.number),
                None => "Unknown".into(),
            }
        } else {
            fs_name(&self.id_type, "")
        }
    }

    /// Partition type description (`Linux Filesystem`, `W95 FAT32 (LBA) (Bootable)`).
    pub fn partition_type(&self) -> Option<String> {
        let p = self.partition.as_ref()?;
        let mut s = parttypes::name(&p.table_type, &p.type_id);
        if p.table_type == "dos" && p.flags & parttypes::DOS_FLAG_BOOTABLE != 0 {
            s.push_str(" (Bootable)");
        }
        Some(s)
    }

    /// Iterates this volume and all contained volumes, pre-order.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Volume>) {
        out.push(self);
        for c in &self.children {
            c.walk(out);
        }
    }
}

impl Drive {
    /// Sidebar/page title, e.g. `2.0 TB Hard Disk`.
    pub fn title(&self) -> String {
        let size = if self.size > 0 { format!("{} ", format::size(self.size)) } else { String::new() };
        match self.kind {
            DriveKind::Loop => format!("{size}Loop Device"),
            DriveKind::Disk if self.optical => "CD/DVD Drive".into(),
            DriveKind::Disk if self.media.starts_with("flash") || (self.media_removable && !self.optical) => {
                if self.media_available {
                    format!("{size}Drive")
                } else {
                    "Card Reader".into()
                }
            }
            DriveKind::Disk if self.removable || self.connection_bus == "usb" => format!("{size}Drive"),
            DriveKind::Disk if self.rotation_rate == 0 => format!("{size}Solid-State Disk"),
            DriveKind::Disk => format!("{size}Hard Disk"),
        }
    }

    /// Vendor + model, or the backing file for loop devices.
    pub fn model_name(&self) -> String {
        if self.kind == DriveKind::Loop {
            return self.backing_file.clone().unwrap_or_default();
        }
        [self.vendor.trim(), self.model.trim()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ")
    }

    pub fn subtitle(&self) -> String {
        let m = self.model_name();
        if m.is_empty() {
            self.device.clone()
        } else {
            m
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self.kind {
            DriveKind::Loop => "media-optical-symbolic",
            DriveKind::Disk if self.optical => "drive-optical-symbolic",
            DriveKind::Disk if self.media.starts_with("flash") => "media-flash-symbolic",
            DriveKind::Disk if self.removable && self.connection_bus == "usb" => "drive-harddisk-usb-symbolic",
            DriveKind::Disk if self.removable || self.media_removable => "drive-removable-media-symbolic",
            DriveKind::Disk if self.rotation_rate == 0 => "drive-harddisk-solidstate-symbolic",
            DriveKind::Disk => "drive-harddisk-symbolic",
        }
    }

    pub fn partitioning(&self) -> String {
        match &self.table_type {
            Some(t) => parttypes::table_name(t).into(),
            None if self.size == 0 => "No Media".into(),
            None => "None".into(),
        }
    }

    pub fn all_volumes(&self) -> Vec<&Volume> {
        let mut out = Vec::new();
        for v in &self.volumes {
            v.walk(&mut out);
        }
        out
    }

    pub fn find_volume(&self, key: &str) -> Option<&Volume> {
        self.all_volumes().into_iter().find(|v| v.key() == key)
    }
}

impl Model {
    pub fn find_drive(&self, key: &str) -> Option<&Drive> {
        self.drives.iter().find(|d| d.key == key)
    }

    /// Finds `(drive key, volume key)` for a device file like `/dev/sdb1`.
    pub fn find_device(&self, device: &str) -> Option<(String, Option<String>)> {
        for d in &self.drives {
            if d.device == device {
                return Some((d.key.clone(), None));
            }
            if let Some(v) = d.all_volumes().into_iter().find(|v| v.device == device) {
                return Some((d.key.clone(), Some(v.key())));
            }
        }
        None
    }
}

// ---------------------------------------------------------------- building

fn iface<'a>(snap: &'a Snapshot, path: &str, name: &str) -> Option<&'a Props> {
    snap.get(path)?.get(name)
}

fn has(snap: &Snapshot, path: &str, name: &str) -> bool {
    iface(snap, path, name).is_some()
}

/// Natural ordering key for device names (`loop2` < `loop10`).
fn natural_key(s: &str) -> (String, u64) {
    let digits: String = s.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    let prefix = s[..s.len() - digits.len()].to_string();
    (prefix, digits.parse().unwrap_or(0))
}

fn smart_from(p: &Props) -> Smart {
    Smart {
        supported: p.bool("SmartSupported"),
        enabled: p.bool("SmartEnabled"),
        failing: p.bool("SmartFailing"),
        temperature_k: p.f64("SmartTemperature"),
        power_on_seconds: p.u64("SmartPowerOnSeconds"),
        num_bad_sectors: p.i64("SmartNumBadSectors"),
        attrs_failing: p.i64("SmartNumAttributesFailing"),
        attrs_failed_in_past: p.i64("SmartNumAttributesFailedInThePast"),
        selftest_status: p.str("SmartSelftestStatus"),
        selftest_percent_remaining: p.i64("SmartSelftestPercentRemaining"),
        updated: p.u64("SmartUpdated"),
    }
}

/// Builds a volume for block `path` (not free space), including children.
fn block_volume(snap: &Snapshot, path: &str, depth: usize) -> Volume {
    let empty = Props::new();
    let b = iface(snap, path, iface::BLOCK).unwrap_or(&empty);
    let partition = iface(snap, path, iface::PARTITION).map(|p| {
        let table_path = p.path("Table").unwrap_or_default();
        let table_type = iface(snap, &table_path, iface::TABLE).map(|t| t.str("Type")).unwrap_or_default();
        PartitionInfo {
            number: p.u64("Number") as u32,
            type_id: p.str("Type"),
            name: p.str("Name"),
            uuid: p.str("UUID"),
            flags: p.u64("Flags"),
            is_container: p.bool("IsContainer"),
            is_contained: p.bool("IsContained"),
            table_path,
            table_type,
        }
    });
    let offset = iface(snap, path, iface::PARTITION).map(|p| p.u64("Offset")).unwrap_or(0);
    // Extended partitions' block devices report 1 KiB; the partition entry
    // has the real extent.
    let size = iface(snap, path, iface::PARTITION).map(|p| p.u64("Size")).filter(|s| *s > 0).unwrap_or_else(|| b.u64("Size"));
    let filesystem = iface(snap, path, iface::FILESYSTEM).map(|f| f.bytestrings("MountPoints"));
    let encrypted = iface(snap, path, iface::ENCRYPTED).map(|e| {
        e.path("CleartextDevice").or_else(|| {
            snap.iter()
                .find(|(_, ifs)| ifs.get(iface::BLOCK).and_then(|b| b.path("CryptoBackingDevice")).as_deref() == Some(path))
                .map(|(p, _)| p.clone())
        })
    });
    let swap_active = iface(snap, path, iface::SWAP).map(|s| s.bool("Active"));

    let mut children = Vec::new();
    if let Some(Some(clear)) = &encrypted {
        if depth < 4 && has(snap, clear, iface::BLOCK) {
            let mut c = block_volume(snap, clear, depth + 1);
            c.offset = offset;
            children.push(c);
        }
    }
    if let Some(p) = &partition {
        if p.is_container {
            children = logical_volumes(snap, &p.table_path, offset, size);
        }
    }

    Volume {
        kind: VolumeKind::Block,
        block_path: Some(path.to_string()),
        device: b.bytestring("Device"),
        offset,
        size,
        id_usage: b.str("IdUsage"),
        id_type: b.str("IdType"),
        id_version: b.str("IdVersion"),
        label: b.str("IdLabel"),
        uuid: b.str("IdUUID"),
        read_only: b.bool("ReadOnly"),
        partition,
        filesystem,
        encrypted,
        swap_active,
        children,
    }
}

fn partitions_of(snap: &Snapshot, table: &str) -> Vec<String> {
    let mut v: Vec<(u64, String)> = snap
        .iter()
        .filter_map(|(path, ifs)| {
            let p = ifs.get(iface::PARTITION)?;
            (p.path("Table").as_deref() == Some(table)).then(|| (p.u64("Offset"), path.clone()))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

fn free_volume(table: &str, span: Span, logical: bool) -> Volume {
    Volume {
        kind: VolumeKind::Free { table: table.to_string(), logical },
        block_path: None,
        device: String::new(),
        offset: span.offset,
        size: span.size,
        id_usage: String::new(),
        id_type: String::new(),
        id_version: String::new(),
        label: String::new(),
        uuid: String::new(),
        read_only: false,
        partition: None,
        filesystem: None,
        encrypted: None,
        swap_active: None,
        children: Vec::new(),
    }
}

fn merge_sorted(mut vols: Vec<Volume>) -> Vec<Volume> {
    vols.sort_by_key(|v| v.offset);
    vols
}

fn logical_volumes(snap: &Snapshot, table: &str, ext_offset: u64, ext_size: u64) -> Vec<Volume> {
    let ext_end = ext_offset + ext_size;
    let logical: Vec<Volume> = partitions_of(snap, table)
        .iter()
        .filter(|p| {
            iface(snap, p, iface::PARTITION)
                .is_some_and(|pp| pp.bool("IsContained") && pp.u64("Offset") >= ext_offset && pp.u64("Offset") < ext_end)
        })
        .map(|p| block_volume(snap, p, 1))
        .collect();
    let used: Vec<Span> = logical.iter().map(|v| Span { offset: v.offset, size: v.size }).collect();
    // Each logical partition is preceded by an EBR padded to 1 MiB, so only
    // gaps larger than that are useful free space.
    let free = layout::free_spans(ext_offset, ext_end, &used, MIB + 1);
    let mut all = logical;
    all.extend(free.into_iter().map(|s| free_volume(table, s, true)));
    merge_sorted(all)
}

/// Top-level volumes of a whole-disk block.
fn disk_volumes(snap: &Snapshot, block: &str, disk_size: u64) -> (Option<String>, Vec<Volume>) {
    if let Some(t) = iface(snap, block, iface::TABLE) {
        let ttype = t.str("Type");
        let top: Vec<Volume> = partitions_of(snap, block)
            .iter()
            .filter(|p| !iface(snap, p, iface::PARTITION).is_some_and(|pp| pp.bool("IsContained")))
            .map(|p| block_volume(snap, p, 0))
            .collect();
        let used: Vec<Span> = top.iter().map(|v| Span { offset: v.offset, size: v.size }).collect();
        let (start, end) = layout::table_region(&ttype, disk_size);
        let free = layout::free_spans(start, end, &used, layout::min_free_gap(disk_size));
        let mut all = top;
        all.extend(free.into_iter().map(|s| free_volume(block, s, false)));
        return (Some(ttype), merge_sorted(all));
    }
    if disk_size == 0 {
        return (None, Vec::new());
    }
    (None, vec![block_volume(snap, block, 0)])
}

fn whole_disk_block(snap: &Snapshot, drive: &str) -> Option<String> {
    let mut v: Vec<&String> = snap
        .iter()
        .filter(|(path, ifs)| {
            ifs.get(iface::BLOCK).is_some_and(|b| b.path("Drive").as_deref() == Some(drive) && b.path("CryptoBackingDevice").is_none())
                && !has(snap, path, iface::PARTITION)
        })
        .map(|(p, _)| p)
        .collect();
    v.sort_by_key(|p| natural_key(p));
    v.first().map(|p| p.to_string())
}

/// Builds the model from a snapshot of all UDisks2 objects.
pub fn build(snap: &Snapshot) -> Model {
    let mut disks = Vec::new();
    for (path, ifs) in snap {
        let Some(d) = ifs.get(iface::DRIVE) else { continue };
        let block_path = whole_disk_block(snap, path);
        let b = block_path.as_deref().and_then(|bp| iface(snap, bp, iface::BLOCK));
        let disk_size = b.map(|b| b.u64("Size")).filter(|s| *s > 0).unwrap_or(d.u64("Size"));
        let (table_type, volumes) = match &block_path {
            Some(bp) => disk_volumes(snap, bp, b.map(|b| b.u64("Size")).unwrap_or(0)),
            None => (None, Vec::new()),
        };
        disks.push(Drive {
            key: path.clone(),
            kind: DriveKind::Disk,
            drive_path: Some(path.clone()),
            block_path: block_path.clone(),
            device: b.map(|b| b.bytestring("Device")).unwrap_or_default(),
            vendor: d.str("Vendor"),
            model: d.str("Model"),
            revision: d.str("Revision"),
            serial: d.str("Serial"),
            size: disk_size,
            removable: d.bool("Removable"),
            media_removable: d.bool("MediaRemovable"),
            media_available: d.bool("MediaAvailable") || disk_size > 0,
            ejectable: d.bool("Ejectable"),
            can_power_off: d.bool("CanPowerOff"),
            optical: d.bool("Optical"),
            rotation_rate: d.i64("RotationRate"),
            connection_bus: d.str("ConnectionBus"),
            media: d.str("Media"),
            sort_key: d.str("SortKey"),
            read_only: b.is_some_and(|b| b.bool("ReadOnly")),
            smart: ifs.get(iface::ATA).map(smart_from).filter(|s| s.supported),
            table_type,
            backing_file: None,
            autoclear: false,
            volumes,
        });
    }
    disks.sort_by(|a, b| (a.removable, &a.sort_key, &a.device).cmp(&(b.removable, &b.sort_key, &b.device)));

    let mut loops = Vec::new();
    for (path, ifs) in snap {
        let (Some(l), Some(b)) = (ifs.get(iface::LOOP), ifs.get(iface::BLOCK)) else { continue };
        if ifs.contains_key(iface::PARTITION) || b.u64("Size") == 0 {
            continue;
        }
        let size = b.u64("Size");
        let (table_type, volumes) = disk_volumes(snap, path, size);
        let backing = l.bytestring("BackingFile");
        loops.push(Drive {
            key: path.clone(),
            kind: DriveKind::Loop,
            drive_path: None,
            block_path: Some(path.clone()),
            device: b.bytestring("Device"),
            vendor: String::new(),
            model: String::new(),
            revision: String::new(),
            serial: String::new(),
            size,
            removable: true,
            media_removable: false,
            media_available: true,
            ejectable: false,
            can_power_off: false,
            optical: false,
            rotation_rate: -1,
            connection_bus: String::new(),
            media: String::new(),
            sort_key: String::new(),
            read_only: b.bool("ReadOnly"),
            smart: None,
            table_type,
            backing_file: (!backing.is_empty()).then_some(backing),
            autoclear: l.bool("Autoclear"),
            volumes,
        });
    }
    loops.sort_by_key(|d| natural_key(&d.device));
    disks.extend(loops);
    Model { drives: disks }
}

/// Text dump of the model (for `--list`).
pub fn dump(model: &Model) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    fn vol(s: &mut String, v: &Volume, indent: usize) {
        let _ = writeln!(
            s,
            "{:indent$}- {} [{}] {} @{} {}{}",
            "",
            v.short_name(),
            if v.device.is_empty() { "-" } else { &v.device },
            format::size(v.size),
            v.offset,
            v.contents(),
            v.partition_type().map(|t| format!(" ({t})")).unwrap_or_default(),
            indent = indent
        );
        for c in &v.children {
            vol(s, c, indent + 2);
        }
    }
    for d in &model.drives {
        let _ = writeln!(s, "{} — {} ({})", d.title(), d.subtitle(), d.device);
        let _ = writeln!(s, "  Partitioning: {}", d.partitioning());
        if let Some(sm) = &d.smart {
            let _ = writeln!(s, "  Assessment: {}", sm.assessment());
        }
        for v in &d.volumes {
            vol(&mut s, v, 2);
        }
    }
    s
}

#[cfg(test)]
pub mod fixtures {
    use super::iface;
    use crate::props::{bytes, props, Interfaces, Prop, Props, Snapshot};

    pub const DRIVE: &str = "/org/freedesktop/UDisks2/drives/ACME_SSD_123";
    pub const SDA: &str = "/org/freedesktop/UDisks2/block_devices/sda";
    pub const SDA1: &str = "/org/freedesktop/UDisks2/block_devices/sda1";
    pub const SDA2: &str = "/org/freedesktop/UDisks2/block_devices/sda2";
    pub const DM0: &str = "/org/freedesktop/UDisks2/block_devices/dm_2d0";
    pub const USB: &str = "/org/freedesktop/UDisks2/drives/Stick";
    pub const SDB: &str = "/org/freedesktop/UDisks2/block_devices/sdb";
    pub const SDB1: &str = "/org/freedesktop/UDisks2/block_devices/sdb1";
    pub const SDB2: &str = "/org/freedesktop/UDisks2/block_devices/sdb2";
    pub const SDB5: &str = "/org/freedesktop/UDisks2/block_devices/sdb5";
    pub const LOOP0: &str = "/org/freedesktop/UDisks2/block_devices/loop0";
    pub const LOOP1: &str = "/org/freedesktop/UDisks2/block_devices/loop1";
    pub const LOOP10: &str = "/org/freedesktop/UDisks2/block_devices/loop10";

    const MIB: u64 = 1024 * 1024;
    const GB: u64 = 1_000_000_000;

    fn block(dev: &str, size: u64, drive: &str, usage: &str, ty: &str, label: &str) -> Props {
        props([
            ("Device", bytes(dev)),
            ("PreferredDevice", bytes(dev)),
            ("Size", Prop::U64(size)),
            ("Drive", Prop::Path(drive.into())),
            ("CryptoBackingDevice", Prop::Path("/".into())),
            ("IdUsage", Prop::Str(usage.into())),
            ("IdType", Prop::Str(ty.into())),
            ("IdVersion", Prop::Str(if ty == "ext4" { "1.0".into() } else { String::new() })),
            ("IdLabel", Prop::Str(label.into())),
            ("IdUUID", Prop::Str(format!("uuid-{dev}"))),
            ("ReadOnly", Prop::Bool(false)),
        ])
    }

    fn part(table: &str, number: u64, offset: u64, size: u64, ty: &str) -> Props {
        props([
            ("Table", Prop::Path(table.into())),
            ("Number", Prop::U64(number)),
            ("Offset", Prop::U64(offset)),
            ("Size", Prop::U64(size)),
            ("Type", Prop::Str(ty.into())),
            ("Name", Prop::Str(String::new())),
            ("Flags", Prop::U64(0)),
            ("IsContainer", Prop::Bool(ty == "0x05")),
            ("IsContained", Prop::Bool(number >= 5)),
        ])
    }

    fn obj<const N: usize>(ifs: [(&str, Props); N]) -> Interfaces {
        ifs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    /// A 256 GB GPT SSD with an EFI partition, a LUKS root (unlocked) and
    /// free space at the end; a 16 GB MBR USB stick with a FAT partition,
    /// an extended partition holding one logical and some free space; and
    /// three loop devices (one without backing file).
    pub fn snapshot() -> Snapshot {
        let disk = 256 * GB;
        let usb = 16 * GB;
        let mut s = Snapshot::new();
        s.insert(
            DRIVE.into(),
            obj([
                (
                    iface::DRIVE,
                    props([
                        ("Vendor", Prop::Str("ACME".into())),
                        ("Model", Prop::Str("Fast SSD".into())),
                        ("Serial", Prop::Str("SN123".into())),
                        ("Size", Prop::U64(disk)),
                        ("RotationRate", Prop::I64(0)),
                        ("Removable", Prop::Bool(false)),
                        ("MediaAvailable", Prop::Bool(true)),
                        ("SortKey", Prop::Str("00coldplug/00fixed/sd____a".into())),
                    ]),
                ),
                (
                    iface::ATA,
                    props([
                        ("SmartSupported", Prop::Bool(true)),
                        ("SmartEnabled", Prop::Bool(true)),
                        ("SmartUpdated", Prop::U64(1_700_000_000)),
                        ("SmartTemperature", Prop::F64(303.15)),
                    ]),
                ),
            ]),
        );
        s.insert(
            SDA.into(),
            obj([
                (iface::BLOCK, block("/dev/sda", disk, DRIVE, "", "", "")),
                (iface::TABLE, props([("Type", Prop::Str("gpt".into())), ("Partitions", Prop::Paths(vec![SDA2.into(), SDA1.into()]))])),
            ]),
        );
        s.insert(
            SDA1.into(),
            obj([
                (iface::BLOCK, block("/dev/sda1", 512 * MIB, DRIVE, "filesystem", "vfat", "EFI")),
                (iface::PARTITION, part(SDA, 1, MIB, 512 * MIB, "c12a7328-f81f-11d2-ba4b-00a0c93ec93b")),
                (iface::FILESYSTEM, props([("MountPoints", Prop::ByteStrings(vec![b"/boot/efi\0".to_vec()]))])),
            ]),
        );
        s.insert(
            SDA2.into(),
            obj([
                (iface::BLOCK, block("/dev/sda2", 200 * GB, DRIVE, "crypto", "crypto_LUKS", "")),
                (iface::PARTITION, part(SDA, 2, 513 * MIB, 200 * GB, "ca7d7ccb-63ed-4c53-861c-1742536059cc")),
                (iface::ENCRYPTED, props([("CleartextDevice", Prop::Path(DM0.into()))])),
            ]),
        );
        let mut clear = block("/dev/dm-0", 200 * GB - 16 * MIB, "/", "filesystem", "ext4", "root");
        clear.insert("CryptoBackingDevice".into(), Prop::Path(SDA2.into()));
        s.insert(
            DM0.into(),
            obj([
                (iface::BLOCK, clear),
                (iface::FILESYSTEM, props([("MountPoints", Prop::ByteStrings(vec![b"/\0".to_vec()]))])),
            ]),
        );

        s.insert(
            USB.into(),
            obj([(
                iface::DRIVE,
                props([
                    ("Vendor", Prop::Str("Generic".into())),
                    ("Model", Prop::Str("Flash Disk".into())),
                    ("Size", Prop::U64(usb)),
                    ("Removable", Prop::Bool(true)),
                    ("Ejectable", Prop::Bool(true)),
                    ("CanPowerOff", Prop::Bool(true)),
                    ("ConnectionBus", Prop::Str("usb".into())),
                    ("RotationRate", Prop::I64(-1)),
                    ("MediaAvailable", Prop::Bool(true)),
                    ("SortKey", Prop::Str("01hotplug/1".into())),
                ]),
            )]),
        );
        s.insert(
            SDB.into(),
            obj([
                (iface::BLOCK, block("/dev/sdb", usb, USB, "", "", "")),
                (iface::TABLE, props([("Type", Prop::Str("dos".into()))])),
            ]),
        );
        s.insert(
            SDB1.into(),
            obj([
                (iface::BLOCK, block("/dev/sdb1", 4 * GB, USB, "filesystem", "vfat", "STICK")),
                (iface::PARTITION, part(SDB, 1, MIB, 4 * GB, "0x0c")),
                (iface::FILESYSTEM, props([("MountPoints", Prop::ByteStrings(vec![]))])),
            ]),
        );
        // Extended partition from 8 GB to 16 GB, one logical of 2 GB.
        s.insert(
            SDB2.into(),
            obj([
                (iface::BLOCK, block("/dev/sdb2", 1024, USB, "", "", "")),
                (iface::PARTITION, part(SDB, 2, 8 * GB, 8 * GB, "0x05")),
            ]),
        );
        s.insert(
            SDB5.into(),
            obj([
                (iface::BLOCK, block("/dev/sdb5", 2 * GB, USB, "other", "swap", "")),
                (iface::PARTITION, part(SDB, 5, 8 * GB + MIB, 2 * GB, "0x82")),
                (iface::SWAP, props([("Active", Prop::Bool(false))])),
            ]),
        );

        let lp = |dev: &str, size: u64, file: &str| {
            obj([
                (iface::BLOCK, block(dev, size, "/", "filesystem", "squashfs", "")),
                (iface::LOOP, props([("BackingFile", bytes(file)), ("Autoclear", Prop::Bool(true))])),
                (iface::FILESYSTEM, props([("MountPoints", Prop::ByteStrings(vec![]))])),
            ])
        };
        s.insert(LOOP10.into(), lp("/dev/loop10", 50 * MIB, "/var/lib/snapd/snaps/b.snap"));
        s.insert(LOOP1.into(), lp("/dev/loop1", 100 * MIB, "/home/u/image.iso"));
        s.insert(LOOP0.into(), lp("/dev/loop0", 0, ""));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn drives_listed_in_order() {
        let m = build(&snapshot());
        let keys: Vec<&str> = m.drives.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, vec![DRIVE, USB, LOOP1, LOOP10]);
    }

    #[test]
    fn ssd_drive_properties() {
        let m = build(&snapshot());
        let d = m.find_drive(DRIVE).unwrap();
        assert_eq!(d.title(), "256 GB Solid-State Disk");
        assert_eq!(d.subtitle(), "ACME Fast SSD");
        assert_eq!(d.device, "/dev/sda");
        assert_eq!(d.block_path.as_deref(), Some(SDA));
        assert_eq!(d.icon_name(), "drive-harddisk-solidstate-symbolic");
        assert_eq!(d.partitioning(), "GUID Partition Table");
        assert_eq!(d.serial, "SN123");
        assert_eq!(d.smart.as_ref().unwrap().assessment(), "Disk is OK (30° C / 86° F)");
    }

    #[test]
    fn gpt_volumes_with_free_space_and_encryption() {
        let m = build(&snapshot());
        let d = m.find_drive(DRIVE).unwrap();
        assert_eq!(d.volumes.len(), 3);
        let efi = &d.volumes[0];
        assert_eq!(efi.device, "/dev/sda1");
        assert_eq!(efi.short_name(), "EFI");
        assert_eq!(efi.contents(), "FAT — Mounted at /boot/efi");
        assert_eq!(efi.partition_type().as_deref(), Some("EFI System"));
        assert!(efi.is_mounted());

        let luks = &d.volumes[1];
        assert_eq!(luks.contents(), "LUKS Encryption — Unlocked");
        assert!(luks.is_unlocked());
        assert_eq!(luks.children.len(), 1);
        let root = &luks.children[0];
        assert_eq!(root.device, "/dev/dm-0");
        assert_eq!(root.contents(), "Ext4 (version 1.0) — Mounted at /");
        assert_eq!(root.offset, luks.offset);

        let free = &d.volumes[2];
        assert!(free.is_free());
        assert_eq!(free.contents(), "Unallocated Space");
        assert_eq!(free.offset, 513 * 1024 * 1024 + 200 * GB);
        assert_eq!(free.offset + free.size, 256 * GB - 33 * 512);
        assert_eq!(free.kind, VolumeKind::Free { table: SDA.into(), logical: false });
        assert_eq!(d.find_volume(&free.key()).unwrap(), free);
    }

    #[test]
    fn mbr_with_extended_partition() {
        let m = build(&snapshot());
        let d = m.find_drive(USB).unwrap();
        assert_eq!(d.title(), "16 GB Drive");
        assert_eq!(d.icon_name(), "drive-harddisk-usb-symbolic");
        assert_eq!(d.partitioning(), "Master Boot Record");
        // sdb1, free gap (4 GB + 1 MiB .. 8 GB), extended.
        let kinds: Vec<String> = d.volumes.iter().map(|v| v.short_name()).collect();
        assert_eq!(kinds, vec!["STICK", "Free Space", "Extended Partition"]);
        assert_eq!(d.volumes[0].contents(), "FAT — Not Mounted");
        assert_eq!(d.volumes[0].partition_type().as_deref(), Some("W95 FAT32 (LBA)"));
        let ext = &d.volumes[2];
        assert_eq!(ext.contents(), "Extended Partition");
        assert_eq!(ext.size, 8 * GB);
        assert_eq!(ext.children.len(), 2);
        assert_eq!(ext.children[0].device, "/dev/sdb5");
        assert_eq!(ext.children[0].contents(), "Swap — Not Active");
        assert!(ext.children[1].is_free());
        assert_eq!(ext.children[1].kind, VolumeKind::Free { table: SDB.into(), logical: true });
        assert_eq!(ext.children[1].offset + ext.children[1].size, 16 * GB);
    }

    #[test]
    fn loop_devices() {
        let m = build(&snapshot());
        let l = m.find_drive(LOOP1).unwrap();
        assert_eq!(l.kind, DriveKind::Loop);
        assert_eq!(l.title(), "105 MB Loop Device");
        assert_eq!(l.subtitle(), "/home/u/image.iso");
        assert!(l.autoclear);
        assert_eq!(l.partitioning(), "None");
        assert_eq!(l.volumes.len(), 1);
        assert_eq!(l.volumes[0].contents(), "SquashFS — Not Mounted");
        assert!(m.find_drive(LOOP0).is_none());
    }

    #[test]
    fn find_device_paths() {
        let m = build(&snapshot());
        assert_eq!(m.find_device("/dev/sda"), Some((DRIVE.into(), None)));
        assert_eq!(m.find_device("/dev/dm-0"), Some((DRIVE.into(), Some(DM0.into()))));
        assert_eq!(m.find_device("/dev/sdb5"), Some((USB.into(), Some(SDB5.into()))));
        assert_eq!(m.find_device("/dev/nope"), None);
    }

    #[test]
    fn drive_without_media() {
        let mut s = snapshot();
        let b = s.get_mut(SDB).unwrap().get_mut(iface::BLOCK).unwrap();
        b.insert("Size".into(), crate::props::Prop::U64(0));
        s.get_mut(USB).unwrap().get_mut(iface::DRIVE).unwrap().insert("Size".into(), crate::props::Prop::U64(0));
        s.get_mut(SDB).unwrap().remove(iface::TABLE);
        let m = build(&s);
        let d = m.find_drive(USB).unwrap();
        assert!(d.volumes.is_empty());
        assert_eq!(d.partitioning(), "No Media");
        assert_eq!(d.title(), "Drive");
    }

    #[test]
    fn locked_and_unknown_contents() {
        let mut s = snapshot();
        s.remove(DM0);
        s.get_mut(SDA2).unwrap().get_mut(iface::ENCRYPTED).unwrap().insert("CleartextDevice".into(), crate::props::Prop::Path("/".into()));
        let m = build(&s);
        let v = &m.find_drive(DRIVE).unwrap().volumes[1];
        assert_eq!(v.contents(), "LUKS Encryption — Locked");
        assert!(v.children.is_empty());
        assert!(!v.is_unlocked());
        let sdb2 = &m.find_drive(USB).unwrap().volumes[2];
        assert_eq!(sdb2.short_name(), "Extended Partition");
    }

    #[test]
    fn dump_mentions_everything() {
        let out = dump(&build(&snapshot()));
        assert!(out.contains("256 GB Solid-State Disk"));
        assert!(out.contains("/dev/sdb5"));
        assert!(out.contains("Free Space"));
    }

    #[test]
    fn natural_ordering() {
        assert!(natural_key("loop2") < natural_key("loop10"));
        assert_eq!(fs_name("vfat", "FAT32"), "FAT (32-bit version)");
    }
}
