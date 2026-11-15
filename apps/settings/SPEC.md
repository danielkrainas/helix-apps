# Settings

Drop-in replacement for GNOME Settings.

- **App ID**: `org.helix.Settings`
- **Binary**: `helix-settings-app`
- **Crate**: `apps/settings` (package `helix-settings-app`)

The package and binary are deliberately **not** named `helix-settings`:
helix-desktop already has a crate called `helix-settings`, and two packages
or installed binaries with the same name would clash when both repos are
built or installed side by side.

## Purpose
Primary configuration UI for the desktop and system.

## Main windows
- Sidebar of panels (Wi-Fi, Bluetooth, Network, Appearance, Displays, Sound, Power, etc.).
- Search across panels.
- Modal dialogs for complex settings (displays, network connections).

## Contracts used
- GSettings schemas under `org.gnome.desktop.*`, `org.gnome.settings-daemon.*`, NetworkManager, etc.
- XDG portals: Settings, Background, Lockdown.
- Session D-Bus: SessionManager, Shell, DisplayConfig, IdleMonitor, NetworkManager, UPower, etc.

## Non-goals
- Every panel from day one. Implement panels incrementally.
- GNOME-specific private APIs that are not part of the public contracts.

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow` (default 980×680) containing an
  `AdwNavigationSplitView`. It collapses to a single pane below 700 px wide
  (`AdwBreakpoint`); selecting a panel then pushes it and the back button
  returns to the list.
- The window's minimum size is 360×294. Every implemented panel fits that
  width without clipping: groups of cards, swatches and thumbnails wrap
  onto more lines instead of widening the page, and sliders in rows are at
  least 150 px wide (growing with the row).
- **Sidebar**: header bar titled *Settings* with a search toggle and the
  primary menu (*About Settings*), a search entry
  (shown with the toggle or `Ctrl+F`), and the panel list grouped by
  separators:
  1. Wi-Fi, Network, Bluetooth
  2. Displays, Sound, Power, Multitasking, Appearance
  3. Apps, Notifications
  4. Mouse & Touchpad, Keyboard
  5. Accessibility, Privacy & Security
  6. Date & Time, Users, About
- Each row: symbolic icon + title. Panels that are not implemented yet are
  listed dimmed with the subtitle *Not yet available*; selecting one shows a
  status page saying the panel will come in a later release.
- **Content**: an `AdwNavigationPage` with a header bar titled after the
  panel and an `AdwPreferencesPage` built on first visit (panels are built
  lazily and cached for the window's lifetime).
- The window opens on *Appearance* unless a panel is given on the command
  line.

### Search
- Typing filters the sidebar. The query is lower-cased and split on
  whitespace; a panel matches when **every** term is a substring of its title
  or of one of its keywords (each panel carries keywords for the settings it
  contains, e.g. Appearance: *dark, light, style, theme, accent, color,
  wallpaper, background*).
- Results are ordered: title starts with the query, then a term occurs in the
  title, then keyword-only matches; registry order within each rank. Group
  separators are hidden while searching.
- `Enter` in the search entry opens the first result; `Escape` clears and
  closes the search.

### Panel registry
- A static table of `PanelInfo { id, title, icon, keywords, group,
  available, aliases }`. Available panels have a builder implementing the
  `Panel` trait (`fn build(&self, ctx) -> gtk::Widget`).
- Panel ids and accepted aliases (aliases include GNOME Settings panel names
  so scripts that launch `gnome-control-center <panel>` keep working):

| Id | Aliases | Available |
|---|---|---|
| `wifi` | `wi-fi`, `wireless` | no |
| `network` | `wired` | no |
| `bluetooth` | | no |
| `display` | `displays`, `screen` | no |
| `sound` | `audio` | no |
| `power` | | yes |
| `multitasking` | `workspaces` | yes |
| `appearance` | `background`, `wallpaper` | yes |
| `apps` | `applications`, `default-apps` | no |
| `notifications` | | yes |
| `extensions` | `shell-extensions`, `add-ons` | yes |
| `mouse` | `touchpad`, `mouse-touchpad` | yes |
| `keyboard` | `input-sources` | yes |
| `accessibility` | `universal-access`, `a11y` | yes |
| `privacy` | `security`, `screen-lock`, `location` | yes |
| `datetime` | `date-time`, `time` | yes |
| `users` | `user-accounts` | no |
| `about` | `info-overview`, `info`, `system` | yes |

### Settings bindings
- Every control reads its key on build and writes on change. When the key
  exists in the installed schema it goes through GSettings and the control
  follows external changes (`changed::key`). When the schema or key is
  missing (for example `accent-color` on schemas older than GNOME 47) the
  value lives in memory only (`helix_app_common::Prefs`) and the row says
  *Not supported by this system*.
- `data/org.helix.Settings.gnome-compat.gschema.xml` holds GNOME-compatible
  definitions of exactly the keys used (same schema ids, paths, types,
  defaults and enum nicks), for systems that do not ship the GNOME schemas.
- Delay choices are combo rows over fixed presets (seconds). A stored value
  that is not a preset is added as an extra entry (e.g. "7 minutes") so it is
  shown, not silently changed.

### Panels

#### Appearance
- **Style**: two toggle cards, *Default* and *Dark*, writing
  `org.gnome.desktop.interface color-scheme` `default` / `prefer-dark`.
  `prefer-light` (set by other tools) selects *Default* and is kept until the
  user picks a card.
- **Accent Color**: nine round swatches (blue, teal, green, yellow, orange,
  red, pink, purple, slate) → `accent-color`, wrapping onto a second line
  when narrow. The two style cards stack likewise.
- **Icons**: an *Icon Theme* row listing the installed icon themes by their
  `index.theme` `Name`, writing the theme's directory name to
  `icon-theme`. Themes are found in `$XDG_DATA_HOME/icons`, `~/.icons` and
  `icons` under each `$XDG_DATA_DIRS` entry (an earlier directory's theme
  hides a later one of the same name); `hicolor`, `default`, hidden themes
  and cursor-only themes (no `Directories`) are not listed. A value set by
  other tools that names no installed theme is listed as it is. Running apps
  follow the change.
- **Background**: a grid of thumbnails (at least two per line at 360 px,
  up to six) of the images in
  `/usr/share/backgrounds` (plus one level of subdirectories) and
  `$XDG_DATA_HOME/backgrounds`. Files named `<stem>-light.<ext>` /
  `<stem>-dark.<ext>` (also `-l` / `-d`) pair into one entry. Choosing an
  entry writes `org.gnome.desktop.background picture-uri` (light image) and
  `picture-uri-dark` (dark image, or the same image). The current entry is
  highlighted when either URI matches. Thumbnails load on worker threads.
- **Add Picture…**: file chooser filtered to images; the file is copied into
  `$XDG_DATA_HOME/backgrounds/` (name de-duplicated) and applied to both
  keys.

#### Mouse & Touchpad
- General: *Primary Button* Left/Right (`peripherals.mouse left-handed`).
- Mouse: *Pointer Speed* (`mouse speed`, −1…1), *Mouse Acceleration*
  (`accel-profile`: on = `default`, off = `flat`), *Natural Scrolling*
  (`mouse natural-scroll`).
- Touchpad: *Touchpad* on/off (`touchpad send-events` `enabled` /
  `disabled`), *Disable While Typing*, *Pointer Speed* (`touchpad speed`),
  *Secondary Click* (`click-method`: `fingers` = Two Finger Push, `areas` =
  Corner Push), *Tap to Click*, *Two-Finger Scrolling* / *Edge Scrolling*
  (`two-finger-scrolling-enabled` / `edge-scrolling-enabled`, mutually
  exclusive), *Natural Scrolling* (`touchpad natural-scroll`).

#### Keyboard
- **Input Sources**: `org.gnome.desktop.input-sources sources` (`a(ss)`)
  shown with human-readable names from `/usr/share/X11/xkb/rules/evdev.lst`
  (layouts and `layout+variant`; `ibus` sources show the engine id). Rows
  have *Move Up*, *Move Down* and *Remove*. *Add Input Source…* opens a
  searchable dialog of all XKB layouts and variants. *Switch input sources
  individually for each window* → `per-window`.
- **Typing**: *Repeat Keys* (`peripherals.keyboard repeat`), *Delay*
  (`delay`, 100–2000 ms) and *Speed* (`repeat-interval`; slider position
  *p* ∈ [0,1], right = faster, maps to `round(110 − 100·p)` ms). Both
  sliders are insensitive while repeat is off.

#### Accessibility
- Seeing: *High Contrast* (`a11y.interface high-contrast`), *Large Text*
  (`interface text-scaling-factor`: on = 1.25, off = 1.0; shown on when
  ≥ 1.25), *Cursor Size* (`cursor-size` 24/32/48/64/96), *Reduce Animation*
  (inverse of `interface enable-animations`), *Always Show Scrollbars*
  (inverse of `overlay-scrolling`), *Screen Reader*
  (`a11y.applications screen-reader-enabled`), *Zoom*
  (`screen-magnifier-enabled`).
- Typing: *Screen Keyboard* (`screen-keyboard-enabled`), *Cursor Blinking*
  (`interface cursor-blink`), *Sticky Keys*, *Slow Keys*, *Bounce Keys*
  (`a11y.keyboard stickykeys-enable` / `slowkeys-enable` /
  `bouncekeys-enable`).
- Pointing: *Double-Click Delay* (`peripherals.mouse double-click`
  100–1000 ms), *Locate Pointer* (`interface locate-pointer`), *Mouse Keys*
  (`a11y.keyboard mousekeys-enable`).
- *Always Show Accessibility Menu*
  (`a11y always-show-universal-access-status`).

#### Privacy & Security
- Screen Lock: *Blank Screen Delay* (`session idle-delay`: 1, 2, 3, 4, 5, 8,
  10, 12, 15 minutes, Never = 0), *Automatic Screen Lock*
  (`screensaver lock-enabled`), *Automatic Screen Lock Delay*
  (`screensaver lock-delay`: Screen Turns Off = 0, 30 seconds, 1, 2, 3, 5, 30
  minutes, 1 hour; insensitive while auto lock is off), *Lock Screen
  Notifications* (`notifications show-in-lock-screen`).
- File History & Trash: *File History* (`privacy remember-recent-files`),
  *File History Duration* (`recent-files-max-age` days: 1, 7, 30, Forever
  = −1), *Automatically Delete Trash Content* (`remove-old-trash-files`),
  *Automatically Delete Temporary Files* (`remove-old-temp-files`),
  *Automatically Delete Period* (`old-files-age` days: 1, 2, 3, 4, 5, 6, 7,
  14, 30).
- Location: *Location Services* (`org.gnome.system.location enabled`).
- Devices: *Camera* (inverse `privacy disable-camera`), *Microphone*
  (inverse `disable-microphone`).
- Diagnostics: *Automatic Problem Reporting* (`report-technical-problems`).

#### Notifications
- *Do Not Disturb* (inverse of `org.gnome.desktop.notifications
  show-banners`), *Lock Screen Notifications* (`show-in-lock-screen`).
- *Applications*: one switch row per id in `application-children`, bound to
  `enable` of the relocatable schema
  `org.gnome.desktop.notifications.application` at
  `/org/gnome/desktop/notifications/application/<id>/`. Names and icons come
  from the matching desktop file when found. The group is hidden when the
  relocatable schema is not installed or the list is empty.

#### Extensions
Helix shell extensions (helix-desktop-spec `contracts/extensions/README.md`),
through `org.helix.Extensions1` at `/org/helix/Extensions1` on the session
bus (owned by the shell, `org.gnome.Shell`). The shell's own "Extensions…"
menu entry opens this panel when Settings is installed.

- Until the first read of the list answers, the page shows a spinner
  labelled *Loading Extensions…* (also its accessible name).
- When reading the list fails (not a Helix session, or the shell not
  running) the page is a status page: *Extensions Are Not Available* /
  "They need the Helix shell." It is never shown before that answer. The
  list follows the service: it is read when the panel is built, again on
  every `Changed(id)` signal (at most every 0.3 s), and when the service
  appears or goes away.
- *Use Extensions*: a switch row, the inverse of `org.helix.extensions
  disable-all` (marked *Not supported by this system* without the schema).
- One expander row per extension, user and system ones by name, then
  development ones (with "Development" in the subtitle). Title: its name;
  subtitle: its version and state: *Running*, *Starting*, *Not running*,
  *Off*, *Off for this session* (Shift held at login), *Needs approval*,
  *Cannot run: <reason>*, *Stopped working* (with the last log line when
  there is one). A suffix switch is on when it is enabled and approved
  (development extensions have none: they run without enabling).
  - Turning it on calls `Enable(id, true)` on a worker thread, with no
    reply timeout: the shell may first ask the user (its approval prompt).
    The switch is insensitive meanwhile. Refused
    (`org.freedesktop.DBus.Error.AccessDenied`): the switch goes back off
    and a toast says "<name> was not turned on". Turning it off calls
    `Disable(id)`.
  - Rows inside: the description; *Access*: "Sandboxed" (with "Can: …"
    and the permission texts below), or "Full access: it can do anything
    you can" for one that is not sandboxed, plus the permission texts;
    *Problem* (invalid or crashed) with the reason and a *Show Log* button
    (a dialog with `GetLog(id)`, newest last); *Restart* (running or
    crashed) calls `Restart(id)`.
  - Its settings (`GetSettings(id)`, in that order), each written with
    `SetSetting(id, key, value)` when changed: `bool` a switch row;
    `int` a spin row (step 1, `min`/`max` when declared, else
    −2147483648…2147483647); `double` a spin row (two decimals; step a
    twentieth of `max` − `min` when both are declared, else 0.1); `enum` a
    combo row of its choices; `string` an entry row applied with its
    apply button; `string-list` an entry row of comma-separated values
    (spaces around them dropped, empty ones left out), applied the same
    way. The setting's `help` is the row's subtitle (entry rows: its
    tooltip). A refused value (`InvalidArgs`) gives a toast with the
    error and the row shows the stored value again.
  - Its commands (`commands` of `ListExtensions`): one row each, its
    title, with the key as a shortcut label ("None" when unbound) and
    *Change…*. *Change…* opens a dialog "Press the new keys for <title>";
    the first combination with Ctrl, Alt or Super (or a function key
    alone) is sent with `BindCommand(command, accelerator)` and closes it;
    Escape cancels; Backspace clears the key (`""`). A combination the
    shell refuses (in use, or does not parse) is shown in the dialog,
    which stays open.
- Changes made elsewhere (the shell, `helix-extensions`) show through
  `Changed`; the rows are updated in place, and rebuilt when the set of
  extensions or of their settings changes.

#### Multitasking
- General: *Hot Corner* (`interface enable-hot-corners`), *Active Screen
  Edges* (`org.gnome.mutter edge-tiling`).
- Workspaces: *Dynamic Workspaces* / *Fixed Number of Workspaces* (radio
  rows on `mutter dynamic-workspaces`), *Number of Workspaces*
  (`wm.preferences num-workspaces` 1–36, sensitive only when fixed).
- Multi-Monitor: *Workspaces on Primary Display Only*
  (`mutter workspaces-only-on-primary`).
- App Switching: *Include Apps From the Current Workspace Only*
  (`org.gnome.shell.app-switcher current-workspace-only`).

#### Power
- Battery (shown only when UPower reports a present battery; read-only):
  level bar, percentage, state (Charging, Discharging, Fully Charged, …) and
  time to empty / full, from the UPower `DisplayDevice`
  (`org.freedesktop.UPower.Device`: `IsPresent`, `Type`, `Percentage`,
  `State`, `TimeToEmpty`, `TimeToFull`), fetched on a worker thread when the
  panel is built and every 30 s afterwards.
- Power Saving: *Dim Screen* (`settings-daemon.plugins.power idle-dim`),
  *Screen Blank* (`session idle-delay`, same presets as Privacy),
  *Automatic Suspend* when plugged in / on battery: switch (type `suspend`
  vs `nothing`) + delay (`sleep-inactive-{ac,battery}-timeout`: 15, 20, 25,
  30, 45, 60, 80, 90, 100, 120 minutes). The battery row is shown only when a
  battery is present.
- General: *Power Button Behavior* (`power-button-action`: Suspend, Power
  Off = `interactive`, Hibernate, Nothing), *Show Battery Percentage*
  (`interface show-battery-percentage`).

#### Date & Time
- Reads `org.freedesktop.timedate1` (`Timezone`, `NTP`, `CanNTP`,
  `NTPSynchronized`) on a worker thread.
- *Automatic Date & Time* → `SetNTP(b, interactive=true)` (insensitive when
  `CanNTP` is false). *Automatic Time Zone* →
  `org.gnome.desktop.datetime automatic-timezone`.
- *Date & Time* row shows the current time in the system time zone, updated
  every second, in the selected clock format. Setting the clock manually
  (`SetTime`) is not offered yet.
- *Time Zone* opens a searchable dialog of time zones (from
  `ListTimezones`, falling back to `/usr/share/zoneinfo/zone1970.tab`);
  choosing one calls `SetTimezone(tz, interactive=true)`. Errors (e.g. polkit
  denial) are shown as a toast and the displayed value is reloaded.
- *Time Format* 24-hour / AM/PM (`interface clock-format`).
- Clock & Calendar: *Week Day*, *Date*, *Seconds*
  (`interface clock-show-weekday` / `clock-show-date` /
  `clock-show-seconds`), *Week Numbers*
  (`org.gnome.desktop.calendar show-weekdate`).

#### About
- *Device Name* (`org.freedesktop.hostname1` `PrettyHostname`, else
  `Hostname`) in an entry row; applying calls `SetPrettyHostname(name, true)`
  and `SetStaticHostname(static, true)` where *static* is the name
  lower-cased, spaces/underscores → `-`, other characters outside
  `[a-z0-9-]` dropped, repeated `-` collapsed, leading/trailing `-` trimmed,
  at most 64 characters (empty → `localhost`).
- Hardware model (`HardwareVendor` + `HardwareModel`, falling back to
  `/sys/class/dmi/id/{sys_vendor,product_name}`), memory (`MemTotal` of
  `/proc/meminfo`), processor (`model name` of `/proc/cpuinfo` × logical
  CPUs), disk capacity (`filesystem::size` of `/`), OS name (`PRETTY_NAME`
  of `/etc/os-release`, else `NAME VERSION`), OS type (64-bit / 32-bit),
  desktop (`XDG_CURRENT_DESKTOP`), windowing system (Wayland / X11 from the
  GDK display), kernel (`/proc/sys/kernel/osrelease`).
- *Copy* puts all of it on the clipboard as text and confirms with a
  "System information copied" toast (which screen readers speak).

### D-Bus
- All D-Bus traffic uses `zbus` (blocking API) on worker threads; results
  return to the GTK thread over `async-channel`. Missing services leave the
  affected rows showing *Unavailable*.
- Mutating system calls (`SetTimezone`, `SetNTP`, `SetPrettyHostname`,
  `SetStaticHostname`) are issued only in response to an explicit user
  action, with `interactive = true` so polkit can prompt.

### Shortcuts
`Ctrl+F` search, `Escape` close search, `Ctrl+Q` quit, `Ctrl+W` close
window, `F1` about.

### Command line
- `helix-settings-app [PANEL]` opens (or raises the running instance on)
  the given panel; ids and aliases as in the registry table.
- `helix-settings-app --list` prints `id<TAB>title<TAB>available|unavailable`
  for every panel and exits.
- Unknown panel names print an error with the list of ids and exit with
  status 2.
- `-h` / `--help` prints usage.

### Follow-up panels (listed, not yet available)
- **Wi-Fi / Network**: NetworkManager D-Bus (`org.freedesktop.NetworkManager`)
  for devices, connections and VPN; connection editor dialogs; proxy via
  `org.gnome.system.proxy`.
- **Bluetooth**: BlueZ (`org.bluez` Adapter1 / Device1, pairing through an
  `Agent1`).
- **Displays**: `org.gnome.Mutter.DisplayConfig` (`GetCurrentState` /
  `ApplyMonitorsConfig`, with a revert-countdown confirmation), night light
  via `org.gnome.settings-daemon.plugins.color`.
- **Sound**: PipeWire (or the PulseAudio-compatible API) for devices and
  volumes; `org.gnome.desktop.sound` for alert sounds and
  `allow-volume-above-100-percent`.
- **Users**: `org.freedesktop.Accounts`.
- **Apps**: default applications (`gio::AppInfo` defaults) and per-app
  permissions through the permission store.
- Not listed yet: Region & Language, Online Accounts, Sharing, Printers,
  Search, Color, Wacom, power mode (`org.freedesktop.UPower.PowerProfiles`).

## Decisions
- Style cards: no third *Light* card; match GNOME with *Default* and *Dark*
  only (`prefer-light` set elsewhere shows as *Default* and is kept).
- Focus is kept across list rebuilds (`helix_app_common::focus`): Input sources: Move Up / Move Down keep focus on the moved source's button (so repeated presses keep moving it; at the top or bottom focus stays in the row), Remove moves focus to the neighbouring source's Remove button, or to *Add Input Source…* when no source is left.
- Per-row buttons name their row for assistive technology: "Move English
  (US) Up", "Remove English (US)"; an extension's buttons "Show Log for X",
  "Restart X" and "Change Shortcut for Command (X)".
