# Settings

Drop-in replacement for GNOME Settings.

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
