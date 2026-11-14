# Calendar

Drop-in replacement for GNOME Calendar.

- **App ID**: `org.helix.Calendar`
- **Binary**: `helix-calendar`
- **Crate**: `apps/calendar`

## Purpose
View and edit events from local and online calendars.

## Main windows
- Month / week / day views.
- Event editor.
- Calendar management.

## Contracts used
- Evolution Data Server (EDS) or equivalent calendar backend.
- GNOME Online Accounts.
- XDG portals: Notification, OpenURI.
- GSettings: `org.gnome.calendar`.

## Non-goals
- Full EDS plugin compatibility.

---

## Behaviour (derived spec)

### Data source
- Events live in **Evolution Data Server**, reached over D-Bus through the
  shared crate `crates/helix-eds` (zbus; no EDS client libraries): source
  registry `org.gnome.evolution.dataserver.Sources5`, factory
  `…Calendar8` (`OpenCalendar`), per-calendar `Calendar` and
  `CalendarView` objects. iCalendar parsing/serialization, time-zone
  conversion and RRULE expansion are pure Rust in `helix-eds`.
- Every enabled source with a `[Calendar]` extension is listed (local
  calendars, Birthdays & Anniversaries, Weather, and whatever GNOME Online
  Accounts configured in EDS: Google, Nextcloud/CalDAV, Exchange…).
  Calendars whose `Selected` flag is set are opened and shown.
- For the visible range (with a margin) each shown calendar gets a live
  view with `(occur-in-time-range? …)`; added/modified/removed components
  update the views at once. Changing the visible range replaces the views.
- Recurring events are expanded client-side (DAILY/WEEKLY/MONTHLY/YEARLY
  with INTERVAL, COUNT, UNTIL, BYDAY incl. ordinals, BYMONTHDAY, BYMONTH,
  BYSETPOS, WKST; EXDATE, RDATE, detached RECURRENCE-ID overrides). Rules
  using other parts (HOURLY, BYWEEKNO…) show only their first occurrence.
- Times are converted to the system zone (`TZ`, else `/etc/localtime`).
  TZIDs resolve via IANA names, libical-style prefixed ids and VTIMEZONE
  `X-LIC-LOCATION`; unknown zones are treated as local time.
- Reminders are stored as VALARMs; firing them is left to EDS's
  `evolution-alarm-notify` (the Notification portal is not needed by the
  app itself).
- If EDS cannot be reached the window shows a status page with *Retry*.
- **Later phase (not built)**: a Helix-native local iCalendar store with
  direct CalDAV sync for systems without EDS.

### Window
- `AdwApplicationWindow`. Header bar: *New Event* (`+`), *Today*,
  previous/next arrows, the period title ("November 2026", or the week's
  date range), a view switcher (**Week** / **Month**), search toggle and
  the primary menu (*Manage Calendars…*, *Import…*, *Keyboard Shortcuts*,
  *About Calendar*).
- **Sidebar** (`AdwOverlaySplitView`, toggled with `F9`, collapses below
  800sp): a month date picker (`gtk::Calendar`) to jump to any date, the
  list of calendars with colour dots and visibility check boxes, and the
  agenda (events) of the selected day. During a search, the agenda lists
  search results instead.
- **Month view**: 7 columns starting on the locale's first weekday, 5–6
  week rows covering the month; optional ISO week numbers
  (`org.gnome.desktop.calendar show-weekdate`). Each day cell shows its
  number (today highlighted, other-month days dimmed) and as many event
  chips as fit (all-day/multi-day first, coloured with the calendar
  colour; timed events show their start time), then "+N more" opening a
  popover with the full list. Clicking a day selects it; double-clicking
  an empty part of a day creates an all-day event on that day.
- **Week view**: day headers, an all-day strip, then a scrollable
  24-hour grid drawn with cairo. Overlapping events are laid out side by
  side. A red line marks the current time. Double-click an empty slot to
  create a one-hour event there; click an event to edit it.
  `Ctrl`+scroll or `Ctrl +/-/0` zooms the grid (`week-view-zoom-level`,
  0.5–3.0).
- Clicking an event anywhere opens the **event editor**.

### Keyboard and screen readers in the grids
- **Month view** is an accessible grid (role *grid*, named after the
  month, with row/column counts); each day cell is a focusable *grid
  cell* with a focus ring, row/column index, the *selected* state, and a
  name such as "Friday, 13 November 2026, today, selected, 2 events:
  Standup, Lunch". In a focused cell: arrows move by a day / a week,
  `Home` / `End` to the first / last day of the week, `Page Up` / `Page
  Down` to the same day of the previous / next month (clamped to its
  last day). Moving selects the day (the agenda follows) and keeps focus
  on the new cell, leaving the month when needed. `Enter` / `Space`
  selects the focused day; `Ctrl+N` creates an event on the selected day.
  Clicking a day also focuses it. `Tab` moves into the day's event chips.
- **Week view**: the day headers are focusable grid cells with the same
  names; `Left` / `Right`, `Home` / `End`, `Page Up` / `Page Down` (a
  week) move between days, `Enter` selects. The hour grid (cairo-drawn)
  is itself focusable and has a **slot cursor**: a half-hour outline on
  the selected day, drawn while the grid has focus. Arrows move it by
  half an hour / a day, `Home` / `End` to 00:00 / 23:30, `Page Up` /
  `Page Down` by a week (changing the shown week as needed); the grid
  scrolls to keep it visible; `Enter` / `Space` creates a one-hour event
  at the cursor. The grid's accessible name describes the cursor slot
  ("Wednesday, 11 November 2026, 07:30, Standup") and each move is
  announced, because GTK has no accessible children for a drawing area.
  Clicking the grid moves the cursor there and focuses it. Timed events
  remain focusable buttons over the grid.
- **Event chips and week events** are named "Title, Fri 13 Nov, 09:00 –
  09:15, Calendar" (date always included; the tooltip also shows the
  date).
- Rebuilding the month grid or the week headers keeps focus on the same
  day.

### Event editor (dialog)
- Title, calendar (writable calendars only), location, all-day switch,
  start date + time, end date + time (end is moved along when start moves
  past it), repeat (*No Repeat, Daily, Weekdays, Weekly, Monthly,
  Yearly*; an existing custom rule is kept and shown as *Custom*), repeat
  end (*Forever*, *After N occurrences*, *Until date*), reminders (list
  with remove buttons; add 5/10/15/30 minutes, 1 hour, 1 day, 2 days,
  1 week before), notes.
- Time entries accept `9`, `930`, `9:30`, `9:30pm`, `21:30`; dates come
  from a calendar popover.
- New timed events are written with `TZID` = the system zone; all-day
  events use `VALUE=DATE` with exclusive end.
- *Save* creates (`CreateObjects`) or modifies (`ModifyObjects`) the event;
  *Delete* removes it. Moving an event to another calendar creates it
  there and removes the original.
- For an occurrence of a recurring event, *Save* and *Delete* ask:
  *Only This Event* (mod `this`: a detached RECURRENCE-ID instance, or an
  EXDATE on delete) or *All Events* (mod `all`; time changes shift the
  whole series by the same offset).
- Read-only calendars open the editor read-only (no Save/Delete).
- Unknown properties and sub-components (attendees, attachments, X-props)
  are preserved on save.

### Calendar management
- *Manage Calendars…* opens a preferences dialog listing calendars grouped
  by account: a colour button (writes `Color`), a visibility switch
  (writes `Selected`) and, for removable calendars, a delete button
  (confirmation required). Local calendars can be renamed (entry row with
  apply button).
- *New Calendar*: name + colour → a local calendar (`Parent=local-stub`,
  `BackendName=local`) via `SourceManager.CreateSources`.
- Online calendars are added through GNOME Online Accounts (Settings); the
  dialog says so.
- *New Calendar from URL* (same dialog): address (`http://`, `https://`,
  `webcal://`, `webcals://`), optional name (default: the feed's file name
  without `.ics`, else the host) and colour → *Subscribe*.
  - Validation before anything is created: no spaces, supported scheme,
    a valid host name / IPv4 / bracketed IPv6, port 1–65535, no
    `user:password@` (credentials are not stored). Errors appear in red
    under the fields and the address row is marked; editing clears them.
  - `webcal`/`webcals` mean HTTPS. The source is created via
    `SourceManager.CreateSources` with the layout Evolution / GNOME
    Calendar use: `[Data Source] Parent=webcal-stub`,
    `[Calendar] BackendName=webcal` + `Color`/`Selected`,
    `[Authentication] Host/Port/Method=none`, `[Security] Method=tls|none`,
    `[WebDAV Backend] ResourcePath/ResourceQuery`,
    `[Refresh] Enabled=true IntervalMinutes=30`,
    `[Offline] StaySynchronized=true`. EDS downloads and refreshes it.
  - Subscribed calendars are read-only (no editing of their events); the
    dialog shows their URL (selectable) instead of a rename field; they
    can be recoloured, hidden and removed.
  - Errors from creating the source, or a view that completes with an
    error, are shown as toasts. Limitation: EDS does not report a failed
    download over D-Bus (the view completes normally; only the source's
    `ConnectionStatus` goes `connecting` → `disconnected` without passing
    `connected`), so an unreachable feed simply shows no events.

### Search
- `Ctrl+F` opens a search entry. Results (summary, description or
  location matches, `(contains? …)` over all shown calendars) are listed
  in the sidebar with dates; activating one jumps to the date and opens
  the editor.

### Import
- *Import…* or passing `.ics` files on the command line parses VEVENTs
  and asks which writable calendar to add them to; existing UIDs are
  replaced with fresh ones only when the target already holds them.

### GSettings
- `org.gnome.calendar` (installed schema; `data/org.gnome.calendar.gschema.xml`
  carries only the keys used): `window-maximized` (b, true),
  `window-size` (`(ii)`, (768, 600)), `active-view`
  (enum `org.gnome.calendar.GcalWindowView`: `week`|`month`, default
  `month`), `week-view-zoom-level` (d, 1.0).
- Read only: `org.gnome.desktop.interface clock-format` (`12h`/`24h`),
  `org.gnome.desktop.calendar show-weekdate`.
- `org.gnome.Evolution.DefaultSources default-calendar` (s): default
  calendar for new events (read; updated when the user picks a calendar
  for a new event).

### First weekday
- From the locale's `LC_TIME` definition (`week` + `first_weekday` in
  `/usr/share/i18n/locales/<locale>`, following `copy`), else Monday.

### Keyboard shortcuts
`Ctrl+N` new event, `Ctrl+T` today, `Alt+Left` previous, `Alt+Right`
next, `Ctrl+1` week view, `Ctrl+2`
month view, `Ctrl+F` search, `F9` sidebar, `Ctrl+Plus` / `Ctrl+Minus` /
`Ctrl+0` zoom week grid, `Ctrl+I` import, `Ctrl+?` shortcuts, `Ctrl+Q`
quit. (Bare keys such as `Home` are not bound so they keep working in
text fields; the grid keys above only act in a focused day cell, day
header or hour grid, and are listed in the shortcuts window.)

### Command line
- `helix-calendar [--date YYYY-MM-DD] [--uuid UID] [FILE.ics…]`.
  `--date` opens at that date; `--uuid` opens the editor for that event
  (searched in the shown calendars); `.ics` files are imported.

### Testing
- `crates/helix-eds` unit tests: iCalendar parse/serialize, RRULE
  expansion (RFC 5545 examples), time-zone resolution, durations.
- `crates/helix-eds/tests/private_bus.rs` (via `tests/run-private-eds.sh`)
  creates a calendar source, events, detached instances, removals and
  visibility changes on EDS daemons on a **private** bus; it also
  subscribes a `webcal` calendar to a feed served by an in-test HTTP
  server on 127.0.0.1 and checks EDS downloads its event.
- `helix-eds` unit tests cover URL validation and the webcal source layout.
- App unit tests: date grids, week layout, time input parsing, repeat
  presets, locale first weekday, default names for URL calendars.

## Decisions
- Backend: Evolution Data Server over D-Bus (zbus), sharing
  `crates/helix-eds` with Contacts; a local iCalendar store with CalDAV
  sync is a later phase.
- Views: GNOME Calendar's schema only knows `week` and `month`
  (`GcalWindowView`), so Helix offers those two plus the day agenda in the
  sidebar instead of a separate day view.

- Webcal: Helix has its own "New Calendar from URL" flow creating an EDS
  `webcal` source through the sources registry (EDS fetches/refreshes it).

### Accessibility
- Focus is kept across list rebuilds (`helix_app_common::focus`): Rebuilding the calendar list keeps focus on the toggled check box; rebuilding the agenda keeps focus on the same event (or a neighbour) and the scroll position.
