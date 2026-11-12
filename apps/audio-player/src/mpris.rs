//! MPRIS 2 (`org.mpris.MediaPlayer2` + `.Player`) server.
//!
//! Runs on its own thread with a blocking zbus connection. The UI pushes
//! [`Snapshot`]s through a [`Handle`]; the thread diffs them and emits
//! `PropertiesChanged`. Remote calls become [`Command`]s sent to the UI.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.HelixAudioPlayer";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
pub const MIME_TYPES: &[&str] = &[
    "audio/mpeg",
    "audio/x-mp3",
    "audio/ogg",
    "audio/x-vorbis+ogg",
    "audio/x-opus+ogg",
    "audio/flac",
    "audio/x-flac",
    "audio/x-wav",
    "audio/wav",
    "audio/aac",
    "audio/mp4",
    "audio/x-m4a",
    "audio/webm",
    "audio/x-aiff",
    "audio/x-ms-wma",
    "audio/x-matroska",
];

/// Requests from MPRIS clients.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Raise,
    Quit,
    Play,
    Pause,
    PlayPause,
    Stop,
    /// Relative seek in microseconds.
    Seek(i64),
    /// Absolute position in microseconds.
    SetPosition(i64),
    OpenUri(String),
    SetRate(f64),
    SetVolume(f64),
    SetLoop(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Playing => "Playing",
            Status::Paused => "Paused",
            Status::Stopped => "Stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    /// Increments for every opened file.
    pub id: u64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub url: String,
    pub length_us: Option<i64>,
}

impl Track {
    pub fn object_path(&self) -> String {
        format!("/org/helix/AudioPlayer/Track/{}", self.id)
    }
}

/// Player state as seen over D-Bus.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub status: Status,
    pub loop_track: bool,
    pub rate: f64,
    pub volume: f64,
    pub position_us: i64,
    pub track: Option<Track>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            status: Status::Stopped,
            loop_track: false,
            rate: 1.0,
            volume: 1.0,
            position_us: 0,
            track: None,
        }
    }
}

fn ov<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
    v.into()
        .try_to_owned()
        .expect("value without file descriptors")
}

fn path_value(p: &str) -> OwnedValue {
    ov(ObjectPath::try_from(p.to_string())
        .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked(NO_TRACK)))
}

/// Builds the `Metadata` dictionary.
pub fn build_metadata(track: Option<&Track>) -> HashMap<String, OwnedValue> {
    let mut m = HashMap::new();
    let Some(t) = track else {
        m.insert("mpris:trackid".into(), path_value(NO_TRACK));
        return m;
    };
    m.insert("mpris:trackid".into(), path_value(&t.object_path()));
    if let Some(len) = t.length_us {
        m.insert("mpris:length".into(), ov(len));
    }
    m.insert("xesam:title".into(), ov(t.title.as_str()));
    if let Some(a) = &t.artist {
        m.insert("xesam:artist".into(), ov(vec![a.as_str()]));
    }
    if let Some(a) = &t.album {
        m.insert("xesam:album".into(), ov(a.as_str()));
    }
    m.insert("xesam:url".into(), ov(t.url.as_str()));
    m
}

fn can_play(s: &Snapshot) -> bool {
    s.track.is_some()
}

/// Player properties that differ between `old` and `new` (never `Position`).
pub fn changed_properties(old: &Snapshot, new: &Snapshot) -> HashMap<&'static str, OwnedValue> {
    let mut c = HashMap::new();
    if old.status != new.status {
        c.insert("PlaybackStatus", ov(new.status.as_str()));
    }
    if old.loop_track != new.loop_track {
        c.insert("LoopStatus", ov(loop_status(new)));
    }
    if old.rate != new.rate {
        c.insert("Rate", ov(new.rate));
    }
    if old.volume != new.volume {
        c.insert("Volume", ov(new.volume));
    }
    if old.track != new.track {
        c.insert("Metadata", ov(build_metadata(new.track.as_ref())));
    }
    if can_play(old) != can_play(new) {
        for name in ["CanPlay", "CanPause", "CanSeek"] {
            c.insert(name, ov(can_play(new)));
        }
    }
    c
}

fn loop_status(s: &Snapshot) -> &'static str {
    if s.loop_track {
        "Track"
    } else {
        "None"
    }
}

struct Root {
    cmds: async_channel::Sender<Command>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        let _ = self.cmds.try_send(Command::Raise);
    }
    fn quit(&self) {
        let _ = self.cmds.try_send(Command::Quit);
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn fullscreen(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_set_fullscreen(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Audio Player"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "org.helix.AudioPlayer"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        MIME_TYPES.iter().map(|s| s.to_string()).collect()
    }
}

struct Player {
    cmds: async_channel::Sender<Command>,
    shared: Arc<Mutex<Snapshot>>,
}

impl Player {
    fn snap(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }
    fn send(&self, c: Command) {
        let _ = self.cmds.try_send(c);
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {}
    fn previous(&self) {}
    fn pause(&self) {
        self.send(Command::Pause);
    }
    fn play_pause(&self) {
        self.send(Command::PlayPause);
    }
    fn stop(&self) {
        self.send(Command::Stop);
    }
    fn play(&self) {
        self.send(Command::Play);
    }
    fn seek(&self, offset: i64) {
        self.send(Command::Seek(offset));
    }
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let s = self.snap();
        let Some(t) = &s.track else { return };
        if track_id.as_str() != t.object_path() || position < 0 {
            return;
        }
        if t.length_us.is_some_and(|l| position > l) {
            return;
        }
        self.send(Command::SetPosition(position));
    }
    fn open_uri(&self, uri: String) {
        self.send(Command::OpenUri(uri));
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.snap().status.as_str().into()
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        loop_status(&self.snap()).into()
    }
    #[zbus(property)]
    fn set_loop_status(&mut self, value: String) {
        self.send(Command::SetLoop(value != "None"));
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        self.snap().rate
    }
    #[zbus(property)]
    fn set_rate(&mut self, value: f64) {
        // A rate of 0 means pause per the spec; ignore it instead.
        if value > 0.0 {
            self.send(Command::SetRate(crate::seek::clamp_rate(value)));
        }
    }
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn set_shuffle(&mut self, _value: bool) {}
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        build_metadata(self.snap().track.as_ref())
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.snap().volume
    }
    #[zbus(property)]
    fn set_volume(&mut self, value: f64) {
        self.send(Command::SetVolume(value.clamp(0.0, 1.0)));
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.snap().position_us
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        crate::seek::MIN_RATE
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        crate::seek::MAX_RATE
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        can_play(&self.snap())
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        can_play(&self.snap())
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        can_play(&self.snap())
    }
    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

enum Msg {
    Update(Snapshot),
    Seeked(i64),
}

/// UI-side handle; dropping it stops the server thread.
pub struct Handle {
    tx: mpsc::Sender<Msg>,
}

impl Handle {
    pub fn update(&self, s: Snapshot) {
        let _ = self.tx.send(Msg::Update(s));
    }
    pub fn seeked(&self, position_us: i64) {
        let _ = self.tx.send(Msg::Seeked(position_us));
    }
}

/// Starts the server on the session bus. Errors are logged on the thread.
pub fn spawn(cmds: async_channel::Sender<Command>) -> Handle {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("mpris".into())
        .spawn(move || {
            if let Err(e) = serve(cmds, rx) {
                eprintln!("helix-audio-player: MPRIS unavailable: {e}");
            }
        })
        .expect("spawn MPRIS thread");
    Handle { tx }
}

fn serve(cmds: async_channel::Sender<Command>, rx: mpsc::Receiver<Msg>) -> zbus::Result<()> {
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let conn = zbus::blocking::connection::Builder::session()?
        .serve_at(OBJECT_PATH, Root { cmds: cmds.clone() })?
        .serve_at(
            OBJECT_PATH,
            Player {
                cmds,
                shared: shared.clone(),
            },
        )?
        .build()?;
    if conn.request_name(BUS_NAME).is_err() {
        conn.request_name(format!("{BUS_NAME}.instance{}", std::process::id()))?;
    }
    let path = ObjectPath::try_from(OBJECT_PATH)?;
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Update(new) => {
                let changed = {
                    let mut cur = shared
                        .lock()
                        .map_err(|_| zbus::Error::Failure("poisoned".into()))?;
                    let changed = changed_properties(&cur, &new);
                    *cur = new;
                    changed
                };
                if !changed.is_empty() {
                    conn.emit_signal(
                        None::<zbus::names::BusName>,
                        &path,
                        "org.freedesktop.DBus.Properties",
                        "PropertiesChanged",
                        &(PLAYER_IFACE, changed, Vec::<String>::new()),
                    )?;
                }
            }
            Msg::Seeked(pos) => {
                conn.emit_signal(
                    None::<zbus::names::BusName>,
                    &path,
                    PLAYER_IFACE,
                    "Seeked",
                    &(pos,),
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: 3,
            title: "Song".into(),
            artist: Some("Band".into()),
            album: None,
            url: "file:///tmp/song.ogg".into(),
            length_us: Some(61_500_000),
        }
    }

    fn get<'a>(m: &'a HashMap<String, OwnedValue>, k: &str) -> &'a Value<'static> {
        m.get(k).unwrap_or_else(|| panic!("missing {k}"))
    }

    #[test]
    fn metadata_full() {
        let m = build_metadata(Some(&track()));
        assert_eq!(
            get(&m, "mpris:trackid"),
            &Value::from(ObjectPath::try_from("/org/helix/AudioPlayer/Track/3").unwrap())
        );
        assert_eq!(get(&m, "mpris:length"), &Value::from(61_500_000i64));
        assert_eq!(get(&m, "xesam:title"), &Value::from("Song"));
        assert_eq!(get(&m, "xesam:url"), &Value::from("file:///tmp/song.ogg"));
        let artists: Vec<String> = get(&m, "xesam:artist")
            .try_clone()
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(artists, vec!["Band".to_string()]);
        assert!(!m.contains_key("xesam:album"));
    }

    #[test]
    fn metadata_without_track() {
        let m = build_metadata(None);
        assert_eq!(m.len(), 1);
        assert_eq!(
            get(&m, "mpris:trackid"),
            &Value::from(ObjectPath::try_from(NO_TRACK).unwrap())
        );
    }

    #[test]
    fn metadata_unknown_length() {
        let t = Track {
            length_us: None,
            ..track()
        };
        assert!(!build_metadata(Some(&t)).contains_key("mpris:length"));
    }

    #[test]
    fn property_diff() {
        let a = Snapshot::default();
        assert!(changed_properties(&a, &a).is_empty());
        let b = Snapshot {
            position_us: 5,
            ..a.clone()
        };
        assert!(
            changed_properties(&a, &b).is_empty(),
            "position never signals"
        );
        let c = Snapshot {
            status: Status::Playing,
            track: Some(track()),
            loop_track: true,
            ..a.clone()
        };
        let ch = changed_properties(&a, &c);
        let mut keys: Vec<_> = ch.keys().copied().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "CanPause",
                "CanPlay",
                "CanSeek",
                "LoopStatus",
                "Metadata",
                "PlaybackStatus"
            ]
        );
        assert_eq!(
            ch["PlaybackStatus"],
            OwnedValue::from(zbus::zvariant::Str::from("Playing"))
        );
        assert_eq!(
            ch["LoopStatus"],
            OwnedValue::from(zbus::zvariant::Str::from("Track"))
        );
        let d = Snapshot {
            rate: 1.5,
            volume: 0.25,
            ..c.clone()
        };
        let mut keys: Vec<_> = changed_properties(&c, &d).keys().copied().collect();
        keys.sort();
        assert_eq!(keys, ["Rate", "Volume"]);
    }
}
