# Connections

Drop-in replacement for GNOME Connections.

## Purpose
Remote desktop client (RDP / VNC).

## Main windows
- Connection list.
- Remote session view.
- Connection editor.

## Contracts used
- FreeRDP / VNC libraries.
- XDG portals: FileChooser, OpenURI.
- GSettings: `org.gnome.Connections`.
- Secret service for saved passwords.

## Non-goals
- Full feature parity with every remote protocol on day one.
