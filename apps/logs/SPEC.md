# Logs

Drop-in replacement for GNOME Logs.

- **App ID**: `org.helix.Logs`
- **Binary**: `helix-logs`
- **Crate**: `apps/logs`

## Purpose
View system and application logs.

## Main windows
- Category sidebar and log list.
- Search and filters.

## Contracts used
- systemd-journal or equivalent.
- GSettings: `org.gnome.Logs`.

## Non-goals
- Full journal export features on day one.

---

## Behaviour (derived spec)

### Journal access
- libsystemd is not linked. The journal is read by spawning
  `journalctl --no-pager -o json [--reverse] [--boot=ID] [--since=…]
  [-n N] [--priority=…] [FIELD=VALUE [+ FIELD=VALUE …]]` on a worker thread
  and parsing one JSON object per stdout line.
- The child process is streamed: the worker reads lines lazily, so the pipe
  applies back-pressure and only as many entries as the UI asked for are
  parsed. Changing the view (category, boot, search, sort) drops the query;
  the worker kills its `journalctl` child and exits.
- JSON field values may be: a string; an array of byte values (non-UTF-8 or
  control-character payloads) which is decoded as UTF-8 (lossy); `null`
  (field too large to be serialised, shown as "[data omitted]"); or an
  array of the above when a field occurs several times in one entry (each
  occurrence is kept).
- Derived values: **time** = `_SOURCE_REALTIME_TIMESTAMP`, else
  `__REALTIME_TIMESTAMP` (µs since epoch); **priority** = `PRIORITY` (0–7,
  default 6); **process** = `SYSLOG_IDENTIFIER`, else `_COMM`, else
  "kernel" for kernel transport; **unit** = `_SYSTEMD_USER_UNIT`, else
  `_SYSTEMD_UNIT`, else `UNIT`.

### Window
- `AdwApplicationWindow` with an `AdwNavigationSplitView`:
  - **Sidebar**: category list — *Important*, *All*, *Applications*,
    *System*, *Security*, *Hardware*. Default: *Important*.
  - **Content**: an `AdwNavigationView` with the event list page and,
    when an event is activated, a detail page.
- Event list header: boot selector drop-down (title area), search toggle
  (`Ctrl+F`), primary menu (*Export…*, *Sort: Newest First / Oldest First*,
  *Refresh*, *About*).
- A banner (`AdwBanner`) is shown when the user cannot read system logs
  (not root and not in `systemd-journal`, `adm` or `wheel`, or journalctl
  printed its "not seeing messages from other users and the system" hint).
  Its *Don't Show Again* button sets `ignore-warning` to true.
- Window size is remembered in the app's config dir (`window.json`).

### Categories
Classification works on an entry's fields; an entry may belong to several.
| Category | Rule | journalctl pre-filter |
|---|---|---|
| Important | `PRIORITY <= 3` (emerg, alert, crit, err) | `--priority=3` |
| All | every entry | none |
| Applications | not kernel/audit transport; `_UID` or `_SYSTEMD_OWNER_UID` equals the current user's uid; and has `_SYSTEMD_USER_UNIT` or `SYSLOG_IDENTIFIER` (for root only `_SYSTEMD_USER_UNIT` counts) | `_UID=uid + _SYSTEMD_OWNER_UID=uid` |
| System | not kernel transport and not Applications | none |
| Security | `_TRANSPORT=audit`, or `SYSLOG_FACILITY` 4 (auth) or 10 (authpriv) | `_TRANSPORT=audit + SYSLOG_FACILITY=4 + SYSLOG_FACILITY=10` |
| Hardware | `_TRANSPORT=kernel` | `_TRANSPORT=kernel` |
The pre-filter only narrows what journalctl emits; the rule is always
applied client-side too.

### Boots
- Boot list from `journalctl --list-boots -o json` (array of
  `{index, boot_id, first_entry, last_entry}`); if that fails, the plain-text
  table (`IDX BOOT_ID FIRST… LAST…`) is parsed.
- Shown newest first. Index 0 is "Current Boot", -1 "Previous Boot",
  others by their first entry's date ("Oct 13 01:23 – Oct 31 04:42").
- If the list cannot be obtained a single "Current Boot" (`--boot=0`) is used.

### Event list
- Lazy `GtkListView`. Each row: time (today: `HH:MM:SS`; same year:
  `Mon DD HH:MM`; else `YYYY-MM-DD`), message (single line, ellipsized),
  process name (dim). Priority ≤ 3 rows get the `error` style class,
  priority 4 the `warning` class.
- Incremental loading: 200 matching entries are requested first; when the
  view scrolls within ~1.5 pages of the end, the next 200 are requested.
  A spinner shows while the worker is reading. An empty result shows an
  `AdwStatusPage` ("No Results" / "No Events").
- Sort order follows `sort-order` (`descending-time` = newest first, uses
  `--reverse`).
- Activating a row opens the **detail page**: header title = process,
  subtitle = full timestamp; a list of all fields (name, value — values
  selectable, multi-line allowed), with `MESSAGE`, `PRIORITY` and the
  derived time first and the rest alphabetical. A *Copy* button copies the
  entry as `FIELD=value` lines.

### Live updates
- When the selected boot is the current boot, new journal entries are
  followed while the view is open. Before the paged query starts, the
  worker reads the cursor of the boot's newest entry
  (`journalctl -o json --boot=… --reverse --lines=1`, unfiltered), then runs
  `journalctl -o json --boot=… --follow --after-cursor=<cursor>
  [--priority=…] [matches…]` (`--lines=0` instead of the cursor when the
  journal is empty). Nothing written between the two is missed.
- Followed entries go through the same category rule and search as the
  paged query; entries are de-duplicated by `__CURSOR` against what is
  already shown (the lazy paged query can reach new entries itself).
- Newest-first view: new entries are inserted at the top. Oldest-first
  view: they are appended at the end, but only once the paged query has
  loaded everything (until then they are held back).
- Scrolling: if the view is at the edge where new entries appear (the top
  for newest-first, the bottom for oldest-first) it scrolls to show them;
  otherwise its position is kept (the list stays on the rows in view).
- An empty view switches from "No Events/No Results" to the list when a
  matching entry arrives. Changing category, boot, search or sort restarts
  both the query and the follow process; closing the window kills it.
  Old boots are not followed.

### Search
- Search bar with an entry and a field selector: *All Fields*, *Message*,
  *Process*, *Unit*.
- Matching is case-insensitive substring; the text is split on whitespace
  and every word must match (in the selected field(s); for All Fields any
  of message/process/unit, where each word may match a different one).
- Free text of the form `FIELD=value` (field name uppercase, e.g.
  `_PID=123`, `SYSLOG_IDENTIFIER=sshd`) restricts that word to the named
  journal field (substring, case-insensitive), regardless of the selector.
- Searching restarts the query: matches are found in the whole selected
  boot/category, not only the rows already loaded. Typing is debounced
  (`GtkSearchEntry` search-changed).

### Export
- *Export…* (`Ctrl+S`) opens a `GtkFileDialog` save dialog
  (default name `logs.txt`) and writes the rows currently loaded in the
  view, in view order, one per line:
  `YYYY-MM-DD HH:MM:SS process[pid]: message` (local time; continuation
  lines of multi-line messages are indented by a tab).

### GSettings (`org.gnome.Logs`)
| Key | Type | Use |
|---|---|---|
| `sort-order` | enum `ascending-time` \| `descending-time` (default descending) | list order; changed from the menu |
| `ignore-warning` | b (default false) | suppress the permission banner |

### Shortcuts
`Ctrl+F` search, `Escape` close search / leave detail page, `Ctrl+S`
export, `F5`/`Ctrl+R` refresh, `Ctrl+Q` quit, `Ctrl+W` close window.

### CLI
`helix-logs` (no options other than `--help`).

## Decisions
- Live updates: follow the journal and append new entries to the current
  view, respecting filters and keeping the scroll position unless at the
  live edge (see "Live updates"). Category rules unchanged.

## Open questions
- GNOME Logs' exact rule for the *Applications* and *System* categories is
  not published; the rules above are an approximation (user-owned
  identifiers vs. everything else that isn't kernel).

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): "No results" for a search and the "Unable to read logs" error (assertive).
