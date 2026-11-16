# Help

Drop-in replacement for Yelp.

- **App ID**: `org.helixdesktop.Help`
- **Binary**: `helix-help`
- **Crate**: `apps/help`

## Purpose
View help documentation.

## Main windows
- Document view with sidebar and search.

## Contracts used
- Mallard / DocBook / HTML help formats.
- XDG portals: OpenURI.
- GSettings: `org.gnome.yelp`.

## Non-goals
- Full Mallard editing.

---

## Behaviour (derived spec)

### Clean-room note
Rendering is done by Helix's own Rust transformer (`mallard.rs`,
`docbook.rs`) and its own stylesheet (`style.css`). Nothing from the
yelp-xsl stylesheets, Yelp's CSS or Yelp's code is used. The Mallard
behaviour is derived from the public Mallard specification
(projectmallard.org); DocBook from the public DocBook element reference.

### Window
- One `AdwApplicationWindow` with an `AdwOverlaySplitView`: a **sidebar**
  listing the current document's pages as an outline (guide hierarchy) and
  the **content**, a WebKitGTK 6 `WebView`.
- Header bar: **Back** / **Forward** buttons (sensitive per the WebView's
  history), a **Home** button (the document list), a **sidebar** toggle, a
  **search** toggle, and a primary menu (*Print…*, *Larger Text*,
  *Smaller Text*, *Normal Size*, *Caret Browsing*, *Keyboard Shortcuts*,
  *About Help*). The window title
  follows the page title.
- A search bar below the header. Activating it searches the current
  document (title, description and body text of every page) and shows a
  ranked result page. On the home page it searches all installed documents.
- The WebKit sandbox is always left enabled.

### Sandbox check
- WebKit aborts the whole process when bubblewrap cannot create a user
  namespace, so on activation, before any WebView exists, the app
  re-executes itself with the internal argument `--helix-probe-userns`;
  that child (same binary, same AppArmor profile) does
  `unshare(CLONE_NEWUSER)` and writes its uid/gid maps like bubblewrap.
  Skipped inside Flatpak.
- On failure the window shows only an `AdwStatusPage` "Help Content
  Sandbox Unavailable" explaining the cause (AppArmor restriction via
  `kernel.apparmor_restrict_unprivileged_userns`, user namespaces disabled
  by sysctl, or unknown). For AppArmor it shows and offers to copy a
  profile for the running binary plus the install commands
  (`sudo tee /etc/apparmor.d/helix-help < helix-help.apparmor`,
  `sudo apparmor_parser -r /etc/apparmor.d/helix-help`), and a Quit button.
  Same behaviour as Helix Web.
- If the web process terminates later (not by API), a "Page Crashed"
  status page with a Reload button covers the view.

### URIs
| URI | Meaning |
|---|---|
| `help:` or `help-list:` | Home page: all installed help documents |
| `help:<doc>` | Index page of document `<doc>` |
| `help:<doc>/<page>[#id]` | A page (Mallard page id / DocBook chunk id) |
| `help:<doc>/<file path>` | A resource (image, video) inside the document directory |
| `help:<doc>?search=<q>` | Search results within `<doc>` |
| `help:?search=<q>` | Search across all documents |
| `ghelp:<doc>[?<page>]` | Legacy form, mapped to `help:` |
| `ghelp:/abs/dir[?<page>]`, `file:///…`, `/path`, `./path` | A document outside the help directories (a directory, an `index.page`, any `.page`, `.docbook`/`.xml`, or `.html`) |
- `help:`/`ghelp:` are registered as custom URI schemes on the WebKit
  `WebContext`; content is produced on a worker thread and the request is
  finished on the main thread.
- Plain HTML help is loaded directly through `file://` so its own relative
  links keep working.
- `http`, `https`, `mailto` and other non-help links are not navigated
  inside the viewer; they are opened with `gtk::UriLauncher` (OpenURI
  portal when sandboxed).
- Resource paths are confined to the document directory (canonicalized;
  `..` escapes are refused).

### Discovery
- Search path: `$XDG_DATA_HOME/help` then each `$XDG_DATA_DIRS/*/help`.
- Locale fallback from `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`:
  `pt_BR.UTF-8@euro` → `pt_BR@euro`, `pt_BR`, `pt@euro`, `pt`, then `C`.
  Each document is resolved independently: the first data dir/locale that
  contains `<doc>/` with an entry file wins.
- Entry file, in order: `index.page` (Mallard: every `*.page` in the
  directory), `index.docbook`, `<doc>.xml` (DocBook), `index.html` (HTML).
- Home page lists every discovered document with its title (Mallard index
  title / DocBook title / HTML `<title>`), sorted by title.

### Mallard support
- Pages of type `guide` and `topic`; nested `section`s with ids.
- Titles (`title`, sort title `title type="sort"`), `info/desc` used in link
  lists.
- Automatic topic links on guide pages and guide sections: pages that
  declare `<link type="guide" xref="G"/>` (or `G#section`) plus the guide's
  own `<link type="topic"/>`; placed at the `<links type="topic"/>`
  element(s) or appended at the end. Groups: `links/@groups` and
  `link/@group`; `first` sorts before, `last` after, `default` added
  implicitly; within a group links sort by sort title.
- Section links (`<links type="section"/>`), "See Also" (seealso links are
  reciprocal), "More Information" (the page's guides), and a breadcrumb
  trail following the first guide link up to `index`.
- Blocks: `p`, `list` (bulleted / numbered), `steps`, `terms`, `tree`,
  `code`, `screen`, `synopsis`, `example`, `figure`, `listing`, `quote`,
  `note` with styles (`tip`, `important`, `warning`, `caution`, `advanced`,
  `bug`, `danger`, `sidebar`), `media` (image, video, audio; paths relative
  to the document dir), basic `table`, `comment` hidden.
- Inline: `gui`, `guiseq` (`A ▸ B`), `key`, `keyseq` (`Ctrl+C`), `menu`,
  `app`, `cmd`, `code`, `file`, `sys`, `input`, `output`, `var`, `em`,
  `link` (`xref` → page/section, `href` → external), `span`, inline `media`.
- `if:` conditionals (`if:if`, `if:choose/if:when/if:else`, `if:test`
  attribute): comma = OR, whitespace = AND, `!` negates; true tokens are
  `target:html`, `target:help`, `platform:gnome`, `platform:gnome-shell`,
  `platform:linux`, `platform:helix`; unknown tokens are false.
- `xi:include` of `.xml` fragments (DocBook and Mallard).

### DocBook support (4 and 5)
- `book`, `article`, `part`, `chapter`, `appendix`, `preface`, `section`,
  `sect1`–`sect5`, `simplesect`, `bookinfo`/`articleinfo`/`info` titles.
- Chunking: the root is the index page with a table of contents; each
  `chapter`/`appendix`/`preface` (book) or top-level section (article) is a
  page addressed by its `id`/`xml:id` (`chunk-N` when it has none). Links to
  any other id go to its containing chunk plus `#id`.
- `para`, `simpara`, `formalpara`, `itemizedlist`, `orderedlist`,
  `variablelist`, `programlisting`, `screen`, `literallayout`, `note`,
  `tip`, `important`, `warning`, `caution`, `link` (`linkend` or
  `xlink:href`), `ulink`, `xref`, `email`, `emphasis`, `literal`,
  `filename`, `command`, `guilabel`, `guimenu`, `menuchoice`, `keycombo`,
  `keycap`, `mediaobject`/`imageobject`/`imagedata`, `figure`, basic
  `table`/`informaltable` (`tgroup/thead/tbody/row/entry`), `blockquote`.
- Undeclared named entities (DocBook DTD entities such as `&mdash;`) are
  replaced from a small built-in table before parsing.

### Search
- Terms are lower-cased words. Score per page: phrase in title +20, each
  term in title +10, in description +4, in body +1 per occurrence (capped at
  5). Pages missing any term are dropped. Results sorted by score then
  title, with a description/snippet.

### GSettings (`org.gnome.yelp`)
- `font-adjustment` (i): integer font-size offset in points (not a zoom
  factor), clamped to −3…10 (the range noted in the published schema).
  Body text is 11 pt + n and monospace 10 pt + n, applied as WebKit's
  `default-font-size` / `default-monospace-font-size` (pixels, pt × 96/72);
  Helix's stylesheet sizes body text as `medium` so it follows, and plain
  HTML help using relative sizes follows too. Larger/Smaller Text change it
  by ±1, Normal Size resets it to 0; the page zoom level stays 100 %.
- `show-cursor` (b): caret browsing.

### Shortcuts
| Key | Action |
|---|---|
| Alt+Left / Alt+Right | Back / Forward |
| Alt+Home | Home (document list) |
| Ctrl+F | Search |
| F9 | Toggle sidebar |
| Ctrl+P | Print |
| Ctrl+plus / Ctrl+minus / Ctrl+0 | Larger / smaller / normal text |
| F7 | Caret browsing |
| Ctrl+W, Ctrl+Q | Close / Quit |

### CLI
`helix-help [URI]` — opens the URI (any form in the table above); with no
argument shows `help:gnome-help` if installed, else the home page.

### Packaging files
- `data/apparmor.d/helix-help` installs to `/etc/apparmor.d/helix-help`
  (load with `apparmor_parser -r`). It attaches to `/usr/bin/helix-help`
  and `/usr/local/bin/helix-help`, is `flags=(unconfined)` and only grants
  `userns` (bubblewrap and WebKit's helper processes are children of
  helix-help and inherit it), with `include if exists <local/helix-help>`.
  A unit test keeps it identical to the profile the failure page generates.
- `data/org.helixdesktop.Help.desktop` → `/usr/share/applications/`;
  `data/org.gnome.yelp.gschema.xml` → `/usr/share/glib-2.0/schemas/` only
  where Yelp's schema is not installed.

## Decisions
- `font-adjustment` is an integer font-size offset (points), as in the
  installed `org.gnome.yelp` schema; implemented as WebKit default font
  sizes, not zoom.
- WebKit sandbox on hosts restricting user namespaces: ship an AppArmor
  profile granting `userns` (`data/apparmor.d/helix-help`) and show a
  friendly status page when the sandbox cannot start; never disable it.
