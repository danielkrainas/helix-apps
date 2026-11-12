# Files

Drop-in replacement for GNOME Files (Nautilus).

## Purpose
File manager for local and remote locations.

## Main windows
- Browser window with sidebar (places, bookmarks, devices), path bar, view (icon/list/grid), and status bar.
- Multiple windows and tabs.
- Preferences dialog.

## Contracts used
- XDG portals: FileChooser (when acting as chooser), OpenURI, Trash, Background.
- GSettings: `org.gnome.nautilus.preferences`, `org.gnome.nautilus.window-state`, `org.gnome.nautilus.list-view`.
- GVFS for remote and virtual locations.
- Standard freedesktop trash and recent-files specs.

## Non-goals
- Exact visual clone of current Nautilus.
- Extensions API compatibility.
