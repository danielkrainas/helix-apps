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
- Library folders: the XDG Music directory (`G_USER_DIRECTORY_MUSIC`, i.e.
  `XDG_MUSIC_DIR` from `user-dirs.dirs`) plus any extra folders the user
  adds in Preferences. If the XDG Music directory is unset, `~/Music` is
  used. Only music below these folders is shown (like GNOME Music).
- **LocalSearch/Tracker first**: when `org.freedesktop.Tracker3.Miner.Files`
  owns a name on the session bus, the scan worker queries its SPARQL
  endpoint (`org.freedesktop.Tracker3.Endpoint.Query` at
  `/org/freedesktop/Tracker3/Endpoint`, via zbus; tinysparql is not linked).
  Queries are read-only. The result rows arrive on a passed socket fd as
  `i32 n_columns, i32 types[n], i32 end_offsets[n]`, then NUL-terminated
  values (host byte order, type 0 = unbound); `tracker.rs` parses this.
  - A library folder counts as *covered* when the index holds any
    `nfo:FileDataObject` below it (`ASK`); for covered folders every
    `nmm:MusicPiece` whose `nie:isStoredAs/nie:url` starts with the folder
    URI is taken with `nie:title`, `nmm:artist/nmm:artistName`,
    `nmm:musicAlbum` (`nie:title`, `nmm:albumArtist/nmm:artistName`),
    `nfo:genre`, `nie:contentCreated` (year), `nmm:trackNumber`,
    `nmm:musicAlbumDisc/nmm:setNumber`, `nfo:duration` (s) and the file's
    `nrl:added` (initial "date added").
  - Folders Tracker does not cover (e.g. extra folders outside its
    configured locations) are scanned by the built-in scanner below.
  - Tracker stores no cover art, so Helix still extracts covers (embedded
    picture via `lofty`, else folder image) for indexed files, only when a
    file's mtime/size changed.
  - Changes: the worker subscribes to the endpoint's `GraphUpdated` signal
    (classes containing MusicPiece/Playlist/Audio) and triggers the same
    debounced (2 s) rescan as file monitors; file monitors are only set on
    folders the built-in scanner walked.
  - `HELIX_MUSIC_NO_TRACKER=1` disables Tracker. If the miner is absent or a
    query fails, everything falls back to the built-in scanner.
- **Built-in scanner** (fallback): runs on a worker thread. Files are recognised by extension
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
- Index entries record whether their metadata came from Tracker; when a
  folder falls back to the built-in scanner such entries are re-read with
  `lofty` (date added is kept).
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
- **ReplayGain**: setting *Off / Track / Album*. When on, GStreamer's
  `rgvolume` is plugged in as playbin's `audio-filter` and applies the
  stream's ReplayGain tags (`album-mode` false for Track, true for Album;
  `pre-amp` and `fallback-gain` 0 dB). Track ↔ Album switches apply
  immediately; turning it on or off changes the pipeline and applies from
  the next track loaded. Without the `rgvolume` plugin the setting is
  ignored (logged).
- **Notifications**: on every track change while playing (user start or
  gapless advance), if the window is not focused, a `gio::Notification`
  (id `now-playing`, low priority) shows the title, "Artist — Album", the
  cached cover (or `audio-x-generic`) and a *Next* button (`app.next-track`);
  clicking it activates the app (presents the window). Focusing the window
  withdraws it. Can be turned off in Preferences.
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
The bundled `data/org.gnome.Music.gschema.xml` is identical to the published
GNOME Music schema (all five keys are used).
- `window-size` (`ai`, default `[768, 600]`), `window-maximized` (`b`,
  default `true`): window geometry.
- `repeat` (enum `none`/`song`/`all`/`shuffle`): GNOME Music folds shuffle
  into the repeat mode. Helix keeps repeat and shuffle independent; it writes
  `shuffle` when shuffle is on, otherwise the repeat mode, and keeps the
  repeat mode used together with shuffle in its own state file.
- `inhibit-suspend` (`b`, default false): inhibit suspend while playing
  (Preferences switch).
- `replaygain` (`b`, default false): ReplayGain on/off. GNOME's key only
  says "for albums"; Helix stores the Track/Album choice in its state file
  and uses Album when the key is turned on elsewhere.

### Helix-only state
`$XDG_CONFIG_HOME/helix-music/state.json`: extra library folders, volume,
repeat mode while shuffling, `replaygain_mode` (`track`/`album`),
`notifications` (bool).

### Playlists
- Stored in `$XDG_DATA_HOME/helix-music/playlists/`. **M3U is the primary
  format**: new playlists are `*.m3u` (extended M3U, UTF-8, absolute paths,
  `#EXTINF:<secs>,<artist> - <title>` lines, `#PLAYLIST:<name>` for the
  display name). Reading accepts plain and extended M3U/M3U8, BOM, CRLF,
  relative paths and `file://` URIs; other URIs are skipped.
- **XSPF** (`*.xspf`) files in the folder are read and saved in XSPF
  (`<title>`, `<trackList>/<track>` with `<location>` `file://` URIs,
  `<title>`, `<creator>`, `<duration>` in ms); the first usable of several
  `<location>`s is used; relative locations resolve against the file.
- **Import Playlist…** (main menu): opens an M3U/M3U8/XSPF/PLS file via the
  FileChooser portal and copies it into a new M3U playlist; entries not in
  the library are dropped (a toast says so if none remain).
- **Import Playlists from Search Index** (main menu): reads the playlist
  files LocalSearch has indexed (`nmm:Playlist` with
  `nfo:hasMediaFileListEntry` → `nfo:entryUrl`, ordered by
  `nfo:listPosition`; read-only) and imports those with songs in the
  library as M3U, skipping ones already present (same name and songs).
- **Export Playlist…** (button on a user playlist page): saves a copy as
  M3U or XSPF, chosen by the file extension (default `.m3u`).

### Preferences
`AdwPreferencesDialog`: Library folders (the XDG Music folder, read-only,
plus extra folders with remove buttons and "Add Folder…" via
`gtk::FileDialog`, i.e. the FileChooser portal when available; the
description mentions when LocalSearch's index is used), and Playback:
*Inhibit Suspend* switch, *ReplayGain* combo (Off/Track/Album),
*Notifications* switch.

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
  (debugging aid; no files written). Uses the Tracker index for covered
  folders like the app does, and reports how many tracks came from it.

## Decisions
- **Library source**: use LocalSearch/Tracker over D-Bus (zbus SPARQL
  queries, read-only) when the miner runs; fall back to the built-in
  scanner for folders it does not cover or when it is absent.
- **GSettings**: aligned with the real `org.gnome.Music` schema — defaults
  `window-size [768, 600]`, `window-maximized true`; `replaygain` (b) added;
  there is no `notifications` key (the toggle is Helix state).
- **Playlists**: M3U primary (read/write, unit-tested); XSPF read/write;
  PLS import; LocalSearch-indexed playlist files can be imported.
- **ReplayGain**: `rgvolume` with Off/Track/Album.
- **Notifications**: `gio::Notification` on track change when the window is
  not focused.

## Open questions
- **GNOME Music's own playlists/favourites**: GNOME Music keeps user
  playlists in a private Tracker database of its own, not in the
  LocalSearch miner the session bus exposes (that one only lists playlist
  *files*, which can be imported). Reading the private database would need
  tinysparql (not installed) or spawning `tracker3 endpoint --database …`
  as a D-Bus endpoint; no such database exists on this machine to verify
  against, so it is not implemented. Should Helix try the `tracker3
  endpoint` route when the database is present?
