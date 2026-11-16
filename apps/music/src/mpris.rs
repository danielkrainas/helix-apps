//! MPRIS 2 server (`org.mpris.MediaPlayer2[.Player]`) on the session bus.
//!
//! The D-Bus side runs on its own thread. Method calls are forwarded to the
//! GTK main loop as [`Command`]s; the UI pushes state changes with
//! [`Mpris::update`], which emits `PropertiesChanged`.

use helix_app_common::i18n::tr;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.HelixMusic";
const PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Raise,
    Quit,
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative seek in microseconds.
    Seek(i64),
    /// Absolute position (µs) for the given track id.
    SetPosition(String, i64),
    SetVolume(f64),
    SetShuffle(bool),
    SetLoop(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_number: Option<u32>,
    pub length_us: i64,
    pub art_url: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub status: &'static str,
    pub loop_status: &'static str,
    pub shuffle: bool,
    pub volume: f64,
    pub position_us: i64,
    pub metadata: Option<Metadata>,
    pub can_next: bool,
    pub can_prev: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            status: "Stopped",
            loop_status: "None",
            shuffle: false,
            volume: 1.0,
            position_us: 0,
            metadata: None,
            can_next: false,
            can_prev: false,
        }
    }
}

/// Track id object path for a file path.
pub fn track_id(path: &std::path::Path) -> String {
    format!(
        "/org/helix/Music/Track/{:016x}",
        crate::index::fnv1a(path.as_os_str().as_encoded_bytes())
    )
}

fn ov<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).expect("plain values have no fds")
}

pub fn metadata_map(m: Option<&Metadata>) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let Some(m) = m else {
        map.insert(
            "mpris:trackid".into(),
            ov(ObjectPath::from_static_str_unchecked(NO_TRACK)),
        );
        return map;
    };
    let id = ObjectPath::try_from(m.track_id.clone())
        .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked(NO_TRACK));
    map.insert("mpris:trackid".into(), ov(id));
    map.insert("mpris:length".into(), ov(m.length_us));
    map.insert("xesam:title".into(), ov(m.title.clone()));
    map.insert("xesam:artist".into(), ov(vec![m.artist.clone()]));
    map.insert("xesam:album".into(), ov(m.album.clone()));
    if !m.album_artist.is_empty() {
        map.insert("xesam:albumArtist".into(), ov(vec![m.album_artist.clone()]));
    }
    if let Some(n) = m.track_number {
        map.insert("xesam:trackNumber".into(), ov(n as i32));
    }
    map.insert("xesam:url".into(), ov(m.url.clone()));
    if let Some(a) = &m.art_url {
        map.insert("mpris:artUrl".into(), ov(a.clone()));
    }
    map
}

type Shared = Arc<Mutex<State>>;
type Tx = async_channel::Sender<Command>;

struct Root {
    tx: Tx,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        let _ = self.tx.try_send(Command::Raise);
    }
    fn quit(&self) {
        let _ = self.tx.try_send(Command::Quit);
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
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        tr("Music")
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "org.helix.Music"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        [
            "audio/mpeg",
            "audio/flac",
            "audio/ogg",
            "audio/x-vorbis+ogg",
            "audio/opus",
            "audio/mp4",
            "audio/x-wav",
            "audio/aac",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

struct PlayerIface {
    tx: Tx,
    state: Shared,
}

impl PlayerIface {
    fn st(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
    fn send(&self, c: Command) {
        let _ = self.tx.try_send(c);
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn next(&self) {
        self.send(Command::Next);
    }
    fn previous(&self) {
        self.send(Command::Previous);
    }
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
    fn set_position(&self, track_id: OwnedObjectPath, position: i64) {
        self.send(Command::SetPosition(
            track_id.as_str().to_string(),
            position,
        ));
    }
    fn open_uri(&self, _uri: &str) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(
            tr("OpenUri is not supported").into(),
        ))
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.st().status.into()
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        self.st().loop_status.into()
    }
    #[zbus(property)]
    fn set_loop_status(&mut self, value: String) {
        self.send(Command::SetLoop(value));
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn set_rate(&mut self, _value: f64) {}
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.st().shuffle
    }
    #[zbus(property)]
    fn set_shuffle(&mut self, value: bool) {
        self.send(Command::SetShuffle(value));
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata_map(self.st().metadata.as_ref())
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.st().volume
    }
    #[zbus(property)]
    fn set_volume(&mut self, value: f64) {
        self.send(Command::SetVolume(value));
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.st().position_us
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.st().can_next
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.st().can_prev
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.st().metadata.is_some()
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.st().metadata.is_some()
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.st().metadata.is_some()
    }
    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

enum Notify {
    Changed(Box<(State, State)>),
    Seeked(i64),
}

/// Handle used by the UI.
pub struct Mpris {
    state: Shared,
    notify: mpsc::Sender<Notify>,
}

impl Mpris {
    /// Connects on a worker thread; errors are logged.
    pub fn start(tx: Tx) -> Mpris {
        let state: Shared = Arc::default();
        let (ntx, nrx) = mpsc::channel();
        let shared = state.clone();
        std::thread::Builder::new()
            .name("mpris".into())
            .spawn(move || {
                if let Err(e) = serve(tx, shared, nrx) {
                    eprintln!("helix-music: MPRIS unavailable: {e}");
                }
            })
            .ok();
        Mpris { state, notify: ntx }
    }

    pub fn update(&self, f: impl FnOnce(&mut State)) {
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        let old = st.clone();
        f(&mut st);
        let new = st.clone();
        drop(st);
        // Position changes alone are not signalled (MPRIS rule).
        let mut cmp = old.clone();
        cmp.position_us = new.position_us;
        if cmp != new {
            let _ = self.notify.send(Notify::Changed(Box::new((old, new))));
        }
    }

    pub fn seeked(&self, position_us: i64) {
        if let Ok(mut st) = self.state.lock() {
            st.position_us = position_us;
        }
        let _ = self.notify.send(Notify::Seeked(position_us));
    }
}

fn serve(tx: Tx, state: Shared, rx: mpsc::Receiver<Notify>) -> zbus::Result<()> {
    let conn = zbus::blocking::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(PATH, Root { tx: tx.clone() })?
        .serve_at(PATH, PlayerIface { tx, state })?
        .build()?;
    let iface = conn.object_server().interface::<_, PlayerIface>(PATH)?;
    for n in rx {
        let em = iface.signal_emitter();
        let p = iface.get();
        let r = match n {
            Notify::Seeked(pos) => zbus::block_on(PlayerIface::seeked(em, pos)),
            Notify::Changed(states) => zbus::block_on(async {
                let (old, new) = *states;
                if old.status != new.status {
                    p.playback_status_changed(em).await?;
                }
                if old.loop_status != new.loop_status {
                    p.loop_status_changed(em).await?;
                }
                if old.shuffle != new.shuffle {
                    p.shuffle_changed(em).await?;
                }
                if old.volume != new.volume {
                    p.volume_changed(em).await?;
                }
                if old.metadata != new.metadata {
                    p.metadata_changed(em).await?;
                    p.can_play_changed(em).await?;
                    p.can_pause_changed(em).await?;
                    p.can_seek_changed(em).await?;
                }
                if old.can_next != new.can_next {
                    p.can_go_next_changed(em).await?;
                }
                if old.can_prev != new.can_prev {
                    p.can_go_previous_changed(em).await?;
                }
                Ok::<(), zbus::Error>(())
            }),
        };
        if let Err(e) = r {
            eprintln!("helix-music: MPRIS signal failed: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_contents() {
        let m = Metadata {
            track_id: track_id(std::path::Path::new("/m/a.flac")),
            title: "T".into(),
            artist: "A".into(),
            album: "Al".into(),
            album_artist: String::new(),
            track_number: Some(3),
            length_us: 5_000_000,
            art_url: None,
            url: "file:///m/a.flac".into(),
        };
        let map = metadata_map(Some(&m));
        assert!(map["mpris:trackid"]
            .to_string()
            .contains("/org/helix/Music/Track/"));
        assert_eq!(i64::try_from(&map["mpris:length"]).unwrap(), 5_000_000);
        assert!(!map.contains_key("xesam:albumArtist"));
        let empty = metadata_map(None);
        assert_eq!(empty.len(), 1);
        assert_eq!(
            track_id(std::path::Path::new("/m/a.flac")),
            m.track_id,
            "stable ids"
        );
    }
}
