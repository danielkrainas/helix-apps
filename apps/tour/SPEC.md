# Tour

Drop-in replacement for GNOME Tour.

- **App ID**: `org.helixdesktop.Tour`
- **Binary**: `helix-tour`
- **Crate**: `apps/tour`

## Purpose
Welcome and feature tour for new users.

## Main windows
- Multi-page tour.
- Optional first-run checks.

## Contracts used
- GSettings to record completion.
- XDG portals: OpenURI.

## Non-goals
- Complex onboarding flows.

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow`, title "Tour", fixed at 800×640 and not
  resizable (it can still be closed and, by the compositor, maximized —
  content stays centred). Header bar with the window title and a close
  button; no primary menu (About is not reachable from the window, as with
  other single-purpose first-run helpers).
- The window shows one of two views:
  1. **Welcome**: Helix's mark (the official logo), the title
     "Welcome to Helix", a line naming the installed system
     ("You are running *Fedora Linux 41 (Workstation Edition)*."), a short
     invitation, and two buttons: **Start Tour** (suggested, default
     widget) and **No Thanks**.
  2. **Tour**: an `AdwCarousel` of pages, each with an original
     cairo-drawn illustration, a title and a paragraph. Previous/Next
     circular arrow buttons are overlaid on the left/right edges
     (Previous hidden on the first page, Next hidden on the last);
     `AdwCarouselIndicatorDots` sit below the carousel. Swiping and
     scrolling work through the carousel itself.
- Tour pages, in order:
  1. **The Overview** — Super opens the overview with workspace thumbnails
     and live window previews; typing searches; Super+Alt+Up/Down steps
     between desktop, overview and launcher.
  2. **Launching Apps** — Apps button in the top panel or Super+A; type to
     search, arrows and Enter to launch; open windows are listed first.
  3. **Workspaces** — Super+Page Up/Down switches, Shift moves the focused
     window along; dots in the panel; by default an empty workspace is
     always waiting at the end.
  4. **Managing Windows** — drag header bars; drag to the left/right edge
     to fill half the screen, to the top edge to maximize;
     Super+Left/Right/Up/Down; Alt+Tab switches apps; Alt+Space window menu.
  5. **Touchpad Gestures** — three- or four-finger sideways swipes switch
     workspaces; tap-to-click, natural scrolling and speed are set in
     Settings.
  6. **Getting Help** — the Help app (search "Help" in the launcher), the
     Help item / F1 in apps, and a **Learn More About Helix** button opening the Helix
     project page through `gtk::UriLauncher` (OpenURI portal when
     sandboxed).
  7. **That's it! Have a nice day** — closing text and a suggested
     **Close** button. Only this page and the welcome page have buttons
     that end the tour.
- Text only describes features the Helix desktop actually has (checked
  against the helix-desktop README); touchscreen gestures are not
  mentioned because Helix does not have them yet.
- Illustrations: flat vector drawings in a 360×220 area, drawn with cairo
  in `illustrations.rs`; colours switch between a light and a dark palette
  following `AdwStyleManager:dark`. They are decorative (accessible role
  `presentation`).

### Keyboard
| Keys | Action |
|---|---|
| Left / Right | Previous / next tour page (also on welcome: Right starts the tour) |
| Enter | Activates the focused button (Start Tour by default on welcome) |
| Escape | Same as No Thanks on the welcome page; on tour pages, leaves the tour (counts as skipped) |
| Ctrl+W / Ctrl+Q | Close / quit (counts as skipped) |

### First-run state
- Stored as JSON at `$XDG_CONFIG_HOME/helix-tour/state.json`:
  `{"completed": true, "outcome": "completed"|"skipped", "version": "0.8.0"}`.
  `version` is the **last-seen version**: the helix-tour/Helix version that
  was running when the tour last ended (any outcome).
- Finishing the tour (Close on the last page) records `completed`;
  No Thanks, Escape, or closing the window at any point records `skipped`.
  Both set `completed: true`. Already-recorded state is never downgraded:
  skipping after a completion keeps `completed` as the outcome.
- A missing, empty or unparsable file means "not completed".
- Writes go to a temporary file in the same directory and are renamed into
  place.

### OS name
- Read `/etc/os-release`, falling back to `/usr/lib/os-release`
  (freedesktop os-release spec). Values may be unquoted, single- or
  double-quoted; within double quotes `\"`, `\\`, `` \` `` and `\$` are
  unescaped. Comments and blank lines are ignored.
- Display name: `PRETTY_NAME`, else `NAME` + ` ` + `VERSION` (or
  `VERSION_ID`), else `NAME`, else nothing (the line is omitted).

### Command line
- `helix-tour` — always shows the tour.
- `helix-tour --autostart` — exits immediately with status 0 unless the
  tour is due: it has **never run** (no `completed` state), or the
  last-seen version's **major** number is lower than the current version's
  major number (numeric comparison of the leading integer, optional `v`
  prefix; `0.9` → `1.0` re-shows, `1.0` → `1.4` does not, a downgrade does
  not). A missing/unparsable last-seen version counts as older; an
  unparsable current version never re-shows a seen tour. When shown, the
  outcome rewrites the last-seen version, so it shows once per major.
  `--if-first-run` is accepted as an alias (earlier name).
- `--help`, `--version`.

### Autostart (first login)
- The **session**, not the shell, launches the tour. The app ships an XDG
  autostart entry `data/org.helixdesktop.Tour-autostart.desktop`
  (`Exec=helix-tour --autostart`, `TryExec=helix-tour`, `NoDisplay=true`,
  `OnlyShowIn=Helix;`), installed as
  `$sysconfdir/xdg/autostart/org.helixdesktop.Tour.desktop` (normally
  `/etc/xdg/autostart/`). The app decides itself whether to show anything,
  so the entry runs on every login and costs one short-lived process.
- What helix-desktop's session must do: just honour the XDG Autostart spec
  (scan `$XDG_CONFIG_DIRS/autostart` and `$XDG_CONFIG_HOME/autostart`, user
  entries override system ones by file name, respect `Hidden`,
  `OnlyShowIn`/`NotShowIn` against `XDG_CURRENT_DESKTOP` (which contains
  `Helix`), `TryExec`, `X-GNOME-Autostart-enabled`). No Tour-specific
  logic, D-Bus call or GSettings key is needed in the shell or session.
  Users can disable it by putting a `Hidden=true` copy in
  `~/.config/autostart/org.helixdesktop.Tour.desktop`.
- Note: helix-desktop's `helix-session` has an XDG autostart implementation
  (`crates/helix-session/src/autostart.rs`), but it runs only when the
  session config sets `[session] autostart = true` (default `false`);
  packaging/session config must enable it for the tour to appear.

### GSettings
None. GNOME Tour itself has no published schema; GNOME Shell's welcome
dialog uses `org.gnome.shell welcome-dialog-last-shown-version`, which is
owned by the shell and is deliberately not written here. Completion lives
in the state file above.

## Decisions
- Launch: the session (not the shell) starts the tour on first login via
  an XDG autostart `.desktop` running `helix-tour --autostart`.
- Re-show: the tour reappears at autostart when the stored last-seen
  version's major number is lower than the current major.
