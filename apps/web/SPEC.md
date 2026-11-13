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
- Every view gets its own `WebKitUserContentManager`. Normal (non-private)
  views carry the password form script, injected at document end into top
  frames in the isolated script world `helix` together with the script
  message handler `helixPasswords`; page scripts can see neither.
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
- `helix-web --web-app=ID URL` runs an installed web application (see Web
  applications). `--helix-probe-userns` is internal (sandbox probe).

### Sandbox check
- WebKit aborts the whole UI process ("Failed to fully launch dbus-proxy")
  when bubblewrap cannot create a user namespace, so the primary instance
  checks first, in `startup`, before any WebView exists: it re-executes
  itself with `--helix-probe-userns`; that child (same binary, same
  AppArmor profile) does `unshare(CLONE_NEWUSER)` and writes
  `/proc/self/uid_map` / `gid_map` the way bubblewrap does. Skipped inside
  Flatpak (`/.flatpak-info`).
- On failure the cause is read from
  `/proc/sys/kernel/apparmor_restrict_unprivileged_userns` (AppArmor),
  `kernel.unprivileged_userns_clone` / `user.max_user_namespaces` (disabled)
  or unknown, and instead of browser windows the app shows one window with
  an `AdwStatusPage` "Web Content Sandbox Unavailable" explaining the cause.
  For AppArmor it shows (and offers to copy) a profile for this binary's
  real path plus the standard paths, and the commands
  `sudo tee /etc/apparmor.d/helix-web < helix-web.apparmor` and
  `sudo apparmor_parser -r /etc/apparmor.d/helix-web` (only the latter when
  `/etc/apparmor.d/helix-web` already exists but does not apply). Quit
  button; no web content is ever loaded without the sandbox.
- If a web process later terminates (crash or memory limit, not
  `TerminatedByApi`), the tab shows a "Page Crashed" status page over the
  view with a Reload button instead of a blank page.

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
  Bookmarks button (popover with a filter entry and the bookmark list),
  Reader View button (only when the page has a readable article; highlighted
  while reader view is shown).
- Main menu: New Tab, New Window, New Private Window, zoom row
  (−, current %, +), Reopen Closed Tab, Find…, Print…, Fullscreen,
  Install Site as Web Application… (http/https pages, normal windows),
  History, Passwords, Import Bookmarks…, Export Bookmarks…, Preferences,
  About Web.
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
- A leading bang word (`!w rust`) searches with the engine whose `bang`
  matches; unknown bangs are searched with the default engine as typed.
- Built-in engines: DuckDuckGo `!ddg` (default), Google `!g`, Bing `!b`,
  Wikipedia `!w`, Startpage `!sp`, Brave Search `!brave`; users can
  add/remove custom engines (name + template containing `%s`). When
  `org.gnome.Epiphany` is installed its `search-engine-providers` list is
  used instead (so Epiphany's engines carry over).

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
- "Never save passwords for this site" is stored here too (kind
  `save-password`, always deny) and can be reset from the security popover.

### Password manager
- Logins are stored only in the Secret Service
  (`org.freedesktop.secrets`, freedesktop Secret Service API 0.2), spoken to
  directly with zbus (no libsecret), from worker threads. Default collection
  (`ReadAlias("default")`, created via `CreateCollection` if missing);
  locked items/collections are unlocked through the service's prompt
  (`Prompt` + `Completed`).
- Transfer session: `dh-ietf1024-sha256-aes128-cbc-pkcs7` by default
  (`sscrypto.rs`, pure Rust): Diffie-Hellman over the RFC 2409 Second
  Oakley Group (1024-bit MODP, generator 2, constant-time `crypto-bigint`
  exponentiation, random 1024-bit private exponent); public keys are
  unsigned big-endian `ay` in the `OpenSession` input/output variant; peer
  keys outside 2..p-2 are refused. AES key = first 16 bytes of
  HKDF-SHA256(IKM = shared secret as 128-byte big-endian, no salt, empty
  info). Each secret is AES-128-CBC/PKCS#7 with a fresh random IV in
  `parameters`. A secret that does not decrypt is an error, never shown.
  Only if the service answers `OpenSession` with a D-Bus error (algorithm
  not supported) does the client log it on stderr and fall back to
  `plain`. Private keys, shared secrets, session keys and plaintext
  buffers are zeroized after use. An ignored, opt-in test
  (`HELIX_WEB_SECRET_INTEROP=1 cargo test -- --ignored interop`, run in a
  private `dbus-run-session` with a throwaway `gnome-keyring-daemon`)
  checks interoperability with a real service.
- Item schema (Helix-specific; documented contract):

  | attribute | value |
  |---|---|
  | `xdg:schema` | `org.helix.Web.Password` |
  | `origin` | `scheme://host[:port]` of the page with the login form |
  | `target-origin` | origin of the form's `action` (page origin if none) |
  | `username` | user name, may be empty |

  Label "Password for USER on HOST"; secret = UTF-8 password,
  content type `text/plain; charset=utf8`. Saving replaces the item with the
  same attributes (`CreateItem(…, replace = true)`).
- Saving: the injected script listens for `submit` (capture phase); it
  takes the first password field and the nearest visible text/email/tel
  field before it; forms with more than one filled password field
  (sign-up / change password) are ignored. The message carries action,
  user name and password; the origin is taken from the tab's URI, never
  from the page. If `remember-passwords` is on, the window is not private
  and the site is not on the never list, a bar offers "Save password for
  “USER” on HOST?" (or "Update the saved password…" when the user exists
  with another password) with Never for This Site / Not Now / Save. Nothing
  is offered when the same login is already saved.
- Filling: after a page finishes loading (normal windows, not on pages
  with certificate errors), if the page has a password field the logins for
  its origin are looked up; the first by user name is filled into each form
  with a single, empty password field (input/change events are dispatched).
- Preferences → Privacy → Passwords: Remember Passwords switch and Saved
  Passwords… (lists host + user name, never the password; per-entry
  removal through `Item.Delete`).
- Private windows neither save nor fill.

### Reader mode
- After each http(s) page load the page HTML (`outerHTML`, read in the
  isolated world, ≤ 4 MiB) is analysed on a worker thread by a pure-Rust
  readability-style extractor (`htmlparse.rs` tolerant HTML parser,
  `reader.rs`): non-content elements (script, style, nav, footer, aside,
  forms, iframes, hidden elements, page-level header) and blocks whose
  class/id/role look like page furniture are dropped; paragraph-like blocks
  score 1 + commas + min(len/100, 3), propagated to parent (full),
  grandparent (½) and great-grandparent (⅙); candidate scores start from a
  tag/class weight and are scaled by (1 − link density); the best candidate
  plus siblings scoring ≥ max(10, 20 % of it) or text-rich paragraphs form
  the article. Output goes through a tag/attribute allow-list (links and
  images absolutised, only http/https/mailto/`data:image` URLs kept;
  link-heavy lists/containers dropped). Title from `og:title`, `<title>`
  (site suffix stripped) or `h1`; byline from `author` meta or byline
  elements; site from `og:site_name` or the host.
- The Reader View button appears when the article has ≥ 400 characters of
  non-link text. Ctrl+Alt+R toggles. The reader page is a self-contained
  document with a strict CSP (`default-src 'none'`, no scripts) loaded with
  `load_alternate_html` under the original URI; it uses a clean stylesheet
  (max 42em column, comfortable line height, sans or serif per
  `font-style`, light or dark per `color-scheme`; dark whenever the system
  style is dark). Leaving reloads the original page. Reader page loads are
  not recorded as visits and keep the original page's security indicator.

### Web applications
- "Install Site as Web Application…" uses the XDG DynamicLauncher portal:
  `PrepareInstall("", title, icon, {launcher_type: 2 (web app), target:
  URL, editable_name, editable_icon, modal, handle_token})`, waits for the
  Request `Response`, then `Install(token, "org.helix.Web.WebApp_ID.desktop",
  entry, {})`. Icon: the site's favicon as PNG, else the app icon from the
  theme. ID = sanitised chosen name + 6 hex digits of a hash of the URL.
  The desktop entry (Name/Icon are added by the portal):
  `Exec=<helix-web path> --web-app=ID "URL"` (Exec quoting per the Desktop
  Entry spec, `%` doubled, line breaks removed), `StartupWMClass` = app ID,
  `Categories=Network;GTK;`, `X-Helix-Web-App-Url=URL`.
- Web-app mode: application ID `org.helix.Web.WebApp_ID` (own instance),
  data (cookies, history, permissions) in
  `$XDG_DATA_HOME/helix-web/web-apps/ID/`, cache in
  `$XDG_CACHE_HOME/helix-web/web-apps/ID/`; no session restore. The window
  has no address entry (title + host instead), no bookmarks or tab-overview
  buttons; menu: zoom, Find, Print, Fullscreen, Open in Browser,
  Uninstall Web Application… (portal `Uninstall`, then quit; site data is
  kept), Web Application Settings…, About. User-initiated navigations and
  pop-ups to other sites (anything not on the app's host or its
  subdomains, nor on an extra allowed site) open in the default browser.
- Extra allowed sites (like Epiphany's `additional-urls`): edited in Web
  Application Settings… (an `AdwPreferencesDialog` with an "Additional
  Allowed Sites" list, per-entry remove buttons and an "Add site" entry
  row that turns red on invalid input), stored in
  `$XDG_DATA_HOME/helix-web/web-apps/ID/web-app.json` as
  `{"additional-urls": [...]}`. Entries are canonicalised to
  `[scheme://][*.]host[:port]` (lower case, IDNA, path dropped, default
  port dropped; http/https only, no user info). Matching: no scheme →
  http and https; a scheme pins it; `*.host` → the host and all its
  subdomains, else the exact host; no port → the link's default port
  only, else exactly that port.

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
  After crash / Never), Default zoom; Reader Mode: Font Style (Sans /
  Serif), Color Scheme (Light / Dark).
- Search: default engine (combo), engine list with remove, add engine
  (name + URL template with `%s`).
- Privacy: Enable JavaScript, Block pop-up windows, Cookies (Always
  accept / Block third-party (default) / Block all); Passwords (Remember
  Passwords, Saved Passwords…); Clear Browsing Data… (cookies & site data,
  cache, history — all time).

### Settings storage
- All preferences are written to `$XDG_CONFIG_HOME/helix-web/settings.json`.
  When the GNOME schemas are installed, these keys (names, types, enums
  and defaults as in the real `org.gnome.epiphany` schema) are also read
  from / written to GSettings so settings carry over:
  - `org.gnome.Epiphany` (`/org/gnome/epiphany/`): `homepage-url` (s;
    empty = start page), `default-search-engine` (s, engine name),
    `search-engine-providers` (aa{sv}: `name`, `url` with `%s`, `bang`),
    `restore-session-policy` (enum `EphyPrefsRestoreSessionPolicy`:
    `always` | `crashed`).
  - `org.gnome.Epiphany.web` — relocatable, opened at
    `/org/gnome/epiphany/web/` as Epiphany does for the browser itself:
    `enable-popups` (b), `remember-passwords` (b), `default-zoom-level` (d).
  - `org.gnome.Epiphany.reader` (`/org/gnome/epiphany/reader/`):
    `font-style` (enum `sans` | `serif`), `color-scheme` (enum `light` |
    `dark`).
- Not in Epiphany's schema, so only in `settings.json`: the JavaScript
  toggle (Epiphany has no such key), the cookie policy (Epiphany has no
  cookie-policy key), and the restore policy `never` (Epiphany's enum only
  has `always` / `crashed`; while `never` is selected the GSettings key is
  left untouched).
- `data/org.gnome.Epiphany.gschema.xml` is a GNOME-compatible subset of
  exactly these keys for systems without Epiphany.

### Packaging files
- `data/apparmor.d/helix-web` installs to `/etc/apparmor.d/helix-web`
  (load with `apparmor_parser -r`). It attaches to
  `/usr/bin/helix-web` and `/usr/local/bin/helix-web`, is
  `flags=(unconfined)` and only grants `userns` (bubblewrap and WebKit's
  helper processes are children of helix-web and inherit it), with
  `include if exists <local/helix-web>` for site additions. The
  in-app failure page generates the same profile with the running binary's
  path added; a unit test keeps the shipped file identical to the generator.
- `data/org.helix.Web.desktop` → `/usr/share/applications/`.
- `data/org.gnome.Epiphany.gschema.xml` → `/usr/share/glib-2.0/schemas/`
  only where Epiphany's schema is not installed.

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
| Ctrl+Alt+R | Reader view |
| Ctrl+, | Preferences |
| Ctrl+Q | Quit |
| Ctrl+Tab, Ctrl+PgUp/PgDn, Alt+1…9 | Tab switching (AdwTabView built-ins) |

## Decisions
- Password manager: implemented with a pure-Rust Secret Service client
  over zbus (no libsecret), Helix schema `org.helix.Web.Password`; tests run
  only against an in-process mock service on a private peer-to-peer
  connection.
- Web apps: "Install Site as Web Application" via the XDG DynamicLauncher
  portal, with a web-app mode (`--web-app=ID URL`).
- Secret Service transfer: encrypted `dh-ietf1024-sha256-aes128-cbc-pkcs7`
  by default (pure-Rust crypto), `plain` only when the service rejects it.
- Web apps get a per-app extra-allowed-sites list (Epiphany's
  `additional-urls`), edited in a Web Application Settings dialog and
  stored with the web app's data.
- Reader mode: pure-Rust readability-style extractor (unit-tested) shown
  with a clean, script-free stylesheet.
- WebKit sandbox on restricted hosts: ship an AppArmor profile granting
  `userns` (`data/apparmor.d/helix-web`) and detect the failure up front,
  showing an explanatory status page; the sandbox is never disabled.
- Search engine / JavaScript / other keys aligned with the real
  `org.gnome.epiphany` schema; keys Epiphany does not have (JavaScript,
  cookie policy, restore `never`) stay in `settings.json`.
