# Helix Apps

Companion application suite for [Helix](https://github.com/danielkrainas/helix-desktop).

These apps are intended as a drop-in replacement for the GNOME core application suite. They use only public contracts (XDG portals, GSettings, standard session D-Bus interfaces) so they can be developed and tested against a normal GNOME session and run unchanged under Helix.

The core desktop (session, compositor, shell, portals) lives in [helix-desktop](https://github.com/danielkrainas/helix-desktop). The living specification lives in [helix-desktop-spec](https://github.com/danielkrainas/helix-desktop-spec).

## Target suite

The list below matches the current GNOME core apps (as published on apps.gnome.org). Each entry is a candidate for a Helix implementation.

| App | GNOME name | Purpose |
|-----|------------|---------|
| Files | Nautilus | File manager |
| Settings | GNOME Settings | Desktop configuration |
| Software | GNOME Software | App installation and updates |
| Text Editor | GNOME Text Editor | Text editing |
| Terminal | Console (kgx) | Terminal emulator |
| Calculator | GNOME Calculator | Calculator |
| Calendar | GNOME Calendar | Calendar and scheduling |
| Contacts | GNOME Contacts | Contact management |
| Clocks | GNOME Clocks | Clocks, alarms, timers |
| Weather | GNOME Weather | Weather |
| Maps | GNOME Maps | Maps |
| Characters | GNOME Characters | Character map |
| Camera | Snapshot | Camera |
| Image Viewer | Loupe | Image viewer |
| Document Viewer | Papers | Document (PDF) viewer |
| Document Scanner | Simple Scan | Scanner |
| Audio Player | Decibels | Audio playback |
| Video Player | Showtime | Video playback |
| Music | GNOME Music | Music library |
| Disks | gnome-disk-utility | Disk management |
| Disk Usage Analyzer | Baobab | Disk usage |
| System Monitor | GNOME System Monitor | System resources |
| Logs | GNOME Logs | System logs |
| Fonts | gnome-font-viewer | Font viewer |
| Help | Yelp | Help viewer |
| Connections | GNOME Connections | Remote desktop client |
| Web | Epiphany | Web browser |
| Tour | GNOME Tour | Welcome / tour |

Beyond the core list:

| App | GNOME name | Purpose |
|-----|------------|---------|
| Screenshot | GNOME Screenshot (gnome-screenshot) | Screenshots of the screen, a window or an area |

## Notes

- **Settings** is the largest and most tightly integrated app. It can be developed in parallel but will exercise more of the portal and session surface than the others.
- Smaller utilities (Calculator, Characters, Clocks, Weather, Fonts, etc.) are good first targets.
- Apps that are primarily front-ends to existing system services (Disks, Logs, System Monitor, Software) can largely reuse those services.
- The suite is not required for Helix to host existing GTK / libadwaita applications. It is the set that makes a Helix session feel complete on its own.

## Building and packages

The apps form one Cargo workspace (gtk4-rs + libadwaita; GTK 4.14 and
libadwaita 1.5 or newer). The build needs the GTK 4, libadwaita,
GStreamer, VTE (GTK 4), WebKitGTK 6.0, poppler-glib, libshumate and SANE
development packages. `.github/workflows/ci.yml` lists the Ubuntu names.

- **CI** (`.github/workflows/ci.yml`): clippy with warnings as errors; the
  tests, run headless on a private D-Bus session; the Evolution Data Server
  tests; and schema, desktop-file and packaging-layout checks.
- **Tests and D-Bus**: run the tests as CI does with
  `tools/testing/run-tests.sh cargo test --workspace` (headless, in-memory
  GSettings, a private session bus that can start no services). A test that
  needs its own bus uses `helix_app_common::test_bus::TestBus`, which loads
  no service files (or only the test's own fakes), so a call to a missing
  service fails instead of starting the real one;
  `tools/testing/check-test-buses.sh` (in CI) rejects buses started any
  other way.
- **Release** (`.github/workflows/release.yml`): a `vX.Y.Z` tag that matches
  the workspace version builds the `helix-apps` package as .deb, .rpm and
  tarball (x86_64, aarch64) and as an Arch Linux package (x86_64), and
  publishes them as a GitHub release.
- **Installed layout** (`packaging/stage.sh`):
  - binaries in `/usr/bin`, plus System Monitor's privileged helper in
    `/usr/libexec/helix-system-monitor/`;
  - desktop files, the D-Bus service file and the polkit policy in their
    standard places;
  - Tour's and Clocks' autostart entries in `/etc/xdg/autostart`;
  - each app's icon (`org.helix.<App>.svg`) in
    `/usr/share/icons/hicolor/scalable/apps`;
  - the Web and Help AppArmor profiles in `/etc/apparmor.d`.

  Helix's copies of the GNOME app schemas go in
  `/usr/share/helix-apps/schemas`, not the global schema directory, so
  they never conflict with installed GNOME apps. The apps use the system's
  schema when it exists and fall back to these copies.

## App icons

Every app ships its own scalable icon, named after its application id
(`apps/<app>/data/icons/org.helix.<App>.svg`), so launchers, the dock,
About dialogs and window lists show it whatever icon theme is in use.
They share one Helix style: a rounded tile with a diagonal gradient in
the app's colour and a white symbol with a soft shadow, drawn from plain
shapes so they stay legible at 24 pixels. They are generated: edit
`tools/icons/make_icons.py` and run `python3 tools/icons/make_icons.py`.

## Symbolic icons

Apps only use symbolic icon names that exist on a stock system: the
Adwaita icon theme (Helix's default), hicolor, or Helix's own set. Helix
draws its own 16 px symbolics for what Adwaita lacks (globe, stopwatch,
timer, speedometer, QR code, crop, synchronizing) in
`crates/helix-app-common/icons/scalable/actions/`; they are compiled
into a GResource at build time (needs `glib-compile-resources`) and added
to the icon theme by `install_standard_actions` at startup, so a theme
that has its own icon of the same name still wins. They are plain filled
shapes in `#2e3436` so GTK can recolour them. The `icon_names` test of
`helix-app-common` fails when any `"…-symbolic"` literal in an app
resolves to none of these. Notification icons are drawn by the shell,
which cannot see the bundled set, so they use Adwaita names only.

## Accessibility

Shared helpers live in `helix_app_common::a11y`.

- **Every button has an accessible name.** A tooltip is only the
  accessible *description*, so icon-only buttons are built with
  `icon_button` / `icon_toggle_button` / `icon_menu_button`, or get
  `.labelled("…")` / `set_accessible_label` after their builder. Names
  are short and leave out keyboard shortcuts: `.with_shortcut(label,
  accel)` / `set_label_and_shortcut` names the button `label` and shows
  "label (Ctrl+F)" as its tooltip (`shortcut_tooltip` for the tooltip
  alone), so no name is cut out of translated text. Buttons whose
  meaning follows a state (Play / Pause) change icon, tooltip and name
  together with `set_icon_and_label`. Toggle buttons keep one name and expose their
  state as "pressed". Controls outside an `Adw*Row` that sit next to a
  visible label are tied to it with `label_widget`.
- **Status changes are announced.** Changes shown without focus moving
  (search result counts, long operations finishing, playback and
  loading errors, timers going off, new tracks) are spoken with
  `announce` (polite for results, assertive only for failures and
  alarms), `announce_change` (only when the text differs from the last
  one for that widget) or `announce_settled` (after typing pauses, for
  search counts). Announce outcomes, not every progress tick; repeats
  within two seconds are dropped. Toasts and dialogs need nothing extra.
- **Rebuilt lists keep focus.** Lists rebuilt after a change tag rows
  with a stable key and focusable controls with a part name
  (`helix_app_common::focus::set_key` / `set_part`) and rebuild inside
  `rebuild_keeping_focus`: focus returns to the same control of the same
  row (or a neighbour after a removal) and the scroll position is kept;
  background refreshes never move focus. `remove_row_keeping_focus`
  does the same for a single removed `ListBox` row.
- **Checking.** Run any app with `HELIX_A11Y_CHECK=1` to list, on
  standard error, every visible button without an accessible name (the
  window is re-checked every two seconds; each problem is printed once)
  and every announcement made.

## Translations

The apps are translated into Spanish, French, German, Brazilian
Portuguese, Russian, Simplified Chinese, Japanese, Italian, Polish,
Dutch, Korean and Ukrainian. These first translations are machine
translations that no native speaker has reviewed yet; corrections and
reviews are welcome. See [TRANSLATING.md](TRANSLATING.md) for each
language's status and how to help.

## License

CC0-1.0 (same as the rest of Helix).
