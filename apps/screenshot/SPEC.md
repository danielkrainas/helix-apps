# Screenshot

Drop-in replacement for the classic GNOME Screenshot app
(`gnome-screenshot`).

- **App ID**: `org.helixdesktop.Screenshot`
- **Binary**: `helix-screenshot`
- **Crate**: `apps/screenshot`

## Purpose
Take a screenshot of the whole screen, of the focused window or of an
area the user drags out, after an optional delay; then save it as a PNG
file or copy it to the clipboard. Also usable from the command line and
from keyboard shortcuts without showing a window.

Wayland does not let ordinary clients read the screen, so every capture
is made by the desktop shell. This is what X11-only tools such as
xfce4-screenshooter cannot do under Helix (area selection in particular).

## Main windows
- **Options**: what to capture, delay, pointer, *Take Screenshot*.
- **Preview**: the captured image with *Retake*, *Copy to Clipboard*,
  *Save…* and *New Screenshot*.

## Contracts used
- D-Bus `org.gnome.Shell.Screenshot` (bus `org.gnome.Shell`, object
  `/org/gnome/Shell/Screenshot`): `Screenshot`, `ScreenshotWindow`,
  `ScreenshotArea`, `SelectArea`
  (`helixdesktop/spec/contracts/dbus/org.gnome.Shell.Screenshot.md`).
- D-Bus well-known name `org.gnome.Screenshot` on the session bus, owned
  while a capture runs: the shell only answers callers that own it.
- Helix-private `org.helixdesktop.ScreenshotWindowPicker.PickWindow` at the same
  object (optional; see "Window").
- XDG user directories (`XDG_PICTURES_DIR`), `XDG_RUNTIME_DIR`.
- The clipboard (`image/png` and the other image formats GTK offers for a
  texture).

## Non-goals
- File formats other than PNG (the shell always writes PNG).
- Effects on the captured window (drop shadow, border effects) and ICC
  profile embedding.
- The `org.gnome.gnome-screenshot` GSettings schema: it is not installed
  on GNOME 51 systems (the app was retired in favour of the shell's
  screenshot UI), so the choices are kept in an app-private file instead.
- Taking screenshots without a shell that implements the interface (X11
  window managers, other Wayland compositors): the app reports the error.

---

## Behaviour (derived spec)

### Options page
- One `AdwApplicationWindow` (default 560×520, minimum 360×400) holding
  both pages in a `GtkStack`. Header bar with the primary menu (*About
  Screenshot*, *Quit*). Content, top to bottom:
  - under the heading "Take", a row of three linked toggle buttons (icon
    above label, with mnemonics) choosing the mode, exactly one active:
    **Screen** (the whole desktop, all monitors), **Window** (a window),
    **Area** (a rectangle the user drags out); for assistive technology
    the row is a group named "Take";
  - under "Options", a boxed list with **Delay in Seconds** (`AdwSpinRow`,
    0–99, step 1) and **Show Pointer** (`AdwSwitchRow`), the latter
    insensitive in Area mode because the shell's area capture has no
    pointer option;
  - a **Take Screenshot** button (pill, suggested action). Enter activates
    it.
- The choices (mode, delay, pointer) and the last folder a screenshot was
  saved to are kept in `$XDG_CONFIG_HOME/helix-screenshot/settings.json`
  (written when *Take Screenshot* is pressed and after a save) and
  restored at start. A missing or broken file means the defaults: Screen,
  0 s, pointer off. Command-line choices override them for that run.

### Taking the screenshot
1. The window hides (it is never part of the image).
2. **Area**: the shell's area selection runs (`SelectArea`). If the user
   cancels it (Escape, right click), the window comes back unchanged and
   nothing else happens.
3. The delay runs (with the window still hidden). A short settling pause
   (about 0.3 s) follows hiding the window or ending the selection so that
   neither shows in the image.
4. The capture: `Screenshot(pointer, flash, file)`,
   `ScreenshotWindow(true, pointer, flash, file)` or
   `ScreenshotArea(x, y, w, h, flash, file)` with the rectangle from step
   2. `flash` is true. `file` is a fresh absolute path in
   `$XDG_RUNTIME_DIR` (the user cache directory when unset), named
   `helix-screenshot-<pid>-<n>.png`.
5. The file the shell reports (`filename_used`) is read into memory and
   deleted, so no temporary file outlives the capture. The image is shown
   on the preview page and the window comes back.
- All D-Bus traffic for one capture (name request, selection, capture)
  uses one private session-bus connection that owns `org.gnome.Screenshot`;
  it is closed after the capture, which releases the name. If the name is
  already owned (another screenshot tool is busy) the capture fails with
  "Another screenshot is already being taken."
- The D-Bus calls run on a worker thread (`gio::spawn_blocking`); the UI
  never blocks.

### Window mode
- `ScreenshotWindow` captures the *focused* window. Because this window is
  hidden, that is the window that had focus before (or the one the user
  focused during the delay), as in the classic app; the window frame Helix
  draws for X11 windows is included.
- From the options page with a delay of 0, the shell's window picker is
  used instead when it is offered (`org.helixdesktop.ScreenshotWindowPicker`
  `PickWindow`): the user clicks the window to capture; Escape cancels and
  returns to the window, as for Area. When the shell does not have the
  picker (`UnknownMethod`/`UnknownInterface`/`UnknownObject`), the focused
  window is captured.

### Errors
- Any failure shows an `AdwAlertDialog` "Could Not Take Screenshot" with
  the reason, and the options page returns. Reasons:
  - no session bus;
  - no shell: `org.gnome.Shell` has no owner — "The desktop shell does
    not offer screenshots. Screenshot needs Helix or GNOME Shell.";
  - access denied by the shell (the D-Bus error message is shown);
  - `success` false: "The shell could not take the screenshot." plus, in
    Window mode, "Is a window focused?" (the shell also refuses while the
    session is locked);
  - the file could not be read or is not an image.

### Preview page
- Header bar: a back button (*New Screenshot*, also `Escape`/`Alt+Left`)
  returns to the options page and drops the image; title "Screenshot".
- The image, scaled down to fit (never up), inside a card frame; its pixel
  size as the subtitle ("1920 × 1080"). Its alternative text says what was
  captured and the size ("Screenshot of the whole screen, 1920 × 1080";
  "… of a window", "… of a selected area").
- Bottom: **Retake** (`Ctrl+R`), **Copy to Clipboard** (`Ctrl+C`) and
  **Save…** (suggested, `Ctrl+S`). In one row, centred, when the window is wide enough
  for it; otherwise a centred column of equally wide buttons, so long
  labels never make the window wider (`row_or_column.rs`).
- **Retake** takes the same screenshot again, with the same options
  (delay, pointer): the whole screen again; in Window mode the window
  again (clicked again where the shell offers the window picker); in Area
  mode a new area is selected. The new image replaces the shown one. If
  the new selection is cancelled, the image already shown stays; if the
  capture fails, the options page comes back with the reason, as for
  *Take Screenshot*.
- **Save…** opens a `GtkFileDialog` (save mode) in the last folder a
  screenshot was saved to, or the pictures directory, or the home
  directory, with the name
  `Screenshot from YYYY-MM-DD HH-MM-SS.png` (local time of the capture).
  The PNG bytes are written as they came from the shell; the folder is
  remembered; a toast "Screenshot saved" follows. A write error shows an
  alert with the reason.
- **Copy to Clipboard** puts the image on the clipboard as a texture and
  shows the toast "Screenshot copied to clipboard".

### Keeping the clipboard
- Wayland drops a clipboard's content when the app that owns it exits. So
  when the window is closed (or the app quits through `Ctrl+Q`) while this
  app still owns the clipboard, the window goes away but the process stays
  in the background until another app takes the clipboard, then exits.

### Command line
`helix-screenshot [OPTION…]`, options compatible with `gnome-screenshot`:

| Option | Meaning |
|---|---|
| `-w`, `--window` | Window mode |
| `-a`, `--area` | Area mode |
| `-d N`, `--delay N`, `--delay=N` | Delay in seconds (0–99) |
| `-p`, `--include-pointer` | Show the pointer (Screen and Window) |
| `-b`, `--include-border` | Include the window frame (default) |
| `-B`, `--remove-border` | Leave out the window frame |
| `-c`, `--clipboard` | Copy the result to the clipboard |
| `-f PATH`, `--file PATH`, `--file=PATH` | Save to PATH (overwriting) |
| `-i`, `--interactive` | Open the options page with these choices |
| `-h`, `--help` | Usage |
| `--version` | Version |

Short flags can be grouped (`-ac`, `-d5`). Unknown options or a bad delay
print the usage to stderr and exit with status 2.

- No options, or `-i`: the window opens on the options page (with the
  given mode, delay and pointer).
- With `-c` and/or `-f`: the screenshot is taken at once, nothing is shown
  except the shell's own selection UI in Area mode, and the result goes to
  the clipboard and/or the file. Errors go to stderr with status 1; a
  cancelled selection exits quietly with status 1.
  - `-f` alone does not start GTK at all. A relative PATH is relative to
    the working directory; the file is written as PNG whatever its
    extension.
  - `-c`: Wayland only lets the focused app set the clipboard, so a small
    window ("Copying screenshot…") appears for a moment, takes the
    clipboard, and hides; the process then stays in the background until
    another app takes the clipboard (see "Keeping the clipboard").
- Mode, delay or pointer options without `-c`, `-f` or `-i` (e.g. a
  shortcut bound to `helix-screenshot -a`): the screenshot is taken at
  once and the window opens on its preview page. If the selection is
  cancelled the app exits.

### Shortcuts
| Keys | Action |
|---|---|
| `Enter` | Take Screenshot (options page) |
| `Ctrl+S` | Save… (preview) |
| `Ctrl+R` | Retake (preview) |
| `Ctrl+C` | Copy to Clipboard (preview) |
| `Escape`, `Alt+Left` | New Screenshot (preview) |
| `Ctrl+W` | Close window |
| `Ctrl+Q` | Quit |

### Process model
- `NON_UNIQUE`: every invocation is its own process (a shortcut can take a
  screenshot while a window is open; a background process keeping the
  clipboard does not swallow later invocations).

## Decisions
- Capture through the shell's `org.gnome.Shell.Screenshot` (as the GNOME
  app does) rather than the Screenshot portal: the portal asks for
  permission and offers no area selection of its own.
- Temporary file in the runtime directory, deleted at once; the image
  lives in memory until saved, so a closed window leaves no files.
