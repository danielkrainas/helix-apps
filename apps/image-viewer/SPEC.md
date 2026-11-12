# Image Viewer

Drop-in replacement for Loupe.

## Purpose
View images.

## Main windows
- Image view with zoom, rotate, and slideshow.
- Thumbnail sidebar.

## Contracts used
- XDG portals: FileChooser, OpenURI, Wallpaper.
- GSettings: `org.gnome.Loupe` or equivalent.
- Standard image formats via gdk-pixbuf or glycin.

## Non-goals
- Full editor feature set.
