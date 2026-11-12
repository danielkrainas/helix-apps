# Web

Drop-in replacement for Epiphany (GNOME Web).

- **App ID**: `org.helix.Web`
- **Binary**: `helix-web`
- **Crate**: `apps/web`

## Purpose
Web browser.

## Main windows
- Browser window with tabs, bookmarks, and reader mode.
- Preferences and history.

## Contracts used
- WebKitGTK.
- XDG portals: FileChooser, OpenURI, Notification, Settings.
- GSettings: `org.gnome.Epiphany`.
- Secret service for passwords.

## Non-goals
- Full extension ecosystem.
- Exact UI clone.

---

## Behaviour (derived spec)

### Engine
- WebKitGTK 6.0 (`webkit6` crate, API level 2.46; 2.52 installed). WebKit's
  web-process sandbox is always left enabled; the app never sets any
  environment variable or API option that weakens it.
- One persistent `WebKitNetworkSession` per process with data in
  `$XDG_DATA_HOME/helix-web/webkit` and cache in
  `$XDG_CACHE_HOME/helix-web/webkit`; favicons enabled; cookies stored in
  SQLite by WebKit inside that data dir.
- Each private window gets its own ephemeral `NetworkSession`
  (nothing written to disk; history, session and permission decisions are
  not recorded; per-origin decisions already stored are still honoured).
- One shared `WebKitSettings` object per session kind; preferences update it
  live (JavaScript, popups, default zoom applies to new tabs).

### Application
- Single-instance `AdwApplication` with `HANDLES_COMMAND_LINE`.
  `helix-web [--private] [--new-window] [URL|FILE…]`: each argument is
  classified like address-bar input (existing local paths become `file://`
  URIs, relative to the caller's cwd). URLs open as tabs in the active
  normal window (or a new window with `--new-window`); `--private` opens
  them in a new private window. `--help`, `--version` print and exit.
- The first activation restores the previous session (see Session) and
  then opens command-line URLs; with no URLs and nothing restored it opens
  one window with the start page.
- The app quits when its last window closes.

### Window
- `AdwApplicationWindow` → `AdwTabOverview` → `AdwToolbarView` with a header
  bar and an `AdwTabBar` (autohides with one tab) on top.
- Header bar, start: Back, Forward, Reload/Stop (icon swaps while loading),
  Home (hidden unless a homepage is configured).
- Title: the address entry (max width ~ 720 px).
  - Primary icon = security indicator: padlock (valid TLS), broken padlock
    (http or TLS errors), globe/page (local/internal). Clicking opens a popover
    with host, connection status, certificate subject/issuer/expiry and the
    site's stored permission decisions with a "Reset" button per entry.
  - Secondary icon = bookmark star (filled when the page is bookmarked).
    Clicking it bookmarks the page (if needed) and opens an edit popover
    (title, tags comma-separated, Remove).
  - The entry shows load progress (`progress-fraction`).
  - While focused it shows the full URI; otherwise the URI as loaded.
  - Typing shows a suggestion popover below the entry: first row
    "Search <engine> for “text”" (or the classified URL), then up to 8
    matches from bookmarks and history (title + URL). Up/Down select,
    Enter activates, Alt+Enter / Ctrl+Enter open in a new tab, Escape
    closes the popover and restores the current URI.
- Header bar, end: main menu, Tab overview button (`AdwTabButton`),
  Downloads button (visible once a download exists in this window),
  Bookmarks button (popover with a filter entry and the bookmark list).
- Main menu: New Tab, New Window, New Private Window, zoom row
  (−, current %, +), Reopen Closed Tab, Find…, Print…, Fullscreen,
  History, Import Bookmarks…, Export Bookmarks…, Preferences, About Web.
  (No shortcuts window; shortcuts are listed below.)
- Ctrl+Shift+W closes the window (Ctrl+W closes the tab).
- Private windows: title "Private Browsing", header bar carries a
  "Private" indicator icon, and they never appear in the saved session.
- Find bar (`GtkSearchBar` below the header): entry, previous/next,
  match count ("3 matches" / "Not found"). Searching is case-insensitive
  and wraps around.
- Hovering a link shows its URL in a small overlay label at the bottom-left
  of the page.
- Fullscreen: F11 toggles window fullscreen; HTML5 fullscreen requests from
  pages fullscreen the window; top bars are hidden while fullscreen.

### Tabs
- `AdwTabView`; title = page title (falls back to URI or "New Tab"), icon =
  favicon, spinner while loading (`loading` property), audio indicator when
  playing audio (click toggles mute).
- New tab opens the start page and focuses the address entry.
- Links: middle-click or Ctrl+click opens in a background tab;
  Ctrl+Shift+click in a foreground tab; `window.open`/target=_blank
  (`create` signal) opens a related web view in a new tab next to the
  opener, shown at `ready-to-show`; `close` from script closes that tab.
- Closing tabs pushes them onto a per-window "recently closed" stack
  (max 10) for Ctrl+Shift+T.
- Start page (`about:overview`): a locally generated page showing the most
  visited sites (top 12 from history) as tiles; in private windows a short
  notice about private browsing.

### Address input classification
- Leading/trailing whitespace trimmed; empty input does nothing.
- Leading `?` forces a search for the remainder.
- Known schemes (`http`, `https`, `file`, `about`, `data`, `blob`,
  `view-source`, `ftp`, `javascript`, `mailto`, `webkit`) or any
  `scheme://…` are loaded as-is.
- `/abs/path` and `~/path` become `file://` URIs.
- Input containing whitespace is a search.
- `localhost[:port][/…]`, IPv4 literals and `[ipv6]` literals get `http://`.
- `host.tld[:port][/…]` where every label is a valid hostname label and the
  final label is alphabetic (≥2 chars) or `xn--…` gets `https://`.
- Anything else is a search with the default engine; the query is
  form-encoded (`+` for space, `%XX` for reserved bytes) into the engine's
  template at `%s`.
- Built-in engines: DuckDuckGo (default), Google, Bing, Wikipedia,
  Startpage, Brave Search; users can add/remove custom engines
  (name + template containing `%s`).

### History
- Stored as JSON (`$XDG_DATA_HOME/helix-web/history.json`). Rationale:
  the store is small (capped at 5 000 URLs, oldest pruned), loaded once,
  queried in memory, and JSON needs no C library or schema migrations;
  SQLite (rusqlite bundled) would add a C build for no practical gain at
  this size. Writes are debounced (2 s) and done on a worker thread.
- A visit is recorded at `load-changed: committed` for http/https/file
  pages (not private windows); titles update when known.
- Suggestions: all whitespace-separated terms must appear (case-
  insensitively) in the title or URL; ranked by a frecency score
  (visits weighted by recency) with a bonus for host prefix matches.
- History dialog (Ctrl+H): search entry, newest-first list (max 500 rows
  shown) with per-row remove; click opens in a new tab; "Clear All…"
  with confirmation.

### Bookmarks
- JSON store (`bookmarks.json`): url, title, tags, added time; unique by URL.
- Ctrl+D bookmarks the current page and opens the edit popover.
- Import / export Netscape bookmark HTML (`NETSCAPE-Bookmark-file-1`):
  import accepts nested `<DL>` folders (folder names become tags) and
  `TAGS="a,b"` attributes, skips duplicates; export writes one folder
  per tag (untagged bookmarks at the top level) — re-importing yields the
  same URL/title/tag set.

### Downloads
- Navigation responses the engine cannot display are downloaded; context
  menu "Download …" also works.
- Destination: XDG Download directory (fallback `~/Downloads`) with the
  server-suggested name sanitised (no path separators, control chars or
  leading dots; empty → `download`) and uniqued as `name (1).ext`,
  `name (2).ext` … (keeping compound extensions such as `.tar.gz`); names
  of in-flight downloads are reserved too. Existing files are never
  overwritten.
- Downloads popover per window: file name, progress bar, status
  (`1.2 MB of 3.4 MB`, Finished, Failed, Cancelled), Cancel while running,
  Open and Open Folder (`GtkFileLauncher`, portal) when finished, "Clear
  finished".

### Permissions
- Geolocation, notifications, microphone, camera and clipboard requests
  show a bar at the top of the tab: "<host> wants to …" with Deny / Allow.
  The decision is saved per origin (`scheme://host[:port]`) in
  `permissions.json` and reused silently later; the security popover can
  reset it. Pending requests are denied if the page navigates or the tab
  closes. Camera+microphone requests need both kinds allowed.
- Pointer lock is allowed; DRM (media key system) and other request types
  are denied. `query-permission-state` answers from the store.
- Private windows honour stored decisions but do not save new ones.

### JavaScript dialogs
- alert / confirm / prompt / beforeunload are shown as `AdwAlertDialog`s
  over the window, answered asynchronously.

### Other page features
- Zoom Ctrl+plus / Ctrl+minus / Ctrl+0 per tab (steps 30%…300%); new tabs
  use the default zoom preference.
- Printing (Ctrl+P) via `WebKitPrintOperation` dialog.
- File uploads use WebKit's default file chooser (portal).
- TLS errors: WebKit's error page plus a tab bar "certificate not trusted"
  with "Accept Risk and Continue" (allows that certificate for the host for
  this session).

### Session
- Normal windows' tabs (URI, title) and selected tab are saved to
  `session.json` (debounced, on worker thread, and at quit).
- On startup, restore according to `restore-session-policy`:
  `always` (default) restores; `crashed` restores only if the previous run
  did not exit cleanly; `never` does not. Restored tabs are loaded lazily
  when first selected.

### Preferences (Ctrl+,)
- General: Homepage (empty = start page), Restore session (Always /
  After crash / Never), Default zoom.
- Search: default engine (combo), engine list with remove, add engine
  (name + URL template with `%s`).
- Privacy: Enable JavaScript, Block pop-up windows, Cookies (Always
  accept / Block third-party (default) / Block all), Clear Browsing
  Data… (cookies & site data, cache, history — all time).

### Settings storage
- All preferences are written to `$XDG_CONFIG_HOME/helix-web/settings.json`.
  When the GNOME schemas are installed, these keys are also read from /
  written to GSettings so settings carry over:
  - `org.gnome.Epiphany`: `homepage-url` (s; `about:overview` = start page),
    `restore-session-policy` (enum `always` | `crashed` | `never`).
  - `org.gnome.Epiphany.web`: `enable-popups` (b), `cookies-policy`
    (s: `always` | `no-third-party` | `never`), `default-zoom-level` (d).
  Search engines and the JavaScript toggle live only in `settings.json`.

### Shortcuts
| Keys | Action |
|---|---|
| Ctrl+T | New tab |
| Ctrl+N / Ctrl+Shift+N | New window / New private window |
| Ctrl+W, Ctrl+Shift+T | Close tab, reopen closed tab |
| Ctrl+L, F6, Alt+D | Focus address bar |
| Alt+Left / Alt+Right | Back / Forward |
| Ctrl+R, F5 / Ctrl+Shift+R, Shift+F5 | Reload / Reload bypassing cache |
| Escape (page focused) | Stop loading |
| Alt+Home | Home |
| Ctrl+D | Bookmark page |
| Ctrl+H | History |
| Ctrl+Shift+O | Tab overview |
| Ctrl+F, Ctrl+G, Ctrl+Shift+G | Find, next, previous |
| Ctrl+P | Print |
| Ctrl+plus / Ctrl+minus / Ctrl+0 | Zoom in / out / reset |
| F11 | Fullscreen |
| Ctrl+, | Preferences |
| Ctrl+Q | Quit |
| Ctrl+Tab, Ctrl+PgUp/PgDn, Alt+1…9 | Tab switching (AdwTabView built-ins) |

## Open questions
- **Password manager**: requires libsecret / Secret Service; libsecret is
  not available, so saving/filling passwords is not implemented. Should a
  pure-Rust Secret Service client (zbus `org.freedesktop.secrets`) be used?
- **Web apps** ("Install site as web application") need the
  `org.freedesktop.portal.DynamicLauncher` portal and a separate web-app
  profile mode; deferred.
- **Reader mode** needs a readability-style article extractor (a JS port
  injected into the page or a Rust crate); deferred until a clean-room
  implementation choice is made.
- **WebKit sandbox on this host**: WebKit's bubblewrap sandbox needs
  unprivileged user namespaces, which this machine restricts
  (`kernel.apparmor_restrict_unprivileged_userns = 1`; `bwrap` fails with
  "setting up uid map: Permission denied"), so the web process aborts
  ("Failed to fully launch dbus-proxy"). The app never disables the
  sandbox; on such systems an AppArmor profile allowing `userns` for
  `/usr/bin/bwrap` (or for helix-web) is needed. Should the app detect this
  and show a friendly error instead of WebKit's abort?
- Search-engine and JavaScript preferences are not stored in
  `org.gnome.Epiphany` because the exact published key types could not be
  verified here (no schema installed); they live in `settings.json`.
