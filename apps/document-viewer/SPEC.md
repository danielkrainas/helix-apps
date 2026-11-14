# Document Viewer

Drop-in replacement for Papers.

- **App ID**: `org.helix.DocumentViewer`
- **Binary**: `helix-document-viewer`
- **Crate**: `apps/document-viewer`

## Purpose
View PDF and other documents.

## Main windows
- Document view with sidebar (thumbnails, outline).
- Search, annotations (basic), print.

## Contracts used
- XDG portals: FileChooser, Print, OpenURI.
- GSettings: `org.gnome.Papers` or equivalent.
- Poppler or equivalent PDF backend.

## Non-goals
- Full annotation feature parity on day one.

---

## Behaviour (derived spec)

### Backend
- PDF only, through **poppler-glib** (system 24.02) via the `poppler-rs`
  0.24.1 bindings (the newest release built on glib 0.20 / cairo-rs 0.20,
  sharing the workspace's glib). Feature `v22_7` is enabled (`v23_8` does
  not compile in 0.24.1 because it forgets to imply `v23_7`).
- Rendered tiles never leave the render thread as cairo objects: the
  thread sends plain premultiplied BGRA bytes which the UI wraps in
  `gdk::MemoryTexture`s.
- `poppler-rs` does not wrap `PopplerAction` / `PopplerDest` contents
  (link targets, outline actions). Those few fields are read through the
  re-exported `poppler::ffi` (poppler-sys) structs in one small module
  (`links.rs`), which only reads plain fields of structs owned by poppler.
- Poppler documents are not thread-safe. One dedicated **render thread**
  owns the `PopplerDocument` and does everything with it: opening,
  rendering, text extraction, search, link and outline extraction. The UI
  talks to it with a request queue (mutex + condvar) and gets replies over
  an `async-channel`. Printing is the one exception: the print operation
  opens its own, separate document instance on the main thread for the
  duration of the print job.

### Window
- `AdwApplicationWindow` → `AdwToastOverlay` → `AdwToolbarView` with an
  `AdwHeaderBar`. The content is a stack: a **start page** when no document
  is loaded, the **document page** otherwise.
- Header bar (document loaded): start side has *Open* and a sidebar
  toggle; title is an `AdwWindowTitle` (document title from metadata, or the
  file name, with the file name as subtitle when a metadata title exists);
  end side has the primary menu, a zoom menu button showing the current
  zoom percentage, and the page selector (`[entry] of N`).
- Several windows are possible (`app.new-window`, Ctrl+N). Each process is
  non-unique (like the other Helix viewers); several files on the command
  line open one window each.

### Start page
- `AdwStatusPage` "Open a Document" with an *Open…* button, followed by a
  list of **recent documents**: items from `gtk::RecentManager` whose MIME
  type is `application/pdf` and that still exist, most recent first (at most
  20), showing the name, the folder and the last-visited time. Activating a
  row opens it. Every successfully opened document is added to the recent
  manager.

### Opening documents
- CLI: `helix-document-viewer [FILE|URI ...]`, `-h/--help`,
  `-p/--page-label=LABEL` and `-i/--page-index=N` (1-based) jump to a page
  of the first document, `-f/--fullscreen`, `-s/--presentation`.
- Open dialog (Ctrl+O): `gtk::FileDialog` with a "PDF documents" filter
  (`application/pdf`) and "All files".
- Drag and drop of a file onto the window opens it.
- **Password-protected documents**: when poppler reports an encrypted
  document, an `AdwAlertDialog` asks for the password (`gtk::PasswordEntry`);
  *Unlock* retries, a wrong password re-asks with "Incorrect password".
  *Cancel* returns to the previous state. Passwords are never stored.
- Load errors (not a PDF, missing file…) show a toast and keep the previous
  state.

### Document view
- Custom scrollable widget (`gtk::Scrollable` inside a
  `gtk::ScrolledWindow`). Pages are laid out **continuously**, top to
  bottom, centred horizontally, with a 12 px margin around the document
  and 12 px between rows; each page has a white background and a thin
  shadow border.
- **Dual page** mode (`d`, menu): pages are shown in pairs side by side
  (1–2, 3–4, …), with a 12 px gap between the two; rows are as high as the
  taller page and each page is vertically centred in its row.
- **Rotation** (Ctrl+Left / Ctrl+Right, menu): the whole document rotates
  in 90° steps. Rotation is applied when drawing (rendered tiles are
  unrotated), so rotating does not re-render.
- **Zoom**: 100 % = one PDF point is 96/72 logical pixels (real size on
  a 96 dpi screen). Modes:
  - *Fit Width*: widest row (including the dual gap) fills the width minus
    margins.
  - *Fit Page*: like fit width but also limited so the tallest page fits in
    the viewport height.
  - *Automatic* (default): fit width, but never above 100 %.
  - *Free*: an explicit percentage (10 %–1600 %).
  Fit modes recompute on resize; zooming by any other means switches to free.
- Zoom controls: Ctrl+plus / Ctrl+minus (next / previous preset among
  10, 25, 33, 50, 67, 80, 100, 125, 150, 200, 300, 400, 600, 800, 1200,
  1600 %), Ctrl+0 automatic (best fit), Ctrl+scroll (zooms around the
  pointer; smooth-scroll devices zoom continuously), touchpad/touchscreen
  **pinch** (around the gesture centre), and the zoom menu (zoom in/out,
  Fit Width, Fit Page, Automatic, presets and an entry accepting a typed
  percentage). The point under the pointer (or the centre of the view)
  stays fixed while zooming.
- **Rendering**: pages are rendered by the render thread into
  1024×1024-device-pixel **tiles** at the current scale × the widget scale
  factor (so huge zoom levels never allocate whole-page bitmaps). Tiles are
  uploaded as `gdk::MemoryTexture`s and kept in an **LRU cache** bounded to
  256 MiB (least recently drawn tiles are evicted). Visible tiles are
  requested first, then the pages one viewport above and below
  (prefetch); a new request list replaces the pending one so fast
  scrolling never builds a backlog. While a tile is missing, the page's
  thumbnail (if rendered) is drawn stretched as a placeholder, else plain
  white.
- Annotations are drawn as poppler renders them. Editing annotations is
  not supported.
- **Night mode** (Ctrl+I, menu): page colours are inverted with a colour
  matrix (no re-render).

### Page navigation
- Current page = the page with the largest visible area (ties: the upper
  one). It is shown in the header page entry using the page label when the
  document has labels (`"iv (4 of 120)"` style: the entry shows the label,
  the suffix shows `of N` or `(n of N)` when the label differs from the
  number).
- Page entry: typing a label or a 1-based number and pressing Enter jumps
  there (labels are matched first). Ctrl+L focuses the entry.
- Keys: Page Down / Space / Page Up / Shift+Space scroll by a viewport
  (minus a small overlap); `n`/`p` and Ctrl+Page Down / Ctrl+Page Up go to
  the next/previous page; Home / End (and Ctrl+Home / Ctrl+End) go to the
  first/last page; arrow keys scroll.
- **History**: following an internal link or outline entry pushes the
  previous position; Alt+Left goes back, Alt+Right forward.

### Sidebar
- `AdwOverlaySplitView` sidebar (F9 / toggle button) with an
  `AdwViewSwitcher` over three pages:
  - **Thumbnails**: a list of page thumbnails (128 px wide) with page
    labels; the current page is selected; clicking jumps to the page.
    Thumbnails are rendered lazily (only for bound list rows).
  - **Outline** (document index): the PDF outline as an expandable tree
    (`gtk::TreeListModel`); entries open as the PDF requests; activating an
    entry with a destination jumps to it. Disabled when the document has no
    outline. If the document's page mode asks for the outline, the sidebar
    opens on it.
  - **Search** (see below).

### Search
- Ctrl+F (or the search toggle) opens the sidebar on the Search page and
  focuses its entry. Typing starts an incremental search (300 ms debounce)
  on the render thread, page by page; results stream in. Options: *Match
  case*, *Whole words only*.
- The result list shows each match as "page label" + the text of the line
  around the match. Activating a result scrolls to it.
- All matches on visible pages are highlighted (translucent yellow), the
  selected match in orange. Enter / Ctrl+G go to the next match, Shift+Enter
  / Ctrl+Shift+G to the previous one (wrapping), starting from the current
  page. Escape in the entry clears it and returns focus to the document.
- Poppler returns match rectangles with a bottom-left origin; they are
  flipped to top-left page coordinates before display.

### Text selection and copy
- Dragging with the primary button selects text (glyph-precise, across
  pages: the start page from the press point to its end, intermediate pages
  fully, the end page from its top to the release point). Double-click
  selects a word, triple-click a line. Ctrl+A selects all.
- **Caret navigation** (F7, the main menu or the context menu toggle it):
  a text caret is drawn in the page and the arrow keys move it by
  character / line instead of scrolling (Ctrl+Left / Ctrl+Right by word,
  Home / End to the line start / end), crossing to the neighbouring page at
  the first / last line. Shift with any of these extends a selection from
  where the caret was, which Ctrl+C copies exactly as shown. Poppler's
  per-character boxes (`poppler_page_get_text_layout`, read through
  `poppler::ffi` because `poppler-rs` does not wrap it) place the caret and
  the selection. Turning it on puts the caret at the start of the current
  page unless it is already on a visible page.
- Selection is drawn as poppler's selected region in the accent colour.
- Ctrl+C (and the context menu *Copy*) copies the selected text to the
  clipboard; the selection is also offered as the PRIMARY selection.
  The PDF "no copy" permission is not enforced (see open questions).
- The context menu (Copy, Select All, Caret Navigation) opens with the
  secondary button, or with Menu / Shift+F10 at the focused link, the
  caret, or the middle of the view.
- The pointer shows a text cursor over pages, a hand over links.

### Links
- Link areas come from poppler's link mapping (bottom-left origin, flipped).
  Hovering a link shows a hand cursor and a tooltip (URI, or "Go to page X").
- Clicking: internal destinations (explicit or named, resolved on the
  render thread) scroll to the target page and, if the destination has a
  top coordinate, to that position; named actions NextPage / PrevPage /
  FirstPage / LastPage / GoBack / GoForward are honoured; URIs open through
  `gtk::UriLauncher` (OpenURI portal); remote-file links (`GoToR`) open the
  referenced file relative to the current document in a new window.
  Launch / JavaScript / other actions are ignored.
- **Keyboard**: every link on the visible pages has an invisible focusable
  stand-in over it, in reading order (top to bottom, then left to right).
  Tab from the document moves through them, then out of the view;
  Shift+Tab goes back. The focused link shows a focus ring and is scrolled
  into view; Enter follows it. Pointer input is unaffected (the stand-ins
  are not pointer targets).

### Document properties
- Alt+Return / menu: an `AdwDialog` with a preferences page listing Title,
  Subject, Author, Keywords, Producer, Creator, Created, Modified (localised
  date/time), Pages, Page size (of the first page, in mm and paper name
  when it matches A3/A4/A5/Letter/Legal), File size, PDF version,
  Location, Security (password protected yes/no, plus which of print / copy
  / modify / annotate are not permitted), Linearized ("Fast web view").
  Missing fields show "—". Values are selectable.

### Presentation mode
- F5 (or `-s`): a separate fullscreen window shows one page at a time,
  centred and fitted to the screen, on black. Next: Right, Down, Page Down,
  Space, Return, `n`, primary click, scroll down. Previous: Left, Up,
  Page Up, Backspace, `p`, secondary click, scroll up. Home/End first/last.
  Escape or F5 leaves and the main view jumps to the last presented page.
  The pointer hides after 2 s without motion.

### Fullscreen
- F11 toggles window fullscreen; Escape leaves it.

### Printing
- Ctrl+P: `gtk::PrintOperation` (Print portal when sandboxed) with the
  page count, current page and job name set. Each page is rendered with
  `poppler_page_render_for_printing` on the print cairo context, scaled to
  fit the printable area and centred, rotated 90° when the page's
  orientation differs from the paper's. Print settings are kept for the
  session.

### Per-document state
- `data_dir("helix-document-viewer")/state.json` maps the document URI to
  `{ page, zoom_mode, zoom, rotation, dual, sidebar, last_used }`. It is
  written (atomically, temp file + rename) when the page changes (debounced
  1 s) and when the window closes, and restored on open (unless a page was
  given on the command line). At most 1000 entries are kept; the least
  recently used are dropped.

### GSettings
- None. The `org.gnome.Papers` key set is not used because its exact
  current key names/types could not be confirmed from public docs; the
  per-document state above covers the persisted behaviour.

### Shortcuts
| Keys | Action |
|---|---|
| Ctrl+O | Open |
| Ctrl+N | New window |
| Ctrl+W / Ctrl+Q | Close window / quit |
| Ctrl+P | Print |
| Alt+Return | Properties |
| F9 | Toggle sidebar |
| Ctrl+F | Search |
| Ctrl+G / Ctrl+Shift+G | Next / previous match |
| Ctrl+plus / Ctrl+minus / Ctrl+0 | Zoom in / out / automatic |
| Ctrl+Left / Ctrl+Right | Rotate left / right |
| d | Dual page |
| Ctrl+I | Night mode |
| Ctrl+L | Go to page |
| Home / End, PgUp / PgDn, n / p | Navigation |
| Alt+Left / Alt+Right | History back / forward |
| Ctrl+A / Ctrl+C | Select all / copy |
| F7 | Caret navigation (arrows move the caret, Shift+arrows select) |
| Tab / Shift+Tab, Enter | Move between links on visible pages, follow |
| Menu / Shift+F10 | Context menu |
| F5 | Presentation |
| F11 | Fullscreen |

## Open questions
- **Other formats** (DjVu, XPS, comic book archives, TIFF): not supported;
  each would need a backend library not currently installed. Which, if any,
  should be added?
- **GSettings**: should the app adopt the `org.gnome.Papers` schema (e.g.
  for default zoom / continuous / dual / night mode) once its key list is
  confirmed?
- **Copy restrictions**: the app does not enforce the PDF "no copy"
  permission (only reports it in Properties). Enforce it?
- **Annotation editing** (adding highlights / notes, saving): deferred.

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): the search result count when
  a search completes, and the page ("Page 3 of 12", or "Page iv (4 of
  120)" with labels) once the current page settles after navigation or
  scrolling.
- The document view has the *document* role, is named after the document
  (its title, else the file name) and described by the current page
  ("Page 3 of 12").
- It implements GTK's accessible text interface (`GtkAccessibleText`):
  assistive technology reads the text of one page — the caret's page in
  caret navigation, otherwise the current page — by character, word,
  sentence, line or paragraph, with the caret offset and the keyboard
  selection. Page text comes from the render thread on demand; switching
  page reports the old text removed and the new text inserted. Caret
  moves are reported, so a screen reader speaks the character, word or
  line moved over.
- Links on visible pages are exposed as *link* objects (the link stand-ins
  above), named by the text under the link (poppler's text for the link
  area, whitespace collapsed) or, if there is none, by the target, and
  described by the target ("Go to page 4", or the address).
- Entries are named: the page entry "Current Page" (described by the "of
  12" beside it), the zoom entry "Zoom Level", the search entry "Search
  Document", and the unlock dialog's entry "Password" (described by the
  dialog's message). Icon-only buttons have explicit names without
  shortcut text.
