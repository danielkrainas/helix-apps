//! UDisks operations as plain data, plus pure planning helpers.
//!
//! The UI builds `Op` values; `udisks::run` executes them. Nothing in this
//! module talks to D-Bus.

use crate::model::{Drive, Volume};
use crate::smart::Attribute;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Mount {
        block: String,
    },
    Unmount {
        block: String,
    },
    SwapStart {
        block: String,
    },
    SwapStop {
        block: String,
    },
    Unlock {
        block: String,
        passphrase: String,
    },
    Lock {
        block: String,
    },
    /// `Block.Format`. `fs_type` may be `empty`, `gpt`, `dos` or a filesystem.
    Format {
        block: String,
        fs_type: String,
        label: String,
        erase: bool,
        passphrase: Option<String>,
        update_partition_type: bool,
    },
    CreatePartition {
        table: String,
        offset: u64,
        size: u64,
        part_type: String,
        name: String,
        fs_type: String,
        label: String,
        passphrase: Option<String>,
    },
    DeletePartition {
        block: String,
    },
    SetPartitionType {
        block: String,
        part_type: String,
    },
    SetPartitionName {
        block: String,
        name: String,
    },
    SetPartitionFlags {
        block: String,
        flags: u64,
    },
    SetLabel {
        block: String,
        label: String,
    },
    Check {
        block: String,
    },
    Repair {
        block: String,
    },
    PowerOff {
        drive: String,
    },
    Eject {
        drive: String,
    },
    LoopDetach {
        block: String,
    },
    SmartAttributes {
        drive: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpOutput {
    None,
    Bool(bool),
    Path(String),
    Attributes(Vec<Attribute>),
}

impl Op {
    /// Present-participle description used in error headings.
    pub fn verb(&self) -> &'static str {
        match self {
            Op::Mount { .. } => "mounting filesystem",
            Op::Unmount { .. } => "unmounting filesystem",
            Op::SwapStart { .. } => "starting swap",
            Op::SwapStop { .. } => "stopping swap",
            Op::Unlock { .. } => "unlocking encrypted device",
            Op::Lock { .. } => "locking encrypted device",
            Op::Format { .. } => "formatting volume",
            Op::CreatePartition { .. } => "creating partition",
            Op::DeletePartition { .. } => "deleting partition",
            Op::SetPartitionType { .. } => "setting partition type",
            Op::SetPartitionName { .. } => "setting partition name",
            Op::SetPartitionFlags { .. } => "setting partition flags",
            Op::SetLabel { .. } => "setting filesystem label",
            Op::Check { .. } => "checking filesystem",
            Op::Repair { .. } => "repairing filesystem",
            Op::PowerOff { .. } => "powering off drive",
            Op::Eject { .. } => "ejecting media",
            Op::LoopDetach { .. } => "detaching loop device",
            Op::SmartAttributes { .. } => "reading SMART data",
        }
    }
}

fn teardown_rec(v: &Volume, out: &mut Vec<Op>) {
    for c in &v.children {
        teardown_rec(c, out);
    }
    let Some(block) = &v.block_path else { return };
    if v.is_mounted() {
        out.push(Op::Unmount {
            block: block.clone(),
        });
    }
    if v.swap_active == Some(true) {
        out.push(Op::SwapStop {
            block: block.clone(),
        });
    }
    if v.is_unlocked() {
        out.push(Op::Lock {
            block: block.clone(),
        });
    }
}

/// Operations that release a volume: unmount filesystems, stop swap and
/// lock encrypted devices, innermost first.
pub fn teardown(v: &Volume) -> Vec<Op> {
    let mut out = Vec::new();
    teardown_rec(v, &mut out);
    out
}

/// Teardown of every volume on a drive.
pub fn teardown_drive(d: &Drive) -> Vec<Op> {
    let mut out = Vec::new();
    for v in &d.volumes {
        teardown_rec(v, &mut out);
    }
    out
}

/// Plans powering off / ejecting / detaching a drive.
pub fn plan_remove(d: &Drive, last: Op) -> Vec<Op> {
    let mut ops = teardown_drive(d);
    ops.push(last);
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{build, fixtures::*};

    #[test]
    fn teardown_order_is_innermost_first() {
        let m = build(&snapshot());
        let d = m.find_drive(DRIVE).unwrap();
        let ops = teardown_drive(d);
        assert_eq!(
            ops,
            vec![
                Op::Unmount { block: SDA1.into() },
                Op::Unmount { block: DM0.into() },
                Op::Lock { block: SDA2.into() },
            ]
        );
        let ops = plan_remove(
            d,
            Op::PowerOff {
                drive: DRIVE.into(),
            },
        );
        assert_eq!(
            ops.last(),
            Some(&Op::PowerOff {
                drive: DRIVE.into()
            })
        );
    }

    #[test]
    fn teardown_of_idle_drive_is_empty() {
        let m = build(&snapshot());
        let d = m.find_drive(USB).unwrap();
        assert!(teardown_drive(d).is_empty());
        assert_eq!(Op::Eject { drive: USB.into() }.verb(), "ejecting media");
    }
}
