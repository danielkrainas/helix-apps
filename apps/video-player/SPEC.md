# Video Player

Drop-in replacement for Showtime.

- **App ID**: `org.helix.VideoPlayer`
- **Binary**: `helix-video-player`
- **Crate**: `apps/video-player`

## Purpose
Play video files.

## Main windows
- Video view with controls.
- Fullscreen.

## Contracts used
- XDG portals: FileChooser, OpenURI, Inhibit.
- MPRIS.
- GStreamer or equivalent.
- GSettings for playback preferences.

## Non-goals
- Full media library.

---

## Behaviour (derived spec)

### Window
- **Single instance**: a unique `GApplication` (`HANDLES_OPEN`). Launching
  the app again (CLI or the desktop file's `Exec=helix-video-player %U`)
  forwards the file to the running instance over D-Bus
  (`org.gtk.Application.Open`) and exits; the running window opens it
  (saving the previous video's position first) and is presented. Without a
  file the running window is just presented. One `AdwApplicationWindow`
  per instance, created on first activate/open. Default size 960×600. Content: an `AdwToastOverlay` around a
  `GtkStack` with three pages:
  - **empty** – header bar + `AdwStatusPage` “Watch Videos” with an
    *Open…* button and the hint “Drag and drop videos here”.
  - **error** – header bar + `AdwStatusPage` “Cannot Play File” with the
    GStreamer error text, or, if GStreamer reported a missing plugin,
    “The required codec is not installed: <codec description>” (from the
    pbutils missing-plugin message). A file that prerolls with no decodable
    video stream while a plugin is missing also gets this page; a missing
    codec for a secondary stream (e.g. audio only) just shows a toast
    “Missing codec: …”. An *Open…* button lets the user pick another file.
  - **video** – a black area showing the video, with overlaid header bar
    (title = file name, Open button, primary menu, fullscreen button) at the
    top and a controls bar at the bottom. A `GtkSpinner` is shown while the
    pipeline is prerolling/buffering.
- The window title is the file name of the current video (or “Video
  Player”).

### Opening videos
- **Command line**: `helix-video-player [FILE|URI]` – the first argument is
  opened (relative paths and `file://` URIs resolved via
  `gio::File::for_commandline_arg` in the launching process, so relative
  paths work when forwarded; other URIs gstreamer can handle are passed
  through). It is handed to `GApplication` as `-- <uri>`, so it reaches the
  `open` handler of the primary instance. `-h/--help` prints usage;
  unknown options exit 2.
- **Open dialog** (Ctrl+O, header button, empty-state button): a
  `gtk::FileDialog` (FileChooser portal when available) with a *Videos*
  (`video/*`) filter and an *All Files* filter.
- **Drag and drop** of a file onto the window opens it (`gdk::FileList`, first
  file wins).
- When a new file is opened the previous one’s position is saved to the
  resume store first.

### Rendering
- Pipeline: `playbin` with
  - `video-sink` = an `appsink` (`sync=true`, `qos=true`, `max-buffers=1`,
    `drop=true`, `enable-last-sample=false`) behind a `videoconvert`
    accepting `video/x-raw` in `BGRx`, `BGRA` or `RGBA` (GPU-free path, no
    `glupload`).
  - `audio-sink` = `$HELIX_AUDIO_SINK` parsed as a bin description if set
    (e.g. `fakesink sync=true`, `fakeaudiosink`), else `autoaudiosink`.
    Tests and smoke runs always set it so nothing reaches the speakers.
  - `audio-filter` = `scaletempo` when available (pitch-preserving speed).
- Frames (both preroll and playing samples) are stored in a single-slot
  “latest frame” mailbox from the streaming thread; a bounded(1) channel
  wakes the main thread, which converts only the newest sample into a
  `gdk::MemoryTexture` (zero-copy: the mapped `gst::Buffer` is wrapped in
  `glib::Bytes`). Intermediate frames that arrive while the main thread is
  busy are dropped; appsink QoS lets decoders skip late frames.
- A custom `gdk::Paintable` exposes the display size (storage width ×
  pixel-aspect-ratio) as its intrinsic size and draws the texture
  letter/pillar-boxed and centred in whatever area it gets (aspect-fit math
  in `fit.rs`). It is shown in a `GtkPicture` (`content-fit=contain`) on a
  black background.

### Controls
- Bottom bar (OSD style): play/pause, elapsed time, seek bar, remaining time
  (`-m:ss`; `h:mm:ss` once the duration is ≥ 1 h), volume menu button
  (popover: mute toggle + volume slider, cubic mapping), playback-speed menu
  (0.5×, 0.75×, 1×, 1.25×, 1.5×, 2×), track menu (Audio Track radio list,
  Subtitles radio list with *None*, *Load Subtitle File…*), fullscreen.
- Seek bar: dragging seeks (key-unit while dragging, accurate on release).
  Hovering shows a popover above the pointer with the time at that point.
  No thumbnail preview (deferred).
- Auto-hide: the header bar and controls fade out (revealers) and the cursor
  is hidden 2 s after the last pointer motion while playing and the pointer
  is not over the controls and no popover/menu is open. Any motion,
  click or key shows them again. Paused/ended videos keep controls visible.
- Double-click on the video toggles fullscreen. Single click does nothing
  but reveal the controls.
- At end of stream the video pauses on the last frame, the play button shows
  “play”, and pressing play restarts from the beginning; the resume entry is
  cleared.

### Tracks and subtitles
- Audio tracks: `n-audio` / `current-audio`; labels from the stream tags
  (`language-code` → language name, `title`, `audio-codec`) falling back to
  “Track N”.
- Subtitles: `n-text` / `current-text`; *None* clears the `text` play-flag.
  Track lists are rebuilt when playbin reports `audio-changed` /
  `text-changed` (forwarded to the main thread as application bus
  messages) and after preroll.
- External subtitles: *Load Subtitle File…* opens a file dialog filtered to
  `.srt`, `.vtt`, `.ass`, `.ssa`, `.sub`; the URI is set as `suburi` (the
  pipeline is brought to READY, reconfigured and resumed at the same
  position and play state).
- Automatic discovery: when a local video is opened, a sibling file with the
  same stem (case-insensitive) and a subtitle extension is loaded
  automatically. Exact `stem.ext` beats `stem.<lang>.ext`; among equals
  the extension priority is srt > vtt > ass > ssa > sub, then name order.

### Playback position memory (resume)
- `$XDG_DATA_HOME/helix-video-player/positions.json`:
  `{"version":1,"entries":{"<uri>":{"position":secs,"duration":secs,"updated":unix}}}`.
- Saved when switching files, closing the window, pausing, and every 10 s
  during playback. Positions < 10 s or within 10 s / 95 % of the end are not
  stored (the entry is removed instead). At most 300 entries; oldest dropped.
- On opening a file with a stored position, playback starts from the
  beginning and a toast “Resume from 12:34?” with a **Resume** button is
  shown for 10 s.

### Screenshot
- *Take Screenshot* (Ctrl+Alt+S, primary menu) saves the current frame at
  its storage resolution as PNG to the XDG Pictures directory (falling back
  to `$HOME`), named `<video stem> <h-mm-ss>.png`; an existing name gets
  ` (2)`, ` (3)` … appended. A toast reports the file name.

### Idle inhibit
- While playing, `gtk::Application::inhibit(window, IDLE, "Playing video")`
  (Inhibit portal under Flatpak); released on pause/stop/close.

### MPRIS
- `zbus` 5 (blocking API, async-io) on a worker thread; bus name
  `org.mpris.MediaPlayer2.HelixVideoPlayer.instance<pid>`, object
  `/org/mpris/MediaPlayer2`.
- `org.mpris.MediaPlayer2`: Raise, Quit; CanQuit/CanRaise true,
  HasTrackList false, Identity “Video Player”, DesktopEntry
  `org.helix.VideoPlayer`, SupportedUriSchemes `file`, SupportedMimeTypes
  (list from the .desktop file), Fullscreen/CanSetFullscreen.
- `org.mpris.MediaPlayer2.Player`: Play, Pause, PlayPause, Stop, Seek,
  SetPosition, OpenUri, Next/Previous (no-ops); PlaybackStatus, Rate
  (0.5–2.0), Metadata (`mpris:trackid`, `mpris:length`, `xesam:title`,
  `xesam:url`), Volume, Position, CanPlay/CanPause/CanSeek/CanControl;
  `Seeked` signal. Property changes are emitted as PropertiesChanged.
- A failure to connect to the session bus is logged and ignored.

### Keyboard shortcuts
| Key | Action |
|---|---|
| Space | Play / pause |
| Left / Right | Seek −10 s / +10 s |
| Up / Down | Volume +5 % / −5 % |
| M | Mute toggle |
| F, F11 | Toggle fullscreen |
| Escape | Leave fullscreen |
| , / . | Previous / next frame (when paused) |
| Ctrl+O | Open |
| Ctrl+Alt+S | Take screenshot |
| Ctrl+W / Ctrl+Q | Close / quit |

Frame step forward uses a GStreamer `Step` event (1 buffer); backward is
an accurate seek to position − one frame duration (from the caps
framerate, 1/25 s when unknown).

### Remembered settings
- `org.gnome.totem` has no volume or playback-speed keys (its keys cover
  colour balance, subtitles, plugins, …), so these are app-private:
  `$XDG_CONFIG_HOME/helix-video-player/settings.json`
  `{"volume": 0.0–1.0 (linear), "rate": 0.5–2.0}` (defaults 1.0 / 1.0;
  out-of-range values are clamped, invalid ones reset).
- Volume (slider, Up/Down keys, MPRIS `Volume`) and speed (speed menu,
  MPRIS `Rate`) changes are written 500 ms after the last change (and on
  window close; written via a temporary file + rename). Mute is not
  remembered.
- At start the volume is applied to `playbin`; the speed is applied to each
  opened video once it has prerolled (loading resets the pipeline to 1×),
  and the speed menu / MPRIS `Rate` show the remembered value from the
  start.
- No GSettings schema is read or shipped.

### Environment variables
- `HELIX_AUDIO_SINK` – GStreamer bin description used as the audio sink.

## Deferred
- Seek-bar thumbnail previews.
- Playlists / next-previous file navigation.
- Hardware-accelerated (GL/dmabuf) frame upload.

## Decisions
- Single instance: files opened from the CLI / desktop file go to the
  running window (unique application with `HANDLES_OPEN`).
- Volume and playback speed are remembered across sessions in an
  app-private JSON file (no suitable `org.gnome.totem` keys exist).
