# Camera

Drop-in replacement for Snapshot.

- **App ID**: `org.helix.Camera`
- **Binary**: `helix-camera`
- **Crate**: `apps/camera`

## Purpose
Take photos and videos from webcams.

## Main windows
- Preview with capture controls.
- Gallery of recent captures (v1: reduced to "open last capture", see Decisions).

## Contracts used
- PipeWire / V4L2 for camera access.
- XDG portals: Camera, FileChooser, OpenURI.
- GSettings: `org.gnome.Snapshot` or equivalent.

## Non-goals
- Advanced photo editing.

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow` (default 800×640, size/maximized restored from GSettings, dark colour scheme forced like
  other camera apps). A transparent header bar holds the primary menu
  (*Countdown* submenu, *Video Format* submenu, *Play Shutter Sound*
  toggle, *Keyboard Shortcuts*, *About Camera*).
- The **viewfinder** fills the window (a `gtk::Picture` with
  `ContentFit::Contain` on a black background).
- Below the viewfinder a **control bar**:
  - left: **gallery button** — a round thumbnail of the most recent capture
    (or a placeholder icon when there is none). Clicking it opens the file
    in the default viewer through `gtk::FileLauncher` (OpenURI portal when
    sandboxed). Insensitive with no capture.
  - centre: the **shutter button** (big round). In photo mode it takes a
    photo (or starts / cancels the countdown); in video mode it starts /
    stops recording. While recording it shows a stop icon and the window
    shows a red elapsed-time label (`m:ss`).
  - right: **camera switcher** button (`camera-switch-symbolic`), visible
    only when more than one camera is present; it cycles to the next camera.
  - Under the shutter: a **Photo / Video** mode switcher (two linked toggle
    buttons). The Video toggle is insensitive when no video format can be
    encoded.
- **Status pages** replace the viewfinder when there is nothing to show:
  - *No Camera Found* (`camera-disabled-symbolic`): no video source
    devices. The page updates automatically when a camera is plugged in.
  - *Permission Denied*: the Camera portal refused access; explains that
    access can be granted in Settings → Apps.
  - *Camera Error*: the pipeline failed (message from GStreamer shown).
- Countdown: a large number is overlaid on the viewfinder and decrements
  each second; the photo is taken at 0. Pressing the shutter (or Escape)
  during a countdown cancels it.
- A brief white flash overlay fades over the viewfinder when a photo is
  taken. Errors while saving are shown as `AdwToast`s.

### Camera access
1. **Sandboxed** (`/.flatpak-info` exists, or `HELIX_CAMERA_PORTAL=1`):
   - Read `IsCameraPresent` on `org.freedesktop.portal.Camera`
     (`/org/freedesktop/portal/desktop`).
   - Call `AccessCamera(a{sv} options)` with `handle_token` set; subscribe
     to `org.freedesktop.portal.Request::Response` on the predicted request
     path `/org/freedesktop/portal/desktop/request/<SENDER>/<TOKEN>` (sender
     unique name without the leading `:`, `.` → `_`) *before* the call.
     Response `0` = granted, `1` = denied by user, `2` = other/failed →
     *Permission Denied* page.
   - `OpenPipeWireRemote(a{sv} {})` returns a file descriptor for a PipeWire
     remote restricted to cameras.
   - Cameras are enumerated with GStreamer's `pipewiredeviceprovider` with
     its `fd` property set to that descriptor; each device creates a
     `pipewiresrc` which also gets `fd=` set (and `target-object` from the
     device).
2. **Not sandboxed** (or the portal is unavailable): a GStreamer
   `DeviceMonitor` filtered on `Video/Source`. Because the same camera is
   reported by several providers, devices are taken from one provider
   only, in preference order PipeWire → libcamera → V4L2.
3. `HELIX_CAMERA_SOURCE` overrides everything (for tests, never touches
   real hardware or the portal):
   - `test`: two virtual cameras (`videotestsrc`, one "front", one "back")
     and `audiotestsrc` for recording audio.
   - `none`: *No Camera Found*. `denied`: *Permission Denied*.
- Hot-plug: device-added / device-removed messages refresh the list; if the
  active camera disappears the next one is opened (or *No Camera Found*).
- **Mirroring**: the preview is mirrored horizontally for front-facing
  cameras (`api.libcamera.location` / `camera.location` = `front`) and for
  cameras of unknown location (typical laptop/USB webcams face the user).
  Back and external cameras are not mirrored. Saved photos and videos are
  never mirrored.

### Pipeline
`source → decodebin (handles raw and MJPEG) → videoconvert → tee`
- preview branch: `queue (leaky) → videoflip → videoconvert → RGBA →
  appsink (max-buffers=1, drop)`; frames go to the GTK main thread as
  `gdk::MemoryTexture`.
- still branch: `queue (leaky, 1 buffer) → fakesink enable-last-sample`;
  a photo encodes the last raw sample with a short
  `appsrc → videoconvert → jpegenc (quality 90) → filesink` pipeline.
- recording: the pipeline is rebuilt with an encoder branch
  (`queue → videoconvert → <video enc> → mux`) plus an audio branch
  (`pulsesrc` / `audiotestsrc` → `audioconvert → audioresample → <audio
  enc> → mux`) → `filesink`. Stopping sends EOS, waits for the muxer to
  finish (5 s timeout) and returns to preview.
- All GStreamer state changes run on a camera worker thread; the UI talks to
  it with commands and receives events.

### Video formats
Detected at runtime from the GStreamer registry (element factory lookups);
menu entries for formats that cannot be encoded are insensitive.
| Format | Video | Audio | Mux | Ext |
|---|---|---|---|---|
| WebM (VP8) — default | `vp8enc` | `opusenc` | `webmmux` | `.webm` |
| WebM (VP9) | `vp9enc` | `opusenc` | `webmmux` | `.webm` |
| MP4 (H.264) | `x264enc` → `openh264enc` | `avenc_aac` → `voaacenc` → `fdkaacenc` | `mp4mux` (+`h264parse`) | `.mp4` |
If the chosen format is unavailable the first available one is used. If
its audio encoder (or a microphone source) is missing the video is
recorded without sound.

### Files
- Output directory: XDG Pictures dir + `/Camera` (created on demand);
  `$HOME/Pictures/Camera` if XDG Pictures is unset;
  `HELIX_CAMERA_OUTPUT_DIR` overrides (tests).
- Names: `Photo from YYYY-MM-DD HH-MM-SS.jpg`, `Video from YYYY-MM-DD
  HH-MM-SS.webm|.mp4` (local time). A clash appends ` (2)`, ` (3)`, ….
- On start, the newest `Photo from*`/`Video from*` file in the output
  directory becomes the gallery thumbnail.
- Shutter sound: `/usr/share/sounds/freedesktop/stereo/camera-shutter.oga`
  through a GStreamer `playbin` (default on; `HELIX_CAMERA_SOURCE=test`
  forces the audio sink to `fakesink` so nothing is played).

### Settings
GSettings schema `org.gnome.Snapshot` (read through `Prefs`, so settings
carry over from/to GNOME Snapshot); `data/org.gnome.Snapshot.gschema.xml`
holds only these keys, copied from the published schema, for systems where
Snapshot is not installed:
| Key | Type | Use |
|---|---|---|
| `countdown` | `i` | countdown delay in seconds (0/3/5/10; other values → 0) |
| `capture-mode` | enum `picture`/`video` | photo / video mode |
| `play-shutter-sound` | `b` | shutter sound toggle |
| `last-camera-id` | `s` | name of the last selected camera (`""` = none) |
| `window-width`, `window-height`, `is-maximized` | `i`, `i`, `b` | window state, saved on close |

App-private `$XDG_CONFIG_HOME/helix-camera/settings.json` holds only
`video_format` (`webm-vp8`/`webm-vp9`/`mp4-h264`): Snapshot's `video-format`
enum only knows `vp8webm`, so it cannot store our choices and is not
touched. When the Snapshot schema is not installed at all, the JSON file
stores every setting instead (`countdown`, `capture_mode`, `video_format`,
`play_shutter_sound`, `last_camera`, `window_width`, `window_height`,
`is_maximized`).

### QR codes
- In photo mode, while streaming and not busy, the preview is scanned for
  QR codes: at most one frame every 400 ms is offered to a scanner thread
  through a one-slot channel (frames are dropped while a scan is running),
  so the main thread never decodes. The scanner converts RGBA to luma,
  downsamples so the longest side is ≤ 960 px and decodes with the
  pure-Rust `rqrr` crate; if the direct read fails, a horizontally mirrored
  read is tried (front-camera previews are mirrored).
- A detected code shows a banner sliding up at the bottom of the
  viewfinder: QR icon, the decoded text (ellipsized, selectable, full text
  in the tooltip), **Copy** (clipboard + "Copied to clipboard" toast),
  **Open** (only for `http(s)://`, `ftp://`, `mailto:`, `tel:`, `sms:`,
  `geo:` URIs or a bare `www.` host, launched with `gtk::UriLauncher` /
  OpenURI portal; other schemes such as `file:` are never opened) and a
  **Dismiss** button.
- Debouncing: the banner stays while the same code is seen, changes when a
  different one is decoded, and hides after 3 consecutive scans without a
  code. A dismissed code is not shown again until it has left the view.
  Switching to video mode, switching camera or losing the stream hides it.

### Shortcuts
| Key | Action |
|---|---|
| `space` / `Return` | Shutter (photo / record toggle / cancel countdown) |
| `Escape` | Cancel countdown |
| `Ctrl+1` / `Ctrl+2` | Photo / Video mode |
| `Ctrl+Tab` | Next camera |
| `Ctrl+G` | Open last capture |
| `Ctrl+Q` / `Ctrl+W` | Quit / close |
| `Ctrl+?` | Keyboard shortcuts |

### CLI
`helix-camera [--help] [--version]`.

Environment (testing): `HELIX_CAMERA_SOURCE=test|none|denied`,
`HELIX_CAMERA_OUTPUT_DIR`, `HELIX_CAMERA_PORTAL=0|1`, `HELIX_CAMERA_SILENT`
(no shutter sound), `HELIX_CAMERA_SMOKE=1` (with `test`: scripted photo,
countdown photo, short video, an injected QR result with copy/dismiss, camera
switch, quit — for headless smoke tests).

## Decisions
- QR codes: detected on the live preview (`rqrr`, throttled, off the main
  thread) with copy/open actions.
- GSettings: the real `org.gnome.Snapshot` keys are used where they exist;
  an app-private JSON file holds the rest (video format).
- Gallery: no in-app gallery for v1; "Open last capture" opens the newest
  capture in the default viewer.
