# Text Editor

Drop-in replacement for GNOME Text Editor.

- **App ID**: `org.helixdesktop.TextEditor`
- **Binary**: `helix-text-editor`
- **Crate**: `apps/text-editor`

## Purpose
Simple text editor.

## Main windows
- Editor window with tabs, search, and basic preferences.
- Open / save dialogs.

## Contracts used
- XDG portals: FileChooser, OpenURI.
- GSettings: `org.gnome.TextEditor`.
- Standard text file handling.

## Non-goals
- Full IDE features.
- Plugin API compatibility with gedit.
- Syntax highlighting (deferred until GtkSourceView is available, see Open
  questions). Documents are shown as plain text.

---

## Behaviour (derived spec)

### Application
- Single-instance `AdwApplication` (`HANDLES_OPEN`). Files given on the
  command line (paths or URIs) are opened as tabs in the active window of the
  running instance; a file that is already open is focused instead of opened
  twice.
- `--standalone` runs a separate, non-unique instance that neither restores
  nor saves the session. `--help` and `--version` print and exit.
- `Ctrl+N` opens a new window. The application quits when its last window
  closes.

### Window
- `AdwApplicationWindow` with an `AdwToolbarView`:
  - Header bar: **Open** split into an *Open* menu button whose popover lists
    recent documents (name + folder, newest first, filterable) and a
    *Browse Files…* button; a **New Tab** button; the title shows the active
    document's name (with a `•` when modified) and its folder as subtitle;
    **Save** button and the primary menu (*New Window*, *Save As…*,
    *Find/Replace*, *Go to Line*, *Zoom In/Out/Reset*, *Preferences*, *Keyboard
    Shortcuts*, *About*).
  - `AdwTabBar` below the header (autohide when only one tab).
  - `AdwTabView` with one page per document.
  - Bottom status bar: cursor position `Ln L, Col C` (clicking it opens *Go to
    line*), indentation summary (`Spaces: 4` / `Tab Width: 8`), encoding and
    line ending (a menu to switch line ending for the document).
- Each tab page holds, top to bottom: a reload banner (external change),
  the find/replace bar, and the editor in a scrolled window.
- Tabs can be reordered and dragged between windows (TabView default).
- Dropping files (from Files or any source offering a file list) on the
  editor area or the tab bar opens each as a tab, exactly like *Open*
  (an already open file is brought forward; a blank current tab is
  reused). The editor-area drop target sees drags in the capture phase so
  a file drag is never inserted as its address; drags that carry only
  text are still inserted at the drop point by the editor.

### Documents
- New documents are titled *New Document* (numbered `New Document 2`, … when
  several exist).
- **Open** uses `gtk::FileDialog` (portal FileChooser when available);
  multi-selection supported. Loading is asynchronous (`gio` I/O).
- If the active tab is an empty unmodified new document, an opened file
  replaces it.
- **Encoding detection**: byte-order mark (UTF-8, UTF-16LE/BE) first; else
  valid UTF-8; else a statistical guess (`chardetng`, e.g. windows-1252,
  Shift_JIS, GBK, ISO-8859-x). The detected encoding and BOM are kept and used
  when saving. If the text can no longer be represented in that encoding the
  file is saved as UTF-8 and a toast says so.
- **Line endings**: the dominant line ending (`LF`, `CRLF`, `CR`) is
  detected; the buffer always uses `\n`; saving writes the original ending
  (switchable from the status bar).
- **Save** (`Ctrl+S`) writes to the document's file, or behaves as *Save As*
  for new documents. **Save As** (`Ctrl+Shift+S`) asks for a location.
  Saved and opened files are pushed onto the recent list.
- **Modified indicator**: tab title is prefixed with `•`; the window
  subtitle/title mirrors it.
- **Closing a tab** (`Ctrl+W`, tab close button) with unsaved changes asks
  *Save changes to “name”?* with **Cancel**, **Discard**, **Save**.
- **Closing a window**:
  - with `restore-session` enabled (and not `--standalone`) and this being
    the last window, nothing is asked: all tabs and unsaved contents are
    stored as the session and restored next launch;
  - otherwise a single dialog lists the unsaved documents with **Cancel**,
    **Discard All**, **Save All**.
- **External modification**: each file-backed document has a
  `gio::FileMonitor`. When the on-disk etag differs from the one last
  loaded/saved, a banner says *The file has changed on disk* with **Reload**
  (reloads, discarding edits) and **Ignore**. If the file was deleted the
  banner says so (saving recreates it).

### Session & drafts (stored under `$XDG_DATA_HOME/helix-text-editor/`)
- `session.json`: windows → tabs (file URI or none, draft id, encoding, BOM,
  line ending, cursor line/column) and the active tab.
- `drafts/<id>.txt`: UTF-8 text of documents with unsaved changes.
- While editing, modified documents are autosaved as drafts and the session
  file is rewritten a few seconds after the last change (crash recovery),
  regardless of `restore-session`.
- On startup (first window, not standalone), when `restore-session` is true or
  drafts exist from a crash, tabs are restored: drafts are loaded (marked
  modified, file association kept), clean files are reloaded from disk,
  missing files without drafts are skipped. When `restore-session` is false
  and the last window closes cleanly, the session and drafts are deleted.
- `recent.json`: up to 20 recently opened/saved URIs.

### Editing
- Plain `gtk::TextView` (custom subclass) with undo/redo (`Ctrl+Z`,
  `Ctrl+Shift+Z`); loading text is not undoable.
- **Line numbers** in a left gutter (current line emphasised).
- **Current line highlight** and **right margin** line, drawn below the text.
- **Word wrap** (`WrapMode::WordChar`) or no wrap.
- **Tab width** sets the tab stops; **indent style** `space` makes `Tab`
  insert spaces up to the next tab stop. With a multi-line selection `Tab`
  indents and `Shift+Tab` unindents the selected lines.
- **Auto indent**: `Enter` copies the current line's leading whitespace, plus
  one indent level after a line ending in `{`, `[` or `(`.
- **Zoom**: `Ctrl++`, `Ctrl+-`, `Ctrl+0` (10 % steps, 30 %–400 %), applies to
  all editors.
- **Font**: `use-system-font` uses `org.gnome.desktop.interface
  monospace-font-name` (live-updated); otherwise `custom-font` (Pango font
  description).
- **Style**: `style-variant` `follow`/`light`/`dark` drives
  `AdwStyleManager:color-scheme`.

### Find & replace
- `Ctrl+F` opens the find bar (pre-filled with the selection), `Ctrl+H`
  opens it with the replace row. `Escape` closes it and returns focus.
- Options: case sensitive, whole words only, regular expression (Rust
  `regex` syntax; replacements can use `$1`/`${name}`; invalid patterns mark
  the entry red).
- All matches are highlighted; the label shows `n of m`. `Enter`/`Ctrl+G`
  next, `Shift+Enter`/`Ctrl+Shift+G` previous (wrapping).
- *Replace* replaces the current match and moves to the next; *Replace All*
  is a single undo step.

### Go to line
- `Ctrl+I` opens a popover with an entry accepting `LINE` or `LINE:COLUMN`;
  values are clamped to the document.

### Preferences (`Ctrl+,`, `AdwPreferencesDialog`)
- Appearance: style (Follow System / Light / Dark), use system font, custom
  font, show line numbers, highlight current line, show right margin,
  right margin position.
- Editor: wrap text, tab width, indent with spaces, auto indent.
- Session: restore session.

### GSettings (`org.gnome.TextEditor`)
Read and written (others ignored); changes from outside are applied live:
`show-line-numbers` (b, false), `wrap-text` (b, true), `tab-width` (u, 8),
`indent-style` (enum `tab|space`, `tab`), `auto-indent` (b, true),
`highlight-current-line` (b, false), `use-system-font` (b, true),
`custom-font` (s, `'Monospace 11'`), `style-variant` (enum `follow|light|dark`,
`follow`), `restore-session` (b, true), `show-right-margin` (b, false),
`right-margin-position` (u, 80).
Also read: `org.gnome.desktop.interface monospace-font-name`.
Without the schema installed, defaults are kept in memory.

### Keyboard shortcuts
| Keys | Action |
|---|---|
| `Ctrl+T` | New tab |
| `Ctrl+N` | New window |
| `Ctrl+O` | Open |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save As |
| `Ctrl+W` | Close tab |
| `Ctrl+Shift+W` | Close window |
| `Ctrl+F` / `Ctrl+H` | Find / Replace |
| `Ctrl+G` / `Ctrl+Shift+G` | Next / previous match |
| `Ctrl+I` | Go to line |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Zoom in / out / reset |
| `Ctrl+PgUp` / `Ctrl+PgDn`, `Alt+1…9` | Switch tabs (TabView) |
| `Ctrl+,` | Preferences |
| `Ctrl+?` | Keyboard shortcuts |
| `Ctrl+Q` | Quit |

### Command line
`helix-text-editor [--standalone] [FILE|URI…]`

## Open questions
- Adopt GtkSourceView 5 for syntax highlighting, undo grouping, search
  context and space drawing once the library is available on Helix systems?
  Until then the editor is a plain `GtkTextView` with no highlighting.
- Window close with `restore-session` enabled stores unsaved documents as
  drafts without asking (matching GNOME Text Editor as understood from its
  user documentation). Confirm this is the desired behaviour.

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): the search-bar match count ("3 of 12", "No results") once typing pauses.
- Controls have specific accessible names: the find-bar options are named
  "Case Sensitive", "Match Whole Words Only" and "Regular Expression"
  (not their glyphs); the find and replace entries are named "Find" and
  "Replace With", the recent-documents search "Search Recent Documents",
  and the Go to Line entry is labelled by its "Go to Line" heading.
