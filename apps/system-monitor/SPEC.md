# System Monitor

Drop-in replacement for GNOME System Monitor.

- **App ID**: `org.helix.SystemMonitor`
- **Binary**: `helix-system-monitor`
- **Crate**: `apps/system-monitor`

## Purpose
View processes, resources, and file systems.

## Main windows
- Processes, Resources, and File Systems tabs.
- Process details and kill.

## Contracts used
- /proc and standard system interfaces.
- Polkit for privileged process actions.
- GSettings: `org.gnome.gnome-system-monitor`.

## Non-goals
- Exact graph rendering of the original.

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow`. The header bar title is an `AdwViewSwitcher`
  with three pages: **Processes**, **Resources**, **File Systems**. The last
  shown page is restored from `current-tab` (`processes|resources|disks`).
- Header bar (end): primary menu with *Show* radio items (My Processes /
  All Processes / Active Processes, Processes page only), *Refresh*,
  *Preferences*, *Keyboard Shortcuts*, *About*. Header bar (start): a search
  toggle (Processes page only).
- Window size and maximized state persist (`window-width`, `window-height`,
  `maximized`).

### Data sampling
- A worker thread samples `/proc` and sends snapshots to the UI thread over a
  channel; the GTK main thread never reads `/proc` in bulk.
- Processes and resource graphs refresh every `update-interval` ms
  (clamped to 250–100000). File systems refresh every `disks-interval` ms.
- Changing the interval in Preferences takes effect immediately.

### Processes page
- `GtkColumnView` with sortable columns: **Process Name**, **User**,
  **% CPU**, **Memory**, **ID**, **Priority**, **State**, **Command Line**.
  Clicking a header sorts (toggle ascending/descending). Default sort: % CPU
  descending.
- Process data per row (from `/proc/<pid>/stat`, `status`, `cmdline`):
  - *Name*: `Name:` from status (the kernel comm, 15 chars) unless the first
    argv element's basename starts with it, in which case the longer basename
    is shown.
  - *User*: login name from `/etc/passwd` for the real UID (numeric fallback).
  - *% CPU*: CPU ticks used since the previous sample divided by the elapsed
    time. In **Solaris mode** (`solaris-mode`, default true) the value is
    divided by the number of CPUs so it never exceeds 100 %; otherwise a
    process can show up to N×100 %.
  - *Memory*: resident private (anonymous) memory, `RssAnon` from status,
    falling back to `VmRSS`. Kernel threads show "N/A".
  - *Priority*: nice value mapped to Very High (≤ −15), High (−14…−5),
    Normal (−4…4), Low (5…14), Very Low (≥ 15), with the nice value shown.
  - *State*: Running, Sleeping, Disk Sleep (uninterruptible), Stopped,
    Tracing Stop, Zombie, Dead, Idle.
- **Show** filter (`show-whose-processes`): `user` = processes with the
  current user's real UID; `all`; `active` = processes in state R or with
  non-zero CPU since the last sample.
- **Search** (Ctrl+F, or start typing): case-insensitive substring match on
  name, user, command line, or exact PID prefix.
- Selection survives refreshes (rows are stable objects keyed by PID).
- **Context menu** (right-click or Menu key) and keyboard shortcuts:
  - *Properties* (Alt+Return): dialog with name, user, state, memory
    (virtual, resident, shared, private), CPU time, start time, nice, PID,
    parent PID, threads, command line. Updates while open.
  - *Change Priority* (Ctrl+N): dialog with Very High / High / Normal / Low /
    Very Low presets and a custom nice slider (−20…19). Applied with
    `setpriority(2)`.
  - *Stop* (Ctrl+S, `SIGSTOP`), *Continue* (Ctrl+C, `SIGCONT`),
    *End* (Ctrl+E, `SIGTERM`), *Kill* (Ctrl+K, `SIGKILL`).
  - End and Kill ask for confirmation when `kill-dialog` is true (default).
- An action bar at the bottom shows *End Process* and *Properties* buttons
  for the selection.
- Signals and renice are applied directly to processes owned by the current
  user. When the direct attempt fails because the process belongs to another
  user, or the kernel refuses (e.g. lowering the nice value of an own
  process), the action is retried as
  `pkexec /usr/libexec/helix-system-monitor/helix-system-monitor-helper …`
  on a worker thread (the polkit authentication dialog does not block the
  UI). Errors (cancelled/denied authentication, helper not installed,
  process gone) are shown as a toast.

### Privileged helper and polkit
- Polkit action **`org.helix.SystemMonitor.manage-processes`**
  (`data/org.helix.SystemMonitor.policy`, installs to
  `/usr/share/polkit-1/actions/`). Modelled on the installed GNOME policy
  (`org.gnome.gnome-system-monitor.kill` / `.renice`, which use
  `auth_admin_keep` for active sessions and `no` otherwise) but a single
  action covers both kill and renice. Its
  `org.freedesktop.policykit.exec.path` annotation names the helper, so
  `pkexec` shows the action's message.
- Helper binary **`helix-system-monitor-helper`** (same crate,
  `src/bin/helper.rs`), installs to
  `/usr/libexec/helix-system-monitor/helix-system-monitor-helper`. For
  development the app honours `HELIX_SYSTEM_MONITOR_HELPER=<path>` (pkexec
  still needs a policy whose exec.path matches). Grammar (strict, parsed in
  `src/helper_args.rs`, unit-tested):
  - `helix-system-monitor-helper [--dry-run] signal <STOP|CONT|TERM|KILL> <PID>`
  - `helix-system-monitor-helper [--dry-run] renice <NICE> <PID>`
  - PID: decimal digits only, no sign/leading zeros, 2…4194304 (PID 1 and
    the helper's own PID are refused; ≤ 0 would address process groups).
    NICE: optional `-`, no leading zeros, −20…19. Any other argument,
    count or order is rejected.
- Exit codes: 0 success, 2 invalid arguments, 3 no such process,
  4 failure (including not running as root). pkexec's own 126/127
  (dismissed / not authorised) are mapped to messages by the app.
- `--dry-run` validates the arguments and that the process exists, prints
  `dry run: would …` and changes nothing; it does not need root. Tests
  (`tests/helper.rs`) cover only validation and the dry-run path; pkexec is
  never invoked by tests.

### Resources page
- **CPU History**: one line per logical CPU, from `/proc/stat` deltas, Y axis
  0–100 %. A legend lists each CPU with its colour and current usage.
- **Memory and Swap History**: memory used (`MemTotal − MemAvailable`) and
  swap used, as a percentage of total; legend shows "used of total (x %)".
  Colours from `mem-color` / `swap-color`.
- **Network History**: receive and send rates summed over all non-loopback
  interfaces of `/proc/net/dev`, auto-scaled Y axis; legend shows the current
  rate and the total received/sent. Colours from `net-in-color` /
  `net-out-color`; `network-in-bits` shows rates in bits/s.
- Graphs keep `graph-data-points` samples (default 60), newest on the right,
  drawn with cairo in `GtkDrawingArea`s, with horizontal grid lines and
  time labels.

### File Systems page
- `GtkColumnView` with columns **Device**, **Directory**, **Type**,
  **Total**, **Free**, **Available**, **Used** (level bar + size + percent).
  Sortable; default sort by directory.
- Source: `/proc/self/mounts` (octal escapes such as `\040` decoded) and
  `statvfs(3)`. Pseudo/virtual file systems (proc, sysfs, tmpfs, cgroup,
  squashfs, overlay, fuse helpers, …) and file systems with zero size are
  hidden unless `show-all-fs` is true. A device mounted several times is
  listed once (first mount point) unless `show-all-fs`.
- Double-click (activate) opens the mount directory with the default file
  manager (`gio::AppInfo::launch_default_for_uri`).

### Units
- Process memory uses SI units (kB, MB, GB) unless `process-memory-in-iec`
  (KiB, MiB, GiB); resources page memory follows `resources-memory-in-iec`.
  File systems always use SI.

### GSettings (`org.gnome.gnome-system-monitor`)
Keys read: `update-interval` (i), `disks-interval` (i),
`graph-data-points` (i), `solaris-mode` (b), `show-all-fs` (b),
`show-whose-processes` (s: `all|user|active`), `current-tab`
(s: `processes|resources|disks`), `kill-dialog` (b),
`process-memory-in-iec` (b), `resources-memory-in-iec` (b),
`network-in-bits` (b), `mem-color`, `swap-color`, `net-in-color`,
`net-out-color` (s), `window-width`, `window-height` (i), `maximized` (b).
Keys written: `update-interval`, `disks-interval`, `solaris-mode`,
`show-all-fs`, `show-whose-processes`, `current-tab`, `kill-dialog`,
`process-memory-in-iec`, `resources-memory-in-iec`, `network-in-bits`,
window state. Missing schema → in-memory defaults.

### Keyboard shortcuts
`Ctrl+1/2/3` switch page, `Ctrl+F` search, `Ctrl+R` refresh,
`Alt+Return` properties, `Ctrl+N` change priority, `Ctrl+S` stop,
`Ctrl+C` continue, `Ctrl+E` end, `Ctrl+K` kill, `Ctrl+,` preferences,
`Ctrl+Q` quit, `Ctrl+W` close.

### Command line
`helix-system-monitor [-p|--show-processes-tab | -r|--show-resources-tab |
-f|--show-file-systems-tab]` opens on the given page.

## Decisions
- Privileged process actions: ship polkit action
  `org.helix.SystemMonitor.manage-processes` plus a small helper binary
  invoked via `pkexec` (see "Privileged helper and polkit").

## Open questions
- Per-column visibility/width persistence (`org.gnome.gnome-system-monitor.proctree`
  `col-N-*`) is not implemented; the GNOME column numbering is an internal
  detail and not mapped yet.
