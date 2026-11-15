# Fonts

Drop-in replacement for gnome-font-viewer.

- **App ID**: `org.helix.Fonts`
- **Binary**: `helix-fonts`
- **Crate**: `apps/fonts`

## Purpose
View and install fonts.

## Main windows
- Font list and preview.
- Install button.

## Contracts used
- Fontconfig.
- XDG portals: FileChooser.
- GSettings if any.

## Non-goals
- Font management beyond view and install.

---

## Behaviour (derived spec)

### Windows and navigation
- One `AdwApplicationWindow` per invocation target, containing an
  `AdwNavigationView`.
- **Font list page** (root when started without files): header bar with a
  search toggle (Ctrl+F), an "Open" button (Ctrl+O) and a main menu
  (About, Quit). Below it, a search bar and a grid of installed font
  *families*. Each tile shows a large "Aa" drawn in that family and the
  family name underneath (with the number of styles when > 1). Typing
  filters the grid (case-insensitive substring over family and style
  names; all whitespace-separated terms must match). An empty result shows
  an "No Fonts Found" status page. Activating a tile pushes the font page.
- While the list is read a spinner shows. If reading fails (see *Data
  sources*) or no answer has come after 75 s, the page shows *Could Not
  List Fonts* with the reason and a *Retry* button, announces "Could not
  list fonts", and logs the reason. A late answer still replaces it; only
  the most recent load may change the page.
- **Font page**: title = family name, subtitle = style. If the family (or
  the opened file, e.g. a TTC collection) has more than one face, a style
  dropdown in the header switches between faces. A view switcher offers
  three views (icons above their labels when the page is narrower than
  500 sp):
  - **Preview**: an editable preview-text entry (defaults to the pangram
    "The quick brown fox jumps over the lazy dog."; if the font does not
    cover most of its letters, a sample built from characters the font
    does cover is used instead), the alphabet/digits line when covered,
    and the text rendered at 8, 10, 12, 18, 24, 36, 48, 72 pt
    (each line labelled with its size). The view never scrolls
    sideways: at any window width (down to 360 px) the entry fits the
    window and sample lines too long for it are cut off at the right edge
    (drawn lines are clipped, Pango-fallback lines end in "…").
  - **Characters**: a grid of every code point mapped in the face's
    `cmap`, each cell drawing the glyph; tooltip shows `U+XXXX`. A count
    of covered characters is shown above the grid.
  - **Info**: rows for Family, Style, Full name, Version, Copyright,
    Designer, Manufacturer, Description, License, License URL, Glyphs,
    Characters, Format, File, Size. Empty values are omitted.
- When the font page was opened for a file (command line or Open dialog),
  the header shows an **Install** button (suggested style). Installing
  copies the file into `$XDG_DATA_HOME/fonts` (default
  `~/.local/share/fonts`), never overwriting an existing different file
  (a numeric suffix is added), then runs `fc-cache -f <dir>` on a worker
  thread. The button then reads "Installed" and is insensitive. If the file
  is already installed (it lives under a system font directory —
  `~/.fonts`, `/usr/local/share/fonts`, `/usr/share/fonts` — or an
  identical copy is present in the user font dir) the button starts as
  "Installed". Errors are shown as a toast. `fc-cache` deliberately waits
  ~2 s (directory mtime granularity), so the button reads "Installing…"
  meanwhile. After installing from a list window, the list reloads.
- Files that are not TrueType/OpenType (e.g. a Type 1 `.pfb` passed on
  the command line) show a "Could Not Open Font" status page; installed
  Type 1 faces still appear in the list (rendered via Pango).

### Rendering
- Grid tiles use Pango with the family name (fontconfig already knows the
  installed families, so this is cheap and lazy via `GtkGridView`).
- The font page renders from the *file* itself: text is shaped with
  `rustybuzz` and glyph outlines from `ttf-parser` are drawn as cairo
  paths. This renders exactly the selected face (including a specific face
  of a TTC) and works for files that are not installed. Files that
  ttf-parser cannot read (Type 1, PCF/BDF bitmap fonts) fall back to Pango
  by family name, and the Characters/Info views show what fontconfig
  reported.

### Data sources
- **Enumeration**: `fc-list -f` with an explicit tab-separated format
  (`file`, `index`, `family`, `style`, `fontformat`). This is the robust
  option: it honours the user's real fontconfig configuration (fonts.conf
  includes, `<dir>`s, `<selectfont>` rejections, sysroot, cache) exactly as
  every other app sees it, whereas a pure-Rust fonts.conf parser would have
  to re-implement fontconfig's configuration language and would drift.
  Output is parsed into faces and grouped by (first) family name; the
  list is sorted case-insensitively. Runs on a worker thread. `fc-list`
  may take up to 60 s (with an empty cache directory it first builds
  fontconfig's cache) and is killed after that. If it cannot run, fails,
  times out or lists nothing, the reason is logged and Helix falls back to
  scanning `$XDG_DATA_HOME/fonts`, `~/.fonts`, `/usr/local/share/fonts`
  and `/usr/share/fonts` with ttf-parser; finding nothing there either is
  a loading failure.
- **Name table**: read with `ttf-parser`; preferred record language is
  English (Windows 0x0409 / Mac 0), then any Unicode record, then Mac
  Roman. Family prefers typographic family (ID 16) over ID 1, style
  prefers ID 17 over ID 2.
- **Format** is derived from the file: "TrueType", "OpenType (CFF)",
  "OpenType (CFF2)", with " Collection" for TTC/OTC files; otherwise
  fontconfig's `fontformat`.

### GSettings
- gnome-font-viewer does not publish a GSettings schema; Helix Fonts uses
  none. Window size is not persisted.

### Shortcuts
- Ctrl+F: toggle search; Escape: close search / go back.
- Ctrl+O: open a font file (`GtkFileDialog`, which uses the
  FileChooser portal when available).
- Ctrl+Q quit, Ctrl+W close window.

### Command line
- `helix-fonts [FILE...]` – with no files shows the installed font list;
  each FILE opens its own window with its preview and Install button.
- `helix-fonts --help`.
- Supported file types: `.ttf`, `.otf`, `.ttc`, `.otc` (desktop file
  registers `font/ttf`, `font/otf`, `font/collection` and legacy
  `application/x-font-ttf`, `application/x-font-otf`).
