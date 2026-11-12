# Web

Drop-in replacement for Epiphany (GNOME Web).

## Purpose
Web browser.

## Main windows
- Browser window with tabs, bookmarks, and reader mode.
- Preferences and history.

## Contracts used
- WebKitGTK.
- XDG portals: FileChooser, OpenURI, Notification, Settings.
- GSettings: `org.gnome.Epiphany`.
- Secret service for passwords.

## Non-goals
- Full extension ecosystem.
- Exact UI clone.
