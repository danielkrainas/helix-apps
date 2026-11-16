//! Well-known partition types (GPT GUIDs and MBR codes).

use helix_app_common::i18n::{n_, tr};
use helix_app_common::nc_;
use helix_app_common::trf;

pub struct PartType {
    /// `"gpt"` or `"dos"`.
    pub table: &'static str,
    /// GUID (lowercase) for GPT, `0xNN` for MBR.
    pub id: &'static str,
    pub name: &'static str,
}

const fn t(table: &'static str, id: &'static str, name: &'static str) -> PartType {
    PartType { table, id, name }
}

pub const LINUX_FS_GPT: &str = "0fc63daf-8483-4772-8e79-3d69d8477de4";
pub const BASIC_DATA_GPT: &str = "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7";
pub const LINUX_SWAP_GPT: &str = "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f";
pub const LINUX_LUKS_GPT: &str = "ca7d7ccb-63ed-4c53-861c-1742536059cc";

pub static TYPES: &[PartType] = &[
    t(
        "gpt",
        "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
        n_("EFI System"),
    ),
    t(
        "gpt",
        "21686148-6449-6e6f-744e-656564454649",
        n_("BIOS Boot"),
    ),
    t("gpt", LINUX_FS_GPT, n_("Linux Filesystem")),
    t(
        "gpt",
        "4f68bce3-e8cd-4db1-96e7-fbcaf984b709",
        n_("Linux Root (x86-64)"),
    ),
    t(
        "gpt",
        "b921b045-1df0-41c3-af44-4c6f280d3fae",
        n_("Linux Root (ARM-64)"),
    ),
    t(
        "gpt",
        "933ac7e1-2eb4-4f13-b844-0e14e2aef915",
        n_("Linux Home"),
    ),
    t(
        "gpt",
        "bc13c2ff-59e6-4262-a352-b275fd6f7172",
        n_("Linux Extended Boot"),
    ),
    t("gpt", LINUX_SWAP_GPT, n_("Linux Swap")),
    t(
        "gpt",
        "e6d6d379-f507-44c2-a23c-238f2a3df928",
        n_("Linux LVM"),
    ),
    t(
        "gpt",
        "a19d880f-05fc-4d3b-a006-743f0f84911e",
        n_("Linux RAID"),
    ),
    t("gpt", LINUX_LUKS_GPT, n_("Linux LUKS")),
    t(
        "gpt",
        "8da63339-0007-60c0-c436-083ac8230908",
        n_("Linux Reserved"),
    ),
    t("gpt", BASIC_DATA_GPT, n_("Basic Data")),
    t(
        "gpt",
        "e3c9e316-0b5c-4db8-817d-f92df00215ae",
        n_("Microsoft Reserved"),
    ),
    t(
        "gpt",
        "de94bba4-06d1-4d40-a16a-bfd50179d6ac",
        n_("Microsoft Windows Recovery Environment"),
    ),
    t(
        "gpt",
        "5808c8aa-7e8f-42e0-85d2-e1e90434cfb3",
        n_("Microsoft LDM Metadata"),
    ),
    t(
        "gpt",
        "af9b60a0-1431-4f62-bc68-3311714a69ad",
        n_("Microsoft LDM Data"),
    ),
    t(
        "gpt",
        "48465300-0000-11aa-aa11-00306543ecac",
        n_("Apple HFS/HFS+"),
    ),
    t(
        "gpt",
        "7c3457ef-0000-11aa-aa11-00306543ecac",
        n_("Apple APFS"),
    ),
    t(
        "gpt",
        "426f6f74-0000-11aa-aa11-00306543ecac",
        n_("Apple Boot"),
    ),
    t(
        "gpt",
        "516e7cb4-6ecf-11d6-8ff8-00022d09712b",
        n_("FreeBSD Data"),
    ),
    t(
        "gpt",
        "516e7cba-6ecf-11d6-8ff8-00022d09712b",
        n_("FreeBSD ZFS"),
    ),
    t(
        "gpt",
        "6a898cc3-1dd2-11b2-99a6-080020736631",
        n_("Solaris /usr or Apple ZFS"),
    ),
    t(
        "gpt",
        "024dee41-33e7-11d3-9d69-0008c781f39f",
        n_("MBR Partition Scheme"),
    ),
    t("dos", "0x83", "Linux"),
    t("dos", "0x82", n_("Linux swap or Solaris")),
    t("dos", "0x8e", n_("Linux LVM")),
    t("dos", "0xfd", n_("Linux RAID auto")),
    t("dos", "0x85", n_("Linux Extended")),
    t("dos", "0x05", n_("Extended")),
    t("dos", "0x0f", n_("W95 Ext d (LBA)")),
    t("dos", "0x07", "HPFS/NTFS/exFAT"),
    t("dos", "0x0b", n_("W95 FAT32")),
    t("dos", "0x0c", n_("W95 FAT32 (LBA)")),
    t("dos", "0x0e", n_("W95 FAT16 (LBA)")),
    t("dos", "0x06", "FAT16"),
    t("dos", "0x01", "FAT12"),
    t("dos", "0x27", n_("Hidden NTFS WinRE")),
    t("dos", "0xef", n_("EFI (FAT-12/16/32)")),
    t("dos", "0xa5", "FreeBSD"),
    t("dos", "0xa6", "OpenBSD"),
    t("dos", "0xaf", n_("HFS / HFS+")),
    t("dos", "0xee", n_("GPT Protective")),
];

/// Normalizes a type id for comparison: lowercase GUIDs, `0xNN` codes.
pub fn normalize(table: &str, id: &str) -> String {
    let id = id.trim().to_ascii_lowercase();
    if table == "dos" {
        let hex = id.trim_start_matches("0x");
        if let Ok(v) = u8::from_str_radix(hex, 16) {
            return format!("0x{v:02x}");
        }
    }
    id
}

/// Human-readable name for a partition type, falling back to the raw id.
pub fn name(table: &str, id: &str) -> String {
    let n = normalize(table, id);
    TYPES
        .iter()
        .find(|t| t.table == table && t.id == n)
        .map(|t| t.name.to_string())
        .unwrap_or_else(|| {
            if id.is_empty() {
                tr("Unknown").into()
            } else {
                trf!("Unknown ({id})", id = id)
            }
        })
}

/// Types offered when editing a partition on a table of type `table`.
pub fn for_table(table: &str) -> Vec<&'static PartType> {
    TYPES.iter().filter(|t| t.table == table).collect()
}

/// Partition type to use when creating a partition for filesystem `fs`.
pub fn default_for_fs(table: &str, fs: &str) -> &'static str {
    match (table, fs) {
        ("gpt", "vfat" | "exfat" | "ntfs") => BASIC_DATA_GPT,
        ("gpt", "swap") => LINUX_SWAP_GPT,
        ("gpt", _) => LINUX_FS_GPT,
        ("dos", "vfat") => "0x0c",
        ("dos", "exfat" | "ntfs") => "0x07",
        ("dos", "swap") => "0x82",
        ("dos", _) => "0x83",
        _ => "",
    }
}

/// Description of a partition table type.
pub fn table_name(table: &str) -> &'static str {
    match table {
        "dos" => tr("Master Boot Record"),
        "gpt" => tr("GUID Partition Table"),
        "" => tr(nc_!("partition table", "None")),
        _ => tr("Unknown"),
    }
}

/// MBR "bootable" flag bit in `Partition.Flags`.
pub const DOS_FLAG_BOOTABLE: u64 = 0x80;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name("dos", "0x83"), "Linux");
        assert_eq!(name("dos", "0X7"), "HPFS/NTFS/exFAT");
        assert_eq!(
            name("gpt", "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"),
            "EFI System"
        );
        assert_eq!(name("gpt", "deadbeef"), "Unknown (deadbeef)");
        assert_eq!(name("gpt", ""), "Unknown");
    }

    #[test]
    fn defaults() {
        assert_eq!(default_for_fs("gpt", "ext4"), LINUX_FS_GPT);
        assert_eq!(default_for_fs("gpt", "exfat"), BASIC_DATA_GPT);
        assert_eq!(default_for_fs("dos", "vfat"), "0x0c");
        assert_eq!(table_name("gpt"), "GUID Partition Table");
        assert!(for_table("dos").iter().all(|t| t.id.starts_with("0x")));
    }
}
