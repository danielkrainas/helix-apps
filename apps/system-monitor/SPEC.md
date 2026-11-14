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
  (clamped to 1000–100000 ms, the schema's `<range>`). File systems refresh every `disks-interval` ms.
- Changing the interval in Preferences takes effect immediately.
- The window's state (views, channel receiver, actions) lives as long as the
  window: it is held in a per-thread registry from creation until the window
  is destroyed, and handlers only hold weak references to it. Sampling
  therefore runs for the whole time the window is open and stops when it is
  closed. Debug builds assert if the state is dropped while the window is
  still shown.

### Processes page
- `GtkColumnView` with sortable columns: **Process Name**, **User**,
  **% CPU**, **Memory**, **ID**, **Priority**, **State**, **Command Line**.
  Clicking a header sorts (toggle ascending/descending). Default sort:
  Process Name ascending (schema default).
- Column state persists in the child schema
  `org.gnome.gnome-system-monitor.proctree` using GNOME's column numbers
  (`src/columns.rs`, unit-tested): Name 0, User 1, State 2 ("Status"),
  % CPU 8, ID 12, Command Line 14, Memory 15, Priority 26.
  - Visibility: right-click any column header for a check list of columns
    (the last visible column cannot be hidden) → `col-N-visible`. Defaults
    from the schema: State and Command Line hidden.
  - Width: a resized column's width is written to `col-N-width` 0.5 s after
    resizing stops; stored widths are applied as the column's fixed width
    (values ≤ 0 ignored, minimum 30 px).
  - Sort: `sort-col` (column number) and `sort-order` (0 ascending,
    1 descending). A stored column Helix does not show falls back to Name
    ascending without overwriting the key.
  - Order: headers can be dragged; `columns-order` is read as column numbers
    in display order (unknown numbers ignored, missing columns appended).
    When saving, slots of Helix columns are refilled in the new order and
    numbers of GNOME-only columns keep their place.
  - Keys are written only when the value actually changes.
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
    `setpriority(2)`. The presets form a radio group labelled by a visible
    "Priority" heading; the slider is named "Nice Value".
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
- For assistive technology each graph is an image named after its section
  ("CPU History") and described by the current values, updated each
  sample ("Average 23.4 %; CPU1 12.5 %, …", "Memory: … , Swap: …",
  "Receiving 1.2 MB/s, Sending 30 kB/s"). Each legend entry reads as one
  text, "CPU3: 12.5 %"; the colour swatches are presentational.

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
Ranges follow the real schema: `update-interval` and `disks-interval`
1000–100000, `graph-data-points` 30–600; values are clamped before use and
before writing.
Keys written: `update-interval`, `disks-interval`, `solaris-mode`,
`show-all-fs`, `show-whose-processes`, `current-tab`, `kill-dialog`,
`process-memory-in-iec`, `resources-memory-in-iec`, `network-in-bits`,
window state. Missing schema → in-memory defaults.

Child schema `org.gnome.gnome-system-monitor.proctree`
(path `/org/gnome/gnome-system-monitor/proctree/`), read and written:
`sort-col` (i), `sort-order` (i), `columns-order` (ai), and
`col-N-width` (i) / `col-N-visible` (b) for N in 0, 1, 2, 8, 12, 14, 15, 26.
`data/org.gnome.gnome-system-monitor.gschema.xml` contains exactly these
keys with the published names, types, ranges and defaults.

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
- Process-list column visibility, widths, order and sort state use the real
  `org.gnome.gnome-system-monitor.proctree` keys (GNOME column numbers),
  so settings carry over in both directions.
