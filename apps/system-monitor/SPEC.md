# System Monitor

Drop-in replacement for GNOME System Monitor.

## Purpose
View processes, resources, and file systems.

## Main windows
- Processes, Resources, and File Systems tabs.
- Process details and kill.

## Contracts used
- /proc and standard system interfaces.
- Polkit for privileged process actions.
- GSettings: `org.gnome.gnome-system-monitor`.

## Non-goals
- Exact graph rendering of the original.
