//! MPRIS 2 (`org.mpris.MediaPlayer2` + `.Player`) over zbus.
//!
//! The D-Bus connection lives on its own thread. Method calls are forwarded
//! to the GTK main thread as [`Command`]s; the main thread publishes its
//! state into a shared [`State`] and asks the worker to emit
//! `PropertiesChanged` / `Seeked` through [`Mpris::changed`] / [`Mpris::seeked`].

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

pub const PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
pub const MIN_RATE: f64 = 0.5;
pub const MAX_RATE: f64 = 2.0;

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
    SetVolume(f64),
    SetRate(f64),
    SetFullscreen(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prop {
    PlaybackStatus,
    Metadata,
    Volume,
    Rate,
    CanSeek,
    Fullscreen,
}

#[derive(Debug, Clone)]
pub struct State {
    pub status: &'static str,
    pub rate: f64,
    pub volume: f64,
    pub position_us: i64,
    pub length_us: Option<i64>,
    pub title: String,
    pub url: String,
    pub track: u64,
    pub has_media: bool,
    pub seekable: bool,
    pub fullscreen: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            status: "Stopped",
            rate: 1.0,
            volume: 1.0,
            position_us: 0,
            length_us: None,
            title: String::new(),
            url: String::new(),
            track: 0,
            has_media: false,
            seekable: false,
            fullscreen: false,
        }
    }
}

impl State {
    pub fn track_path(&self) -> String {
        if self.has_media {
            format!("/org/helix/VideoPlayer/Track/{}", self.track)
        } else {
            NO_TRACK.to_string()
        }
    }

    pub fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();
        let mut put = |k: &str, v: Value<'_>| {
            if let Ok(v) = OwnedValue::try_from(v) {
                map.insert(k.to_string(), v);
            }
        };
        if let Ok(path) = ObjectPath::try_from(self.track_path()) {
            put("mpris:trackid", Value::from(path));
        }
        if !self.has_media {
            return map;
        }
        if let Some(len) = self.length_us {
            put("mpris:length", Value::from(len));
        }
        put("xesam:title", Value::from(self.title.clone()));
        put("xesam:url", Value::from(self.url.clone()));
        map
    }
}

type Shared = Arc<Mutex<State>>;

struct Root {
    tx: async_channel::Sender<Command>,
    state: Shared,
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
    fn can_set_fullscreen(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn fullscreen(&self) -> bool {
        self.state.lock().unwrap().fullscreen
    }
    #[zbus(property)]
    fn set_fullscreen(&self, value: bool) {
        let _ = self.tx.try_send(Command::SetFullscreen(value));
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Video Player"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "org.helix.VideoPlayer"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        [
            "video/mp4", "video/x-matroska", "video/webm", "video/ogg", "video/quicktime",
            "video/x-msvideo", "video/mpeg", "video/x-flv", "video/3gpp", "video/x-ms-wmv",
            "video/mp2t", "application/ogg",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

struct PlayerIface {
    tx: async_channel::Sender<Command>,
    state: Shared,
}

impl PlayerIface {
    fn send(&self, c: Command) {
        let _ = self.tx.try_send(c);
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
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
        let current = self.state.lock().unwrap().track_path();
        if track_id.as_str() == current && position >= 0 {
            self.send(Command::SetPosition(position));
        }
    }
    fn open_uri(&self, uri: &str) {
        self.send(Command::OpenUri(uri.to_string()));
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.state.lock().unwrap().status.to_string()
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        self.state.lock().unwrap().rate
    }
    #[zbus(property)]
    fn set_rate(&self, rate: f64) {
        if rate > 0.0 {
            self.send(Command::SetRate(rate.clamp(MIN_RATE, MAX_RATE)));
        }
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.state.lock().unwrap().metadata()
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.state.lock().unwrap().volume
    }
    #[zbus(property)]
    fn set_volume(&self, v: f64) {
        self.send(Command::SetVolume(v.clamp(0.0, 1.0)));
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.state.lock().unwrap().position_us
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        MIN_RATE
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        MAX_RATE
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
        self.state.lock().unwrap().has_media
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.state.lock().unwrap().has_media
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.has_media && s.seekable
    }
    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

enum Update {
    Changed(Vec<Prop>),
    Seeked(i64),
}

/// Main-thread handle.
pub struct Mpris {
    state: Shared,
    tx: mpsc::Sender<Update>,
}

impl Mpris {
    /// Spawns the D-Bus worker. Commands arrive on `commands`.
    pub fn start(commands: async_channel::Sender<Command>) -> Mpris {
        let state: Shared = Arc::default();
        let (tx, rx) = mpsc::channel();
        let st = state.clone();
        std::thread::Builder::new()
            .name("mpris".into())
            .spawn(move || {
                if let Err(err) = serve(commands, st, rx) {
                    eprintln!("helix-video-player: MPRIS unavailable: {err}");
                }
            })
            .ok();
        Mpris { state, tx }
    }

    /// Updates the shared state (signals are emitted via [`Mpris::changed`]).
    pub fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap());
    }

    pub fn changed(&self, props: &[Prop]) {
        let _ = self.tx.send(Update::Changed(props.to_vec()));
    }

    pub fn seeked(&self, position_us: i64) {
        let _ = self.tx.send(Update::Seeked(position_us));
    }
}

fn serve(commands: async_channel::Sender<Command>, state: Shared, rx: mpsc::Receiver<Update>) -> zbus::Result<()> {
    let name = format!("org.mpris.MediaPlayer2.HelixVideoPlayer.instance{}", std::process::id());
    let conn = zbus::blocking::connection::Builder::session()?
        .name(name)?
        .serve_at(PATH, Root { tx: commands.clone(), state: state.clone() })?
        .serve_at(PATH, PlayerIface { tx: commands, state })?
        .build()?;
    let server = conn.object_server();
    let root = server.interface::<_, Root>(PATH)?;
    let player = server.interface::<_, PlayerIface>(PATH)?;
    for update in rx {
        let result = match update {
            Update::Seeked(us) => zbus::block_on(PlayerIface::seeked(player.signal_emitter(), us)),
            Update::Changed(props) => props.into_iter().try_for_each(|p| {
                let (pi, em) = (player.get(), player.signal_emitter());
                zbus::block_on(async {
                    match p {
                        Prop::PlaybackStatus => pi.playback_status_changed(em).await,
                        Prop::Metadata => {
                            pi.metadata_changed(em).await?;
                            pi.can_play_changed(em).await?;
                            pi.can_pause_changed(em).await?;
                            pi.can_seek_changed(em).await
                        }
                        Prop::Volume => pi.volume_changed(em).await,
                        Prop::Rate => pi.rate_changed(em).await,
                        Prop::CanSeek => pi.can_seek_changed(em).await,
                        Prop::Fullscreen => root.get().fullscreen_changed(root.signal_emitter()).await,
                    }
                })
            }),
        };
        if let Err(err) = result {
            eprintln!("helix-video-player: MPRIS signal failed: {err}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_contents() {
        let mut s = State::default();
        let m = s.metadata();
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("mpris:trackid"));
        s.has_media = true;
        s.track = 3;
        s.length_us = Some(5_000_000);
        s.title = "clip.webm".into();
        s.url = "file:///tmp/clip.webm".into();
        let m = s.metadata();
        assert_eq!(m.len(), 4);
        assert_eq!(i64::try_from(m["mpris:length"].clone()).unwrap(), 5_000_000);
        assert_eq!(s.track_path(), "/org/helix/VideoPlayer/Track/3");
    }
}
