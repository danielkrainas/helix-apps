# Disks

App ID: `org.helix.Disks`
Binary: `helix-disks`
Crate: `helix-disks` (`apps/disks`)

Drop-in replacement for gnome-disk-utility.

## Purpose
Manage disks, partitions, and images.

## Main windows
- Disk list and details.
- Partition editor, SMART, disk image tools.

## Contracts used
- UDisks2 D-Bus.
- Polkit for privileged actions.
- XDG portals: FileChooser.
- GSettings: `org.gnome.Disks`.

## Non-goals
- Every advanced benchmark or graph on day one.

---

## Behaviour (derived spec)

### Data source
- All information comes from UDisks2 on the **system bus**
  (`org.freedesktop.UDisks2`, root object `/org/freedesktop/UDisks2`).
- At start-up the app calls `org.freedesktop.DBus.ObjectManager.GetManagedObjects`
  once and builds an in-memory model. It subscribes to `InterfacesAdded`,
  `InterfacesRemoved` (ObjectManager) and `PropertiesChanged`
  (`org.freedesktop.DBus.Properties`) from the UDisks2 service; any such
  signal schedules a re-fetch, debounced to ~250 ms, and the UI is rebuilt
  from the new model while keeping the current drive/volume selection.
- Interfaces used: `Manager` (`LoopSetup`), `Drive`, `Drive.Ata`, `Block`,
  `Partition`, `PartitionTable`, `Filesystem`, `Encrypted`, `Loop`,
  `Swapspace`.
- The model layer is pure: it is built from a snapshot of
  `object path → interface → property → value` maps and is unit-tested
  from fixtures without D-Bus.
- If the system bus or UDisks2 is unavailable the window shows a status page
  ("Unable to connect to UDisks2") instead of the drive list.

### Drives listed
- Every `Drive` object that has a whole-disk `Block` (a block whose `Drive`
  property points at it, which is not a partition and not a cleartext
  device). Drives without media (empty card readers, empty optical drives)
  are listed with "No Media".
- Every loop device (`Block` + `Loop`) with a non-zero size (i.e. with a
  backing file).
- Order: fixed drives, then removable drives (by `SortKey`), then loop
  devices by device name.

### Window
- `AdwApplicationWindow` with an `AdwNavigationSplitView` (collapses to a
  single pane on narrow windows).
- **Sidebar**: header bar with an *Attach Disk Image…* button and the
  primary menu (*Attach Disk Image…*, *About Disks*).
  The list shows one row per drive: icon, title (`<size> <kind>`, e.g.
  "2.0 TB Hard Disk", "60 GB Solid-State Disk", "8.0 GB Drive",
  "CD/DVD Drive", "1.2 GB Loop Device"), subtitle (vendor + model, or the
  backing file for loop devices).
- **Drive page** (content pane):
  - Header bar title is the drive title, subtitle the device file
    (`/dev/sda`). Header buttons: *Eject* (only for ejectable media),
    *Power Off* (only when `CanPowerOff`), *Detach* for loop devices, and a
    drive menu: *Format Disk…*, *Create Disk Image…*, *Restore Disk Image…*,
    *SMART Data & Self-Tests…* (only for ATA drives with SMART support),
    *Power Off*, *Detach Loop Device*.
  - Drive information group: Model (vendor + model + revision), Serial
    Number, Size (`2.0 TB (2,000,398,934,016 bytes)`), Partitioning
    (`Master Boot Record`, `GUID Partition Table`, `Unknown`, or `None`),
    Assessment (SMART summary, ATA drives only), Backing File (loop only),
    Auto-clear (loop only).
  - **Volumes** group: a volume bar drawn with cairo. The top row shows
    partitions and free space in disk order, with widths proportional to
    size (each at least a minimum width so tiny partitions remain
    clickable). Contained volumes are drawn in a second row under their
    parent: logical partitions under an extended partition, and the
    cleartext filesystem under an unlocked encrypted volume. Each segment
    shows its label (or type) and size. Free space is drawn hatched/dim.
    Mounted filesystems carry a small "mounted" marker. Clicking selects a
    segment (focus ring + highlight); Left/Right arrow keys move the
    selection when the bar has focus.
  - **Volume toolbar** under the bar with context-sensitive buttons:
    Mount / Unmount (filesystems), Activate / Deactivate (swap),
    Unlock / Lock (encrypted), Create Partition (`+`, free space only),
    Delete Partition (`−`), and a volume menu: *Format Partition…*,
    *Edit Partition…*, *Edit Filesystem…*, *Check Filesystem…*,
    *Repair Filesystem…*, *Create Partition Image…*,
    *Restore Partition Image…*.
  - Volume details group: Size (long form), Device, UUID, Partition Type (name of GPT GUID / MBR
    code, with "Bootable" for active MBR partitions), Contents (e.g.
    "Ext4 (version 1.0) — Mounted at /home", "LUKS Encryption (version 2) —
    Unlocked", "Swap (version 1) — Active", "Extended Partition",
    "Unallocated Space", "Unknown"). When mounted, the mount point is a link
    that opens the folder with the default file manager
    (`gtk::UriLauncher`).
- When no drive is selected the content pane shows "No Drive Selected".

### Operations
All calls pass `auth.no_user_interaction = false` so udisks performs polkit
authorization (the polkit agent of the session prompts the user). Errors
from D-Bus are shown in an `AdwAlertDialog` with the UDisks error message;
a polkit "not authorized"/dismissed error is shown the same way.
Long-running calls run on a worker thread; while running the affected
buttons are insensitive and a spinner shows in the header.

**Every destructive operation shows a confirmation dialog that names the
device (`/dev/sdb1`, label, size) and warns that all data on it will be
lost.** The confirm button uses the destructive appearance; the default
response is *Cancel*.

| Action | UDisks call | Confirmation |
|---|---|---|
| Mount / Unmount | `Filesystem.Mount` / `Filesystem.Unmount` | no |
| Activate / deactivate swap | `Swapspace.Start` / `Stop` | no |
| Unlock | `Encrypted.Unlock(passphrase)` – passphrase dialog | no |
| Lock | `Encrypted.Lock` (unmounts cleartext filesystem first) | no |
| Format partition / volume | `Block.Format(type, {label, erase, encrypt.passphrase, encrypt.type="luks2", update-partition-type})` – dialog with name, erase switch, type (Ext4, Btrfs, XFS, FAT, exFAT, NTFS, LUKS+Ext4, "Other": empty/swap) | **yes** |
| Create partition | `PartitionTable.CreatePartitionAndFormat(offset, size, type, name, {}, fstype, {label, …})` – dialog with size (MB, default all free space), name, filesystem type | no (free space only) |
| Delete partition | `Partition.Delete` | **yes** |
| Edit partition | `Partition.SetType`, `Partition.SetName` (GPT only), `Partition.SetFlags` (MBR bootable) | no |
| Edit filesystem label | `Filesystem.SetLabel` | no |
| Check filesystem | `Filesystem.Check` (must be unmounted; result shown) | no |
| Repair filesystem | `Filesystem.Repair` | **yes** |
| Create disk/partition image | `Block.OpenForBackup` → copy the fd to the chosen file with a progress dialog (bytes copied, rate, cancel). File chosen with `gtk::FileDialog` (FileChooser portal) defaulting to `<name>.img` in `image-dir-uri` | no (refuses to overwrite an existing file without a confirm) |
| Restore disk/partition image | file chosen; image size must not exceed the target; `Block.OpenForRestore` → copy file to the fd with progress | **yes** |
| Format whole disk | `Block.Format("gpt" \| "dos" \| "empty", {erase})` on the whole-disk block | **yes** |
| Power off | unmount/lock everything on the drive, then `Drive.PowerOff` | no |
| Eject | unmount everything on the drive, then `Drive.Eject` | no |
| Detach loop device | unmount everything, then `Loop.Delete` | no |
| Attach disk image | file chosen; `Manager.LoopSetup(fd, {read-only})`, read-only by default (switch in the dialog) | no |
| SMART data | `Drive.Ata.SmartGetAttributes` | no |

### SMART dialog
- Summary rows: Last updated (relative time), Self-test result
  (`SmartSelftestStatus`), Powered on (`SmartPowerOnSeconds`, e.g. "4 years,
  11 months"), Temperature (°C / °F from kelvin), Overall assessment.
- Attribute table: ID, Attribute (pretty name derived from the libatasmart
  attribute name), Value (pretty value with unit: msec → duration, sectors,
  millikelvin → temperature, none → count), Normalized, Threshold, Worst,
  Type (Pre-Fail / Old-Age from flags bit 0), Updates (Online/Offline,
  flags bit 1), Assessment (Failing / Failed in the past / OK / N/A).
- Assessment string: `SmartFailing` → "DISK IS LIKELY TO FAIL SOON";
  otherwise bad sectors → "Disk is OK, N bad sectors"; attributes failing
  → "Disk is OK, N attributes are exceeding thresholds"; failed in past →
  "Disk is OK, N attributes failed in the past"; else "Disk is OK";
  followed by the temperature in parentheses. SMART not enabled →
  "SMART is not enabled".

### Size formatting
- Decimal (SI) units, as in GNOME: `B, kB, MB, GB, TB, PB`. Three
  significant digits (`999 MB`, `1.0 GB`, `60 GB`, `2.0 TB`, `512 bytes`).
- Long form adds the exact byte count with thousands separators:
  `2.0 TB (2,000,398,934,016 bytes)`.

### Free space / volume bar layout
- Free space is computed inside each partition table's usable area: first
  1 MiB reserved for alignment; for GPT the last 33 sectors (backup GPT)
  are reserved. Gaps of at least 0.1% of the disk (clamped to 1–16 MiB)
  between/around partitions are shown as free space, so alignment slack is
  not shown as slivers. Inside an extended partition, gaps larger than
  1 MiB (the EBR padding) are shown as free space in the second row.
- Segment widths: proportional to size, with a minimum width (40 px);
  space taken by minimum-width segments is removed proportionally from the
  others so the row always fills the bar exactly.

### GSettings (`org.gnome.Disks`)
- `image-dir-uri` (s): folder last used for disk images; read as the
  initial folder of image file dialogs and written after a successful
  choice. The `org.gnome.Disks.benchmark` keys are not used (no benchmark
  yet).

### Shortcuts
- `Ctrl+I` Attach Disk Image…
- `Ctrl+Shift+S` SMART Data (selected drive)
- `Ctrl+Q` quit, `Ctrl+W` close window, `F10` primary menu.
- Volume bar: click selects; Left/Right (or Up/Down) move the selection.

### Command line
- `helix-disks [--block-device DEVICE]` – starts with the drive containing
  `DEVICE` (e.g. `/dev/sdb1`) selected and that volume selected.
- `helix-disks --list` – prints the drive/volume model read from UDisks2
  to stdout and exits (read-only; no window).

## Deferred
- Benchmarks, drive settings (APM/AAM/standby/write cache), SMART self-tests
  start/abort, resize, MD-RAID, NVMe-specific data, LVM, `/etc/fstab` and
  `/etc/crypttab` editing (`Block.Configuration`), change passphrase,
  take ownership, creating extended partitions on MBR disks.
- Free space inside filesystems (UDisks2 does not expose usage; would need
  `statvfs` on the mount point).

## Open questions
- None blocking. Loop devices from snap packages are listed like GNOME
  does; if that is too noisy, a "hide system loop devices" toggle could be
  added.
