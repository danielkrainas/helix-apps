//! Evolution Data Server client for Helix apps.
//!
//! EDS client libraries are not available, so this crate talks to the EDS
//! daemons directly over D-Bus (`org.gnome.evolution.dataserver.Sources5`,
//! `.AddressBook10`, `.Calendar8`) with zbus, and implements the vCard and
//! iCalendar formats it exchanges in pure Rust.
//!
//! * [`client`] — source registry, address books, calendars, live views.
//! * [`source`] / [`keyfile`] — source descriptions (key-file data).
//! * [`vcard`] — vCard parsing/serialization and the [`vcard::Contact`] model.
//! * [`ical`] / [`rrule`] — iCalendar, [`ical::Event`], time zones and
//!   recurrence expansion.
//! * [`sexp`] — EDS query expressions.

pub mod client;
pub mod contentline;
pub mod ical;
pub mod keyfile;
pub mod rrule;
pub mod sexp;
pub mod source;
pub mod vcard;

pub use client::{Book, Calendar, Eds, Error, ModType, Result, View, ViewEvent, Watch};
pub use ical::{Event, Instance, Time, Zones};
pub use source::{Source, SourceKind};
pub use vcard::Contact;

/// A random 40-hex-digit identifier (the shape EDS uses for UIDs).
pub fn new_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut out = String::with_capacity(40);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    for i in 0..3u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(nanos);
        h.write_u64(n);
        h.write_u64(i);
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out.truncate(40);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_are_unique_hex() {
        let a = super::new_id();
        let b = super::new_id();
        assert_eq!(a.len(), 40);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
