//! Camera worker thread: owns device discovery and the GStreamer pipeline,
//! receives commands from the UI and reports events back.

use crate::devices::{self, Location, Provider, SourceMode};
use crate::encoders::{self, VideoFormat};
use crate::naming::{self, CaptureKind};
use crate::pipeline::{self, AudioInput, Camera, Frame, RecordSpec, Session, SourceSpec};
use crate::portal;
use gst::prelude::*;
use helix_app_common::i18n::tr;
use helix_app_common::trf;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;

pub const THUMB_SIZE: i32 = 96;

#[derive(Debug)]
pub enum Command {
    NextCamera,
    TakePhoto,
    StartRecording(VideoFormat),
    StopRecording,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Starting,
    Streaming,
    NoCamera,
    Denied,
    Error(String),
}

#[derive(Debug)]
pub enum Event {
    Cameras {
        names: Vec<String>,
        active: Option<usize>,
        /// Whether the preview of the active camera is mirrored.
        #[allow(dead_code)]
        mirrored: bool,
    },
    Status(Status),
    PhotoSaved {
        path: PathBuf,
        thumb: Option<Frame>,
    },
    RecordingStarted(PathBuf),
    RecordingSaved {
        path: PathBuf,
        thumb: Option<Frame>,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mode: SourceMode,
    pub output_dir: PathBuf,
    pub remembered_camera: Option<String>,
}

pub struct Handle {
    tx: mpsc::Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    pub frames: async_channel::Receiver<Frame>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn spawn(config: Config) -> Handle {
    let (tx, rx) = mpsc::channel();
    let (etx, erx) = async_channel::unbounded();
    let (ftx, frx) = async_channel::bounded(2);
    let thread = std::thread::Builder::new()
        .name("camera".into())
        .spawn(move || Worker::new(config, etx, ftx).run(rx))
        .expect("spawn camera thread");
    Handle {
        tx,
        events: erx,
        frames: frx,
        thread: Some(thread),
    }
}

/// Where camera devices come from.
enum Watcher {
    Fixed(Vec<Camera>),
    Monitor(gst::DeviceMonitor),
    Provider {
        provider: gst::DeviceProvider,
        fd: OwnedFd,
    },
}

impl Watcher {
    fn cameras(&self) -> Vec<Camera> {
        match self {
            Watcher::Fixed(c) => c.clone(),
            Watcher::Monitor(m) => {
                let devs = m
                    .devices()
                    .into_iter()
                    .map(|d| (Provider::classify(d.type_().name()), d))
                    .collect();
                to_cameras(devices::filter_by_provider(devs), None)
            }
            Watcher::Provider { provider, fd } => {
                let devs = provider
                    .devices()
                    .into_iter()
                    .filter(|d| d.has_classes("Video/Source"))
                    .collect();
                to_cameras(devs, Some(fd.as_raw_fd()))
            }
        }
    }

    fn bus(&self) -> Option<gst::Bus> {
        match self {
            Watcher::Fixed(_) => None,
            Watcher::Monitor(m) => Some(m.bus()),
            Watcher::Provider { provider, .. } => Some(provider.bus()),
        }
    }

    fn stop(&self) {
        match self {
            Watcher::Fixed(_) => {}
            Watcher::Monitor(m) => m.stop(),
            Watcher::Provider { provider, .. } => provider.stop(),
        }
    }
}

fn to_cameras(devs: Vec<gst::Device>, fd: Option<i32>) -> Vec<Camera> {
    let mut out: Vec<Camera> = Vec::new();
    for device in devs {
        let props = device.properties();
        let loc = props.as_ref().and_then(|p| {
            [
                "api.libcamera.location",
                "camera.location",
                "device.location",
            ]
            .iter()
            .find_map(|k| p.get::<String>(*k).ok())
        });
        let mut name = device.display_name().to_string();
        let base = name.clone();
        let mut n = 2;
        while out.iter().any(|c| c.name == name) {
            name = format!("{base} ({n})");
            n += 1;
        }
        out.push(Camera {
            name,
            location: Location::parse(loc.as_deref()),
            spec: SourceSpec::Device { device, fd },
        });
    }
    out
}

/// The two virtual cameras used with `HELIX_CAMERA_SOURCE=test`.
pub fn test_cameras() -> Vec<Camera> {
    vec![
        Camera {
            name: tr("Test Camera (Front)").into(),
            location: Location::Front,
            spec: SourceSpec::Test {
                pattern: "ball",
                width: 640,
                height: 480,
            },
        },
        Camera {
            name: tr("Test Camera (Back)").into(),
            location: Location::Back,
            spec: SourceSpec::Test {
                pattern: "smpte",
                width: 640,
                height: 480,
            },
        },
    ]
}

fn monitor_watcher() -> Option<Watcher> {
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    monitor.start().ok()?;
    Some(Watcher::Monitor(monitor))
}

fn portal_watcher(fd: OwnedFd) -> Option<Watcher> {
    let provider = gst::DeviceProviderFactory::by_name("pipewiredeviceprovider")?;
    provider.find_property("fd")?;
    provider.set_property("fd", fd.as_raw_fd());
    provider.start().ok()?;
    Some(Watcher::Provider { provider, fd })
}

struct Worker {
    config: Config,
    events: async_channel::Sender<Event>,
    frames: async_channel::Sender<Frame>,
    watcher: Option<Watcher>,
    cameras: Vec<Camera>,
    active: Option<usize>,
    session: Option<Session>,
    record_thumb: Option<Frame>,
}

impl Worker {
    fn new(
        config: Config,
        events: async_channel::Sender<Event>,
        frames: async_channel::Sender<Frame>,
    ) -> Self {
        Worker {
            config,
            events,
            frames,
            watcher: None,
            cameras: Vec::new(),
            active: None,
            session: None,
            record_thumb: None,
        }
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send_blocking(e);
    }

    fn run(mut self, rx: mpsc::Receiver<Command>) {
        if !self.discover() {
            // Permission denied: idle until shut down.
            while !matches!(rx.recv(), Ok(Command::Shutdown) | Err(_)) {}
            return;
        }
        let remembered = self.config.remembered_camera.clone();
        let names = self.names();
        match devices::pick_index(&names, remembered.as_deref()) {
            Some(i) => self.open(i),
            None => {
                self.emit_cameras();
                self.emit(Event::Status(Status::NoCamera));
            }
        }
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.poll_hotplug();
            self.poll_errors();
        }
        if let Some(s) = self.session.take() {
            if s.is_recording() {
                let _ = s.finish(Duration::from_secs(5));
            }
        }
        if let Some(w) = &self.watcher {
            w.stop();
        }
    }

    /// Sets up the device source. Returns false when access was denied.
    fn discover(&mut self) -> bool {
        let watcher = match self.config.mode {
            SourceMode::Test => Some(Watcher::Fixed(test_cameras())),
            SourceMode::None => Some(Watcher::Fixed(Vec::new())),
            SourceMode::Denied => None,
            SourceMode::Real if portal::should_use_portal() => match portal::access_camera() {
                Ok(portal::Access::Granted(fd)) => portal_watcher(fd).or_else(monitor_watcher),
                Ok(portal::Access::NoCamera) => Some(Watcher::Fixed(Vec::new())),
                Ok(portal::Access::Denied) => None,
                Err(_) => monitor_watcher(),
            },
            SourceMode::Real => monitor_watcher(),
        };
        let Some(watcher) = watcher else {
            self.emit(Event::Status(Status::Denied));
            return false;
        };
        self.cameras = watcher.cameras();
        self.watcher = Some(watcher);
        true
    }

    fn names(&self) -> Vec<String> {
        self.cameras.iter().map(|c| c.name.clone()).collect()
    }

    fn emit_cameras(&self) {
        let mirrored = self
            .active
            .and_then(|i| self.cameras.get(i))
            .is_some_and(|c| devices::should_mirror(c.location));
        self.emit(Event::Cameras {
            names: self.names(),
            active: self.active,
            mirrored,
        });
    }

    fn frame_sink(&self) -> pipeline::FrameSink {
        let tx = self.frames.clone();
        Arc::new(move |f| {
            let _ = tx.try_send(f);
        })
    }

    fn open(&mut self, idx: usize) {
        self.session = None;
        self.active = Some(idx);
        self.emit_cameras();
        self.emit(Event::Status(Status::Starting));
        let cam = &self.cameras[idx];
        match Session::start(
            cam,
            devices::should_mirror(cam.location),
            self.frame_sink(),
            None,
        ) {
            Ok(s) => {
                self.session = Some(s);
                self.emit(Event::Status(Status::Streaming));
            }
            Err(e) => self.emit(Event::Status(Status::Error(e))),
        }
    }

    fn audio_input(&self) -> AudioInput {
        match self.config.mode {
            SourceMode::Real => AudioInput::Microphone,
            _ => AudioInput::Test,
        }
    }

    fn handle(&mut self, cmd: Command) {
        let recording = self.session.as_ref().is_some_and(|s| s.is_recording());
        match cmd {
            Command::NextCamera if !self.cameras.is_empty() && !recording => {
                let next = self.active.map_or(0, |a| (a + 1) % self.cameras.len());
                self.open(next);
            }
            Command::TakePhoto => self.take_photo(),
            Command::StartRecording(fmt) if !recording => self.start_recording(fmt),
            Command::StopRecording if recording => self.stop_recording(),
            _ => {}
        }
    }

    fn take_photo(&mut self) {
        let Some(sample) = self.session.as_ref().and_then(|s| s.last_sample()) else {
            self.emit(Event::Failed(tr("No camera image available").into()));
            return;
        };
        let path = {
            let _ = std::fs::create_dir_all(&self.config.output_dir);
            naming::new_capture_path(&self.config.output_dir, CaptureKind::Photo, "jpg")
        };
        match pipeline::save_jpeg(&sample, &path) {
            Ok(()) => {
                let thumb = pipeline::thumbnail(&sample, THUMB_SIZE);
                self.emit(Event::PhotoSaved { path, thumb });
            }
            Err(e) => self.emit(Event::Failed(e)),
        }
    }

    fn start_recording(&mut self, fmt: VideoFormat) {
        let Some(idx) = self.active else { return };
        let Some(plan) = encoders::choose(fmt, &encoders::gst_has) else {
            self.emit(Event::Failed(tr("No video encoder is available").into()));
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&self.config.output_dir) {
            self.emit(Event::Failed(format!(
                "{}: {e}",
                self.config.output_dir.display()
            )));
            return;
        }
        let path = naming::new_capture_path(
            &self.config.output_dir,
            CaptureKind::Video,
            plan.format.extension(),
        );
        self.record_thumb = self
            .session
            .as_ref()
            .and_then(|s| s.last_sample())
            .and_then(|s| pipeline::thumbnail(&s, THUMB_SIZE));
        self.session = None;
        let spec = RecordSpec {
            path: path.clone(),
            plan,
            audio: self.audio_input(),
        };
        let cam = &self.cameras[idx];
        match Session::start(
            cam,
            devices::should_mirror(cam.location),
            self.frame_sink(),
            Some(&spec),
        ) {
            Ok(s) => {
                self.session = Some(s);
                self.emit(Event::RecordingStarted(path));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                self.emit(Event::Failed(trf!("Could not start recording: {e}", e = e)));
                self.open(idx);
            }
        }
    }

    fn stop_recording(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        match session.finish(Duration::from_secs(5)) {
            Ok(Some(path)) => {
                let thumb = self.record_thumb.take();
                self.emit(Event::RecordingSaved { path, thumb });
            }
            Ok(None) => {}
            Err(e) => self.emit(Event::Failed(trf!("Recording failed: {e}", e = e))),
        }
        if let Some(i) = self.active {
            self.open(i);
        }
    }

    fn poll_hotplug(&mut self) {
        let Some(bus) = self.watcher.as_ref().and_then(|w| w.bus()) else {
            return;
        };
        let mut changed = false;
        while let Some(msg) = bus.pop() {
            if matches!(
                msg.view(),
                gst::MessageView::DeviceAdded(_) | gst::MessageView::DeviceRemoved(_)
            ) {
                changed = true;
            }
        }
        if !changed {
            return;
        }
        let current = self
            .active
            .and_then(|i| self.cameras.get(i))
            .map(|c| c.name.clone());
        let recording = self.session.as_ref().is_some_and(|s| s.is_recording());
        self.cameras = self
            .watcher
            .as_ref()
            .map(|w| w.cameras())
            .unwrap_or_default();
        let still_there = current
            .as_ref()
            .and_then(|n| self.cameras.iter().position(|c| &c.name == n));
        match still_there {
            Some(i) => {
                self.active = Some(i);
                self.emit_cameras();
            }
            None if recording => {
                // Active camera vanished mid-recording: finalize what we have.
                self.stop_recording_without_reopen();
                self.reopen_first();
            }
            None => self.reopen_first(),
        }
    }

    fn stop_recording_without_reopen(&mut self) {
        if let Some(s) = self.session.take() {
            match s.finish(Duration::from_secs(2)) {
                Ok(Some(path)) => {
                    let thumb = self.record_thumb.take();
                    self.emit(Event::RecordingSaved { path, thumb });
                }
                _ => self.emit(Event::Failed(tr("Recording interrupted").into())),
            }
        }
    }

    fn reopen_first(&mut self) {
        let remembered = self.config.remembered_camera.clone();
        match devices::pick_index(&self.names(), remembered.as_deref()) {
            Some(i) => self.open(i),
            None => {
                self.session = None;
                self.active = None;
                self.emit_cameras();
                self.emit(Event::Status(Status::NoCamera));
            }
        }
    }

    fn poll_errors(&mut self) {
        let Some(err) = self.session.as_ref().and_then(|s| s.pop_error()) else {
            return;
        };
        let was_recording = self.session.as_ref().is_some_and(|s| s.is_recording());
        self.session = None;
        if was_recording {
            self.emit(Event::Failed(trf!("Recording failed: {err}", err = err)));
        }
        self.emit(Event::Status(Status::Error(err)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next_event(h: &Handle, pred: impl Fn(&Event) -> bool) -> Event {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for event"
            );
            match h.events.try_recv() {
                Ok(e) if pred(&e) => return e,
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    #[test]
    fn worker_end_to_end_with_test_source() {
        gst::init().unwrap();
        let dir = std::env::temp_dir().join(format!("helix-camera-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let h = spawn(Config {
            mode: SourceMode::Test,
            output_dir: dir.clone(),
            remembered_camera: Some("Test Camera (Back)".into()),
        });
        match next_event(&h, |e| matches!(e, Event::Cameras { .. })) {
            Event::Cameras {
                names,
                active,
                mirrored,
            } => {
                assert_eq!(names.len(), 2);
                assert_eq!(active, Some(1));
                assert!(!mirrored);
            }
            _ => unreachable!(),
        }
        next_event(&h, |e| matches!(e, Event::Status(Status::Streaming)));
        let frame = h.frames.recv_blocking().unwrap();
        assert_eq!((frame.width, frame.height), (640, 480));
        std::thread::sleep(Duration::from_millis(200));

        h.send(Command::TakePhoto);
        match next_event(&h, |e| {
            matches!(e, Event::PhotoSaved { .. } | Event::Failed(_))
        }) {
            Event::PhotoSaved { path, thumb } => {
                assert!(path.starts_with(&dir));
                assert!(
                    naming::is_capture_name(&path.file_name().unwrap().to_string_lossy()).is_some()
                );
                let bytes = std::fs::read(&path).unwrap();
                assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
                assert_eq!(thumb.unwrap().width, THUMB_SIZE);
            }
            e => panic!("{e:?}"),
        }

        h.send(Command::StartRecording(VideoFormat::WebmVp8));
        next_event(&h, |e| matches!(e, Event::RecordingStarted(_)));
        std::thread::sleep(Duration::from_millis(1000));
        h.send(Command::StopRecording);
        match next_event(&h, |e| {
            matches!(e, Event::RecordingSaved { .. } | Event::Failed(_))
        }) {
            Event::RecordingSaved { path, .. } => {
                assert!(std::fs::metadata(&path).unwrap().len() > 1000);
            }
            e => panic!("{e:?}"),
        }
        next_event(&h, |e| matches!(e, Event::Status(Status::Streaming)));

        h.send(Command::NextCamera);
        match next_event(&h, |e| matches!(e, Event::Cameras { .. })) {
            Event::Cameras {
                active, mirrored, ..
            } => {
                assert_eq!(active, Some(0));
                assert!(mirrored);
            }
            _ => unreachable!(),
        }
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_camera_and_denied_states() {
        gst::init().unwrap();
        let dir = std::env::temp_dir().join("helix-camera-none");
        let h = spawn(Config {
            mode: SourceMode::None,
            output_dir: dir.clone(),
            remembered_camera: None,
        });
        next_event(&h, |e| matches!(e, Event::Status(Status::NoCamera)));
        drop(h);
        let h = spawn(Config {
            mode: SourceMode::Denied,
            output_dir: dir,
            remembered_camera: None,
        });
        next_event(&h, |e| matches!(e, Event::Status(Status::Denied)));
    }
}
