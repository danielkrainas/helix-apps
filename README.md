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

## Notes

- **Settings** is the largest and most tightly integrated app. It can be developed in parallel but will exercise more of the portal and session surface than the others.
- Smaller utilities (Calculator, Characters, Clocks, Weather, Fonts, etc.) are good first targets.
- Apps that are primarily front-ends to existing system services (Disks, Logs, System Monitor, Software) can largely reuse those services.
- The suite is not required for Helix to host existing GTK / libadwaita applications. It is the set that makes a Helix session feel complete on its own.

## License

CC0-1.0 (same as the rest of Helix).
