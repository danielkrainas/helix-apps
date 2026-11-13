# Disk Usage Analyzer

Drop-in replacement for Baobab.

- **App ID**: `org.helix.DiskUsage`
- **Binary**: `helix-disk-usage`
- **Crate**: `apps/disk-usage`

## Purpose
Visualize disk usage.

## Main windows
- Treemap or ring chart of a location.
- Folder list.

## Contracts used
- XDG portals: FileChooser, OpenURI.
- GSettings: `org.gnome.baobab`.

## Non-goals
- Exact visual style of current Baobab.
- Remote (non-local) locations; only paths on the local file system are scanned.

---

## Behaviour (derived spec)

### Window
One `AdwApplicationWindow` with an `AdwNavigationView` holding two pages:

1. **Devices & Locations** (start page)
   - A boxed list with a **Home folder** row and one row per mounted local
     file system. Each row shows an icon, the name (mount point basename,
     "Computer" for `/`), the mount point / device, a capacity
     **level bar** and "X used of Y (Z free)" text.
   - Mounts come from `/proc/self/mounts` (octal escapes such as `\040`
     decoded) and `statvfs`. Pseudo / virtual file systems (`proc`, `sysfs`,
     `tmpfs`, `devtmpfs`, `cgroup*`, `overlay`, `squashfs`, `fuse.portal`,
     `autofs`, …) and file systems with zero size are skipped; a mount point
     is listed once. The list is gathered on a worker thread (a hung network
     mount must not freeze the UI).
   - Activating a row scans that location.
   - Header bar: **Scan Folder…** button (`Ctrl+O`, `gtk::FileDialog`
     select-folder, which goes through the FileChooser portal when sandboxed),
     a refresh button (re-reads mounts) and the primary menu (*Preferences*,
     *About*).

2. **Results** page (title = scanned folder name, subtitle = path)
   - While scanning: a progress view with a pulsing progress bar, the number
     of items and bytes counted so far, the current folder, and a **Cancel**
     button (`Escape`). Cancelling returns to the start page.
   - When done: a horizontal `GtkPaned` with
     - left: **folder tree** — `GtkColumnView` over a `GtkTreeListModel`.
       Columns: *Name* (with expander, folder/file icon), *Size* (allocated
       size, plus a small bar showing the share of the parent),
       *Contents* (number of items below a folder; empty for files),
       *Modified* (local date). Columns are sortable (default: size,
       descending). Folders are expandable lazily.
     - right: **chart** — a breadcrumb bar (one button per path component
       from the scanned root to the chart's current root, plus an Up
       button), the chart drawing area, and a switch between **Rings** and
       **Treemap**.
   - Header bar: **Rescan** (`Ctrl+R`), and the chart-type switch is
     remembered in `active-chart`.
   - A status line under the tree shows totals: allocated size, apparent
     size, item count, and the number of folders that could not be read.

### Chart
- **Ring chart**: the current root is the centre disc; each ring outward is
  one level deeper (up to 5 levels). A child's angular span is proportional
  to its allocated size within its parent's span. Segments narrower than
  ~1° are not drawn individually.
- **Treemap**: squarified treemap of the current root; folders are drawn
  with a small header strip and their children nested inside (up to 4
  levels, tiles smaller than ~4 px skipped).
- Colours: each top-level child gets a hue; deeper levels are lighter. Files
  are drawn greyer than folders.
- Hover highlights the item under the pointer and shows a tooltip with name,
  size (allocated and apparent) and contents count.
- Left-click a folder: zoom the chart into it. Click the centre disc (rings)
  or Up / breadcrumb buttons to go up. Right-click: item context menu.
- Selecting a folder in the tree makes it the chart root; selecting a file
  makes its parent the chart root.

### Item actions
Available from the right-click menu on tree rows and chart items, acting on
the selected / clicked item:
- **Open Folder** — folders are opened in the file manager; for files the
  containing folder is opened (`gtk::FileLauncher`, OpenURI portal).
- **Copy Path** — copies the absolute path to the clipboard.
- **Move to Trash…** — asks for confirmation (`AdwAlertDialog`), then
  `g_file_trash`. On success the item is removed from the results and its
  size subtracted from every ancestor. Errors are shown in a toast.

### Scanning
- Runs on a worker thread; the GTK thread polls shared atomic counters for
  progress (every 100 ms) and picks up the finished tree.
- For every entry `lstat` is used: **symlinks are never followed** (a link is
  a leaf counted with its own size).
- **Allocated size** = `st_blocks × 512`; **apparent size** = `st_size`.
  A folder's sizes include its own entry plus everything below it.
- **Hard links**: a non-directory inode with `st_nlink > 1` is counted once
  (keyed by `(st_dev, st_ino)`); later links appear with size 0.
- **File-system boundaries** are not crossed by default: a sub-folder whose
  `st_dev` differs from the scanned root's is skipped (Preferences toggle).
- **Excluded locations** (`excluded-uris`) are skipped entirely, together
  with everything below them. Excluding the scan root itself is ignored.
- Unreadable folders are listed with their own size and flagged; the scan
  continues.
- Cancel is checked between entries; a cancelled scan is discarded.

### Preferences (`AdwPreferencesDialog`)
- **Excluded locations**: list with remove buttons and an *Add Location…*
  button (folder chooser). Stored in `excluded-uris`.
- **Cross file system boundaries**: off by default (stored locally, see
  below).

### GSettings (`org.gnome.baobab`)
The real schema layout (as installed by GNOME) is used: the
`org.gnome.baobab` schema only has the children `preferences` and `ui`.
Keys read and written (exact upstream names, types and defaults):
- `org.gnome.baobab.preferences excluded-uris` (`as`, `[]`) — `file://`
  URIs of locations never scanned.
- `org.gnome.baobab.ui active-chart` (`s`, choices `rings` | `treemap`,
  default `'rings'`).
- `org.gnome.baobab.ui window-size` (`(ii)`, default `(960, 600)`) —
  initial window size (minimum 360×300); written on close when not
  maximized.
- `org.gnome.baobab.ui is-maximized` (`b`, default `false`) — written on
  close.

`data/org.gnome.baobab.gschema.xml` mirrors those schemas (install only
where the GNOME schema is absent); a unit test checks it against the
installed schema when present. If a schema is not installed, defaults are
used in memory (via `helix_app_common::Prefs`).

### App-private settings
Only the "Cross file system boundaries" toggle has no upstream key; it is
stored as `cross_filesystems` in
`$XDG_CONFIG_HOME/helix-disk-usage/state.json` (older window-state fields in
that file are ignored).

### Shortcuts
| Key | Action |
|---|---|
| `Ctrl+O` | Scan folder… |
| `Ctrl+R` / `F5` | Rescan (results page) / reload mounts (start page) |
| `Escape` | Cancel scan |
| `Alt+Left` / `Alt+Up` | Back to start page / chart up one level |
| `Ctrl+,` | Preferences |
| `Ctrl+Q`, `Ctrl+W` | Quit, close window |

### Command line
`helix-disk-usage [DIRECTORY]` — with a directory (path or `file://` URI),
the scan starts immediately.

## Decisions
- Use the real `org.gnome.baobab` keys (installed schema:
  `org.gnome.baobab.preferences` / `org.gnome.baobab.ui`, including
  `window-size` and `is-maximized`) wherever they exist; app-private JSON
  only for the rest.

## Open questions
- None.
