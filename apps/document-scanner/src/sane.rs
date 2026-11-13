//! Minimal bindings to the SANE 1.0 C API (`libsane.so.1`) plus safe
//! wrappers. All calls must happen on a single thread (the scanner worker).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;

pub type Word = c_int;
pub type Status = c_int;
pub type Handle = *mut c_void;

pub const STATUS_GOOD: Status = 0;
pub const STATUS_CANCELLED: Status = 2;
pub const STATUS_EOF: Status = 5;
pub const STATUS_NO_DOCS: Status = 7;

pub const TYPE_FIXED: c_int = 2;

pub const CAP_SOFT_SELECT: c_int = 1 << 0;
pub const CAP_INACTIVE: c_int = 1 << 5;

const CONSTRAINT_RANGE: c_int = 1;
const CONSTRAINT_WORD_LIST: c_int = 2;
const CONSTRAINT_STRING_LIST: c_int = 3;

const ACTION_GET_VALUE: c_int = 0;
const ACTION_SET_VALUE: c_int = 1;

pub const FIXED_SCALE: f64 = 65536.0;

#[repr(C)]
struct RawDevice {
    name: *const c_char,
    vendor: *const c_char,
    model: *const c_char,
    kind: *const c_char,
}

#[repr(C)]
struct RawRange {
    min: Word,
    max: Word,
    quant: Word,
}

#[repr(C)]
struct RawOptionDescriptor {
    name: *const c_char,
    title: *const c_char,
    desc: *const c_char,
    kind: c_int,
    unit: c_int,
    size: c_int,
    cap: c_int,
    constraint_type: c_int,
    constraint: *const c_void,
}

#[repr(C)]
#[derive(Default)]
struct RawParameters {
    format: c_int,
    last_frame: Word,
    bytes_per_line: c_int,
    pixels_per_line: c_int,
    lines: c_int,
    depth: c_int,
}

type AuthCallback = Option<unsafe extern "C" fn(*const c_char, *mut c_char, *mut c_char)>;

#[link(name = "sane")]
extern "C" {
    fn sane_init(version: *mut Word, auth: AuthCallback) -> Status;
    fn sane_exit();
    fn sane_get_devices(list: *mut *mut *const RawDevice, local_only: Word) -> Status;
    fn sane_open(name: *const c_char, handle: *mut Handle) -> Status;
    fn sane_close(handle: Handle);
    fn sane_get_option_descriptor(handle: Handle, option: c_int) -> *const RawOptionDescriptor;
    fn sane_control_option(
        handle: Handle,
        option: c_int,
        action: c_int,
        value: *mut c_void,
        info: *mut Word,
    ) -> Status;
    fn sane_get_parameters(handle: Handle, params: *mut RawParameters) -> Status;
    fn sane_start(handle: Handle) -> Status;
    fn sane_read(handle: Handle, data: *mut u8, max_len: c_int, len: *mut c_int) -> Status;
    fn sane_cancel(handle: Handle);
    fn sane_strstatus(status: Status) -> *const c_char;
}

/// A SANE error status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub Status);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SAFETY: sane_strstatus returns a static string (or NULL).
        let s = unsafe { cstr(sane_strstatus(self.0)) };
        if s.is_empty() {
            write!(f, "SANE error {}", self.0)
        } else {
            f.write_str(&s)
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn check(s: Status) -> Result<()> {
    if s == STATUS_GOOD {
        Ok(())
    } else {
        Err(Error(s))
    }
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub vendor: String,
    pub model: String,
    pub kind: String,
}

impl Device {
    /// A human-readable label, e.g. "Canon LiDE 220".
    pub fn label(&self) -> String {
        let vendor = self.vendor.trim();
        let model = self.model.trim();
        if model.is_empty() {
            return self.name.clone();
        }
        if vendor.is_empty() || vendor.eq_ignore_ascii_case("noname") || model.starts_with(vendor) {
            model.to_string()
        } else {
            format!("{vendor} {model}")
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Constraint {
    None,
    Range { min: Word, max: Word, quant: Word },
    Words(Vec<Word>),
    Strings(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptionDesc {
    pub index: c_int,
    pub name: String,
    pub title: String,
    pub kind: c_int,
    pub unit: c_int,
    pub size: c_int,
    pub cap: c_int,
    pub constraint: Constraint,
}

impl OptionDesc {
    /// Active and software-settable.
    pub fn settable(&self) -> bool {
        self.cap & CAP_INACTIVE == 0 && self.cap & CAP_SOFT_SELECT != 0
    }

    pub fn is_fixed(&self) -> bool {
        self.kind == TYPE_FIXED
    }
}

/// The SANE library (initialised on creation, exited on drop).
pub struct Sane {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Sane {
    pub fn init() -> Result<Sane> {
        let mut version: Word = 0;
        // SAFETY: plain FFI call; no auth callback.
        check(unsafe { sane_init(&mut version, None) })?;
        Ok(Sane { _not_send: std::marker::PhantomData })
    }

    pub fn devices(&self, local_only: bool) -> Result<Vec<Device>> {
        let mut list: *mut *const RawDevice = ptr::null_mut();
        // SAFETY: SANE fills `list` with a NULL-terminated array that stays
        // valid until the next sane_get_devices/sane_exit call.
        unsafe {
            check(sane_get_devices(&mut list, local_only as Word))?;
            let mut out = Vec::new();
            if list.is_null() {
                return Ok(out);
            }
            let mut i = 0;
            loop {
                let d = *list.add(i);
                if d.is_null() {
                    break;
                }
                let d = &*d;
                out.push(Device {
                    name: cstr(d.name),
                    vendor: cstr(d.vendor),
                    model: cstr(d.model),
                    kind: cstr(d.kind),
                });
                i += 1;
            }
            Ok(out)
        }
    }

    pub fn open(&self, name: &str) -> Result<DeviceHandle<'_>> {
        let cname = CString::new(name).map_err(|_| Error(4))?;
        let mut h: Handle = ptr::null_mut();
        // SAFETY: valid C string and out-pointer.
        check(unsafe { sane_open(cname.as_ptr(), &mut h) })?;
        Ok(DeviceHandle { handle: h, _sane: self })
    }
}

impl Drop for Sane {
    fn drop(&mut self) {
        // SAFETY: matched with the successful sane_init in `init`.
        unsafe { sane_exit() }
    }
}

/// Parameters of the frame about to be read.
#[derive(Clone, Copy, Debug)]
pub struct Parameters {
    pub format: c_int,
    pub last_frame: bool,
    pub bytes_per_line: c_int,
    pub pixels_per_line: c_int,
    pub lines: c_int,
    pub depth: c_int,
}

/// An open device.
pub struct DeviceHandle<'a> {
    handle: Handle,
    _sane: &'a Sane,
}

impl DeviceHandle<'_> {
    pub fn option_count(&self) -> c_int {
        let mut n: Word = 0;
        // SAFETY: option 0 is always the integer option count.
        let s = unsafe {
            sane_control_option(self.handle, 0, ACTION_GET_VALUE, &mut n as *mut Word as *mut c_void, ptr::null_mut())
        };
        if s == STATUS_GOOD {
            n
        } else {
            0
        }
    }

    pub fn option(&self, index: c_int) -> Option<OptionDesc> {
        // SAFETY: the descriptor stays valid while the handle is open; we
        // copy everything out immediately.
        unsafe {
            let d = sane_get_option_descriptor(self.handle, index);
            if d.is_null() {
                return None;
            }
            let d = &*d;
            let constraint = match d.constraint_type {
                CONSTRAINT_RANGE if !d.constraint.is_null() => {
                    let r = &*(d.constraint as *const RawRange);
                    Constraint::Range { min: r.min, max: r.max, quant: r.quant }
                }
                CONSTRAINT_WORD_LIST if !d.constraint.is_null() => {
                    let w = d.constraint as *const Word;
                    let n = (*w).max(0) as usize;
                    Constraint::Words((1..=n).map(|i| *w.add(i)).collect())
                }
                CONSTRAINT_STRING_LIST if !d.constraint.is_null() => {
                    let l = d.constraint as *const *const c_char;
                    let mut v = Vec::new();
                    let mut i = 0;
                    while !(*l.add(i)).is_null() {
                        v.push(cstr(*l.add(i)));
                        i += 1;
                    }
                    Constraint::Strings(v)
                }
                _ => Constraint::None,
            };
            Some(OptionDesc {
                index,
                name: cstr(d.name),
                title: cstr(d.title),
                kind: d.kind,
                unit: d.unit,
                size: d.size,
                cap: d.cap,
                constraint,
            })
        }
    }

    pub fn options(&self) -> Vec<OptionDesc> {
        (1..self.option_count()).filter_map(|i| self.option(i)).collect()
    }

    pub fn get_word(&self, opt: &OptionDesc) -> Result<Word> {
        if opt.size < std::mem::size_of::<Word>() as c_int {
            return Err(Error(4));
        }
        let n = opt.size as usize / std::mem::size_of::<Word>();
        let mut buf = vec![0 as Word; n.max(1)];
        // SAFETY: buffer is at least `size` bytes.
        check(unsafe {
            sane_control_option(self.handle, opt.index, ACTION_GET_VALUE, buf.as_mut_ptr() as *mut c_void, ptr::null_mut())
        })?;
        Ok(buf[0])
    }

    /// Sets a word option (INT, FIXED or BOOL). Returns the info flags.
    pub fn set_word(&self, opt: &OptionDesc, value: Word) -> Result<Word> {
        let n = (opt.size.max(4) as usize) / std::mem::size_of::<Word>();
        let mut buf = vec![value; n];
        let mut info: Word = 0;
        // SAFETY: buffer is at least `size` bytes.
        check(unsafe {
            sane_control_option(self.handle, opt.index, ACTION_SET_VALUE, buf.as_mut_ptr() as *mut c_void, &mut info)
        })?;
        Ok(info)
    }

    pub fn set_string(&self, opt: &OptionDesc, value: &str) -> Result<Word> {
        let mut buf = vec![0u8; (opt.size.max(1) as usize).max(value.len() + 1)];
        buf[..value.len()].copy_from_slice(value.as_bytes());
        let mut info: Word = 0;
        // SAFETY: NUL-terminated buffer of at least `size` bytes.
        check(unsafe {
            sane_control_option(self.handle, opt.index, ACTION_SET_VALUE, buf.as_mut_ptr() as *mut c_void, &mut info)
        })?;
        Ok(info)
    }

    pub fn start(&self) -> Result<()> {
        // SAFETY: plain FFI call on an open handle.
        check(unsafe { sane_start(self.handle) })
    }

    pub fn parameters(&self) -> Result<Parameters> {
        let mut p = RawParameters::default();
        // SAFETY: out-pointer to a correctly laid out struct.
        check(unsafe { sane_get_parameters(self.handle, &mut p) })?;
        Ok(Parameters {
            format: p.format,
            last_frame: p.last_frame != 0,
            bytes_per_line: p.bytes_per_line,
            pixels_per_line: p.pixels_per_line,
            lines: p.lines,
            depth: p.depth,
        })
    }

    /// Reads up to `buf.len()` bytes; `Ok(0)` never signals EOF — EOF is
    /// returned as `Err(Error(STATUS_EOF))`.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let mut len: c_int = 0;
        let max = buf.len().min(c_int::MAX as usize) as c_int;
        // SAFETY: buffer of `max` bytes.
        check(unsafe { sane_read(self.handle, buf.as_mut_ptr(), max, &mut len) })?;
        Ok(len.max(0) as usize)
    }

    pub fn cancel(&self) {
        // SAFETY: valid open handle.
        unsafe { sane_cancel(self.handle) }
    }
}

impl Drop for DeviceHandle<'_> {
    fn drop(&mut self) {
        // SAFETY: handle came from a successful sane_open.
        unsafe { sane_close(self.handle) }
    }
}

pub fn to_fixed(v: f64) -> Word {
    (v * FIXED_SCALE).round() as Word
}

pub fn from_fixed(w: Word) -> f64 {
    w as f64 / FIXED_SCALE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let d = |vendor: &str, model: &str| Device {
            name: "x:1".into(),
            vendor: vendor.into(),
            model: model.into(),
            kind: "flatbed scanner".into(),
        };
        assert_eq!(d("Canon", "LiDE 220").label(), "Canon LiDE 220");
        assert_eq!(d("Noname", "frontend-tester").label(), "frontend-tester");
        assert_eq!(d("HP", "HP Envy 5000").label(), "HP Envy 5000");
        assert_eq!(d("HP", "").label(), "x:1");
    }

    #[test]
    fn fixed_point() {
        assert_eq!(to_fixed(1.0), 65536);
        assert!((from_fixed(to_fixed(215.9)) - 215.9).abs() < 1e-4);
    }
}
