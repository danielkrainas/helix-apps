//! Sound devices and volumes through the sound server's PulseAudio
//! interface (`pactl`, as the shell uses; PipeWire serves it with
//! pipewire-pulse), the system sounds volume (PipeWire's `route-settings`
//! metadata through `pw-metadata`), and the alert sound (a sound theme of
//! its own).
//! Blocking; worker threads only.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// 100 % in PulseAudio's volume units.
const NORM: f64 = 65536.0;
/// The most the volume goes with over-amplification on (150 %, as the
/// shell allows).
pub const MAX_AMPLIFIED: f64 = 1.5;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// No `pactl`, or no sound server answers.
    Missing,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Missing => f.write_str("no sound server"),
            Error::Other(m) => f.write_str(m),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Which sound server: the session's, or another one (tests).
#[derive(Debug, Clone, Default)]
pub struct Server {
    /// `PULSE_SERVER` for the commands, when not the session's.
    pub address: Option<String>,
    /// `PIPEWIRE_RUNTIME_DIR` for `pw-metadata`, when not the session's.
    pub pipewire: Option<PathBuf>,
}

impl Server {
    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env("LC_ALL", "C");
        if let Some(a) = &self.address {
            c.env("PULSE_SERVER", a);
        }
        if let Some(d) = &self.pipewire {
            c.env("PIPEWIRE_RUNTIME_DIR", d);
        }
        c
    }

    fn pw_metadata(&self, args: &[&str]) -> Result<String> {
        let out = self
            .command("pw-metadata")
            .args(["-n", ROUTE_SETTINGS])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|_| Error::Missing)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(Error::Missing)
        }
    }

    fn pactl(&self, args: &[&str]) -> Result<String> {
        let out = self
            .command("pactl")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|_| Error::Missing)?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("Connection refused") || err.contains("Connection failure") {
            Err(Error::Missing)
        } else {
            Err(Error::Other(err))
        }
    }

    fn json(&self, what: &str) -> Result<Vec<Value>> {
        let text = self.pactl(&["-f", "json", "list", what])?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(a)) => Ok(a),
            _ => Err(Error::Other(format!("unexpected pactl output for {what}"))),
        }
    }
}

/// An output (sink) or input (source).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Device {
    pub index: u64,
    pub name: String,
    pub description: String,
    /// Per channel, 1.0 = 100 %.
    pub volume: Vec<f64>,
    /// Channel positions (`front-left`…).
    pub channels: Vec<String>,
    pub muted: bool,
    /// The card it belongs to, when it has one.
    pub card: Option<u64>,
    /// Its ports (speakers or headphones, internal or external
    /// microphone…), and the one in use.
    pub ports: Vec<Port>,
    pub active_port: String,
}

/// A port of a device: where the sound goes or comes from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Port {
    pub name: String,
    pub description: String,
    /// False when the port is known to be unplugged.
    pub available: bool,
}

const LEFT: [&str; 3] = ["front-left", "rear-left", "side-left"];
const RIGHT: [&str; 3] = ["front-right", "rear-right", "side-right"];
const FRONT: [&str; 3] = ["front-left", "front-right", "front-center"];
const REAR: [&str; 3] = ["rear-left", "rear-right", "rear-center"];

impl Device {
    /// The loudest channel's volume (what the slider shows).
    pub fn level(&self) -> f64 {
        self.volume.iter().copied().fold(0.0, f64::max)
    }

    /// Left/right balance, −1 (left) … 1 (right), for two channels.
    pub fn balance(&self) -> Option<f64> {
        let (l, r) = self.left_right()?;
        let m = self.volume[l].max(self.volume[r]);
        if m <= 0.0 {
            return Some(0.0);
        }
        Some((self.volume[r] - self.volume[l]) / m)
    }

    fn left_right(&self) -> Option<(usize, usize)> {
        let l = self.channels.iter().position(|c| c == "front-left")?;
        let r = self.channels.iter().position(|c| c == "front-right")?;
        Some((l, r))
    }

    fn loudest(&self, set: &[&str]) -> Option<f64> {
        self.channels
            .iter()
            .zip(&self.volume)
            .filter(|(c, _)| set.contains(&c.as_str()))
            .map(|(_, v)| *v)
            .reduce(f64::max)
    }

    /// Front/rear fade, −1 (rear) … 1 (front), for devices with front and
    /// rear speakers.
    pub fn fade(&self) -> Option<f64> {
        let front = self.loudest(&FRONT)?;
        let rear = self.loudest(&REAR)?;
        let m = front.max(rear);
        if m <= 0.0 {
            return Some(0.0);
        }
        Some((front - rear) / m)
    }

    /// Channel volumes for `level`, `balance` (left channels quieter
    /// towards the right, and the other way) and `fade` (rear channels
    /// quieter towards the front, and the other way); the centre and
    /// subwoofer stay at `level` unless faded.
    pub fn volumes(&self, level: f64, balance: f64, fade: f64) -> Vec<f64> {
        let mut v = vec![level; self.channels.len().max(1)];
        for (i, c) in self.channels.iter().enumerate() {
            let c = c.as_str();
            if LEFT.contains(&c) {
                v[i] *= 1.0 - balance.max(0.0);
            }
            if RIGHT.contains(&c) {
                v[i] *= 1.0 + balance.min(0.0);
            }
            if REAR.contains(&c) {
                v[i] *= 1.0 - fade.max(0.0);
            }
            if FRONT.contains(&c) {
                v[i] *= 1.0 + fade.min(0.0);
            }
        }
        v
    }
}

/// An app playing sound (a sink input).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct App {
    pub index: u64,
    pub name: String,
    pub icon: String,
    pub media: String,
    pub volume: f64,
    pub muted: bool,
    /// `media.role` (`event` for system sounds).
    pub role: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub available: bool,
}

/// A sound card and its profiles (HDMI or analogue, A2DP or HFP…).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Card {
    pub index: u64,
    pub name: String,
    pub description: String,
    pub profiles: Vec<Profile>,
    pub active: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sound {
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    pub apps: Vec<App>,
    pub cards: Vec<Card>,
    pub default_output: String,
    pub default_input: String,
}

impl Sound {
    pub fn output(&self) -> Option<&Device> {
        self.outputs
            .iter()
            .find(|d| d.name == self.default_output)
            .or(self.outputs.first())
    }

    pub fn input(&self) -> Option<&Device> {
        self.inputs
            .iter()
            .find(|d| d.name == self.default_input)
            .or(self.inputs.first())
    }

    pub fn card(&self, index: Option<u64>) -> Option<&Card> {
        let i = index?;
        self.cards.iter().find(|c| c.index == i)
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn prop(v: &Value, k: &str) -> String {
    v.get("properties")
        .and_then(|p| p.get(k))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn volumes(v: &Value) -> (Vec<String>, Vec<f64>) {
    let mut channels = Vec::new();
    let mut vols = Vec::new();
    let map: Vec<String> = s(v, "channel_map")
        .split(',')
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(Value::Object(o)) = v.get("volume") {
        // In channel-map order when there is one.
        let mut keys: Vec<&String> = o.keys().collect();
        keys.sort_by_key(|k| map.iter().position(|m| m == *k).unwrap_or(usize::MAX));
        for k in keys {
            let value = o[k].get("value").and_then(Value::as_f64).unwrap_or(0.0);
            channels.push(k.clone());
            vols.push(value / NORM);
        }
    }
    (channels, vols)
}

fn device(v: &Value) -> Device {
    let (channels, volume) = volumes(v);
    let description = s(v, "description");
    Device {
        index: v.get("index").and_then(Value::as_u64).unwrap_or(0),
        name: s(v, "name"),
        description: if description.is_empty() {
            s(v, "name")
        } else {
            description
        },
        volume,
        channels,
        muted: v.get("mute").and_then(Value::as_bool).unwrap_or(false),
        card: v.get("card").and_then(Value::as_u64),
        ports: v
            .get("ports")
            .and_then(Value::as_array)
            .map(|ports| {
                ports
                    .iter()
                    .map(|p| Port {
                        name: s(p, "name"),
                        description: s(p, "description"),
                        available: s(p, "availability") != "not available",
                    })
                    .collect()
            })
            .unwrap_or_default(),
        active_port: s(v, "active_port"),
    }
}

/// Parses `pactl -f json list` output (pure, for tests).
pub fn parse(sinks: &[Value], sources: &[Value], inputs: &[Value], cards: &[Value]) -> Sound {
    let outputs = sinks.iter().map(device).collect();
    let inputs_dev = sources
        .iter()
        .filter(|v| {
            // Monitors of outputs are not microphones.
            let monitor = v
                .get("monitor_of_sink")
                .is_some_and(|m| !m.is_null() && m != "n/a");
            !monitor && !s(v, "name").ends_with(".monitor") && prop(v, "device.class") != "monitor"
        })
        .map(device)
        .collect();
    let apps = inputs
        .iter()
        .map(|v| {
            let (_, vol) = volumes(v);
            let mut name = prop(v, "application.name");
            if name.is_empty() {
                name = prop(v, "media.name");
            }
            App {
                index: v.get("index").and_then(Value::as_u64).unwrap_or(0),
                name,
                icon: prop(v, "application.icon_name"),
                media: prop(v, "media.name"),
                volume: vol.iter().copied().fold(0.0, f64::max),
                muted: v.get("mute").and_then(Value::as_bool).unwrap_or(false),
                role: prop(v, "media.role"),
            }
        })
        .collect();
    let cards = cards
        .iter()
        .map(|v| {
            let mut profiles: Vec<(i64, Profile)> = v
                .get("profiles")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .map(|(name, p)| {
                            (
                                p.get("priority").and_then(Value::as_i64).unwrap_or(0),
                                Profile {
                                    name: name.clone(),
                                    description: s(p, "description"),
                                    available: p
                                        .get("available")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(true),
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            profiles.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
            let mut description = prop(v, "device.description");
            if description.is_empty() {
                description = s(v, "name");
            }
            Card {
                index: v.get("index").and_then(Value::as_u64).unwrap_or(0),
                name: s(v, "name"),
                description,
                profiles: profiles.into_iter().map(|(_, p)| p).collect(),
                active: s(v, "active_profile"),
            }
        })
        .collect();
    Sound {
        outputs,
        inputs: inputs_dev,
        apps,
        cards,
        default_output: String::new(),
        default_input: String::new(),
    }
}

/// Reads everything the panel shows.
pub fn read(server: &Server) -> Result<Sound> {
    let sinks = server.json("sinks")?;
    let sources = server.json("sources")?;
    let inputs = server.json("sink-inputs")?;
    let cards = server.json("cards").unwrap_or_default();
    let mut sound = parse(&sinks, &sources, &inputs, &cards);
    sound.default_output = server.pactl(&["get-default-sink"])?.trim().to_string();
    sound.default_input = server.pactl(&["get-default-source"])?.trim().to_string();
    Ok(sound)
}

fn percents(v: &[f64]) -> Vec<String> {
    v.iter()
        .map(|x| format!("{}%", (x * 100.0).round().max(0.0)))
        .collect()
}

pub fn set_default(server: &Server, output: bool, name: &str) -> Result<()> {
    let cmd = if output {
        "set-default-sink"
    } else {
        "set-default-source"
    };
    server.pactl(&[cmd, name]).map(drop)
}

/// Sets a device's channel volumes (1.0 = 100 %).
pub fn set_volume(server: &Server, output: bool, name: &str, channels: &[f64]) -> Result<()> {
    let cmd = if output {
        "set-sink-volume"
    } else {
        "set-source-volume"
    };
    let p = percents(channels);
    let mut args = vec![cmd, name];
    args.extend(p.iter().map(String::as_str));
    server.pactl(&args).map(drop)
}

pub fn set_mute(server: &Server, output: bool, name: &str, muted: bool) -> Result<()> {
    let cmd = if output {
        "set-sink-mute"
    } else {
        "set-source-mute"
    };
    server
        .pactl(&[cmd, name, if muted { "1" } else { "0" }])
        .map(drop)
}

pub fn set_app_volume(server: &Server, index: u64, volume: f64) -> Result<()> {
    let i = index.to_string();
    let p = percents(&[volume]);
    server
        .pactl(&["set-sink-input-volume", &i, &p[0]])
        .map(drop)
}

pub fn set_app_mute(server: &Server, index: u64, muted: bool) -> Result<()> {
    let i = index.to_string();
    server
        .pactl(&["set-sink-input-mute", &i, if muted { "1" } else { "0" }])
        .map(drop)
}

/// Switches a device to one of its ports.
pub fn set_port(server: &Server, output: bool, device: &str, port: &str) -> Result<()> {
    let verb = if output {
        "set-sink-port"
    } else {
        "set-source-port"
    };
    server.pactl(&[verb, device, port]).map(|_| ())
}

pub fn set_profile(server: &Server, card: &str, profile: &str) -> Result<()> {
    server.pactl(&["set-card-profile", card, profile]).map(drop)
}

/// The metadata object where pipewire-pulse keeps PulseAudio's
/// stream-restore entries (WirePlumber makes it and stores what is
/// written there).
const ROUTE_SETTINGS: &str = "route-settings";
/// The stream-restore entry of event sounds: PulseAudio's
/// `sink-input-by-media-role:event`, as pipewire-pulse names it (it maps
/// the `event` role to PipeWire's `Notification`). GNOME Settings and
/// pavucontrol set the system sounds volume there through libpulse's
/// ext-stream-restore, which pipewire-pulse turns into this key.
pub const EVENT_KEY: &str = "restore.stream.Output/Audio.media.role:Notification";

/// The volume of system (event) sounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventVolume {
    /// 1.0 = 100 %, as the other sliders (PulseAudio's cubic scale).
    pub level: f64,
    pub muted: bool,
}

/// Reads `pw-metadata -n route-settings 0` output (pure, for tests):
/// `None` without the metadata object (no session manager keeping
/// stream-restore entries), else the entry, 100 % when there is none.
pub fn parse_event_volume(listing: &str) -> Option<EventVolume> {
    let mut found = false;
    let mut volume = EventVolume {
        level: 1.0,
        muted: false,
    };
    for line in listing.lines() {
        if line.starts_with(&format!("Found \"{ROUTE_SETTINGS}\"")) {
            found = true;
        }
        let Some(rest) = line.split_once(&format!("key:'{EVENT_KEY}' value:'")) else {
            continue;
        };
        let json = rest.1.rsplit_once("' type:").map_or(rest.1, |r| r.0);
        let Ok(v) = serde_json::from_str::<Value>(json) else {
            continue;
        };
        // The values are linear amplitudes, as pipewire-pulse stores them.
        let linear = v
            .get("volumes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).fold(0.0, f64::max));
        if let Some(l) = linear {
            volume.level = l.max(0.0).cbrt();
        }
        volume.muted = v.get("mute").and_then(Value::as_bool).unwrap_or(false);
    }
    found.then_some(volume)
}

/// The system sounds volume; `Ok(None)` when the sound server keeps no
/// stream-restore entries (no PipeWire session manager).
pub fn read_event_volume(server: &Server) -> Result<Option<EventVolume>> {
    server
        .pw_metadata(&["0"])
        .map(|out| parse_event_volume(&out))
}

/// The entry for `volume`, as pipewire-pulse writes one: linear
/// amplitudes per channel. Stereo, because WirePlumber applies the
/// volumes to a new event sound by position and leaves the right
/// channel of a stereo sound silent with a mono entry; a mono sound
/// takes the first.
pub fn event_entry(volume: &EventVolume) -> String {
    let v = volume.level.max(0.0).powi(3);
    format!(
        "{{ \"mute\": {}, \"volumes\": [ {v:.6}, {v:.6} ], \"channels\": [ \"FL\", \"FR\" ] }}",
        volume.muted
    )
}

/// Sets the system sounds volume for the sounds to come (WirePlumber
/// applies it to each new event sound and keeps it).
pub fn set_event_volume(server: &Server, volume: &EventVolume) -> Result<()> {
    let entry = event_entry(volume);
    server
        .pw_metadata(&["0", EVENT_KEY, &entry, "Spa:String:JSON"])
        .map(drop)
}

/// Calls `changed` when the system sounds volume changes (or the
/// metadata object appears); returns when it returns false or the sound
/// server goes away.
pub fn watch_event_volume(server: &Server, mut changed: impl FnMut() -> bool) {
    let Ok(mut child) = server
        .command("pw-metadata")
        .args(["-m", "-n", ROUTE_SETTINGS])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            let shown = line.starts_with("Found ") || line.contains(&format!("key:'{EVENT_KEY}'"));
            if shown && !changed() {
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Calls `changed` for every change the server reports; returns when it
/// returns false or the server goes away.
pub fn watch(server: &Server, mut changed: impl FnMut() -> bool) {
    let Ok(mut child) = server
        .command("pactl")
        .arg("subscribe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            // Client and module events are not what the panel shows.
            let shown = ["sink", "source", "card", "server"]
                .iter()
                .any(|k| line.contains(&format!(" on {k}")));
            if shown && !changed() {
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A running level meter; dropping it stops it.
pub struct Meter {
    child: std::process::Child,
}

impl Drop for Meter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts reporting an input's peak level (0…1), about twenty times a
/// second, to `level` (on a thread of its own) until the meter is dropped
/// or `level` returns false.
pub fn meter(
    server: &Server,
    source: &str,
    mut level: impl FnMut(f32) -> bool + Send + 'static,
) -> Option<Meter> {
    const RATE: usize = 8000;
    let mut child = server
        .command("parec")
        .args([
            "--raw",
            "--format=float32le",
            "--channels=1",
            &format!("--rate={RATE}"),
            "--latency-msec=50",
            "--property=media.role=filter",
            "-d",
            source,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut buf = vec![0u8; RATE / 20 * 4];
        while out.read_exact(&mut buf).is_ok() {
            let peak = buf
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b).abs())
                .fold(0.0f32, f32::max);
            if !level(peak.min(1.0)) {
                break;
            }
        }
    });
    Some(Meter { child })
}

/// A short tone, to hear which speaker is which.
pub fn tone() -> Vec<u8> {
    const RATE: f32 = 48000.0;
    let n = (RATE * 0.6) as usize;
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let t = i as f32 / RATE;
        // Fade in and out to avoid clicks.
        let env = (t / 0.05).min(1.0).min((0.6 - t) / 0.05).max(0.0);
        let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.3 * env;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Plays the tone on one channel of an output.
pub fn test_speaker(server: &Server, sink: &str, channel: &str) -> Result<()> {
    let mut child = server
        .command("pacat")
        .args([
            "--playback",
            "--raw",
            "--format=float32le",
            "--rate=48000",
            "--channels=1",
            &format!("--channel-map={channel}"),
            "--property=media.role=test",
            "-d",
            sink,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| Error::Missing)?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&tone());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::Other(e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// The name of the sound theme Helix writes for a chosen alert sound.
pub const CUSTOM_THEME: &str = "__custom";
/// The sound names the alert sound stands for.
const BELLS: [&str; 2] = ["bell-terminal.oga", "bell-window-system.oga"];

/// Alert sounds to choose from: (name shown, file), from
/// `sounds/gnome/default/alerts` in the data directories.
pub fn alert_choices(data_dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for d in data_dirs {
        let Ok(entries) = std::fs::read_dir(d.join("sounds/gnome/default/alerts")) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("ogg" | "oga" | "wav")
                )
            })
            .collect();
        found.sort();
        for p in found {
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut name: Vec<char> = stem.replace(['-', '_'], " ").chars().collect();
            if let Some(c) = name.first_mut() {
                *c = c.to_ascii_uppercase();
            }
            let name: String = name.into_iter().collect();
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, p));
            }
        }
    }
    out
}

/// The alert sound file chosen with [`set_alert`], when `theme` is ours.
pub fn current_alert(data_home: &Path, theme: &str) -> Option<PathBuf> {
    if theme != CUSTOM_THEME {
        return None;
    }
    std::fs::read_link(data_home.join("sounds").join(CUSTOM_THEME).join(BELLS[1])).ok()
}

/// Makes `file` the alert sound: a sound theme of its own that inherits
/// from `previous` and links the bell sounds to the file. Returns the
/// theme name to store in `org.gnome.desktop.sound theme-name`. With
/// `None`, the theme's own bell is used again (returns `previous`).
pub fn set_alert(data_home: &Path, file: Option<&Path>, previous: &str) -> std::io::Result<String> {
    let dir = data_home.join("sounds").join(CUSTOM_THEME);
    let Some(file) = file else {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(previous.to_string());
    };
    std::fs::create_dir_all(&dir)?;
    let parent = if previous == CUSTOM_THEME || previous.is_empty() {
        // Keep what the theme inherited before.
        std::fs::read_to_string(dir.join("index.theme"))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("Inherits=").map(str::to_owned))
            })
            .unwrap_or_else(|| "freedesktop".into())
    } else {
        previous.to_string()
    };
    std::fs::write(
        dir.join("index.theme"),
        format!("[Sound Theme]\nName={CUSTOM_THEME}\nInherits={parent}\nDirectories=.\n\n[.]\nOutputProfile=stereo\n"),
    )?;
    for bell in BELLS {
        let link = dir.join(bell);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(file, &link)?;
    }
    Ok(CUSTOM_THEME.to_string())
}

/// The theme `set_alert` inherits from, for going back to the default.
pub fn inherited_theme(data_home: &Path) -> String {
    std::fs::read_to_string(
        data_home
            .join("sounds")
            .join(CUSTOM_THEME)
            .join("index.theme"),
    )
    .ok()
    .and_then(|t| {
        t.lines()
            .find_map(|l| l.strip_prefix("Inherits=").map(str::to_owned))
    })
    .unwrap_or_else(|| "freedesktop".into())
}

/// A channel position in words.
pub fn channel_label(position: &str) -> &'static str {
    use helix_app_common::i18n::tr;
    match position {
        "front-left" => tr("Front Left"),
        "front-right" => tr("Front Right"),
        "front-center" => tr("Front Center"),
        "rear-left" => tr("Rear Left"),
        "rear-right" => tr("Rear Right"),
        "rear-center" => tr("Rear Center"),
        "side-left" => tr("Side Left"),
        "side-right" => tr("Side Right"),
        "lfe" => tr("Subwoofer"),
        "mono" => tr("Mono"),
        _ => tr("Speaker"),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::process::Child;

    /// A private PipeWire with pipewire-pulse in a temporary runtime
    /// directory (never the user's sound server), with three virtual
    /// devices.
    pub struct Private {
        children: Vec<Child>,
        pub dir: PathBuf,
        pub server: Server,
    }

    impl Drop for Private {
        fn drop(&mut self) {
            for c in self.children.iter_mut().rev() {
                let _ = c.kill();
                let _ = c.wait();
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    pub fn private() -> Option<Private> {
        let dir = std::env::temp_dir().join(format!(
            "helix-sound-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos()
        ));
        let conf = dir.join(".config/pipewire/pipewire.conf.d");
        std::fs::create_dir_all(&conf).ok()?;
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).ok()?;
        // A "default" metadata object, as a session manager would make.
        std::fs::write(
            conf.join("meta.conf"),
            "context.objects = [ { factory = metadata args = { metadata.name = default } } ]\n",
        )
        .ok()?;
        let spawn = |program: &str| {
            Command::new(program)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &dir)
                .env("XDG_RUNTIME_DIR", &dir)
                .env("PIPEWIRE_RUNTIME_DIR", &dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
        };
        let mut p = Private {
            children: Vec::new(),
            server: Server {
                address: Some(format!("unix:{}", dir.join("pulse/native").display())),
                pipewire: Some(dir.clone()),
            },
            dir: dir.clone(),
        };
        p.children.push(spawn("pipewire")?);
        std::thread::sleep(std::time::Duration::from_millis(400));
        p.children.push(spawn("pipewire-pulse")?);
        for _ in 0..50 {
            if dir.join("pulse/native").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        for (name, props) in [
            ("speakers", "device.description=Speakers"),
            ("hdmi", "device.description=HDMI"),
            (
                "mic",
                "device.description=Microphone media.class=Audio/Source/Virtual",
            ),
        ] {
            let name = format!("sink_name={name}");
            let props = format!("sink_properties={props}");
            p.server
                .pactl(&["load-module", "module-null-sink", &name, &props])
                .ok()?;
        }
        Some(p)
    }

    /// Plays the session manager's part: the default output is `name`.
    fn resolve_default(p: &Private, key: &str, name: &str) {
        let _ = Command::new("pw-metadata")
            .env_clear()
            .env("XDG_RUNTIME_DIR", &p.dir)
            .env("PIPEWIRE_RUNTIME_DIR", &p.dir)
            .args([
                "-n",
                "default",
                "0",
                key,
                &format!("{{\"name\":\"{name}\"}}"),
            ])
            .stdout(Stdio::null())
            .status();
    }

    fn until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..50 {
            if f() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn against_a_private_pipewire() {
        let Some(p) = private() else {
            eprintln!("pipewire or pipewire-pulse unavailable; skipping");
            return;
        };
        let server = &p.server;
        let s = read(server).unwrap();
        let outs: Vec<&str> = s.outputs.iter().map(|d| d.description.as_str()).collect();
        assert_eq!(outs, ["Speakers", "HDMI"]);
        let ins: Vec<&str> = s.inputs.iter().map(|d| d.description.as_str()).collect();
        assert_eq!(ins, ["Microphone"], "monitors are not inputs");
        assert_eq!(s.outputs[0].channels, ["front-left", "front-right"]);
        // No session manager, so no stream-restore entries: the system
        // sounds volume is not available.
        assert_eq!(read_event_volume(server), Ok(None));

        // Live changes are reported.
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = server.clone();
        std::thread::spawn(move || watch(&watcher, || tx.send(()).is_ok()));
        std::thread::sleep(std::time::Duration::from_millis(300));

        let speakers = &s.outputs[0];
        set_volume(server, true, "speakers", &speakers.volumes(0.5, 0.5, 0.0)).unwrap();
        assert!(rx.recv_timeout(std::time::Duration::from_secs(3)).is_ok());
        assert!(until(|| {
            let d = read(server).unwrap().outputs[0].clone();
            (d.level() - 0.5).abs() < 0.01 && (d.balance().unwrap() - 0.5).abs() < 0.02
        }));
        set_mute(server, true, "speakers", true).unwrap();
        assert!(until(|| read(server).unwrap().outputs[0].muted));
        set_volume(server, false, "mic", &[0.7]).unwrap();
        assert!(until(|| (read(server).unwrap().inputs[0].level() - 0.7)
            .abs()
            < 0.01));

        set_default(server, true, "hdmi").unwrap();
        resolve_default(&p, "default.audio.sink", "hdmi");
        assert!(until(
            || read(server).unwrap().output().unwrap().name == "hdmi"
        ));

        // The level meter runs until dropped. (Nothing links streams on
        // this server; the fake audio session below has a linking policy
        // and checks the levels.)
        let m = meter(server, "mic", |l| (0.0..=1.0).contains(&l)).unwrap();
        let started = std::time::Instant::now();
        std::mem::drop(m);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        // An app playing sound shows, with its own volume.
        let mut player = p
            .server
            .command("pacat")
            .args([
                "--playback",
                "--raw",
                "--property=application.name=Rhythm Test",
                "-d",
                "speakers",
                "/dev/zero",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(until(|| read(server)
            .unwrap()
            .apps
            .iter()
            .any(|a| a.name == "Rhythm Test")));
        let app = read(server)
            .unwrap()
            .apps
            .into_iter()
            .find(|a| a.name == "Rhythm Test")
            .unwrap();
        set_app_volume(server, app.index, 0.25).unwrap();
        set_app_mute(server, app.index, true).unwrap();
        assert!(until(|| {
            read(server)
                .unwrap()
                .apps
                .iter()
                .any(|a| a.index == app.index && (a.volume - 0.25).abs() < 0.01 && a.muted)
        }));
        let _ = player.kill();
        let _ = player.wait();
    }

    /// The speaker test plays on exactly the channel asked for, on each
    /// channel of a 5.1 null sink, and the input meter moves while a test
    /// source plays into the microphone (fake audio session: a private
    /// PipeWire with null devices and a linking policy, no hardware).
    #[test]
    fn speaker_test_and_meter_with_fake_audio() {
        use helix_test_audio::{FakeAudio, MICROPHONE, SURROUND};
        let Some(audio) = FakeAudio::start() else {
            return;
        };
        let server = Server {
            address: Some(audio.pulse_server()),
            pipewire: Some(audio.runtime_dir().to_path_buf()),
        };
        let s = read(&server).unwrap();
        let surround = s.outputs.iter().find(|d| d.name == SURROUND).unwrap();
        assert_eq!(surround.channels.len(), 6);
        for (i, channel) in surround.channels.iter().enumerate() {
            let (srv, sink, ch) = (server.clone(), SURROUND.to_string(), channel.clone());
            let player = std::thread::spawn(move || test_speaker(&srv, &sink, &ch));
            // The tone lasts 0.6 s; listen to the middle of it.
            std::thread::sleep(std::time::Duration::from_millis(100));
            let peaks = audio
                .peaks(
                    &format!("{SURROUND}.monitor"),
                    6,
                    std::time::Duration::from_millis(300),
                )
                .unwrap();
            player.join().unwrap().unwrap();
            for (j, p) in peaks.iter().enumerate() {
                if i == j {
                    assert!(*p > 0.1, "{channel}: silent: {peaks:?}");
                } else {
                    assert!(*p < 0.01, "{channel}: also on {j}: {peaks:?}");
                }
            }
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let m = meter(&server, MICROPHONE, move |l| tx.send(l).is_ok()).unwrap();
        let next = || {
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("no levels from the meter")
        };
        let quiet = (0..6).map(|_| next()).fold(0.0f32, f32::max);
        assert!(quiet < 0.01, "a silent microphone shows {quiet}");
        let tone = audio
            .play(MICROPHONE, &["MONO"], &["MONO"], 440.0, 0.5)
            .unwrap();
        // Linking the tone into the microphone takes a few policy rounds;
        // slow CI runners need more than a second or two.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut loudest = 0.0f32;
        while loudest <= 0.2 && std::time::Instant::now() < deadline {
            loudest = loudest.max(next());
        }
        if loudest <= 0.2 {
            let links = audio
                .command("pw-link")
                .arg("-l")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            panic!(
                "the meter did not move while the test source played (loudest {loudest})\n\
                 sink inputs:\n{}\nsource outputs:\n{}\nlinks:\n{links}",
                audio
                    .pactl(&["list", "short", "sink-inputs"])
                    .unwrap_or_default(),
                audio
                    .pactl(&["list", "short", "source-outputs"])
                    .unwrap_or_default(),
            );
        }
        drop(tone);
        drop(m);
    }

    #[test]
    fn no_server() {
        let server = Server {
            address: Some("unix:/nonexistent/helix-test/native".into()),
            pipewire: Some("/nonexistent/helix-test".into()),
        };
        assert_eq!(read(&server), Err(Error::Missing));
        assert_eq!(read_event_volume(&server), Err(Error::Missing));
    }

    /// The system sounds volume through the fake audio session's
    /// `route-settings` metadata: read, set, and followed when another
    /// program (pavucontrol or GNOME Settings, through pipewire-pulse)
    /// changes it.
    #[test]
    fn system_sounds_volume_with_fake_audio() {
        use helix_test_audio::FakeAudio;
        let Some(audio) = FakeAudio::start() else {
            return;
        };
        let server = Server {
            address: Some(audio.pulse_server()),
            pipewire: Some(audio.runtime_dir().to_path_buf()),
        };
        let none = EventVolume {
            level: 1.0,
            muted: false,
        };
        assert_eq!(read_event_volume(&server), Ok(Some(none)));

        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = server.clone();
        std::thread::spawn(move || watch_event_volume(&watcher, || tx.send(()).is_ok()));
        // The first line ("Found …") once the watcher is connected.
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();

        let half = EventVolume {
            level: 0.5,
            muted: false,
        };
        set_event_volume(&server, &half).unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert_eq!(read_event_volume(&server), Ok(Some(half)));
        let raw = audio
            .command("pw-metadata")
            .args(["-n", "route-settings", "0"])
            .output()
            .unwrap();
        let raw = String::from_utf8_lossy(&raw.stdout);
        assert!(
            raw.contains(r#""volumes": [ 0.125000, 0.125000 ], "channels": [ "FL", "FR" ]"#),
            "{raw}"
        );

        // As pipewire-pulse writes an ext-stream-restore entry from
        // libpulse: mono, linear, here 20 % and muted.
        audio
            .command("pw-metadata")
            .args([
                "-n",
                "route-settings",
                "0",
                EVENT_KEY,
                r#"{ "mute": true, "volumes": [ 0.008000 ], "channels": [ "MONO" ] }"#,
                "Spa:String:JSON",
            ])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        let v = read_event_volume(&server).unwrap().unwrap();
        assert!((v.level - 0.2).abs() < 1e-6 && v.muted, "{v:?}");

        // Removed: back to 100 %.
        audio
            .command("pw-metadata")
            .args(["-n", "route-settings", "-d", "0", EVENT_KEY])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert_eq!(read_event_volume(&server), Ok(Some(none)));
    }

    #[test]
    fn event_volume_entries() {
        let listing = "Found \"route-settings\" metadata 39\n\
            update: id:0 key:'restore.stream.Output/Audio.media.role:Movie' value:'{ \"volumes\": [ 1.0 ] }' type:'Spa:String:JSON'\n\
            update: id:0 key:'restore.stream.Output/Audio.media.role:Notification' value:'{ \"mute\": false, \"volumes\": [ 0.064000, 0.027000 ], \"channels\": [ \"FL\", \"FR\" ] }' type:'Spa:String:JSON'\n";
        let v = parse_event_volume(listing).unwrap();
        assert!((v.level - 0.4).abs() < 1e-9, "the louder channel: {v:?}");
        assert!(!v.muted);
        // No entry: 100 %; no metadata object: not available.
        assert_eq!(
            parse_event_volume("Found \"route-settings\" metadata 39\n"),
            Some(EventVolume {
                level: 1.0,
                muted: false
            })
        );
        assert_eq!(parse_event_volume(""), None);
        let e = event_entry(&EventVolume {
            level: 0.5,
            muted: true,
        });
        assert_eq!(
            e,
            r#"{ "mute": true, "volumes": [ 0.125000, 0.125000 ], "channels": [ "FL", "FR" ] }"#
        );
        let back = format!("Found \"route-settings\" metadata 1\nupdate: id:0 key:'{EVENT_KEY}' value:'{e}' type:'Spa:String:JSON'\n");
        let v = parse_event_volume(&back).unwrap();
        assert!((v.level - 0.5).abs() < 1e-6 && v.muted);
    }

    #[test]
    fn cards_and_balance() {
        let cards: Vec<Value> = serde_json::from_str(
            r#"[{"index":3,"name":"alsa_card.pci-0000_00_1f.3","active_profile":"output:hdmi-stereo",
                "properties":{"device.description":"Built-in Audio"},
                "profiles":{"off":{"description":"Off","priority":0,"available":true},
                            "output:analog-stereo":{"description":"Analog Stereo Output","priority":6500,"available":true},
                            "output:hdmi-stereo":{"description":"Digital Stereo (HDMI) Output","priority":5900,"available":false}}}]"#,
        )
        .unwrap();
        let sinks: Vec<Value> = serde_json::from_str(
            r#"[{"index":1,"name":"out","description":"Out","card":3,"mute":false,"channel_map":"front-left,front-right",
                 "volume":{"front-right":{"value":32768},"front-left":{"value":65536}}}]"#,
        )
        .unwrap();
        let s = parse(&sinks, &[], &[], &cards);
        let c = s.card(s.outputs[0].card).unwrap();
        assert_eq!(c.description, "Built-in Audio");
        let names: Vec<&str> = c.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["output:analog-stereo", "output:hdmi-stereo", "off"]);
        assert!(!c.profiles[1].available);
        let d = &s.outputs[0];
        assert_eq!(d.volume, [1.0, 0.5], "channel-map order");
        assert_eq!(d.balance(), Some(-0.5));
        assert_eq!(d.volumes(0.8, -0.5, 0.0), [0.8, 0.4]);
        assert_eq!(d.volumes(0.8, 0.25, 0.0), [0.6000000000000001, 0.8]);
        assert_eq!(d.fade(), None, "no rear speakers");
        assert!(d.ports.is_empty());
        assert_eq!(channel_label("lfe"), "Subwoofer");
    }

    #[test]
    fn fade_and_ports() {
        let sinks: Vec<Value> = serde_json::from_str(
            r#"[{"index":1,"name":"surround","description":"Surround","mute":false,
                 "channel_map":"front-left,front-right,rear-left,rear-right,front-center,lfe",
                 "volume":{"front-left":{"value":65536},"front-right":{"value":65536},
                           "rear-left":{"value":32768},"rear-right":{"value":32768},
                           "front-center":{"value":65536},"lfe":{"value":65536}},
                 "active_port":"analog-output-speaker",
                 "ports":[{"name":"analog-output-speaker","description":"Speakers","availability":"available"},
                          {"name":"analog-output-headphones","description":"Headphones","availability":"not available"}]}]"#,
        )
        .unwrap();
        let s = parse(&sinks, &[], &[], &[]);
        let d = &s.outputs[0];
        assert_eq!(d.fade(), Some(0.5), "towards the front");
        assert_eq!(d.balance(), Some(0.0));
        // Rear quieter (fade to the front), right quieter (balance left).
        assert_eq!(d.volumes(1.0, -0.5, 0.5), [1.0, 0.5, 0.5, 0.25, 1.0, 1.0]);
        assert_eq!(d.volumes(1.0, 0.0, -1.0), [0.0, 0.0, 1.0, 1.0, 0.0, 1.0]);
        assert_eq!(d.active_port, "analog-output-speaker");
        let ports: Vec<(&str, bool)> = d
            .ports
            .iter()
            .map(|p| (p.description.as_str(), p.available))
            .collect();
        assert_eq!(ports, [("Speakers", true), ("Headphones", false)]);
    }

    #[test]
    fn alert_sound_theme() {
        let home = std::env::temp_dir().join(format!("helix-alert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let data = home.join("share");
        let alerts = data.join("sounds/gnome/default/alerts");
        std::fs::create_dir_all(&alerts).unwrap();
        for f in ["swing.ogg", "click.ogg", "notes.txt"] {
            std::fs::write(alerts.join(f), b"").unwrap();
        }
        let choices = alert_choices(std::slice::from_ref(&data));
        let names: Vec<&str> = choices.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Click", "Swing"]);
        let user = home.join("user");
        assert_eq!(
            set_alert(&user, Some(&choices[1].1), "Yaru").unwrap(),
            CUSTOM_THEME
        );
        assert_eq!(
            current_alert(&user, CUSTOM_THEME),
            Some(choices[1].1.clone())
        );
        assert_eq!(current_alert(&user, "Yaru"), None);
        // Choosing again keeps the theme it inherits from.
        set_alert(&user, Some(&choices[0].1), CUSTOM_THEME).unwrap();
        assert_eq!(inherited_theme(&user), "Yaru");
        assert_eq!(set_alert(&user, None, "Yaru").unwrap(), "Yaru");
        assert!(!user.join("sounds/__custom").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
