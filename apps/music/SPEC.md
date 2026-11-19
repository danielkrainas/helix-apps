# Music

Drop-in replacement for GNOME Music.

- **App ID**: `org.helixdesktop.Music`
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
  case-insensitive). Covers are scaled to at most 512 px (sharp at Now
  Playing's 256 logical px on scale-2 screens) and cached in
  `$XDG_CACHE_HOME/helix-music/covers-512/<hash>.jpg` (opaque images) or
  `.png` (with transparency); identical images share one file. The old
  256 px cache (`covers/`) is not reused: tracks whose cover is missing
  from the new folder are read again on the next scan (date added kept)
  and the old folder is deleted. Covers are loaded at the size they are
  shown times the window's scale factor, rounded up to 64/128/256/512 px,
  so small covers do not hold large images.
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
- Feedback: the first scan's status page counts the songs handled so far
  ("N songs found", updated at most every 200 ms from the scan thread);
  later scans show a spinner (`AdwSpinner`) in the header bar. After
  *Rescan Library* a toast says "Library updated: N songs" or "No
  changes found"; rescans triggered by file changes are only announced to
  screen readers.

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
- **Genres** come from the genre tags, split at `;`, grouped case- and
  accent-insensitively (the first spelling found is shown) and sorted by
  name. A genre lists its songs in album order and the albums containing
  at least one of them.
- **Sorting** (Albums and Artists views, remembered): albums by *Title*
  (the default above), *Artist* (then year, undated last), *Year*
  (newest first, undated last), *Date Added* (newest song first) or *Most
  Played* (sum of the album's play counts); artists by *Name*, *Most
  Albums* or *Most Played*. Ties keep the default order.

### Window
- `AdwApplicationWindow` with an `AdwToolbarView`. Header bar: a search
  toggle (Ctrl+F) on the left, an `AdwViewSwitcher` (Albums, Artists, Songs,
  Playlists, Genres) as title, primary menu on the right (*New Playlist…*,
  *Rescan Library*, *Import Playlist…*, *Import Playlists from Search
  Index*, *Preferences*, *Keyboard Shortcuts*, *About Music*). Next to the
  menu, buttons for the visible view: *Sort* (Albums, Artists), *Select*
  (Songs), *New Playlist…* (`list-add-symbolic`, Playlists).
- The minimum size is 360×294. Below 550 sp wide the window switches to
  its narrow layout: the header shows the title "Music" and the views move
  to an `AdwViewSwitcherBar` at the bottom; the player bar takes two rows
  (cover, title, Previous / Play / Next and a *More Controls* button whose
  popover holds favourite, shuffle, repeat, volume and queue; then the
  seek slider across the whole width), so every control stays reachable
  and seeking stays usable; album and playlist headers
  put a 128 px cover above the text, their buttons wrapping onto more
  lines; the Songs table hides its Album, Genre and Plays columns (and
  scrolls sideways rather than widening the window); Now Playing's queue
  becomes an overlay sidebar.
- Content is an `AdwNavigationView`; the root page is the view stack, album,
  artist, playlist and *Now Playing* pages are pushed on top (Back/Alt+Left/
  Escape pops).
- Empty library: `AdwStatusPage` "No Music Found" with the folder path and a
  button to add a folder. While the first scan runs a spinner/status shows.
- **Covers** everywhere are rounded (8 px, 6 px below 96 px) with a light
  shadow and fill their square (cropped). Missing art shows a tinted tile
  with a small note icon (40 % of the size, 16–64 px) instead of a full-size
  glyph, in light and dark styles.
- **Albums**: a grid of 160 px cover tiles (cover, album title, artist,
  all on the cover's left edge, the text ellipsized at the cover's width),
  with 12 px outer padding and gutters; at least two columns. Activating
  a tile opens the album page; right-click or long-press opens its menu
  (*Play*, *Shuffle*, *Play Next*, *Add to Queue*, *Add to Playlist…*,
  *Go to Artist*).
- **Artists**: a list (initials avatar, name, album count), only as tall
  as its rows; activating opens the artist page.
- **Genres**: a list (name, album and song counts); activating opens the
  genre page: header (placeholder tile, name, counts, Play / Shuffle /
  menu), the genre's albums as tiles, then its songs. The narrow bottom
  bar has room for four views only (each switcher button is as wide as its
  wide label), so on narrow windows Genres leaves the bar and becomes a
  *Genres* row above the Artists list (Alt+5 too) that opens the same list
  as a page.
- **Songs**: a `GtkColumnView` with a *Playing* column (the playing
  track's cell shows a play icon named "Now Playing"), sortable columns
  Title, Artist, Album, Length, Plays and a *Favorite* column. Activating a row plays the full song
  list (in the current sort order) starting there. The playing icon, play
  counts and favourites update in place as they change (the table's cells
  are registered like track rows; the model is not rebuilt). A sortable
  *Genre* column (songs without one last) follows Album; a last column
  holds each row's menu button, and right-click or long-press on a row
  opens the same track menu at the pointer (Menu / Shift+F10 on a focused
  row too). *Select* (header) turns on selection mode: rows are selected
  by click, Ctrl+click, Shift+click or Ctrl+A instead of played, and an
  action bar shows "N songs selected" with *Play*, *Add to Queue* and
  *Add to Playlist…* (each ends selection mode, as does Escape or leaving
  the view). The bar's buttons and count shrink (ellipsize) so the hidden
  bar never makes the window wider than 360 px.
- **Playlists**: smart playlists first — *Favorite Songs*, *Most Played*
  (top 50 with ≥ 1 play), *Recently Played* (last 50 played),
  *Recently Added* (50 newest) — then user playlists. Empty smart
  playlists stay listed but dimmed. *New Playlist…* (header button while
  the view is shown, and the main menu) asks for a name. Playlist page:
  Play / Shuffle buttons and a menu (*Play Next*, *Add to Queue*, *Add to
  Playlist…* for all its songs), track list; for user playlists a
  `view-more-symbolic` *Playlist Menu* in the page's header bar holds
  *Rename…*, *Export…* and, in its own section, *Delete…* (kept out of
  the Play row). Rows of a user playlist can be dragged onto another row
  to move there, or moved with *Move Up* / *Move Down* in their menu;
  the playlist file is saved at once. Long track lists get their first
  100 rows at once and the rest in idle batches of 200.
- **Search** (type-to-search or Ctrl+F): case- and accent-insensitive
  substring match over title, artist, album artist, album, genre; results page
  shows matching Albums, Artists and Songs sections. Escape closes search.
  Each section heading shows its number of matches. The first 50 albums,
  50 artists and 200 songs are listed; *Show All* adds the remaining
  albums or artists in place, and for songs opens "Songs Matching “…”", a
  page listing every match in a `GtkListView` (rows built only on
  screen). Playing a song from the results plays all matching songs.
- **Album page**: large cover, title, artist (a link to the artist page,
  unless "Various Artists"), year, track count and total length, Play and
  Shuffle buttons and a menu (*Play Next*, *Add to Queue*, *Add to
  Playlist…* for the album), track list grouped by disc ("Disc 2" headers
  when there is more than one disc).
- **Artist page**: the artist's initials (`AdwAvatar`) instead of an album
  cover, name, album and song counts, Play / Shuffle / menu; their albums
  as cover tiles (as in Albums, with the same menu); then each album's
  title (a link to the album) with the artist's tracks on it.
- Track rows everywhere: number/position, title, artist (when it differs),
  length, favourite heart toggle, and a menu: *Play Next*, *Add to Queue*
  (not in the queue), *Add to Playlist…*; *Go to Album*, *Go to Artist*;
  *Song Details*, *Show in Files*; for user playlist and queue rows *Move
  Up*, *Move Down* and *Remove from Playlist* / *Remove from Queue*.
  Right-click, long-press, Menu or Shift+F10 on a row opens it too.
  The currently playing track shows a speaker icon.
- **Song Details** (`AdwDialog`): *Song* (title, artist, album artist,
  album, genre, year, track, disc, length), *File* (format, bit rate,
  sample rate, bit depth, channels, size, location with a *Show in Files*
  button) and *Listening* (plays, last played, date added); values are
  selectable, rows without a value are left out. Technical values are read
  with `lofty` on a worker thread when the dialog opens. *Show in Files*
  uses `GtkFileLauncher::open_containing_folder` (the file manager selects
  the file); failures show a toast.

### Player bar
- Shown at the bottom once anything is queued: cover thumbnail, title and
  artist (clicking opens *Now Playing*), Previous / Play-Pause / Next, elapsed
  time, seek slider, total time, favourite heart, shuffle toggle, repeat
  toggle cycling **None → All → One** (checked for All and One, like
  shuffle; `media-playlist-repeat-symbolic` for None and All,
  `…-repeat-song-symbolic` for One), volume slider in a popover, and a
  queue button opening *Now Playing*.
- **Now Playing** page: an `AdwOverlaySplitView`. Content: large cover,
  title, the artist and the album as links to their pages (insensitive
  when there is no page), or the lyrics (below). Sidebar (end, shown on
  wide windows, an overlay toggled by the header's *Queue* button on
  narrow ones): "Queue", its length, *Save as Playlist…* and *Clear
  Queue*, then the queue in play order as a `GtkListView` (rows recycled;
  the model is rebuilt only when the queue changes). Clicking a row jumps
  to it; rows can be dragged onto another row, moved with *Move Up* /
  *Move Down* or removed from their menu. Without shuffle a move reorders
  the queue itself; with shuffle only the play order.
- Seek slider: while it is dragged (or the trough is held) the knob
  follows the pointer and the elapsed time previews the target; one seek
  (and one MPRIS `Seeked`) happens on release. Arrow keys and other
  keyboard steps seek at once.
- Previous restarts the current track when more than 3 s have elapsed,
  otherwise goes to the previous queue item.

### Lyrics
- Read for the playing song only, on a worker thread, never stored in the
  index: a sidecar `<stem>.lrc` (extension in any case) next to the file,
  else the first lyrics tag (`ItemKey::Lyrics`: ID3 `USLT`, Vorbis
  `LYRICS`, MP4 `©lyr`, APE…).
- Text with LRC time tags (`[mm:ss]`, `[mm:ss.xx]`, several per line,
  `[offset:±ms]`; ID tags and enhanced word times ignored) is synced;
  anything else is shown as plain text.
- Limits against crafted files (each time tag copies its line): a sidecar
  or tag over 1 MiB is ignored; at most 100 time tags per line, 10 000
  synced lines and 4 MiB of synced text are kept (the rest is dropped).
- Now Playing shows a *Lyrics* toggle (`media-view-subtitles-symbolic`)
  when the song has lyrics; on, the lyrics replace the cover. Synced
  lyrics highlight the current line (accent colour) from the position
  updates and keep it in the middle of the view; plain lyrics are
  selectable. The toggle's state carries over to the next song.

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
- **Queue restore**: the queue (items, play order, current item) and the
  position in the current song are saved to
  `$XDG_DATA_HOME/helix-music/queue.json` a second after the queue or
  track changes, on pause/stop, when the window closes and at shutdown.
  On start the saved queue is restored after the index loads, dropping
  songs no longer in the library (if the current song went, the next one
  becomes current and starts from the beginning); nothing plays, the
  player bar shows the song and position, MPRIS says `Paused`, and Play
  (or Space, MPRIS Play) continues from the saved position (seeking once
  the stream has started).
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
  (Identity "Music", DesktopEntry `org.helixdesktop.Music`, Raise, Quit) and
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
`notifications` (bool), `gnome_music_imported` (bool),
`background_playback` (bool, default false), `album_sort`
(`title`/`artist`/`year`/`added`/`plays`) and `artist_sort`
(`name`/`albums`/`plays`). The queue is kept in
`$XDG_DATA_HOME/helix-music/queue.json` (see *Queue restore*).

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

### Import from GNOME Music (one time)
GNOME Music keeps favourites and user playlists in a private Tracker 3
database (Nepomuk ontology), not in the LocalSearch miner.
- **Location** (general knowledge; not verifiable here because no such
  database exists on this machine): `$XDG_CACHE_HOME/gnome-music/db`
  (native) or `~/.var/app/org.gnome.Music/cache/gnome-music/db` (Flatpak);
  the first directory containing `meta.db` is used.
  `HELIX_MUSIC_GNOME_DB=DIR` overrides it (tests);
  `HELIX_MUSIC_NO_GNOME_IMPORT=1` disables the import.
- **When**: after the first library scan of a session, as long as
  `gnome_music_imported` in `state.json` is false. It is set to true once a
  database was found and read successfully, so the import happens once.
  Without a database Helix simply uses its own store (`stats.json`,
  `playlists/`) and checks again at the next start (existence check only).
- **Read-only access**: tinysparql is not linked, so the `tracker3` CLI is
  spawned (`tracker3 sparql --database DIR --query Q`, argv only, no shell,
  `LC_ALL=C.UTF-8`) on a worker thread with a 20 s timeout per query (the
  process is killed on timeout). To guarantee GNOME Music's files are never
  modified, `meta.db`, `meta.db-wal`, `meta.db-shm` and `ontologies.gvdb`
  are first copied into a private 0700 directory under
  `$XDG_RUNTIME_DIR` and only that copy is opened; it is deleted afterwards.
  Only `SELECT` queries are run.
- **Queries**: favourites are resources with
  `nao:hasTag nao:predefined-tag-favorite`, their file from
  `nie:isStoredAs/nie:url` (or `nie:url`, or the `nie:isStoredAs` IRI);
  playlists are `nmm:Playlist` with `nie:title` and
  `nfo:hasMediaFileListEntry` → `nfo:entryUrl`, ordered by
  `nfo:listPosition`. Each query returns one column built with
  `CONCAT("@", ENCODE_FOR_URI(…), "|", …)` so the CLI's text output parses
  unambiguously.
- **Matching**: by file URI (converted to a path, so percent-encoding
  differences do not matter). References that are not `file://` URIs (song
  resource IRIs) are resolved through LocalSearch
  (`?s nie:isStoredAs/nie:url`, read-only) when it runs. Only songs in the
  Helix library are imported: favourites set the Helix favourite flag;
  each playlist becomes an M3U playlist (skipped if one with the same name
  and songs exists, dropped if none of its songs are in the library). A
  toast reports what was imported.

### Preferences
`AdwPreferencesDialog`: Library folders (the XDG Music folder, read-only,
plus extra folders with remove buttons and "Add Folder…" via
`gtk::FileDialog`, i.e. the FileChooser portal when available; the
description mentions when LocalSearch's index is used), and Playback:
*Inhibit Suspend* switch, *ReplayGain* combo (Off/Track/Album),
*Notifications* switch, *Background Playback* switch ("Keep playing when
the window is closed", Helix only, off by default).

### Keyboard shortcuts
| Keys | Action |
| --- | --- |
| Space / Ctrl+Space | Play / pause (Space only when no text entry has focus) |
| Ctrl+B / Ctrl+N | Previous / next track |
| Ctrl+R | Cycle repeat mode |
| Ctrl+S | Toggle shuffle |
| Ctrl+Left / Ctrl+Right | Seek 10 s back / forward (past the end: next song) |
| Ctrl+Up / Ctrl+Down | Volume up / down by 5 % |
| Ctrl+M | Mute (again: back to the volume before) |
| Ctrl+D | Add the playing song to / remove it from Favorites |
| Ctrl+L | Now Playing |
| Ctrl+F | Search |
| Alt+1…5 | Albums / Artists / Songs / Playlists / Genres |
| F5 | Rescan library |
| Ctrl+, | Preferences |
| Ctrl+? | Keyboard shortcuts |
| Ctrl+Q / Ctrl+W | Quit / close window |
| Escape / Alt+Left | Leave selection mode / close search / go back |

Ctrl+arrows are bubble-phase shortcuts (`add_bubble_shortcuts`): a
focused text entry, slider or list uses them first.

Closing the window quits (playback stops), like GNOME Music. MPRIS `Quit`
closes the window the same way (saving window state). With *Background
Playback* on and a song playing, closing hides the window instead and
holds the application: playback, notifications and MPRIS go on;
activating the app again or MPRIS `Raise` shows the window; it quits for
real when playback stops (end of the queue, Stop), on MPRIS `Quit` or
Ctrl+Q (pausing keeps it running so it can be resumed from the shell's
media controls).

### Command line
- `helix-music` — open the app.
- `helix-music --scan DIR…` — index the given folders into a temporary
  in-memory index and print `artist — album — disc.track — title` lines
  (debugging aid; no files written). Uses the Tracker index for covered
  folders like the app does, and reports how many tracks came from it.
- `helix-music --gnome-music [DB-DIR]` — print the favourites and playlists
  that would be imported from GNOME Music's database (read via a copy;
  debugging aid, nothing written).

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
- **GNOME Music's own playlists/favourites** (user decision, round 4): read
  its private Tracker database read-only through the `tracker3` CLI on a
  worker thread with a timeout and import once; fall back to Helix's own
  store when it is absent (see *Import from GNOME Music*).

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): the track that starts playing ("Now playing: Title by Artist"), once per track; playback errors show a toast.
- Recycled list rows (queue, all search matches) say "Now playing" as
  their accessible description while their song plays, like list-box
  rows. Header buttons, avatars and covers are named or presentational.
- The seek slider ("Position") gives its value as text ("1:23 of 4:56")
  and the volume slider as a percentage ("75%"). The playing track's row
  in track lists has the accessible description "Now playing" (its play
  icon is presentational). The playlist-name entry is labelled "Playlist
  Name".
