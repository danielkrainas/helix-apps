# Clocks

Drop-in replacement for GNOME Clocks.

- **App ID**: `org.helix.Clocks`
- **Binary**: `helix-clocks`
- **Crate**: `apps/clocks`

## Purpose
World clocks, alarms, stopwatch, and timer.

## Main windows
- World clocks list.
- Alarms list and editor.
- Stopwatch and timer views.

## Contracts used
- XDG portals: Notification, Background.
- GSettings: `org.gnome.clocks`.
- System time / timezone services.

## Non-goals
- Exact alarm sound set.
- Weather-station based locations (libgweather is not available). Locations
  are derived from the IANA time zone database instead.

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow` (default 870×690). The header bar's title is an
  `AdwViewSwitcher` with four pages: **World**, **Alarms**, **Stopwatch**,
  **Timer**. Below 550sp width the switcher moves to an `AdwViewSwitcherBar`
  at the bottom.
- The header bar's start holds an **Add** button (`+`) that is visible on the
  World, Alarms and Timer pages and opens the page's "new" dialog. The end
  holds the primary menu (*Keyboard Shortcuts*, *About Clocks*).
- Window size, maximized state and the last visible page are restored from
  `org.gnome.clocks.state.window` (`size`, `maximized`, `panel-id`).
- The app is single-instance (`GApplication` unique): launching again
  presents the existing window.

### Time sources
- All logic takes the current wall-clock time (`DateTime<Utc>`) and a
  monotonic timestamp as parameters (an injectable `Clock` trait), so it is
  unit-tested with fake clocks. The UI uses the system clock.
- The local time zone comes from GLib (`TZ`, `/etc/localtime`); when its
  identifier is not a known IANA name, UTC offsets from the system are used.
- Time zone math uses the IANA database compiled into `chrono-tz`.
- Times are shown in 24-hour or 12-hour (`h:mm AM`) form according to
  `org.gnome.desktop.interface clock-format` (read-only, live-updated).

### World
- A boxed list of world clocks. Each row shows: a day/night icon
  (`weather-clear-symbolic` / `weather-clear-night-symbolic`), the city
  name, a subtitle, and the current time (no seconds).
  - Subtitle: `<country> • <relative day> • <offset>`, where the country is
    omitted when unknown, the relative day is *Tomorrow* or *Yesterday*
    (comparing the location's calendar date with the local date; omitted
    when it is the same day), and the offset from local time reads e.g.
    `9 hours ahead`, `3½ hours behind`, `5¾ hours ahead`,
    `5 hours 20 minutes ahead`, `Current time zone`.
  - Day/night: when the location has coordinates, it is *day* if the sun is
    above the horizon (sunrise/sunset computed with the NOAA approximation,
    standard −0.833° altitude); polar day/night handled. Without coordinates,
    day is 07:00–19:00 local to the location.
- Clicking a row opens a **details page** (`AdwNavigationView` push; the
  header bar shows a back button in place of *Add*): large time with
  seconds, full date, time zone ID and abbreviation, UTC offset, offset from
  local time, sunrise, sunset and day length (if coordinates are known).
- Each row has a remove button. The empty state is an `AdwStatusPage`
  ("Add World Clocks", with an *Add Location* button).
- **Add location dialog** (`AdwDialog`): a search entry and a results list.
  The searchable set is built offline:
  1. every zone in `/usr/share/zoneinfo/zone1970.tab` (or `zone.tab`) gives a
     principal city (last path component with `_` → space, e.g.
     `America/Argentina/Buenos_Aires` → *Buenos Aires*), coordinates, and
     country names from `iso3166.tab`;
  2. any other canonical `chrono-tz` zone (e.g. `Etc/GMT+5`, `UTC`) is
     added without coordinates, with its region as the subtitle.
  Search is case- and accent-insensitive and matches the city, the zone ID,
  the country name or the country code; prefix matches on the city sort
  first. Zones already in the list are marked and cannot be added twice.
  Selecting a result adds it and closes the dialog.
- If `geolocation` is on, a *Current Location* row derived from the system
  time zone (no GeoClue lookup) is shown first; it is not stored.

### Alarms
- A boxed list of alarms sorted by time. Each row shows the time (large),
  the name and the repeat summary (*Weekdays*, *Weekends*, *Every Day*,
  `Mon, Wed, Fri`, or nothing for one-shot), an **enable switch**, and is
  activatable to edit. Snoozed alarms show *Snoozed until HH:MM*.
- **New / edit dialog**: hour and minute spin buttons (hour 0–23 in 24h
  mode; in 12h mode 1–12 plus an AM/PM toggle), seven day toggle buttons
  (Monday first),
  a name entry (default "Alarm"), ring duration and snooze
  duration combo rows (1, 2, 3, 5, 10, 15, 20, 30, 60 minutes; defaults:
  ring 5 min, snooze 10 min). Edit mode adds a *Delete* button.
- **Next ring computation**: for a repeating alarm, the next local
  occurrence of `hour:minute` on one of the selected weekdays strictly after
  now; for a one-shot alarm, the next occurrence on any day. Local times
  that do not exist (DST gap) ring at the first valid instant after the gap
  (wall time + gap length); ambiguous times ring at the earlier instant.
- **Ringing**: checked every second. When the scheduled instant is reached
  (and is not older than the ring duration, e.g. after suspend) the alarm
  rings: an `AdwAlertDialog` with the alarm's name and time and **Snooze** /
  **Stop** buttons is presented, a `gio::Notification` (id `alarm-<id>`,
  urgent priority, buttons *Snooze*/*Stop* wired to `app.alarm-snooze` /
  `app.alarm-stop`) is sent, and the display bell is sounded every 2
  seconds. Ringing stops by itself after the ring duration. Snooze reschedules
  the alarm `snooze_minutes` after now. Stop (or timeout) for a one-shot
  alarm disables it; a repeating alarm is rescheduled for its next day.
  An occurrence missed by more than the ring duration is skipped and a
  *Missed alarm* notification is sent.
- **Alarms only ring while Helix Clocks is running** (see open questions).
  While any alarm is enabled or any timer runs, the window has
  `hide-on-close` set, so closing it hides it and the process keeps running;
  launching Clocks again presents it. *Quit* (Ctrl+Q) really quits.

### Stopwatch
- Large `MM:SS.d` display (hours added when ≥ 1 h; centiseconds `.cc` in the
  lap list). Buttons: **Start** / **Pause** / **Resume**, **Lap** (while
  running), **Reset** (while paused). Space toggles start/pause; `L` laps;
  `Delete`/`BackSpace` resets while paused. Once a lap exists, the current
  lap's number and running split are shown under the main display.
- The lap list (newest first) shows lap number, lap split, total time and
  the difference to the previous lap (`+0:01.23` / `−0:00.40`).
- The stopwatch measures monotonic time; it is not persisted.

### Timer
- When no timers exist: a grid of **presets** (1, 2, 3, 5, 10, 15, 30 min,
  1 h) and a **custom** row (hours/minutes/seconds spin buttons + name
  entry + *Start*). Picking a preset or custom duration creates the timer
  and starts it.
- Multiple timers are listed, each with its name, a large countdown
  (`H:MM:SS` / `MM:SS`), and buttons **Start/Pause/Resume**, **Reset**,
  **Delete**. A running timer's row has a progress bar.
- When a timer reaches zero it stops, resets to its full duration, sends a
  `gio::Notification` ("Time is up!", the timer's name) and sounds the bell
  three times.
- Timer definitions (name, duration) are persisted; running state is not.

### GSettings (`org.gnome.clocks`)
All values are `aa{sv}` per the published schema (`geolocation` is `b`).
The dictionary contents below are what Helix writes and expects;
unknown keys are ignored on read.

| Key | Dict entries |
|---|---|
| `world-clocks` | `location`: `v` holding a GWeather-style serialized location `(uv)` = `(2, <(ssba(dd)a(dd))>)` (name, station code `''`, is-city `true`, `[(lat, lon)]` city coordinates in radians, `[]` station coordinates). Helix adds `helix-timezone`: `s` (IANA zone ID). On read, `helix-timezone` wins; otherwise the zone whose `zone1970.tab` principal city is nearest to the city (or station) coordinates is used (same rule as the shell's world clocks; e.g. Munich resolves to `Europe/Zurich`, which has identical rules today), and no country is shown for such entries; locations with neither are dropped. Duplicates (same name and zone) are dropped. |
| `alarms` | `id` `s` (random hex), `name` `s`, `active` `b`, `hour` `i`, `minute` `i`, `days` `ai` (1 = Monday … 7 = Sunday; empty = one-shot), `ring_minutes` `i`, `snooze_minutes` `i`. |
| `timers` | `duration` `i` (seconds), `name` `s`. |
| `geolocation` | `b` – shows the *Current Location* row (see World). |

`org.gnome.clocks.state.window`: `size` `(ii)`, `maximized` `b`, `panel-id`
enum `world|alarm|stopwatch|timer`.

`org.gnome.desktop.interface clock-format` (`24h`|`12h`) is read only.

If a schema is not installed, values live in memory for the session.
`data/org.gnome.clocks.gschema.xml` documents (and can install) the keys used.

### Keyboard shortcuts
| Shortcut | Action |
|---|---|
| `Alt+1` … `Alt+4` | World / Alarms / Stopwatch / Timer |
| `Ctrl+N` | New location / alarm / timer (current page) |
| `Space`, `L`, `Delete` | Stopwatch start-pause, lap, reset (stopwatch page) |
| `Ctrl+Q` | Quit |
| `Ctrl+W` | Close window |
| `Ctrl+?` | Keyboard shortcuts |

### Command line
`helix-clocks` takes no options besides GApplication's standard ones
(`--help`).

## Open questions
- **Background running**: alarms only ring while the process runs. Should
  Helix request autostart / background permission via the XDG Background
  portal (`org.freedesktop.portal.Background.RequestBackground` with
  `autostart=true` and a `--gapplication-service` command line) so alarms
  survive logout/login? Not done yet; currently the app stays alive after
  the window closes only while an alarm is enabled.
- **Sound**: no GStreamer/libcanberra is available, so ringing uses the
  display bell (`gdk::Display::beep`) every 2 s, and timers beep three
  times. A PipeWire-based alarm tone could be added later.
- **Geolocation**: no GeoClue lookup is done; the *Current Location* row
  uses the system time zone. Should Helix query GeoClue2 (via zbus) for a
  real place name?
- **GWeather interoperability**: GNOME Clocks expects a weather station code
  in serialized locations; Helix writes an empty code plus coordinates. It is
  unverified whether GNOME Clocks resolves such entries; Helix itself reads
  GNOME-written entries via coordinates.
