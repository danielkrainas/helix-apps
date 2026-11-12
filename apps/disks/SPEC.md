# Disks

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
