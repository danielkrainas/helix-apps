# Terminal

Drop-in replacement for GNOME Console (kgx).

## Purpose
Terminal emulator.

## Main windows
- Terminal window with tabs.
- Preferences (font, theme, bell).

## Contracts used
- VTE or equivalent terminal widget.
- XDG portals: OpenURI.
- GSettings: `org.gnome.Console` or equivalent.
- Standard session environment.

## Non-goals
- Full GNOME Terminal profile compatibility on day one.
