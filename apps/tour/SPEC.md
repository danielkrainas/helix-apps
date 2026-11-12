# Tour

Drop-in replacement for GNOME Tour.

- **App ID**: `org.helix.Tour`
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
  1. **Welcome**: a drawn Helix illustration, the title
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
  `{"completed": true, "outcome": "completed"|"skipped", "version": "0.1.0"}`.
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
- `helix-tour --if-first-run` — exits immediately with status 0 if the
  tour was already completed or skipped; otherwise shows it. Intended for
  session autostart.
- `--help`, `--version`.

### GSettings
None. GNOME Tour itself has no published schema; GNOME Shell's welcome
dialog uses `org.gnome.shell welcome-dialog-last-shown-version`, which is
owned by the shell and is deliberately not written here. Completion lives
in the state file above.

## Open questions
- Should Helix's shell/session autostart `helix-tour --if-first-run`
  (an autostart `.desktop` file), or should the shell launch it like GNOME
  Shell's welcome dialog does? Not shipped yet; the flag is ready either way.
- Should the tour reappear after a major Helix upgrade? The state file
  records the version so this can be added later; for now it shows once.
