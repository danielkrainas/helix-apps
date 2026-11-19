# Document Scanner

Drop-in replacement for Simple Scan.

- **App ID**: `org.helixdesktop.DocumentScanner`
- **Binary**: `helix-document-scanner`
- **Crate**: `apps/document-scanner`

## Purpose
Scan documents and photos.

## Main windows
- Scanner selection and preview.
- Page list and save/export.

## Contracts used
- SANE backend.
- XDG portals: FileChooser, Print.
- GSettings: `org.gnome.SimpleScan`.

## Non-goals
- Advanced image correction on day one.

---

## Behaviour (derived spec)

### Scanner access (SANE)
- SANE (`libsane.so.1`, API 1.0) is used through small local FFI bindings
  (`sane_init`, `get_devices`, `open`, `get_option_descriptor`,
  `control_option`, `start`, `get_parameters`, `read`, `cancel`, `close`,
  `exit`). No Rust SANE crate is used.
- All SANE calls run on one **worker thread** that owns `sane_init` for the
  process lifetime. The UI sends commands (refresh devices, open device,
  scan, shut down) over a channel; the worker sends events back (device
  list, device capabilities, scan progress, finished page, errors, scan
  finished). Cancelling sets a shared flag that the worker checks between
  `sane_read` calls and then calls `sane_cancel`.
- Device enumeration happens on start-up and on *Refresh* (network devices
  included). Enumeration only reads; it never changes device settings.
- When a device is selected the worker opens it and reports its
  capabilities: available sources (`source` option), colour modes (`mode`),
  and resolutions (`resolution` word list, or a ladder of common values
  75/100/150/200/300/400/600/1200 inside a range constraint).
- Option mapping when scanning (only options that exist, are active and
  settable are touched):
  - `source`: the user's chosen source; *Automatic* picks a flatbed-like
    source for single/flatbed scans and a feeder source for feeder scans
    (duplex feeder when `page-side` is `both`, a back-side feeder for
    `back`, otherwise the plain/front feeder).
  - `mode`: colour → `Color`/`Colour`/`RGB`…, grayscale → `Gray`/
    `Grayscale`…, black & white → `Lineart`/`Binary`/`Black & White`…
    (case-insensitive; falls back lineart → gray → colour).
  - `resolution` (INT or FIXED): nearest supported value to the requested
    DPI.
  - Scan area `tl-x/tl-y/br-x/br-y` (mm): automatic paper size scans the
    whole area; a fixed paper size scans from the top-left corner, clamped.
  - `brightness` / `contrast`: only when the setting is non-zero, mapped
    linearly from −100…100 onto the option's range around its midpoint.
- Frames: grey and RGB, plus three-pass R/G/B frames; depths 1 (lineart,
  1 = black), 8 and 16 (reduced to 8 bits). Unknown line counts (`lines`
  = −1) are handled by reading to EOF. The frame size the backend reports
  (from the device itself for `net` and `airscan`) is not trusted: a
  negative or overflowing size counts as unknown, at most 256 MiB is
  reserved up front (a larger frame grows as data arrives), and a read
  reporting more bytes than the buffer holds is cut to the buffer. Option
  value buffers are rounded up to whole words, never smaller than the
  option's reported size.
- Scan types:
  - **Single Page** (`Ctrl+1`): one page.
  - **All Pages From Feeder** (`Ctrl+F`): pages until the backend reports
    *no documents*.
  - **Multiple Pages From Flatbed** (`Ctrl+M`): repeats, waiting
    `page-delay` ms between pages, until stopped.
  - **Stop** (`Escape`) cancels.
- Each page records its resolution and whether it was scanned as text or
  photo.

### Window
- `AdwApplicationWindow` with header bar:
  - start: **Scan** button with a menu (scan types, *Text*/*Image* document
    type radio, Preferences) and a **Stop** button while scanning;
  - title: "Document Scanner", subtitle = selected scanner;
  - end: primary menu (New Document, Reorder Pages, Copy Page, Print,
    Email, Refresh Scanners, Preferences, Keyboard Shortcuts, About) and a **Save** button.
- Content states:
  - *Searching for scanners…* (spinner) while enumerating;
  - *No Scanners Detected* with a Refresh button;
  - *Ready to Scan* with a scanner drop-down (when the document is empty);
  - the **document**: a page list (thumbnails with page numbers) on the
    left and the selected page large on the right.
- A progress bar is shown at the top of the content while scanning; errors
  appear as toasts (and a dialog for device errors).
- Bottom action bar for the selected page: Rotate Left (`[`), Rotate Right
  (`]`), Crop menu, Move Left (`Ctrl+Page Up`), Move Right
  (`Ctrl+Page Down`), Delete (`Delete`). The page view has the same
  actions in its right-click menu.

### Document and pages
- Pages are kept in memory as 8-bit grey or RGB pixels.
- **Rotate** left/right rotates the pixels 90° (the crop rectangle rotates
  with the page).
- **Crop** is non-destructive: the crop menu offers *None*, *Custom*,
  *A4*, *A5*, *A6*, *US Letter*, *US Legal*, *4×6* and *Rotate Crop*
  (swap orientation). Preset sizes are computed from the page resolution
  and centred, clamped to the page. The crop rectangle is drawn on the page
  view (darkened outside) and can be moved by dragging inside it and resized
  by dragging its edges/corners. The crop is applied when saving, printing
  and copying.
- **Reorder** dialog: *Combine sides* (interleave first half with second
  half), *Combine sides (reverse)* (interleave with the second half
  reversed), *Reverse*, *Flip odd pages upside down*.
- **Delete** removes the selected page; **New Document** (`Ctrl+N`) clears
  all pages (asking first if unsaved).
- Closing the window with unsaved pages asks *Save / Discard / Cancel*.

### Saving
- **Save** (`Ctrl+S`) opens the file chooser (portal-backed `GtkFileDialog`)
  in `save-directory` (default: XDG Documents) with the name
  "Scanned Document.<ext>" where the extension follows `save-format`.
  The chosen extension decides the format (`.pdf`, `.png`, `.jpg`/`.jpeg`);
  no/unknown extension → `save-format`, and the extension is appended.
- **PDF**: a pure-Rust writer (no PDF crate). One page per scanned page,
  media box from pixel size and DPI; photo pages are embedded as JPEG
  (`DCTDecode`, quality `jpeg-quality`), text pages lossless (`FlateDecode`).
- **PNG / JPEG**: a single page is saved as the chosen name; several pages
  are saved as `name-1.ext`, `name-2.ext`, … JPEG records the DPI.
- Encoding runs off the main thread. After saving, `save-directory` and
  `save-format` are updated and the document is marked clean.
- **Copy Page** (`Ctrl+C`) copies the selected (cropped) page to the
  clipboard as an image; **Print** (`Ctrl+P`) prints all pages, each scaled
  to fit the printable area.

### Post-processing (Simple Scan compatible)
- Keys (copied exactly from the installed `org.gnome.SimpleScan` schema):
  `postproc-enabled` (b, `false`), `postproc-script` (s, `''`),
  `postproc-arguments` (s, `''`), `postproc-keep-original` (b, `false`).
- When enabled and a script is set, after every successful **Save** the
  script runs once per written file (PDF: one file; multi-page PNG/JPEG:
  each `name-N.ext`) as
  `SCRIPT MIME-TYPE KEEP-ORIGINAL FILE [ARGUMENTS…]`:
  1. MIME type of the saved file (`application/pdf`, `image/png`,
     `image/jpeg`);
  2. `true`/`false` from `postproc-keep-original`;
  3. absolute path of the saved file;
  4. the words of `postproc-arguments`.
  The order follows Simple Scan's documented script interface (general
  knowledge of its help/release notes; not verified against source).
- Executed directly (`std::process::Command`, no shell): arguments are split
  with shell-style quoting (`g_shell_parse_argv`: quotes and backslashes
  group words) but nothing is expanded or executed (`$HOME`, `;`, `|` stay
  literal). A leading `~/` in the script path is expanded to the home
  directory; a bare name is looked up in `PATH`. stdin is `/dev/null`.
- With keep-original on, Helix first copies the saved file to
  `NAME_orig.EXT` (the `_orig` suffix the schema documents) so the original
  survives whatever the script does; the flag is still passed so scripts
  written for Simple Scan behave the same.
- Runs on a worker thread (`gio::spawn_blocking`), no timeout (OCR can be
  slow); a failure (cannot start, non-zero exit) is shown as a toast.
- Preferences → *Post-processing* group: *Enable Post-processing* switch,
  *Script* entry (apply button + file chooser button), *Script Arguments*
  entry (marked as error and not stored when the quoting is invalid),
  *Keep Original File* switch; the rows are insensitive while disabled.

### Email (`Ctrl+E`)
- Saves the document into a new private (0700) folder
  `$XDG_CACHE_HOME/helix-document-scanner/email/<pid>-<time>/` as
  "Scanned Document.pdf" for the PDF `save-format`, or one image per page
  ("Scanned Document[-N].png/.jpg") for the image formats (encoding off
  the main thread). Folders older than a day are removed on the next Email.
- Calls `org.freedesktop.portal.Email.ComposeEmail(s parent_window,
  a{sv} options)` on `org.freedesktop.portal.Desktop`
  (`/org/freedesktop/portal/desktop`) with `attachment_fds` (`ah`,
  read-only fds of the files) and `subject` "Scanned Document"; the parent
  window handle is empty. If the portal call fails (portal absent), runs
  `xdg-email --subject "Scanned Document" --attach FILE…` (argv, no shell).
  Errors are shown as a toast. Mail is never sent by Helix; the user's mail
  client only opens a compose window.
- Tested with a mock `org.freedesktop.portal.Email` on a private
  `dbus-daemon` without service activation (checks fds and contents).

### Preferences (AdwPreferencesDialog, `Ctrl+,`)
- Scanning: Scanner, Source (Automatic + device sources), Color Mode
  (Automatic, Color, Grayscale, Black & White), Scan Sides (front/back/
  both), Page Size (Automatic, A6, A5, A4, Letter, Legal, 4×6), Text
  Resolution, Image Resolution, Brightness, Contrast, Delay between pages.
- Saving: JPEG quality.
- Post-processing: see above.
- *Automatic* colour mode means grayscale for Text and colour for Image
  documents.

### GSettings (`org.gnome.SimpleScan`)
Keys read and written (others ignored; the bundled schema contains only
these, copied with the original names, types and defaults):
`selected-device` (s), `document-type` (s: `text|photo`), `paper-width`
(i, tenths of mm, 0 = automatic), `paper-height` (i), `brightness` (i),
`contrast` (i), `text-dpi` (i, 150), `photo-dpi` (i, 300), `page-side`
(enum `front|back|both`), `save-directory` (s), `save-format` (s, MIME),
`jpeg-quality` (i 0–100, 75), `page-delay` (i ms 0–15000, 1000),
`postproc-enabled` (b, false), `postproc-script` (s, ''),
`postproc-arguments` (s, ''), `postproc-keep-original` (b, false).
`save-directory` is written as a `file://` URI; a plain path is accepted
when read. Checked against the installed
`/usr/share/glib-2.0/schemas/org.gnome.SimpleScan.gschema.xml`: type `s`,
default `''`, summary "Directory to save files to", description "The
directory to save files to. Defaults to the documents directory if
unset." The schema does not specify path vs. URI, so it does not
contradict storing a URI (and the user's own value is unset, `''`). The
bundled
`data/org.gnome.SimpleScan.gschema.xml` installs to
`/usr/share/glib-2.0/schemas/` only where Simple Scan's own schema is not
installed. `data/org.helixdesktop.DocumentScanner.desktop` installs to
`/usr/share/applications/`.

## Decisions
- **Post-processing** (user decision, round 4): implemented with the four
  `postproc-*` keys, Simple Scan's argument order, no shell, worker thread,
  Preferences UI.
- **Email** (user decision, round 4): temp PDF/images per `save-format`,
  XDG Email portal with `attachment_fds`, `xdg-email --attach` fallback.
- **`save-directory`** (user decision, round 4): keep storing a `file://`
  URI; the real schema (type `s`, "The directory to save files to …")
  was checked and allows it — confirmed.

### Helix-private settings
`$XDG_CONFIG_HOME/helix-document-scanner/settings.json`:
`source` (string, empty = automatic) and `color-mode`
(`auto|color|gray|lineart`).

### Shortcuts
| Keys | Action |
|---|---|
| `Ctrl+1` | Scan single page |
| `Ctrl+F` | Scan all pages from feeder |
| `Ctrl+M` | Scan multiple pages from flatbed |
| `Escape` | Stop scan |
| `Ctrl+N` | New document |
| `Ctrl+S` | Save |
| `Ctrl+C` | Copy page |
| `Ctrl+P` | Print |
| `Ctrl+E` | Email |
| `[` / `]` | Rotate page left / right |
| `Ctrl+Page Up` / `Ctrl+Page Down` | Move page left / right |
| `Delete` | Delete page |
| `Ctrl+,` | Preferences |
| `Ctrl+?` | Keyboard shortcuts |
| `Ctrl+Q` / `Ctrl+W` | Quit / close window |

### Command line
`helix-document-scanner [OPTIONS] [DEVICE]`
- `DEVICE`: SANE device name to select on start-up.
- `--list-devices`: print `name<TAB>vendor model (type)` per device and
  exit (no window).
- `-d, --debug`: log SANE activity to stderr.
- `-v, --version`, `-h, --help`.

The SANE configuration follows the usual environment (`SANE_CONFIG_DIR`),
which is how tests restrict SANE to the virtual `test` backend.
