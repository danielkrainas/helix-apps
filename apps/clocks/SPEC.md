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
- XDG portals: Notification, Background, Location; GeoClue2 (fallback).
- GStreamer (alarm and timer tones, PipeWire output).
- GWeather location database `libgweather-4/Locations.xml` (read-only data file, optional).
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
- If `geolocation` is on, a *Current Location* row is shown first; it is
  not stored. It starts out derived from the system time zone; at startup
  Helix asks for the position once (see *Current location* below) and, on
  success, replaces it with the nearest place.

### Current location
- Position: the XDG **Location portal** (`org.freedesktop.portal.Location`:
  `CreateSession` with `accuracy` = city, `Start`, first
  `LocationUpdated`, then `Session.Close`); if the portal is missing or
  refuses, **GeoClue2** on the system bus (`Manager.GetClient`, `DesktopId`
  = `org.helix.Clocks`, `RequestedAccuracyLevel` = city (4), `Start`, first
  `LocationUpdated` → `Location.Latitude/Longitude`, then `Stop` and
  `DeleteClient`). Runs on a worker thread; gives up after 30 s.
- Naming: the nearest `<city>` (≤ 100 km) of the GWeather location database
  (`$XDG_DATA_DIRS/libgweather-4/Locations.xml`, read-only data file) gives
  the name, country and time zone (`tz-hint` of the city/state/country);
  otherwise the zone of the nearest `zone1970.tab` city is used.

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
  `app.alarm-stop`) is sent, and a **tone** plays in a loop (see *Sound*).
  Ringing stops by itself after the ring duration. Snooze reschedules
  the alarm `snooze_minutes` after now. Stop (or timeout) for a one-shot
  alarm disables it; a repeating alarm is rescheduled for its next day.
  An occurrence missed by more than the ring duration is skipped and a
  *Missed alarm* notification is sent.
- **Background running.** While any alarm is enabled or any timer runs,
  the application holds itself (`g_application_hold`) and the window has
  `hide-on-close` set, so closing it hides it and the process keeps
  running; launching Clocks again presents it. *Quit* (Ctrl+Q) really quits.
- **Background portal.** When alarms become enabled (at startup or later),
  Helix calls `org.freedesktop.portal.Background.RequestBackground` with
  `reason`, `autostart=true`, `commandline=['helix-clocks', '--hidden']`
  and `dbus-activatable=false` (preceded by the best-effort host-app
  `org.freedesktop.host.portal.Registry.Register('org.helix.Clocks')` for
  newer portals). When the last alarm is disabled during the session it
  calls it again with `autostart=false` to remove the login entry. If the
  portal is unavailable or refuses background running, the Alarms page shows
  "Alarms only ring while Clocks is running."
- **Autostart fallback** (no portal): `data/org.helix.Clocks-autostart.desktop`
  is installed by packagers as `/etc/xdg/autostart/org.helix.Clocks.desktop`
  (`Exec=helix-clocks --hidden`, `NoDisplay=true`). It is not installed by
  the build and costs nothing when unused: `--hidden` exits at once when no
  alarm is enabled.
- **`--hidden` start**: registers the application; if an instance already
  runs it exits without showing anything. Otherwise it loads the alarms and,
  if any is enabled, keeps running without a window (the window is presented
  when an alarm rings or the user launches Clocks); else it exits.

### Sound
- GStreamer (`playbin`) plays the sound theme's event sound —
  `alarm-clock-elapsed` for alarms (looped until the alarm stops), `complete`
  for timers (once) — looked up as
  `<data dir>/sounds/<theme>/stereo/<id>.{oga,ogg,wav}` in
  `$XDG_DATA_HOME` then `$XDG_DATA_DIRS`, for the theme in
  `org.gnome.desktop.sound theme-name` (read only) and then `freedesktop`.
- If no file exists, a generated 880 Hz sine from `audiotestsrc` is gated
  in a beep-beep-pause cadence (200 ms on, 100 off, 200 on, 500 off); timers
  play it for 2 s.
- Output: `pipewiresink` (stream property `media.role=Alarm`) when the
  element exists, else `autoaudiosink`. `$HELIX_AUDIO_SINK` (a gst-launch
  description, e.g. `fakesink`) overrides it (tests use this).
- If GStreamer cannot play (init, pipeline or runtime error), Helix falls
  back to the display bell (every 2 s while an alarm rings, three times for
  a timer).

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
  `gio::Notification` ("Time is up!", the timer's name) and plays the timer
  sound once (not while an alarm is ringing).
- Timer definitions (name, duration) are persisted; running state is not.

### GSettings (`org.gnome.clocks`)
All values are `aa{sv}` per the published schema (`geolocation` is `b`),
checked against the installed `/usr/share/glib-2.0/schemas/org.gnome.clocks.gschema.xml`
(key names, types and defaults match). The user's own values
(`gsettings get org.gnome.clocks world-clocks` / `alarms` / `timers`, and
`org.gnome.shell.world-clocks locations`) were all empty (`@aa{sv} []`), so
the dictionary layouts below could not be checked against real GNOME-written
data; they follow GNOME's documented/observed format (see Open questions).
Unknown keys are ignored on read, and Helix writes nothing GNOME would
reject.

| Key | Dict entries |
|---|---|
| `world-clocks` | `location`: `v` holding a GWeather serialized location `(uv)` = `(uint32 2, <(ssba(dd)a(dd))>)`: name, METAR station code, is-city `true`, `[(lat, lon)]` city coordinates and `[(lat, lon)]` station coordinates, both in radians — exactly what GNOME Clocks writes. Helix fills the station with the nearest weather station from the GWeather database (`libgweather-4/Locations.xml`); without the database the code is `''` and the station list empty. Entries read from GNOME keep their station unchanged on rewrite. Helix adds `helix-timezone`: `s` (IANA zone ID; ignored by GNOME). On read the zone is: `helix-timezone`, else the station's zone from the GWeather database (`tz-hint` of station/state/country, or the country's only zone), else the zone whose `zone1970.tab` principal city is nearest to the coordinates. The country comes from the zone's principal city (if the name matches) or the station's country. Entries with no resolvable zone are dropped; duplicates (same name and zone) are dropped. |
| `alarms` | `id` `s` (random hex), `name` `s`, `active` `b`, `hour` `i`, `minute` `i`, `days` `ai` (0 = Monday … 6 = Sunday, GNOME's numbering; empty = one-shot; `7` is read as Sunday for entries written by early Helix builds), `ring_minutes` `i`, `snooze_minutes` `i`. |
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
`helix-clocks [--hidden]`: `--hidden` starts in the background without a
window (used by autostart; see Alarms). Plus GApplication's standard options
(`--help`).

### Packaging
- `data/org.helix.Clocks.desktop` → `/usr/share/applications/`.
- `data/org.helix.Clocks-autostart.desktop` →
  `/etc/xdg/autostart/org.helix.Clocks.desktop` (non-portal autostart
  fallback; optional).
- `data/org.gnome.clocks.gschema.xml` → only when the GNOME schema is not
  installed.

## Decisions
- **Background running**: request background + autostart via the XDG
  Background portal (`--hidden` command line), hold the application while
  alarms are enabled, ship an autostart `.desktop` as the non-portal
  fallback.
- **Sound**: ring with a real tone through GStreamer/PipeWire (sound-theme
  file if present, else a generated `audiotestsrc` tone), not the display
  bell (kept only as a last-resort fallback).
- **Geolocation**: use the Location portal, falling back to GeoClue2 over
  D-Bus.
- **Saved-locations format**: read and write GNOME's exact GWeather
  serialization, with real station codes from the GWeather location
  database when it is installed (verified against the installed schema;
  the user's own values are empty).

## Open questions
- **Alarm weekday numbering**: Helix now writes GNOME's 0 = Monday … 6 =
  Sunday numbering. This could not be confirmed from real data (the user has
  no saved alarms); if a GNOME-written alarm shows up shifted by a day,
  the numbering needs revisiting.
