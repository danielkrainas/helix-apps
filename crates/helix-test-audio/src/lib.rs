//! A fake audio session for tests: a private PipeWire daemon with null
//! (hardware-free) devices, its built-in PulseAudio server, and a minimal
//! linking policy in place of a session manager.
//!
//! Nothing here touches the host's audio: the daemon runs in its own
//! runtime directory with a configuration that loads no ALSA, Bluetooth,
//! JACK, D-Bus or realtime modules, and every client this crate starts
//! gets an environment that points at that directory only. The daemon is
//! killed by its own PID when the session is dropped.
//!
//! Devices (see [`SPEAKERS`], [`SURROUND`], [`MICROPHONE`]):
//!
//! | node | class | channels |
//! |---|---|---|
//! | `helix-test-speakers` | `Audio/Sink` | FL FR |
//! | `helix-test-surround` | `Audio/Sink` | FL FR FC LFE RL RR |
//! | `helix-test-microphone` | `Audio/Source/Virtual` | MONO |
//!
//! The linking policy (a thread polling `pw-dump`) links every playback
//! stream to its target (`target.object`, else the default sink) and every
//! recording stream from its target (a source, or a sink's monitor), channel
//! by channel (a one-channel stream with a position, such as a speaker test
//! on FL, to that position only; a MONO one to every channel); it also makes a default chosen through PulseAudio
//! (`default.configured.audio.*`) the default, as a session manager would.
//! It does not move streams that are already linked, and the devices have
//! no cards or profiles.
//!
//! Tests play into a device with [`FakeAudio::play`] (also into the
//! microphone, which then "hears" the tone) and read levels back with
//! [`FakeAudio::peaks`]. Not for use in the apps.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Stereo speakers (FL FR), the default sink.
pub const SPEAKERS: &str = "helix-test-speakers";
/// A 5.1 sink (FL FR FC LFE RL RR).
pub const SURROUND: &str = "helix-test-surround";
/// A mono microphone, the default source. Whatever is played into it
/// (`FakeAudio::play(MICROPHONE, …)`) is what it records.
pub const MICROPHONE: &str = "helix-test-microphone";

/// The sample rate of the tones and recordings.
pub const RATE: u32 = 48000;

const TOOLS: &[&str] = &["pipewire", "pw-dump", "pw-link", "pw-metadata", "pw-cat"];
const PULSE_TOOLS: &[&str] = &["pactl", "parec"];

fn tool(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

fn config() -> String {
    // No module-rt (it may ask RTKit on the system bus), no ALSA/BlueZ/JACK
    // device factories, no D-Bus: only null devices and the protocols.
    let sink = |name: &str, desc: &str, class: &str, pos: &str| {
        format!(
            r#"    {{ factory = adapter
        args = {{
            factory.name     = support.null-audio-sink
            node.name        = "{name}"
            node.description = "{desc}"
            media.class      = "{class}"
            audio.position   = [ {pos} ]
            monitor.channel-volumes = true
            object.linger    = true
            adapter.auto-port-config = {{ mode = dsp monitor = true position = preserve }}
        }}
    }}
"#
        )
    };
    format!(
        r#"context.properties = {{
    core.daemon = true
    core.name   = pipewire-0
    link.max-buffers = 16
    support.dbus = false
    mem.allow-mlock = false
}}
context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    audio.adapt     = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}
context.modules = [
    {{ name = libpipewire-module-protocol-native }}
    {{ name = libpipewire-module-metadata }}
    {{ name = libpipewire-module-spa-node-factory }}
    {{ name = libpipewire-module-client-node }}
    {{ name = libpipewire-module-access }}
    {{ name = libpipewire-module-adapter }}
    {{ name = libpipewire-module-link-factory }}
    {{ name = libpipewire-module-protocol-pulse }}
]
pulse.properties = {{
    server.address = [ "unix:native" ]
    server.dbus-name = ""
}}
stream.properties = {{
    adapter.auto-port-config = {{ mode = dsp position = preserve }}
}}
context.objects = [
    {{ factory = metadata args = {{ metadata.name = default }} }}
    {{ factory = spa-node-factory
        args = {{
            factory.name    = support.node.driver
            node.name       = Dummy-Driver
            node.group      = pipewire.dummy
            priority.driver = 20000
        }}
    }}
{speakers}{surround}{mic}]
"#,
        speakers = sink(SPEAKERS, "Test Speakers", "Audio/Sink", "FL FR"),
        surround = sink(
            SURROUND,
            "Test Surround",
            "Audio/Sink",
            "FL FR FC LFE RL RR"
        ),
        mic = sink(
            MICROPHONE,
            "Test Microphone",
            "Audio/Source/Virtual",
            "MONO"
        ),
    )
}

/// A running fake audio session; stopped when dropped.
pub struct FakeAudio {
    daemon: Child,
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    policy: Option<JoinHandle<()>>,
}

impl Drop for FakeAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(policy) = self.policy.take() {
            let _ = policy.join();
        }
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl FakeAudio {
    /// Starts the session. `None` (with a note on stderr) when PipeWire or
    /// the PulseAudio tools are not installed, or the daemon does not come
    /// up; the caller then skips its test.
    pub fn start() -> Option<FakeAudio> {
        if let Some(missing) = TOOLS.iter().chain(PULSE_TOOLS).find(|t| tool(t).is_none()) {
            eprintln!("{missing} not installed; skipping the fake audio session");
            return None;
        }
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "helix-fake-audio-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        // Native clients (pw-cat) read client.conf from here, or
        // client-rt.conf before PipeWire 1.2: give their streams ports the
        // way the daemon's stream.properties does for PulseAudio clients (a
        // session manager would configure them).
        for name in ["client.conf.d", "client-rt.conf.d"] {
            let client_conf = dir.join("config/pipewire").join(name);
            std::fs::create_dir_all(&client_conf).ok()?;
            std::fs::write(
                client_conf.join("helix-fake-audio.conf"),
                "stream.properties = { adapter.auto-port-config = { mode = dsp position = preserve } }\n",
            )
            .ok()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
        }
        let conf = dir.join("helix-fake-audio.conf");
        std::fs::write(&conf, config()).ok()?;
        let log = std::fs::File::create(dir.join("pipewire.log")).ok()?;
        let daemon = Command::new(tool("pipewire")?)
            .arg("-c")
            .arg(&conf)
            .env_clear()
            .envs(env_for(&dir))
            .stdin(Stdio::null())
            .stdout(log.try_clone().ok()?)
            .stderr(log)
            .spawn()
            .ok()?;
        let mut audio = FakeAudio {
            daemon,
            dir,
            stop: Arc::new(AtomicBool::new(false)),
            policy: None,
        };
        // Up once the PulseAudio socket answers and the devices are there.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let ready = audio.dir.join("pulse/native").exists()
                && audio
                    .command("pactl")
                    .args(["list", "short", "sinks"])
                    .output()
                    .is_ok_and(|o| {
                        o.status.success() && String::from_utf8_lossy(&o.stdout).contains(SURROUND)
                    });
            if ready {
                break;
            }
            if Instant::now() > deadline || audio.daemon.try_wait().ok().flatten().is_some() {
                eprintln!(
                    "the fake audio session did not start; skipping:\n{}",
                    std::fs::read_to_string(audio.dir.join("pipewire.log")).unwrap_or_default()
                );
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        audio.set_metadata("default.audio.sink", SPEAKERS);
        audio.set_metadata("default.audio.source", MICROPHONE);
        let stop = audio.stop.clone();
        let dir = audio.dir.clone();
        audio.policy = Some(std::thread::spawn(move || policy(&dir, &stop)));
        Some(audio)
    }

    /// The session's runtime directory (`XDG_RUNTIME_DIR`).
    pub fn runtime_dir(&self) -> &Path {
        &self.dir
    }

    /// The PulseAudio server address, for `PULSE_SERVER` or `pactl -s`.
    pub fn pulse_server(&self) -> String {
        format!("unix:{}", self.dir.join("pulse/native").display())
    }

    /// The environment for a client of this session (and nothing of the
    /// host's audio): `XDG_RUNTIME_DIR`, `PIPEWIRE_RUNTIME_DIR`,
    /// `PULSE_SERVER`, a scratch `HOME`, and `PATH`.
    pub fn env(&self) -> Vec<(String, String)> {
        env_for(&self.dir)
    }

    /// `program` (from `/usr/bin`) with [`FakeAudio::env`] and nothing else
    /// of the caller's environment.
    pub fn command(&self, program: &str) -> Command {
        let mut c = Command::new(tool(program).unwrap_or_else(|| program.into()));
        c.env_clear().envs(self.env()).stdin(Stdio::null());
        c
    }

    fn set_metadata(&self, key: &str, node: &str) {
        let _ = self
            .command("pw-metadata")
            .args(["-n", "default", "0", key])
            .arg(format!("{{\"name\":\"{node}\"}}"))
            .arg("Spa:String:JSON")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// Plays a sine of `freq` Hz into `target` (a sink, or the microphone)
    /// on the channel positions `channels` (e.g. `["FL", "FR"]`) at
    /// amplitude `amplitude` on `loud` (other channels get silence), until
    /// the returned [`Tone`] is dropped.
    pub fn play(
        &self,
        target: &str,
        channels: &[&str],
        loud: &[&str],
        freq: f32,
        amplitude: f32,
    ) -> Option<Tone> {
        // An AU stream on stdin, not `--raw`: pw-cat before PipeWire 1.2
        // (Ubuntu 24.04's 1.0.5, on CI) has no --raw, but every version
        // reads AU (which allows an unknown length) from a pipe.
        let mut child = self
            .command("pw-cat")
            .args(["--playback", "--channel-map"])
            .arg(channels.join(","))
            .arg("--target")
            .arg(target)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdin = child.stdin.take()?;
        let stop = Arc::new(AtomicBool::new(false));
        let mask: Vec<bool> = channels.iter().map(|c| loud.contains(c)).collect();
        let s = stop.clone();
        let header: Vec<u8> = [
            u32::from_be_bytes(*b".snd"),
            24,       // header size
            u32::MAX, // length unknown
            6,        // 32-bit IEEE float, big-endian
            RATE,
            mask.len() as u32,
        ]
        .iter()
        .flat_map(|w| w.to_be_bytes())
        .collect();
        let writer = std::thread::spawn(move || {
            if stdin.write_all(&header).is_err() {
                return;
            }
            let mut n: u64 = 0;
            let block = (RATE / 100) as usize;
            let started = Instant::now();
            while !s.load(Ordering::SeqCst) {
                let mut buf = Vec::with_capacity(block * mask.len() * 4);
                for _ in 0..block {
                    let v = amplitude
                        * (2.0 * std::f32::consts::PI * freq * n as f32 / RATE as f32).sin();
                    n += 1;
                    for on in &mask {
                        buf.extend_from_slice(&(if *on { v } else { 0.0 }).to_be_bytes());
                    }
                }
                if stdin.write_all(&buf).is_err() {
                    break;
                }
                // Keep about 100 ms ahead of real time.
                let ahead = Duration::from_secs_f64(n as f64 / RATE as f64)
                    .saturating_sub(started.elapsed());
                if ahead > Duration::from_millis(100) {
                    std::thread::sleep(ahead - Duration::from_millis(100));
                }
            }
        });
        Some(Tone {
            child,
            stop,
            writer: Some(writer),
        })
    }

    /// Records `duration` from `source` through PulseAudio (a source name,
    /// or `<sink>.monitor` for what a sink plays) and returns the peak
    /// level (0.0–1.0) of each of its `channels` channels. The first
    /// 100 ms are skipped (links settle). `None` if recording failed.
    pub fn peaks(&self, source: &str, channels: usize, duration: Duration) -> Option<Vec<f32>> {
        let mut child = self
            .command("parec")
            .args(["--raw", "--format=float32le", "--latency-msec=20"])
            .arg(format!("--rate={RATE}"))
            .arg(format!("--channels={channels}"))
            .arg(format!("--device={source}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut out = child.stdout.take()?;
        let frame = channels * 4;
        let skip = (RATE as usize / 10) * frame;
        let want = skip + (duration.as_secs_f64() * RATE as f64) as usize * frame;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut data = vec![0u8; want];
            let mut got = 0;
            while got < want {
                match out.read(&mut data[got..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got += n,
                }
            }
            data.truncate(got);
            let _ = tx.send(data);
        });
        // Generous: the stream must be linked before data flows.
        let data = rx.recv_timeout(duration + Duration::from_secs(10));
        let _ = child.kill();
        let _ = child.wait();
        let data = data.ok()?;
        if data.len() <= skip {
            return None;
        }
        let mut peaks = vec![0f32; channels];
        for (i, s) in data[skip..].as_chunks::<4>().0.iter().enumerate() {
            let v = f32::from_le_bytes(*s).abs();
            let c = i % channels;
            if v > peaks[c] {
                peaks[c] = v;
            }
        }
        Some(peaks)
    }

    /// `pactl` against this session, for checks; its stdout.
    pub fn pactl(&self, args: &[&str]) -> Option<String> {
        let out = self.command("pactl").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

fn env_for(dir: &Path) -> Vec<(String, String)> {
    let d = dir.display().to_string();
    vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), d.clone()),
        ("XDG_CONFIG_HOME".into(), format!("{d}/config")),
        ("XDG_RUNTIME_DIR".into(), d.clone()),
        ("PIPEWIRE_RUNTIME_DIR".into(), d.clone()),
        ("PULSE_SERVER".into(), format!("unix:{d}/pulse/native")),
        ("PULSE_RUNTIME_PATH".into(), format!("{d}/pulse")),
        // Nothing may reach the host's buses.
        (
            "DBUS_SYSTEM_BUS_ADDRESS".into(),
            "unix:path=/nonexistent/helix-fake-audio".into(),
        ),
        (
            "DBUS_SESSION_BUS_ADDRESS".into(),
            "unix:path=/nonexistent/helix-fake-audio".into(),
        ),
        ("LANG".into(), "C.UTF-8".into()),
    ]
}

/// A tone being played; stops when dropped.
pub struct Tone {
    child: Child,
    stop: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
}

impl Drop for Tone {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.child.kill();
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone)]
struct Node {
    id: u64,
    serial: Option<String>,
    name: String,
    class: String,
    target: Option<String>,
    capture_sink: bool,
}

#[derive(Debug, Clone)]
struct Port {
    id: u64,
    node: u64,
    input: bool,
    monitor: bool,
    channel: String,
}

/// The graph as `pw-dump` reports it.
#[derive(Debug, Default)]
struct Graph {
    nodes: Vec<Node>,
    ports: Vec<Port>,
    links: HashSet<(u64, u64)>,
    meta: HashMap<String, String>,
}

fn prop_str(props: &Value, key: &str) -> Option<String> {
    match props.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn parse(dump: &Value) -> Graph {
    let mut g = Graph::default();
    for o in dump.as_array().into_iter().flatten() {
        let ty = o.get("type").and_then(Value::as_str).unwrap_or("");
        let id = o.get("id").and_then(Value::as_u64).unwrap_or(0);
        let props = o.pointer("/info/props").cloned().unwrap_or(Value::Null);
        if ty.ends_with(":Node") {
            g.nodes.push(Node {
                id,
                serial: prop_str(&props, "object.serial"),
                name: prop_str(&props, "node.name").unwrap_or_default(),
                class: prop_str(&props, "media.class").unwrap_or_default(),
                target: prop_str(&props, "target.object")
                    .or_else(|| prop_str(&props, "node.target"))
                    .filter(|t| t != "-1"),
                capture_sink: prop_str(&props, "stream.capture.sink").as_deref() == Some("true"),
            });
        } else if ty.ends_with(":Port") {
            let dir = o
                .pointer("/info/direction")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| prop_str(&props, "port.direction"))
                .unwrap_or_default();
            g.ports.push(Port {
                id,
                node: prop_str(&props, "node.id")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0),
                input: dir == "input" || dir == "in",
                monitor: prop_str(&props, "port.monitor").as_deref() == Some("true"),
                channel: prop_str(&props, "audio.channel").unwrap_or_default(),
            });
        } else if ty.ends_with(":Link") {
            let out = o.pointer("/info/output-port-id").and_then(Value::as_u64);
            let inp = o.pointer("/info/input-port-id").and_then(Value::as_u64);
            if let (Some(a), Some(b)) = (out, inp) {
                g.links.insert((a, b));
            }
        } else if ty.ends_with(":Metadata") {
            for m in o
                .get("metadata")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let key = m.get("key").and_then(Value::as_str).unwrap_or("");
                let name = m.pointer("/value/name").and_then(Value::as_str);
                if let Some(name) = name {
                    g.meta.insert(key.to_owned(), name.to_owned());
                }
            }
        }
    }
    g
}

impl Graph {
    fn find(&self, target: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| {
            n.name == target || n.serial.as_deref() == Some(target) || n.id.to_string() == target
        })
    }

    fn ports(&self, node: u64, input: bool, monitor: Option<bool>) -> Vec<&Port> {
        self.ports
            .iter()
            .filter(|p| {
                p.node == node && p.input == input && monitor.is_none_or(|m| p.monitor == m)
            })
            .collect()
    }

    /// Channel-by-channel pairs from `outs` to `ins`: same position, else
    /// by order; one port on either side goes to every port on the other.
    fn pair(outs: &[&Port], ins: &[&Port]) -> Vec<(u64, u64)> {
        if outs.is_empty() || ins.is_empty() {
            return Vec::new();
        }
        // A single positioned channel (a mono stream mapped to FL, as a
        // speaker test is) goes to that position only.
        let positioned = |c: &str| !c.is_empty() && c != "MONO" && c != "UNK";
        if outs.len() == 1 && positioned(&outs[0].channel) {
            if let Some(i) = ins.iter().find(|i| i.channel == outs[0].channel) {
                return vec![(outs[0].id, i.id)];
            }
        }
        if outs.len() == 1 {
            return ins.iter().map(|i| (outs[0].id, i.id)).collect();
        }
        if ins.len() == 1 {
            return outs.iter().map(|o| (o.id, ins[0].id)).collect();
        }
        let named = outs.iter().all(|o| {
            ins.iter()
                .any(|i| i.channel == o.channel && !o.channel.is_empty())
        });
        if named {
            outs.iter()
                .filter_map(|o| {
                    ins.iter()
                        .find(|i| i.channel == o.channel)
                        .map(|i| (o.id, i.id))
                })
                .collect()
        } else {
            outs.iter()
                .zip(ins.iter())
                .map(|(o, i)| (o.id, i.id))
                .collect()
        }
    }

    /// The links a session manager would make that are missing.
    fn missing_links(&self) -> Vec<(u64, u64)> {
        let default_sink = self.meta.get("default.audio.sink").map(String::as_str);
        let default_source = self.meta.get("default.audio.source").map(String::as_str);
        let mut want = Vec::new();
        for n in &self.nodes {
            match n.class.as_str() {
                "Stream/Output/Audio" => {
                    let Some(target) = n.target.as_deref().or(default_sink) else {
                        continue;
                    };
                    let Some(t) = self.find(target) else { continue };
                    let outs = self.ports(n.id, false, None);
                    let ins = self.ports(t.id, true, None);
                    want.extend(Self::pair(&outs, &ins));
                }
                "Stream/Input/Audio" => {
                    let target = n.target.as_deref().or(if n.capture_sink {
                        default_sink
                    } else {
                        default_source
                    });
                    let Some(t) = target.and_then(|t| self.find(t)) else {
                        continue;
                    };
                    let outs = if t.class.starts_with("Audio/Sink") {
                        self.ports(t.id, false, Some(true))
                    } else {
                        self.ports(t.id, false, None)
                    };
                    let ins = self.ports(n.id, true, None);
                    want.extend(Self::pair(&outs, &ins));
                }
                _ => {}
            }
        }
        want.retain(|l| !self.links.contains(l));
        want
    }
}

fn policy(dir: &Path, stop: &AtomicBool) {
    let env = env_for(dir);
    let run = |program: &str, args: &[&str]| {
        Command::new(tool(program).unwrap_or_else(|| program.into()))
            .args(args)
            .env_clear()
            .envs(env.clone())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
    };
    while !stop.load(Ordering::SeqCst) {
        if let Some(out) = run("pw-dump", &[]) {
            if let Ok(dump) = serde_json::from_slice::<Value>(&out.stdout) {
                let g = parse(&dump);
                for (key, configured) in [
                    ("default.audio.sink", "default.configured.audio.sink"),
                    ("default.audio.source", "default.configured.audio.source"),
                ] {
                    if let Some(c) = g.meta.get(configured) {
                        if g.meta.get(key) != Some(c) {
                            let value = format!("{{\"name\":\"{c}\"}}");
                            run(
                                "pw-metadata",
                                &["-n", "default", "0", key, &value, "Spa:String:JSON"],
                            );
                        }
                    }
                }
                for (out_port, in_port) in g.missing_links() {
                    run("pw-link", &[&out_port.to_string(), &in_port.to_string()]);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud(p: f32) -> bool {
        p > 0.2
    }
    fn quiet(p: f32) -> bool {
        p < 0.01
    }

    /// A tone on one channel of the speakers reaches that channel only.
    #[test]
    fn tone_reaches_one_channel_of_a_null_sink() {
        let Some(audio) = FakeAudio::start() else {
            return;
        };
        for (on, expect) in [("FL", [true, false]), ("FR", [false, true])] {
            let _tone = audio
                .play(SPEAKERS, &["FL", "FR"], &[on], 440.0, 0.5)
                .unwrap();
            let peaks = audio
                .peaks(
                    &format!("{SPEAKERS}.monitor"),
                    2,
                    Duration::from_millis(300),
                )
                .unwrap();
            for (p, e) in peaks.iter().zip(expect) {
                assert!(if e { loud(*p) } else { quiet(*p) }, "{on}: {peaks:?}");
            }
        }
    }

    /// What is played into the microphone is what it records; silence
    /// otherwise.
    #[test]
    fn microphone_hears_a_test_source() {
        let Some(audio) = FakeAudio::start() else {
            return;
        };
        let silent = audio
            .peaks(MICROPHONE, 1, Duration::from_millis(200))
            .unwrap();
        assert!(quiet(silent[0]), "{silent:?}");
        let _tone = audio
            .play(MICROPHONE, &["MONO"], &["MONO"], 440.0, 0.5)
            .unwrap();
        let peaks = audio
            .peaks(MICROPHONE, 1, Duration::from_millis(300))
            .unwrap();
        assert!(loud(peaks[0]), "{peaks:?}");
    }

    /// The devices are what PulseAudio clients see, with the defaults set;
    /// a default chosen through PulseAudio becomes the default.
    #[test]
    fn devices_and_defaults() {
        let Some(audio) = FakeAudio::start() else {
            return;
        };
        let info = audio.pactl(&["info"]).unwrap();
        assert!(
            info.contains(&format!("Default Sink: {SPEAKERS}")),
            "{info}"
        );
        assert!(
            info.contains(&format!("Default Source: {MICROPHONE}")),
            "{info}"
        );
        audio.pactl(&["set-default-sink", SURROUND]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !audio
            .pactl(&["get-default-sink"])
            .is_some_and(|s| s.trim() == SURROUND)
        {
            assert!(Instant::now() < deadline, "default sink not changed");
            std::thread::sleep(Duration::from_millis(50));
        }
        // Each channel of the 5.1 sink on its own.
        let all = ["FL", "FR", "FC", "LFE", "RL", "RR"];
        for (i, ch) in all.iter().enumerate() {
            let _tone = audio.play(SURROUND, &all, &[ch], 440.0, 0.5).unwrap();
            let peaks = audio
                .peaks(
                    &format!("{SURROUND}.monitor"),
                    6,
                    Duration::from_millis(300),
                )
                .unwrap();
            for (j, p) in peaks.iter().enumerate() {
                assert!(if i == j { loud(*p) } else { quiet(*p) }, "{ch}: {peaks:?}");
            }
        }
    }

    #[test]
    fn pairs_by_channel_then_order() {
        let p = |id, ch: &str| Port {
            id,
            node: 0,
            input: false,
            monitor: false,
            channel: ch.into(),
        };
        let (a, b, c, d) = (p(1, "FL"), p(2, "FR"), p(3, "FR"), p(4, "FL"));
        assert_eq!(Graph::pair(&[&a, &b], &[&c, &d]), vec![(1, 4), (2, 3)]);
        let (x, y) = (p(5, "UNK"), p(6, "UNK"));
        assert_eq!(Graph::pair(&[&x, &y], &[&c, &d]), vec![(5, 3), (6, 4)]);
        let m = p(7, "MONO");
        assert_eq!(Graph::pair(&[&m], &[&c, &d]), vec![(7, 3), (7, 4)]);
        let fl = p(8, "FL");
        assert_eq!(Graph::pair(&[&fl], &[&c, &d]), vec![(8, 4)], "FL only");
    }
}
