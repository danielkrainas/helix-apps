# Camera

Drop-in replacement for Snapshot.

- **App ID**: `org.helix.Camera`
- **Binary**: `helix-camera`
- **Crate**: `apps/camera`

## Purpose
Take photos and videos from webcams.

## Main windows
- Preview with capture controls.
- Gallery of recent captures.

## Contracts used
- PipeWire / V4L2 for camera access.
- XDG portals: Camera, FileChooser, OpenURI.
- GSettings: `org.gnome.Snapshot` or equivalent.

## Non-goals
- Advanced photo editing.

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow` (default 640×560, dark colour scheme forced like
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
No `org.gnome.Snapshot` keys are used (key names/types not confirmed from
public documentation). Preferences are stored in
`$XDG_CONFIG_HOME/helix-camera/settings.json`: `countdown` (0/3/5/10),
`capture_mode` (`photo`/`video`), `video_format` (`webm-vp8`/`webm-vp9`/
`mp4-h264`), `play_shutter_sound` (bool), `last_camera` (device name).

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
countdown photo, short video, camera switch, quit — for headless smoke
tests).

## Open questions
- **QR code detection** (Snapshot detects QR codes in the viewfinder and
  offers to open/copy them): deferred — needs a decoding crate
  (e.g. `rqrr`) and UI design decisions.
- **GSettings**: should Helix adopt `org.gnome.Snapshot` keys once the key
  list is confirmed? Currently a private JSON file is used.
- An in-app gallery/viewer of recent captures (the stub lists one) is
  reduced to the "open last capture" button; captures open in the default
  image/video viewer.
