# Characters

Drop-in replacement for GNOME Characters.

- **App ID**: `org.helix.Characters`
- **Binary**: `helix-characters`
- **Crate**: `apps/characters`

## Purpose
Character map and emoji picker.

## Main windows
- Character grid with search and categories.
- Details / copy.

## Contracts used
- GSettings: `org.gnome.Characters`.
- Unicode data.

## Non-goals
- Exact font rendering quirks of the original.
- `org.gnome.Shell.SearchProvider2` (shell search integration) is out of
  scope for this round; tracked as later work.
- Locale-specific "Letters" filtering by script, and per-font coverage
  filtering (showing only characters the chosen font can render).

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow` with an `AdwNavigationSplitView`:
  - **Sidebar**: a navigation list of categories, in three sections:
    1. *Recently Used*
    2. *Emoji*: Smileys & People, Animals & Nature, Food & Drink,
       Activities, Travel & Places, Objects, Symbols, Flags.
    3. *Letters & Symbols*: Punctuation, Arrows, Bullets, Pictures,
       Currency, Math, Numbers, Letters.
  - **Content**: header bar (title = current category, search toggle button,
    primary menu with *About* and *Quit*) above a search bar and a
    `gtk::GridView` of characters inside an `AdwToastOverlay`.
- The split view collapses to a single pane on narrow widths (breakpoint at
  560 px); the content page then has a back button to the sidebar.
- The initially selected category is *Recently Used* if it is non-empty,
  otherwise *Smileys & People*.
- Empty states use `AdwStatusPage`: "No Recent Characters" for an empty
  Recent list, "No Results" for a search with no matches.

### Grid
- Each cell shows one character (or emoji sequence) in a large font. Combining
  marks are displayed on a dotted circle (U+25CC) so they are visible; the
  copied text is the mark alone.
- Cells have a tooltip with the character's name.
- Activating a cell (single click, or Enter when focused) opens the
  **details dialog**.
- Large categories (Letters ≈ 130 000 characters) rely on `GridView`
  recycling; category lists are computed off the main thread and cached.

### Categories (data)
- **Emoji** categories come from the CLDR emoji groups (crate `emojis`), in
  CLDR order, default skin tone only. *Smileys & People* is the union of the
  CLDR groups "Smileys & Emotion" and "People & Body".
- **Letters & Symbols** categories are derived from the Unicode General
  Category (crate `unicode-general-category`) and character names (crate
  `unicode_names2`), over all assigned code points excluding controls,
  formats, surrogates, private use, separators and noncharacters:
  | Category | Rule |
  |---|---|
  | Punctuation | General category P* |
  | Arrows | Symbol (S*) whose name contains `ARROW` |
  | Bullets | Name contains `BULLET`, plus a few common bullet-like shapes (◦ ○ ● ◘ ◙ ▪ ▫ ■ □ ◆ ◇ ★ ☆ ✓ ✗ …) |
  | Pictures | General category So that is not an arrow and not an emoji-presentation code point (box drawing, geometric shapes, dingbats, misc. symbols …) |
  | Currency | General category Sc |
  | Math | General category Sm |
  | Numbers | General category N* |
  | Letters | General category L* |
  Categories may overlap (e.g. `→` is both Math and Arrows).

### Search
- `Ctrl+F` or the search button toggles the search bar; typing anywhere in
  the window starts a search (key capture). `Escape` closes it and returns to
  the selected category.
- Search runs over every displayable code point plus every emoji (including
  multi-codepoint sequences), off the main thread, debounced (~150 ms). The
  index is built once in the background at startup.
- Matching is case-insensitive. The query is split on whitespace; every term
  must occur as a substring of the entry's name or keywords. Emoji keywords
  are their GitHub/CLDR shortcodes (with `_` read as space) and, for
  single-codepoint emoji, the Unicode character name (so `smiling` and
  `white smiling face` both find ☺).
- Ranking: exact name match, then name prefix match, then entries where all
  terms match at word starts, then any match; ties prefer emoji, then
  code point / CLDR order. Results are capped at 5 000.
- A new query hides the grid at once (the previous category or results
  are never shown under the "Search" title); if the results take longer
  than 150 ms a spinner ("Loading Characters") shows until they arrive.
  While results are shown no sidebar row is selected; leaving the search
  (Escape, clearing the entry) selects the category again.
- Special queries are put first: a literal single character or emoji (paste
  `é` to find it), and a code point written as `U+XXXX`, `0xXXXX` or `\uXXXX`.

### Details dialog
- `AdwDialog` showing: the glyph very large; its name (CLDR name in sentence
  case for emoji, Unicode name for other characters); the code point(s)
  `U+XXXX`; UTF-8 bytes in hex (`F0 9F 98 80`); a **Copy Character** button
  (suggested action, also `Ctrl+C`).
- **See also**: related characters shown as buttons; clicking one shows it in
  the same dialog. For emoji with skin-tone variants: all tones. For letters:
  the other case form(s) (uppercase / lowercase / titlecase).
- Copy puts the text on the clipboard, adds it to Recent, closes the dialog
  and shows a toast "Character copied to clipboard".

### Recent characters
- Stored in GSettings `org.gnome.Characters` key `recent-characters` (`as`),
  most recent first, de-duplicated, truncated to `max-recent-characters`
  (`u`, default 100). Updated on every copy. When the schema is not installed
  the list is kept in memory only (via `helix_app_common::Prefs`).

### GSettings keys used (`org.gnome.Characters`)
| Key | Type | Use |
|---|---|---|
| `recent-characters` | `as` | Recent list (read/write) |
| `max-recent-characters` | `u` | Recent list cap (read; default 100) |

These are the only keys in the installed `org.gnome.Characters` schema;
`data/org.gnome.Characters.gschema.xml` copies them exactly (names, types,
defaults), for systems without the GNOME schema. No other key is invented
under that schema (an earlier draft read a `font` key, which the real
schema does not have; the glyphs use the theme font).

### Window state (app-private)
- The window opens at the last saved size (default 860×620) and
  maximized state, stored in
  `$XDG_CONFIG_HOME/helix-characters/window.json`
  (`{"width":…, "height":…, "maximized":…}`), written atomically when a
  window closes and on quit. Missing/corrupt files or sizes outside
  200–16384 px fall back to the defaults.

### Shortcuts
| Keys | Action |
|---|---|
| `Ctrl+F` | Toggle search |
| `Escape` | Close search |
| `Ctrl+C` | Copy (details dialog) |
| `Ctrl+W` | Close window |
| `Ctrl+Q` | Quit |

### Command line
- `helix-characters` — open the window.
- `helix-characters --search TEXT` (`-s`) — open with the search filled in.
- `helix-characters --info TEXT` — print name, code points and UTF-8 bytes
  of TEXT and exit (no window; useful for scripting and tests).
- `helix-characters --help`.

## Decisions
- GSettings: use the real `org.gnome.Characters` keys only
  (`recent-characters`, `max-recent-characters`); window size stays
  app-private because the schema has no geometry keys.

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): the number of characters found by a search once typing pauses.
- Characters are read by name, not glyph: each grid cell (the list item
  that takes focus), each "See also" variant button and the details
  dialog's big glyph are named with the character's name in sentence case
  ("Grinning face", "Combining acute accent", "Latin capital letter a" —
  all-capital Unicode names could be spelled out), with the code points as
  the description where it helps (grid cells, details glyph).
