# Audio Player

Drop-in replacement for Decibels.

- **App ID**: `org.helix.AudioPlayer`
- **Binary**: `helix-audio-player`
- **Crate**: `apps/audio-player`

## Purpose
Play audio files.

## Main windows
- Simple player with playlist or queue.
- File open.

## Contracts used
- XDG portals: FileChooser, OpenURI.
- MPRIS for media control.
- GStreamer or equivalent.

## Non-goals
- Full music library management (that is the Music app).

---

## Behaviour (derived spec)

Helix Audio Player is a deliberately small, single-file player in the spirit
of Decibels: open one file, see its waveform, play it. There is no playlist
or queue (see *Decisions*).

### Window
- One `AdwApplicationWindow` (default 600×420, minimum 360×300). The header
  bar has an **Open** button (start) and a primary menu (end) with *Open…*,
  *Keyboard Shortcuts* and *About Audio Player*.
- The content is a stack of three pages:
  - **Empty** (`AdwStatusPage`, icon `audio-x-generic-symbolic`): "Open an
    Audio File" / "Drag and drop a file here", with a pill **Open…** button.
  - **Player**: cover art (embedded tag image, or a generic audio icon),
    title, then "artist — album" (only present parts), the **waveform**, an
    elapsed / remaining time row, and the control row.
  - **Error** (`AdwStatusPage`, icon `dialog-error-symbolic`): "Unable to
    Play File" with the GStreamer error text (or "The file contains no audio
    stream") and an **Open Another File…** button.
- The window title is the track title (falls back to the file name); with no
  file it is "Audio Player".
- Dropping a file (`GdkFileList`) anywhere on the window opens it, replacing
  the current one.

### Opening and metadata
- A file comes from argv, the Open dialog (`GtkFileDialog`, filter "Audio
  files" = `audio/*`, plus "All files"; goes through the FileChooser portal
  when sandboxed), drag and drop, or MPRIS `OpenUri`.
- Opening starts playback immediately from the start at 1× rate; volume,
  mute and repeat are kept for the session (not saved).
- Title / artist / album / cover image come from GStreamer tags posted by
  the playback pipeline (`title`, `artist` falling back to `album-artist`,
  `album`, `image` or `preview-image`). Missing title → file display name
  without extension.
- If the pipeline posts an error, or prerolls with zero audio streams, the
  error page is shown and playback stops.

### Playback
- `playbin` with `video-sink` and `text-sink` = `fakesink` (audio only).
  The audio sink is the default (`autoaudiosink`) unless the environment
  variable `HELIX_AUDIO_SINK` holds a `gst-launch` bin description (e.g.
  `fakeaudiosink`, `fakesink sync=true`), used by tests and smoke runs so
  nothing reaches the speakers.
- If the `scaletempo` element exists it is installed as playbin's
  `audio-filter`, so rate changes preserve pitch.
- **Play/Pause** toggles. At end of stream: with **Repeat** on, seek to 0
  and keep playing; otherwise rewind to 0 and pause (state *Stopped*).
- **Seek ±10 s** buttons and keys clamp to `[0, duration]`.
- **Rate** menu button (label like "1.25×", accessible name "Playback
  Speed: 1.25×", kept current): 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0.
  Applied with a flushing seek at the current position.
- **Volume** menu button: popover with a mute toggle and a 0–100 % slider.
  The slider is cubic (perceptual): playbin volume = slider³. The button
  icon reflects muted / low / medium / high. The slider's accessible value
  is its position as a percentage ("50%", "50%, muted").
- **Repeat** toggle button (`media-playlist-repeat-symbolic`).
- Position is polled every 50 ms while a file is loaded.

### Waveform
- Computed on a worker thread with a separate pipeline:
  `uridecodebin → audioconvert → appsink (F32LE, mono, interleaved,
  sync=false)`. Samples are reduced to peaks (max |sample|) at 100 peaks
  per second of audio. Progress is published to the UI every ~250 ms so
  the waveform fills in while decoding; opening another file cancels the
  worker.
- Results are cached in memory for the session, keyed by URI + size +
  mtime (re-opening the same file is instant).
- Drawing (cairo): peaks are max-downsampled to one bar per 3 px (2 px bar,
  1 px gap), normalised to the loudest peak, mirrored around the centre
  line with a 1 px minimum. Bars left of the playhead use the accent
  colour, the rest the foreground at 35 % opacity; the playhead is a 2 px
  accent line. While decoding, bars are placed by time against the
  playbin duration.
- Click or drag on the waveform seeks (position = x / width × duration,
  clamped); while dragging, the playhead and elapsed time follow the
  pointer and seeks are issued continuously.
- Exposed to accessibility as a *slider* with value now / max / value text.

### Time display
- Elapsed on the left, remaining (prefixed "−") on the right, tabular
  numerals. Format `m:ss`, or `h:mm:ss` when the duration is ≥ 1 h. Unknown
  duration shows `0:00` and `−0:00`.

### MPRIS
- Bus name `org.mpris.MediaPlayer2.HelixAudioPlayer`, falling back to
  `org.mpris.MediaPlayer2.HelixAudioPlayer.instance<pid>` when taken; object
  `/org/mpris/MediaPlayer2`. Served by a worker thread (blocking zbus);
  commands reach the UI through a channel. Set `HELIX_AUDIO_NO_MPRIS=1` to
  disable.
- `org.mpris.MediaPlayer2`: `Raise`, `Quit`; `CanQuit`=true, `CanRaise`=true,
  `HasTrackList`=false, `Fullscreen`/`CanSetFullscreen`=false, `Identity`
  "Audio Player", `DesktopEntry` "org.helix.AudioPlayer",
  `SupportedUriSchemes` ["file"], `SupportedMimeTypes` (same list as the
  desktop file).
- `org.mpris.MediaPlayer2.Player`: `Play`, `Pause`, `PlayPause`, `Stop`,
  `Seek(offset µs)`, `SetPosition(trackid, µs)` (ignored if trackid is not
  current or out of range), `OpenUri`, `Next`/`Previous` (no-ops).
  Properties `PlaybackStatus`, `LoopStatus` ("None"/"Track", writable),
  `Rate` (writable, clamped 0.5–2), `Shuffle` (false), `Metadata`,
  `Volume` (writable, the linear playbin volume), `Position`,
  `MinimumRate` 0.5, `MaximumRate` 2.0, `CanGoNext`/`CanGoPrevious` false,
  `CanPlay`/`CanPause`/`CanSeek` true when a file is loaded, `CanControl`
  true. `PropertiesChanged` is emitted for changed properties (never for
  `Position`); `Seeked` is emitted after user/remote seeks and on repeat.
- Metadata: `mpris:trackid` (`/org/helix/AudioPlayer/Track/<n>`, n
  increments per opened file), `mpris:length` (µs, when known),
  `xesam:title`, `xesam:artist` (array), `xesam:album`, `xesam:url`.

### Keyboard shortcuts
| Keys | Action |
|---|---|
| Space | Play / pause |
| Left / Right | Seek −10 s / +10 s |
| M | Mute toggle |
| Ctrl+O | Open… |
| Ctrl+W | Close window |
| Ctrl+Q | Quit |
| Ctrl+? | Keyboard shortcuts |

Space, Left / Right and M are window shortcuts handled in the bubble
phase (`helix_app_common::keys`), i.e. only when the focused widget does
not use the key: Space on a focused button presses that button, and the
arrow keys on the focused volume slider move the slider. Ctrl+O, Ctrl+?
and the AudioPlay media key stay application accelerators.

### Command line
`helix-audio-player [FILE|URI]` — opens the file (relative paths resolved
against the CWD) and starts playing. `--help`, `--version`.

### Persistence / GSettings
- No GSettings keys are used (Decibels exposes nothing the player needs to
  share), so no schema ships.
- Only the window size / maximized state is remembered, in
  `$XDG_CONFIG_HOME/helix-audio-player/window.json`. Nothing else (last
  file, volume, rate) persists across runs.

## Decisions
- No queue or playlist: the app stays a single-file player (the stub's
  "playlist or queue" is intentionally not implemented).

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): the "Unable to play file" error page appearing (assertive)
  and "Playing TITLE" when a file is opened (polite).
- When the page changes, focus follows if it was on the page being hidden
  (or nowhere): to *Play/Pause* when a file opens, to *Open Another
  File…* on the error page.
- The cover art is presentational (the title below names the track); the
  time labels use the `numeric` style class (tabular figures).
