# Image Viewer

Drop-in replacement for Loupe.

- **App ID**: `org.helix.ImageViewer`
- **Binary**: `helix-image-viewer`
- **Crate**: `apps/image-viewer`

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
- Printing (out of scope for now; no menu entry).

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow`. Inside: a toast overlay, an
  `AdwOverlaySplitView` whose (end-side) sidebar is the **Properties** panel,
  and an `AdwToolbarView` with a header bar on top of the image area.
- Header bar: *Open* button (start), a window title showing the file name
  with `n / total` position in the folder as subtitle, and (end) the primary
  menu and a *Properties* toggle button.
- Several windows are possible (`app.new-window`, Ctrl+N); each has its own
  image list.

### Opening images
- **Command line**: `helix-image-viewer [FILE|DIR|URI ...]`.
  - One file: the file is shown; the image list is every supported image in
    the same folder.
  - Several files: the image list is exactly those files (in argument order,
    unsupported ones dropped).
  - A directory: its first image (natural sort) is shown, list = folder.
  - `-h/--help` prints usage. Unknown files show an error page.
- **Open dialog** (Ctrl+O, header button, empty-state button): a
  `gtk::FileDialog` (portal FileChooser when available) filtered to images,
  multi-select allowed with the same single/multiple rules as above.
- **Drag and drop**: dropping one or more files onto the window opens them
  with the same rules.
- With no image, an **empty state** (`AdwStatusPage`, “View Images”, with an
  *Open Files…* button and a hint that images can be dropped) is shown.

### Decoding
- Decoding runs on a worker thread (`gio::spawn_blocking`); the UI shows a
  spinner while loading and an error status page if decoding fails. Results
  of stale loads (user already navigated away) are discarded.
- Primary decoder: **gdk-pixbuf** (system loaders: PNG, JPEG, GIF, TIFF, BMP,
  ICO, TGA, PNM, XPM, SVG, WebP/HEIF/AVIF when the loaders are installed).
- Secondary decoder: the pure-Rust **`image` crate** restricted to GIF, WebP
  and QOI. Rationale: it gives proper per-frame delays for **animated GIF and
  animated WebP** (pixbuf's animation iterator is wall-clock driven and makes
  frame extraction awkward), and it covers WebP/QOI when pixbuf loaders for
  them are missing. No other formats are enabled, to keep the dependency
  footprint small.
- Animated images play in a loop using each frame's delay (minimum 20 ms,
  0-delay frames use 100 ms). Animations whose decoded frames would exceed
  512 MiB show only the first frame.
- **EXIF orientation** (tag 0x0112, values 1–8) is read with `kamadak-exif`
  and applied to the decoded pixels for JPEG, TIFF, PNG and WebP. HEIF/AVIF
  are skipped because their container transforms are already applied by the
  decoder. Displayed dimensions are the oriented ones.

### Supported files / image list
- Supported extensions are those of the installed pixbuf formats plus
  `gif`, `webp`, `qoi` (case-insensitive).
- The folder list contains non-hidden regular files with supported
  extensions (the currently opened file is always included), sorted by
  **natural sort**: case-insensitive, digit runs compared numerically
  (`img2` < `img10`), ties broken by the raw name.
- The folder is **watched** (`gio::FileMonitor`); additions, removals and
  renames refresh the list while keeping the current image. If the current
  image disappears, the image that took its place in the list is shown (or
  the empty state if none remain).
- Navigation does not wrap around at the ends.

### Viewing
- **Best fit** by default: the image is scaled down to fit the view, never
  scaled up beyond 100 %.
- Zoom range: from min(best-fit, 100 %) down limit to 2000 %. Zoom steps
  multiply/divide by 1.25 (snapping to 100 % and best fit when crossing them).
- Zoom in/out keeps the point under the pointer (scroll/pinch) or the view
  centre (keyboard/buttons) fixed.
- When the scaled image is larger than the view it can be **panned** by
  dragging (grab cursor), by scrolling without Ctrl, or kept centred
  otherwise. Pan offsets are clamped so no blank space appears beyond the
  image edges.
- Double-click toggles between best fit and 100 % around the pointer.
- **Rotate** left/right (90° steps) changes the view only; the file is never
  modified. Rotation is reset when another image is shown.
- Scaling filter: trilinear when shrinking, linear up to 300 %, nearest
  neighbour above (so pixels are visible when zoomed far in).
- Overlay controls on the image (previous / next buttons at the sides; a
  bottom bar with rotate left/right, zoom out, zoom level menu-button with
  best fit / 100 % / 200 %, zoom in) are revealed on pointer motion and
  hidden after 2 s of inactivity.
- **Fullscreen** (F11, or menu): the header bar and controls auto-hide after
  2 s without pointer motion (cursor hidden too) and come back on motion.
  Esc leaves fullscreen.
- **Slideshow** (F5): enters fullscreen and advances every 5 s; stops at the
  last image or on Esc / F5.

### Properties panel (F9 / Alt+Return / header toggle)
- Image: dimensions (`W × H` and megapixels), format, file size
  (`glib::format_size`), folder (with a button that opens it),
  created/modified dates (file system).
- EXIF section (shown only when present): date taken, camera (make + model,
  make not repeated if model starts with it), exposure (`1/250 s`),
  aperture (`ƒ/2.8`), ISO, focal length (`50 mm`), GPS location
  (`48.85840° N, 2.29450° E`, with a button opening a `geo:` URI via
  `gtk::UriLauncher`).

### Actions (primary menu and shortcuts)
| Action | Shortcut | Behaviour |
|---|---|---|
| Open… | Ctrl+O | Open dialog |
| New Window | Ctrl+N | Empty window |
| Open With… | Ctrl+Shift+O | `gtk::FileLauncher` with *always ask* (OpenURI portal) |
| Copy | Ctrl+C | Current (oriented, unrotated) frame to the clipboard as a texture |
| Set as Background… | — | XDG Wallpaper portal `SetWallpaperURI(parent, uri, {show-preview: true, set-on: "background"})` over zbus on a worker thread; waits for the `Request::Response` signal and toasts the result |
| Move to Trash | Delete | `gio::File::trash`, image removed from list, toast “Image moved to trash” with **Undo** which restores the file from the freedesktop trash (`$XDG_DATA_HOME/Trash`, or `$topdir/.Trash-$uid` / `$topdir/.Trash/$uid`, matching the `.trashinfo` `Path` with the latest `DeletionDate`) |
| Previous / Next | Left / Right, Page Up / Page Down | Navigate list (prev/next overlay buttons too) |
| First / Last | Home / End | |
| Zoom in / out | Ctrl+= / Ctrl++ / Ctrl+KP_Add, Ctrl+- / Ctrl+KP_Subtract, Ctrl+scroll, pinch | |
| Best fit | Ctrl+0 | |
| 100 % / 200 % | Ctrl+1 / Ctrl+2 | |
| Rotate right / left | Ctrl+R / Ctrl+Shift+R | View only |
| Properties | F9, Alt+Return | Toggle sidebar |
| Fullscreen | F11 | Toggle; Esc leaves |
| Slideshow | F5 | Toggle |
| Close window / Quit | Ctrl+W / Ctrl+Q | |

### GSettings
None. The `org.gnome.Loupe` schema is not read: nothing the app currently
persists has an established public key (see open questions), so no schema is
shipped.

### Command line
`helix-image-viewer [FILE|DIR|URI ...]` (see *Opening images*).

## Deferred
- Thumbnail sidebar / strip (listed in the stub). The image list and
  navigation exist; a thumbnail strip needs a thumbnailer cache design.
- Saving a rotated image back to disk.
- Printing.
- Preloading neighbouring images, colour management, HDR.

## Open questions
- **Save rotation?** Rotation is view-only. Should Helix offer “Save” of the
  rotated image (rewriting the EXIF orientation tag for JPEG losslessly, or
  re-encoding other formats)? Loupe historically did not modify files.
- **Thumbnail sidebar**: the stub lists one but Loupe has none. Keep, drop,
  or implement as an optional bottom strip?
- **GSettings**: Is there a Loupe key worth carrying over (e.g. a
  “show properties” flag)? None is used today.
