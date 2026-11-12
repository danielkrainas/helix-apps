# Music

Drop-in replacement for GNOME Music.

- **App ID**: `org.helix.Music`
- **Binary**: `helix-music`
- **Crate**: `apps/music`

## Purpose
Music library and player.

## Main windows
- Albums / artists / playlists views.
- Now playing.

## Contracts used
- Tracker / local search for media.
- XDG portals: FileChooser, OpenURI.
- MPRIS.
- GSettings: `org.gnome.Music`.

## Non-goals
- Exact Grilo plugin compatibility.

---

## Behaviour (derived spec)

### Library indexing
- Helix runs its **own indexer** instead of Tracker/LocalSearch (see Open
  questions). Folders indexed: the XDG Music directory
  (`G_USER_DIRECTORY_MUSIC`, i.e. `XDG_MUSIC_DIR` from `user-dirs.dirs`)
  plus any extra folders the user adds in Preferences. If the XDG Music
  directory is unset, `~/Music` is used.
- The scan runs on a worker thread. Files are recognised by extension
  (`mp3 flac ogg oga opus m4a mp4 aac wav wv ape aif aiff mpc spx`), hidden
  files and directories are skipped, symlinked directories are not followed.
- Tags are read with the pure-Rust `lofty` crate: title, artist, album
  artist, album, genre, year, track number, disc number, compilation flag,
  duration. Missing title falls back to the file stem, missing artist to
  "Unknown Artist", missing album to "Unknown Album".
- **Incremental rescan**: each indexed file stores its mtime and size; on
  rescan only new or changed files are re-read, vanished files are dropped.
  The "date added" of an unchanged/updated file is preserved.
- **Cover art**: embedded front cover (or the first embedded picture), else a
  folder image (`cover`, `folder`, `front`, `album` with `.jpg/.jpeg/.png`,
  case-insensitive). Covers are scaled to 256 px and cached as PNG in
  `$XDG_CACHE_HOME/helix-music/covers/<hash>.png`; identical images share
  one file.
- **Storage**: `$XDG_DATA_HOME/helix-music/library.json` (index) and
  `stats.json` (play counts, last played, favourites, keyed by path). JSON
  was chosen over a bundled SQLite: the entire library has to be in memory
  for grouping, sorting and search anyway, ten thousand tracks serialise to
  a few MB that load in well under a second, writes are atomic
  (write temp + rename), and no C library has to be compiled in.
- A rescan happens on start-up and from the menu (*Rescan Library*, F5).
  The library folders are also watched with `gio::FileMonitor` (top-level
  and sub-directories); changes trigger a debounced (2 s) incremental rescan.

### Grouping rules
- Albums group by **album artist + album title** (case-insensitive) when an
  album artist tag exists. Without one, tracks group by album title + parent
  folder; if such a group has several track artists (or a compilation flag)
  its artist is shown as **"Various Artists"**, otherwise the track artist.
- Tracks inside an album sort by disc number (missing = 1), track number
  (missing sorts last), then title. Albums sort by title (case-insensitive,
  leading "The " ignored), then artist.
- An album's year is the most common non-empty year; its cover is the first
  track cover found; its duration is the sum.
- **Artists** list every album artist and every track artist (except the
  synthetic "Various Artists"). An artist page shows each album containing
  that artist: all tracks when the artist is the album artist, otherwise only
  the tracks by that artist (appearances on compilations).

### Window
- `AdwApplicationWindow` with an `AdwToolbarView`. Header bar: a search
  toggle (Ctrl+F) on the left, an `AdwViewSwitcher` (Albums, Artists, Songs,
  Playlists) as title, primary menu on the right (*Rescan Library*,
  *Preferences*, *Keyboard Shortcuts*, *About Music*).
- Content is an `AdwNavigationView`; the root page is the view stack, album,
  artist, playlist and *Now Playing* pages are pushed on top (Back/Alt+Left/
  Escape pops).
- Empty library: `AdwStatusPage` "No Music Found" with the folder path and a
  button to add a folder. While the first scan runs a spinner/status shows.
- **Albums**: a grid of cover tiles (cover, album title, artist). Activating
  a tile opens the album page.
- **Artists**: a list (name, album count); activating opens the artist page.
- **Songs**: a `GtkColumnView` with sortable columns Title, Artist, Album,
  Length, Plays and a favourite column. Activating a row plays the full song
  list (in the current sort order) starting there.
- **Playlists**: smart playlists first — *Favorite Songs*, *Most Played*
  (top 50 with ≥ 1 play), *Recently Played* (last 50 played),
  *Recently Added* (50 newest) — then user playlists. A "New Playlist" button
  asks for a name. Playlist page: Play / Shuffle buttons, track list; user
  playlists also offer Rename, Delete and per-track Remove.
- **Search** (type-to-search or Ctrl+F): case- and accent-insensitive
  substring match over title, artist, album artist, album, genre; results page
  shows matching Albums, Artists and Songs sections. Escape closes search.
- **Album page**: large cover, title, artist, year, track count and total
  length, Play and Shuffle buttons, track list grouped by disc ("Disc 2"
  headers when there is more than one disc).
- Track rows everywhere: number/position, title, artist (when it differs),
  length, favourite heart toggle, and a menu with *Add to Queue*,
  *Play Next*, *Add to Playlist…*, *Go to Album*, *Remove from Playlist*
  (user playlist) / *Remove from Queue* (Now Playing).
  The currently playing track shows a speaker icon.

### Player bar
- Shown at the bottom once anything is queued: cover thumbnail, title and
  artist (clicking opens *Now Playing*), Previous / Play-Pause / Next, elapsed
  time, seek slider, total time, favourite heart, shuffle toggle, repeat
  button cycling **None → All → One**, volume slider in a popover, and a
  queue button opening *Now Playing*.
- **Now Playing** page: large cover, title, artist, album, and the queue
  (in play order); clicking a queue row jumps to it, each row can be removed.
- Previous restarts the current track when more than 3 s have elapsed,
  otherwise goes to the previous queue item.

### Playback
- GStreamer `playbin` (audio only; video/text flags off). Gapless playback:
  the next URI is set from the `about-to-finish` signal, the queue advances
  on the following `stream-start` message.
- Output sink: `HELIX_AUDIO_SINK` (a `gst-launch` bin description such as
  `fakesink` or `fakeaudiosink`) overrides the default `autoaudiosink`.
  Tests and smoke runs always set it.
- Queue: the list started from a view; shuffle produces a random permutation
  with the current track first, so no track repeats before all have played;
  when repeat-all wraps, a fresh permutation is drawn (not starting with the
  track that just played, when possible). Repeat One replays the current
  track on natural end, but Next/Previous still move.
- **Play counts** increase once per playback when the track has played half
  its length or 4 minutes, whichever comes first (time actually spent
  playing; seeking does not count). "Last played" is updated at the same
  moment. With nothing queued, Play (button, Space or MPRIS) plays the whole
  library in album order.
- Volume uses a cubic curve (slider position → playbin linear volume).
- While playing, the app **inhibits suspend** via
  `gtk::Application::inhibit(SUSPEND)` when the `inhibit-suspend` key is
  true.
- Playback errors show a toast and skip to the next track.

### MPRIS
- Bus name `org.mpris.MediaPlayer2.HelixMusic`, object
  `/org/mpris/MediaPlayer2`, interfaces `org.mpris.MediaPlayer2`
  (Identity "Music", DesktopEntry `org.helix.Music`, Raise, Quit) and
  `org.mpris.MediaPlayer2.Player` (Play/Pause/PlayPause/Stop/Next/Previous/
  Seek/SetPosition, PlaybackStatus, LoopStatus, Shuffle, Volume, Metadata
  with `mpris:trackid`, `mpris:length`, `mpris:artUrl`, `xesam:title`,
  `xesam:artist`, `xesam:album`, `xesam:albumArtist`, `xesam:trackNumber`,
  `xesam:url`, Position, Can* flags, Seeked signal).
- Registered on the session bus from a worker thread; failures are logged and
  ignored. `HELIX_MUSIC_NO_MPRIS=1` disables it.

### GSettings (`org.gnome.Music`)
- `window-size` (`ai`), `window-maximized` (`b`): window geometry.
- `repeat` (enum `none`/`song`/`all`/`shuffle`): GNOME Music folds shuffle
  into the repeat mode. Helix keeps repeat and shuffle independent; it writes
  `shuffle` when shuffle is on, otherwise the repeat mode, and keeps the
  repeat mode used together with shuffle in its own state file.
- `inhibit-suspend` (`b`): inhibit suspend while playing (Preferences switch).

### Helix-only state
`$XDG_CONFIG_HOME/helix-music/state.json`: extra library folders, volume,
repeat mode while shuffling. Playlists: `$XDG_DATA_HOME/helix-music/playlists/
*.m3u` (extended M3U, UTF-8, absolute paths, `#EXTINF:<secs>,<artist> -
<title>` lines, `#PLAYLIST:<name>` for the display name).

### Preferences
`AdwPreferencesDialog`: Library folders (the XDG Music folder, read-only,
plus extra folders with remove buttons and "Add Folder…" via
`gtk::FileDialog`, i.e. the FileChooser portal when available), and the
*Inhibit Suspend* switch.

### Keyboard shortcuts
| Keys | Action |
| --- | --- |
| Space / Ctrl+Space | Play / pause (Space only when no text entry has focus) |
| Ctrl+B / Ctrl+N | Previous / next track |
| Ctrl+R | Cycle repeat mode |
| Ctrl+S | Toggle shuffle |
| Ctrl+F | Search |
| Alt+1…4 | Albums / Artists / Songs / Playlists |
| F5 | Rescan library |
| Ctrl+, | Preferences |
| Ctrl+? | Keyboard shortcuts |
| Ctrl+Q / Ctrl+W | Quit / close window |
| Escape / Alt+Left | Close search / go back |

Closing the window quits (playback stops), like GNOME Music. MPRIS `Quit`
closes the window the same way (saving window state).

### Command line
- `helix-music` — open the app.
- `helix-music --scan DIR…` — index the given folders into a temporary
  in-memory index and print `artist — album — disc.track — title` lines
  (debugging aid; no files written).

## Open questions
- **LocalSearch/Tracker**: should Helix query LocalSearch/Tracker over D-Bus
  (SPARQL via `org.freedesktop.Tracker3.Endpoint`) instead of running its own
  indexer? The own indexer works without the daemon and is what is
  implemented now; using Tracker would share the index with other apps.
- **GSettings key list**: the `org.gnome.Music` schema is not installed on
  this system, so the keys above (`window-size`, `window-maximized`,
  `repeat`, `inhibit-suspend`) were written from memory of the published
  schema. Please confirm types/nicks; `replaygain` and `notifications` are
  not used.
- **Playlist format**: M3U was chosen (simple, widely supported). GNOME Music
  stores playlists in Tracker; is there a need to import those or to use
  XSPF instead?
- **ReplayGain** (`rgvolume`) and desktop notifications for track changes
  are not implemented.
