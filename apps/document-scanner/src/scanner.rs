//! The SANE worker thread: owns the SANE library, enumerates devices and
//! runs scans, reporting back to the UI through an async channel.

use crate::options::{self, ColorMode, SourceKind};
use crate::page::{FrameFormat, FrameParams, Page, PageBuilder};
use crate::sane::{self, Constraint, Device, DeviceHandle, OptionDesc, Sane};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}

macro_rules! debug {
    ($($t:tt)*) => {
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!("[scanner] {}", format!($($t)*));
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanKind {
    /// One page.
    Single,
    /// All pages in the document feeder.
    Feeder,
    /// Repeated flatbed scans with a delay until stopped.
    Batch,
}

/// `page-side` GSettings enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSide {
    Front,
    Back,
    Both,
}

impl PageSide {
    pub fn from_nick(s: &str) -> PageSide {
        match s {
            "front" => PageSide::Front,
            "back" => PageSide::Back,
            _ => PageSide::Both,
        }
    }

    pub fn nick(self) -> &'static str {
        match self {
            PageSide::Front => "front",
            PageSide::Back => "back",
            PageSide::Both => "both",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScanRequest {
    pub device: String,
    /// Explicit `source` value; `None` = automatic.
    pub source: Option<String>,
    pub kind: ScanKind,
    pub color: ColorMode,
    pub dpi: i32,
    /// Paper size in millimetres; `None` = whole scan area.
    pub paper: Option<(f64, f64)>,
    pub brightness: i32,
    pub contrast: i32,
    pub page_delay_ms: u32,
    pub page_side: PageSide,
    /// Text document (stored on the resulting pages).
    pub text: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capabilities {
    pub sources: Vec<String>,
    pub modes: Vec<String>,
    pub resolutions: Vec<i32>,
}

pub enum Cmd {
    Refresh,
    Open(String),
    Scan(ScanRequest),
    Quit,
}

#[derive(Debug)]
pub enum Event {
    /// SANE could not be initialised at all.
    Unavailable(String),
    Devices(Vec<Device>),
    Opened { device: String, caps: Capabilities },
    OpenFailed { device: String, error: String },
    ScanStarted,
    /// A new page is being scanned (0-based count within this scan).
    PageStarted(usize),
    /// Progress of the current page, 0…1.
    Progress(f64),
    Page(Page),
    ScanFinished { cancelled: bool, error: Option<String> },
}

pub struct Scanner {
    tx: mpsc::Sender<Cmd>,
    cancel: Arc<AtomicBool>,
}

impl Scanner {
    /// Starts the worker; events arrive on the returned receiver.
    pub fn spawn() -> (Scanner, async_channel::Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = async_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::Builder::new()
            .name("sane-worker".into())
            .spawn(move || worker(rx, etx, c))
            .expect("spawn SANE worker");
        (Scanner { tx, cancel }, erx)
    }

    pub fn send(&self, cmd: Cmd) {
        if matches!(cmd, Cmd::Scan(_)) {
            self.cancel.store(false, Ordering::SeqCst);
        }
        let _ = self.tx.send(cmd);
    }

    /// Requests that the running scan stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Quit);
    }
}

/// Lists devices synchronously (for `--list-devices`).
pub fn list_devices() -> Result<Vec<Device>, String> {
    let sane = Sane::init().map_err(|e| e.to_string())?;
    sane.devices(false).map_err(|e| e.to_string())
}

fn worker(rx: mpsc::Receiver<Cmd>, tx: async_channel::Sender<Event>, cancel: Arc<AtomicBool>) {
    let send = |e: Event| {
        let _ = tx.send_blocking(e);
    };
    let sane = match Sane::init() {
        Ok(s) => s,
        Err(e) => {
            send(Event::Unavailable(e.to_string()));
            return;
        }
    };
    debug!("SANE initialised");
    let mut current: Option<(String, DeviceHandle<'_>)> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Quit => break,
            Cmd::Refresh => match sane.devices(false) {
                Ok(devs) => {
                    debug!("found {} device(s)", devs.len());
                    send(Event::Devices(devs));
                }
                Err(e) => {
                    debug!("get_devices failed: {e}");
                    send(Event::Devices(Vec::new()));
                }
            },
            Cmd::Open(name) => match ensure_open(&sane, &mut current, &name) {
                Ok(h) => send(Event::Opened { device: name, caps: capabilities(h) }),
                Err(e) => send(Event::OpenFailed { device: name, error: e }),
            },
            Cmd::Scan(req) => {
                send(Event::ScanStarted);
                let result = match ensure_open(&sane, &mut current, &req.device) {
                    Ok(h) => scan(h, &req, &cancel, &send),
                    Err(e) => Err(e),
                };
                match result {
                    Ok(cancelled) => send(Event::ScanFinished { cancelled, error: None }),
                    Err(e) => {
                        debug!("scan failed: {e}");
                        send(Event::ScanFinished { cancelled: false, error: Some(e) })
                    }
                }
            }
        }
    }
    drop(current);
    debug!("SANE worker exiting");
}

fn ensure_open<'a, 'b>(
    sane: &'a Sane,
    current: &'b mut Option<(String, DeviceHandle<'a>)>,
    name: &str,
) -> Result<&'b DeviceHandle<'a>, String> {
    if current.as_ref().map(|(n, _)| n.as_str()) != Some(name) {
        *current = None;
        debug!("opening {name}");
        let h = sane.open(name).map_err(|e| format!("Unable to connect to scanner: {e}"))?;
        *current = Some((name.to_string(), h));
    }
    Ok(&current.as_ref().unwrap().1)
}

fn find<'o>(opts: &'o [OptionDesc], name: &str) -> Option<&'o OptionDesc> {
    opts.iter().find(|o| o.name == name)
}

fn strings(o: Option<&OptionDesc>) -> Vec<String> {
    match o.map(|o| &o.constraint) {
        Some(Constraint::Strings(v)) => v.clone(),
        _ => Vec::new(),
    }
}

pub fn capabilities(h: &DeviceHandle) -> Capabilities {
    let opts = h.options();
    let resolutions = find(&opts, "resolution")
        .map(|o| options::resolution_choices(&o.constraint, o.is_fixed()))
        .unwrap_or_default();
    Capabilities {
        sources: strings(find(&opts, "source")),
        modes: strings(find(&opts, "mode")),
        resolutions,
    }
}

/// Sets a string option if present and settable; returns refreshed options.
fn set_string(h: &DeviceHandle, opts: Vec<OptionDesc>, name: &str, value: &str) -> Vec<OptionDesc> {
    let Some(o) = find(&opts, name).filter(|o| o.settable()) else { return opts };
    match h.set_string(o, value) {
        Ok(_) => debug!("{name} = {value}"),
        Err(e) => debug!("setting {name} = {value} failed: {e}"),
    }
    h.options()
}

/// Sets a numeric option given as a real value (converted for FIXED).
fn set_number(h: &DeviceHandle, opts: Vec<OptionDesc>, name: &str, value: f64) -> Vec<OptionDesc> {
    let Some(o) = find(&opts, name).filter(|o| o.settable()) else { return opts };
    let mut w = if o.is_fixed() { sane::to_fixed(value) } else { value.round() as sane::Word };
    if let Constraint::Range { min, max, quant } = o.constraint {
        w = options::clamp_to_range(w, min, max, quant);
    }
    match h.set_word(o, w) {
        Ok(_) => debug!("{name} = {value}"),
        Err(e) => debug!("setting {name} = {value} failed: {e}"),
    }
    h.options()
}

fn word_value(h: &DeviceHandle, opts: &[OptionDesc], name: &str) -> Option<f64> {
    let o = find(opts, name)?;
    let w = h.get_word(o).ok()?;
    Some(if o.is_fixed() { sane::from_fixed(w) } else { w as f64 })
}

fn range_real(o: &OptionDesc) -> Option<(f64, f64)> {
    match o.constraint {
        Constraint::Range { min, max, .. } if o.is_fixed() => Some((sane::from_fixed(min), sane::from_fixed(max))),
        Constraint::Range { min, max, .. } => Some((min as f64, max as f64)),
        _ => None,
    }
}

/// Applies the request's options to the device. Returns the effective DPI.
fn configure(h: &DeviceHandle, req: &ScanRequest) -> f64 {
    let mut opts = h.options();

    // Source.
    let sources = strings(find(&opts, "source"));
    let source = req.source.clone().filter(|s| sources.contains(s)).or_else(|| {
        let kind = match (req.kind, req.page_side) {
            (ScanKind::Feeder, PageSide::Both) => SourceKind::AdfDuplex,
            (ScanKind::Feeder, PageSide::Back) => SourceKind::AdfBack,
            (ScanKind::Feeder, PageSide::Front) => SourceKind::Adf,
            _ => SourceKind::Flatbed,
        };
        options::pick_source(&sources, kind)
    });
    if let Some(s) = source {
        opts = set_string(h, opts, "source", &s);
    }

    // Colour mode.
    if let Some(m) = options::pick_mode(&strings(find(&opts, "mode")), req.color) {
        opts = set_string(h, opts, "mode", &m);
    }

    // Resolution.
    if let Some(o) = find(&opts, "resolution") {
        let choices = options::resolution_choices(&o.constraint, o.is_fixed());
        let dpi = options::nearest_resolution(&choices, req.dpi);
        opts = set_number(h, opts, "resolution", dpi as f64);
    }

    // Scan area (only when expressed in millimetres).
    let mm = |o: &OptionDesc| o.unit == 3;
    let area: Vec<(String, f64, f64)> = ["tl-x", "tl-y", "br-x", "br-y"]
        .iter()
        .filter_map(|n| find(&opts, n).filter(|o| mm(o)).and_then(|o| range_real(o).map(|(a, b)| (n.to_string(), a, b))))
        .collect();
    if area.len() == 4 {
        let (x0, y0) = (area[0].1, area[1].1);
        opts = set_number(h, opts, "tl-x", x0);
        opts = set_number(h, opts, "tl-y", y0);
        let (bx, by) = match req.paper {
            Some((w, hh)) => ((x0 + w).min(area[2].2), (y0 + hh).min(area[3].2)),
            None => (area[2].2, area[3].2),
        };
        opts = set_number(h, opts, "br-x", bx);
        opts = set_number(h, opts, "br-y", by);
    }

    // Brightness / contrast.
    for (name, value) in [("brightness", req.brightness), ("contrast", req.contrast)] {
        if value == 0 {
            continue;
        }
        if let Some((min, max)) = find(&opts, name).and_then(range_real) {
            let v = options::scale_adjustment(value, min.round() as i32, max.round() as i32);
            opts = set_number(h, opts, name, v as f64);
        }
    }

    word_value(h, &opts, "resolution").filter(|d| *d > 0.0).unwrap_or(req.dpi as f64)
}

fn frame_format(f: i32) -> Option<FrameFormat> {
    Some(match f {
        0 => FrameFormat::Gray,
        1 => FrameFormat::Rgb,
        2 => FrameFormat::Red,
        3 => FrameFormat::Green,
        4 => FrameFormat::Blue,
        _ => return None,
    })
}

enum PageResult {
    Page(Page),
    NoDocs,
    Cancelled,
}

/// Runs a scan. Returns `Ok(true)` if it was cancelled.
fn scan(h: &DeviceHandle, req: &ScanRequest, cancel: &AtomicBool, send: &dyn Fn(Event)) -> Result<bool, String> {
    let dpi = configure(h, req);
    let mut count = 0usize;
    let result = loop {
        if cancel.load(Ordering::SeqCst) {
            break Ok(true);
        }
        send(Event::PageStarted(count));
        match scan_page(h, dpi, cancel, send) {
            Ok(PageResult::Page(mut page)) => {
                page.text = req.text;
                send(Event::Page(page));
                count += 1;
            }
            Ok(PageResult::Cancelled) => break Ok(true),
            Ok(PageResult::NoDocs) => {
                break if count == 0 { Err("Document feeder empty".to_string()) } else { Ok(false) };
            }
            Err(e) => break Err(e),
        }
        match req.kind {
            ScanKind::Single => break Ok(false),
            ScanKind::Feeder => {}
            ScanKind::Batch => {
                // Wait between pages, staying responsive to Stop.
                let until = Instant::now() + Duration::from_millis(req.page_delay_ms as u64);
                while Instant::now() < until && !cancel.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    };
    h.cancel();
    result
}

fn scan_page(h: &DeviceHandle, dpi: f64, cancel: &AtomicBool, send: &dyn Fn(Event)) -> Result<PageResult, String> {
    let mut builder = PageBuilder::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut first = true;
    loop {
        match h.start() {
            Ok(()) => {}
            Err(e) if e.0 == sane::STATUS_NO_DOCS && first => return Ok(PageResult::NoDocs),
            Err(e) if e.0 == sane::STATUS_CANCELLED => return Ok(PageResult::Cancelled),
            Err(e) => return Err(format!("Unable to start scan: {e}")),
        }
        first = false;
        let p = h.parameters().map_err(|e| format!("Error communicating with scanner: {e}"))?;
        debug!("frame {:?}", p);
        let format = frame_format(p.format).ok_or_else(|| format!("Unsupported frame format {}", p.format))?;
        let expected = if p.lines > 0 { (p.bytes_per_line as usize) * p.lines as usize } else { 0 };
        let mut data = Vec::with_capacity(expected);
        let mut last_progress = -1.0;
        loop {
            if cancel.load(Ordering::SeqCst) {
                h.cancel();
                return Ok(PageResult::Cancelled);
            }
            match h.read(&mut buf) {
                Ok(n) => data.extend_from_slice(&buf[..n]),
                Err(e) if e.0 == sane::STATUS_EOF => break,
                Err(e) if e.0 == sane::STATUS_CANCELLED => return Ok(PageResult::Cancelled),
                Err(e) => return Err(format!("Error communicating with scanner: {e}")),
            }
            if expected > 0 {
                let progress = (data.len() as f64 / expected as f64).min(1.0);
                if progress - last_progress >= 0.01 {
                    last_progress = progress;
                    send(Event::Progress(progress));
                }
            }
        }
        let params = FrameParams {
            format,
            bytes_per_line: p.bytes_per_line.max(0) as usize,
            pixels_per_line: p.pixels_per_line.max(0) as usize,
            depth: p.depth.max(0) as u32,
        };
        builder.add_frame(&params, &data)?;
        if p.last_frame {
            break;
        }
    }
    builder.finish(dpi).map(PageResult::Page)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BACKEND: &str = "/usr/lib/x86_64-linux-gnu/sane/libsane-test.so.1";

    fn wait(rx: &async_channel::Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            let e = rx.recv_blocking().expect("worker alive");
            let done = pred(&e);
            seen.push(e);
            if done {
                return seen;
            }
        }
    }

    fn pages(events: &[Event]) -> Vec<&Page> {
        events.iter().filter_map(|e| if let Event::Page(p) = e { Some(p) } else { None }).collect()
    }

    fn finished(e: &Event) -> bool {
        matches!(e, Event::ScanFinished { .. })
    }

    /// End-to-end scan against SANE's virtual `test` backend only. SANE is
    /// restricted to that backend through a private SANE_CONFIG_DIR, and
    /// every device is checked to be `test:*` before scanning.
    /// This is the only test in the crate that initialises SANE.
    #[test]
    fn scans_with_test_backend() {
        if !std::path::Path::new(TEST_BACKEND).exists() {
            eprintln!("SANE test backend not installed; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dll.conf"), "test\n").unwrap();
        std::env::set_var("SANE_CONFIG_DIR", dir.path());

        let (scanner, rx) = Scanner::spawn();
        scanner.send(Cmd::Refresh);
        let ev = wait(&rx, |e| matches!(e, Event::Devices(_) | Event::Unavailable(_)));
        let Some(Event::Devices(devs)) = ev.last() else { panic!("SANE unavailable") };
        assert!(!devs.is_empty());
        assert!(devs.iter().all(|d| d.name.starts_with("test:")), "non-test device visible: {devs:?}");

        scanner.send(Cmd::Open("test:0".into()));
        let ev = wait(&rx, |e| matches!(e, Event::Opened { .. } | Event::OpenFailed { .. }));
        let Some(Event::Opened { caps, .. }) = ev.last() else { panic!("open failed: {:?}", ev.last()) };
        assert!(caps.modes.iter().any(|m| m == "Color"));
        assert!(caps.sources.iter().any(|s| s == "Flatbed"));
        assert!(caps.resolutions.contains(&75));

        let req = ScanRequest {
            device: "test:0".into(),
            source: None,
            kind: ScanKind::Single,
            color: ColorMode::Color,
            dpi: 75,
            paper: Some((50.0, 40.0)),
            brightness: 0,
            contrast: 0,
            page_delay_ms: 0,
            page_side: PageSide::Both,
            text: false,
        };
        scanner.send(Cmd::Scan(req.clone()));
        let ev = wait(&rx, finished);
        assert!(matches!(ev.last(), Some(Event::ScanFinished { cancelled: false, error: None })), "{:?}", ev.last());
        let p = pages(&ev);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].channels, 3);
        assert_eq!(p[0].dpi, 75.0);
        // 50 × 40 mm at 75 dpi ≈ 148 × 118 px.
        assert!((p[0].width as i32 - 148).abs() <= 2, "width {}", p[0].width);
        assert!((p[0].height as i32 - 118).abs() <= 2, "height {}", p[0].height);

        // Grey text scan.
        scanner.send(Cmd::Scan(ScanRequest { color: ColorMode::Gray, text: true, ..req.clone() }));
        let ev = wait(&rx, finished);
        let p = pages(&ev);
        assert_eq!((p.len(), p[0].channels, p[0].text), (1, 1, true));

        // Batch scanning stops when cancelled.
        scanner.send(Cmd::Scan(ScanRequest { kind: ScanKind::Batch, page_delay_ms: 10_000, ..req.clone() }));
        let ev = wait(&rx, |e| matches!(e, Event::Page(_)));
        assert_eq!(pages(&ev).len(), 1);
        scanner.cancel();
        let ev = wait(&rx, finished);
        assert!(matches!(ev.last(), Some(Event::ScanFinished { cancelled: true, .. })));

        // Feeder scan delivers pages until the feeder is empty.
        scanner.send(Cmd::Scan(ScanRequest { kind: ScanKind::Feeder, ..req }));
        let ev = wait(&rx, finished);
        assert!(!pages(&ev).is_empty() || matches!(ev.last(), Some(Event::ScanFinished { error: Some(_), .. })));
        drop(scanner);
    }
}
