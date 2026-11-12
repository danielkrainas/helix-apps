# Software

Drop-in replacement for GNOME Software.

## Purpose
Browse, install, and update applications and system updates.

## Main windows
- Explore / Installed / Updates views.
- App details page.
- Update progress and reboot prompts.

## Contracts used
- PackageKit / AppStream.
- Flatpak (optional but expected).
- XDG portals: Background, Notification, DynamicLauncher.
- GSettings: `org.gnome.software`.

## Non-goals
- Exact feature parity with every GNOME Software plugin on day one.
