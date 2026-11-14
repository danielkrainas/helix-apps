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
  - the Web and Help AppArmor profiles in `/etc/apparmor.d`.

  Helix's copies of the GNOME app schemas go in
  `/usr/share/helix-apps/schemas`, not the global schema directory, so
  they never conflict with installed GNOME apps. The apps use the system's
  schema when it exists and fall back to these copies.

## License

CC0-1.0 (same as the rest of Helix).
