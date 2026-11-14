# Terminal

Drop-in replacement for GNOME Console (kgx).

- **App ID**: `org.helix.Terminal`
- **Binary**: `helix-terminal`
- **Crate**: `apps/terminal`

## Purpose
Terminal emulator.

## Main windows
- Terminal window with tabs.
- Preferences (font, theme, bell).

## Contracts used
- VTE or equivalent terminal widget.
- XDG portals: OpenURI.
- GSettings: `org.gnome.Console` or equivalent.
- Standard session environment.

## Non-goals
- Full GNOME Terminal profile compatibility on day one.

---

## Behaviour (derived spec)

A simple, user-friendly terminal in the spirit of GNOME Console, built on
VTE for GTK 4 (`vte-2.91-gtk4` 0.76, Rust `vte4` 0.8).

### Window
- `AdwApplicationWindow` with a header bar: *New Tab* button (start),
  *Tab Overview* button (`AdwTabButton`), *Find* toggle and the primary menu
  (New Window, Preferences, Keyboard Shortcuts, About) at the end.
- Below it an `AdwTabBar` (autohides with a single tab) and an `AdwTabView`
  wrapped in an `AdwTabOverview` (grid of tabs; new tab from the overview).
- Each tab holds a find bar (`GtkSearchBar`) above a scrolled VTE terminal.
- The window title follows the selected tab's title. A tab's title is the
  terminal title (OSC 0/2) or, when the program sets none, the working
  directory shortened with `~`, or else "Terminal". The tab tooltip is the
  working directory.
- **Header tint**: when the foreground process of the selected tab runs as
  root (euid 0, e.g. `sudo -i`, or the terminal itself runs as root) the
  window gets a red-tinted header; when the foreground process is `ssh`
  (or `mosh`, `et`) it gets a purple tint. Polled once a second for the
  selected tab only, from `/proc/<pgid>/{comm,status}`.
- Drag-and-drop: dropping files pastes their paths, shell-quoted and space
  separated; dropping text pastes it.
- Bell: `audible-bell` makes VTE beep; `visual-bell` flashes the terminal
  briefly. A bell in a non-selected tab sets its *needs attention* indicator.

### Tabs and processes
- New tab / window spawn the `shell` setting (an argv list) when it is
  non-empty, else the user's shell: `$SHELL`, else the passwd entry
  (`getpwuid`), else `/bin/sh`, with `COLORTERM=truecolor` and
  `TERM_PROGRAM=helix-terminal` added to the environment.
- New tabs and windows start in the working directory of the current tab:
  the OSC 7 directory URI (`file://host/path`) when the shell reports one,
  else `/proc/<shell pid>/cwd`, else `$HOME`.
- When a shell exits its tab closes; the last tab closing closes the window.
  A command given on the command line (`-- CMD`) keeps its tab after it exits
  and shows a banner ("Process exited with status N") with a *Close* button.
- **Close confirmation**: closing a tab or window asks for confirmation if a
  tab is *busy*, i.e. the pty's foreground process group (`tcgetpgrp` on the
  pty master) differs from the spawned child pid. The dialog names the
  running programs (`/proc/<pgid>/comm`).

### Terminal interaction
- Copy (Ctrl+Shift+C, also as text only), Paste (Ctrl+Shift+V), Select All
  (Ctrl+Shift+A); context menu with Copy, Paste, Select All, Open Link,
  Copy Link, Find, New Tab.
- Find bar (Ctrl+Shift+F): search text with *Match Case* and *Regular
  Expression* toggles; plain text is escaped. VTE regex search with PCRE2
  flags (`UTF | NO_UTF_CHECK | UCP | MULTILINE`, plus `CASELESS` unless
  Match Case). Enter / Ctrl+G next (towards older output), Shift+Enter /
  Ctrl+Shift+G previous; wrap around; Escape closes the bar.
- Clickable URLs: `http(s)`, `ftp`, `file`, `mailto` and `www.` URLs are
  highlighted (VTE match regex); Ctrl+click opens them with
  `GtkUriLauncher` (OpenURI portal when sandboxed); OSC 8 hyperlinks too.
- The context menu's *Open Link* / *Copy Link* act on one link, decided
  when the menu opens and disabled when there is none: for the secondary
  button, the hyperlink or URL under the pointer; from the keyboard (Menu
  or Shift+F10), the selection when it is exactly one URL (surrounding
  spaces, brackets and trailing sentence punctuation ignored; `www.` gets
  `http://`). Any selection change replaces the remembered link, so a
  link from an earlier click is never offered. Selecting a URL and
  pressing Menu, then *Open Link*, opens it without a pointer.
- Zoom: Ctrl++ / Ctrl+= / Ctrl+- / Ctrl+0 and Ctrl+scroll change the
  shared `font-scale` (0.5 – 4.0, ×1.1 steps) for all terminals.

### Appearance
- Style: Follow System / Light / Dark (`theme` key: `auto`, `day`, `night`;
  `hacker` is honoured as a dark green-on-black palette but not offered).
  The adwaita colour scheme follows the choice; terminals use the matching
  built-in palette (own palettes: "Helix Light", "Helix Dark", "Hacker").
- Font: the system monospace font (`org.gnome.desktop.interface
  monospace-font-name`, watched for changes) or a custom font.
- **Transparent background** (off by default): when on, the terminal's
  default background colour is drawn with alpha `1 − transparency / 100`
  (transparency 0–100 %, default 20 %), so the desktop shows through
  where the compositor blends window alpha. Only the default background
  becomes see-through: text, the cursor, selection highlight and cells
  with an explicit (ANSI/true-colour) background stay opaque, as in GNOME
  Terminal. While it is on, each terminal window gets the
  `transparent-background` style class, which clears the window's own
  background so nothing opaque is painted behind the terminal; the header
  bar, tab bar, find bar and exit banner keep their normal opaque
  backgrounds. Turning it off restores a fully opaque background (alpha
  1.0) and removes the class. Changes apply live to every open terminal
  and window.

### GSettings
Schema `org.gnome.Console` (not installed on the development machine; the
bundled `data/org.gnome.Console.gschema.xml` matches the real GNOME Console
schema exactly: names, types, enum values, range and defaults). Read and
written through `Prefs` with in-memory fallback.

| Key | Type (default) | Use |
|---|---|---|
| `theme` | enum `auto`=0/`night`=1/`day`=2/`hacker`=3 (`'night'`) | style + palette |
| `font-scale` | d, range 0.5–4.0 (1.0) | zoom level |
| `custom-font` | s (`""`) | Pango font description |
| `use-system-font` | b (true) | system monospace font |
| `shell` | as (`[]`) | argv for new shells; empty = login shell |
| `scrollback-lines` | x (10000) | scrollback limit; negative = unlimited |
| `last-window-size` | (ii) (`(-1, -1)`) | saved size |
| `last-window-maximised` | b (false) | saved maximised state |
| `restore-window-size` | b (true) | remember size |
| `audible-bell` | b (true) | beep |
| `visual-bell` | b (true) | flash |

There is no separate "unlimited scrollback" key: the *Unlimited Scrollback*
switch stores `scrollback-lines = -1` (VTE's convention for unlimited) and
turning it off restores the spin row's value. The `shell` key has no UI
(as in Console); it is honoured when set externally.

Read-only: `org.gnome.desktop.interface monospace-font-name`.

`org.gnome.Console` has no transparency setting, and the bundled copy must
stay identical to the real schema, so the transparency keys live in
Helix's own schema `org.helix.Terminal` (path `/org/helix/Terminal/`,
`data/org.helix.Terminal.gschema.xml`), read through a second `Prefs`
(in-memory fallback when not installed). The key names and types are the
ones GNOME Terminal uses in its profiles, so the meaning is the same:

| Key | Type (default) | Use |
|---|---|---|
| `use-transparent-background` | b (false) | transparent background on/off |
| `background-transparency-percent` | i, range 0–100 (20) | 0 = opaque, 100 = fully clear |

Out-of-range stored values are clamped to 0–100. Changes made outside the
app (e.g. `gsettings set`) also apply live.

### Preferences dialog (`AdwPreferencesDialog`)
- Appearance: Style (combo), Use System Font (switch), Custom Font
  (font button, enabled when not using the system font), Transparent
  Background (switch), Transparency (0–100 % slider, enabled only while
  Transparent Background is on).
- Behaviour: Unlimited Scrollback (switch), Scrollback Lines (spin),
  Audible Bell, Visual Bell, Restore Window Size.

### Shortcuts
| Action | Keys |
|---|---|
| New tab / window | Ctrl+Shift+T / Ctrl+Shift+N |
| Close tab / window | Ctrl+Shift+W / Ctrl+Shift+Q |
| Copy / Paste / Select all | Ctrl+Shift+C / Ctrl+Shift+V / Ctrl+Shift+A |
| Find / next / previous | Ctrl+Shift+F / Ctrl+Shift+G… (in bar: Enter, Shift+Enter) |
| Zoom in / out / reset | Ctrl++ / Ctrl+- / Ctrl+0 |
| Tab overview | Ctrl+Shift+O |
| Switch tabs | Ctrl+PgUp / Ctrl+PgDn, Alt+1…9 (AdwTabView built-ins) |
| Fullscreen | F11 |
| Preferences / Shortcuts | Ctrl+, / Ctrl+? |

Ctrl+W and Ctrl+Q are deliberately *not* bound (they belong to the shell).

### Command line
`helix-terminal [--working-directory DIR] [--tab] [-- COMMAND ARGS…]`
- `--working-directory DIR` (also `-w`, `--working-directory=DIR`): start
  directory; relative paths resolve against the caller's cwd.
- `--tab`: open in a new tab of the existing window instead of a new window.
- `-- COMMAND ARGS…` (or `-e`/`-x` followed by the command): run it instead
  of the shell.
- The app is single-instance; later invocations are forwarded to the
  running instance with their own cwd.

### Implementation notes
- When `gtk-xft-dpi` is unset (-1, e.g. on Broadway) the app sets 96 DPI;
  otherwise `AdwTabButton` computes a negative font size and Pango emits a
  critical.
- Single instance via `G_APPLICATION_HANDLES_COMMAND_LINE`; `--help`,
  `--version` and invalid options are handled by the invoking process.

## Decisions
- `org.gnome.Console` keys aligned with the real schema: enum renumbered
  (`auto`=0 … `hacker`=3), `font-scale` range added, the non-existent
  `ignore-scrollback-limit` dropped (unlimited = negative
  `scrollback-lines`), `shell` added.
- Background transparency is stored in Helix's own `org.helix.Terminal`
  schema using GNOME Terminal's profile key names, rather than adding
  non-standard keys to `org.gnome.Console`. Default transparency when
  turned on is 20 % (GNOME Terminal uses 50 %; 20 % keeps text readable
  over busy wallpapers).

## Open questions
- Console's exact handling of finished `-- COMMAND` tabs is unknown; we keep
  the tab with an exit banner rather than closing instantly.
