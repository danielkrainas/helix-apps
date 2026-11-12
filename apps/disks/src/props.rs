//! D-Bus-independent representation of UDisks2 objects.
//!
//! A [`Snapshot`] mirrors the result of `GetManagedObjects`:
//! `object path → interface → property → value`. The D-Bus layer converts
//! zvariant values into [`Prop`]; tests build snapshots by hand.

use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Bool(bool),
    U64(u64),
    I64(i64),
    F64(f64),
    Str(String),
    /// `ay` (device files, backing files – NUL terminated byte strings).
    Bytes(Vec<u8>),
    /// `aay` (mount points, symlinks).
    ByteStrings(Vec<Vec<u8>>),
    /// `o`
    Path(String),
    /// `ao`
    Paths(Vec<String>),
    /// `as`
    Strings(Vec<String>),
    Other,
}

pub type Props = HashMap<String, Prop>;
pub type Interfaces = HashMap<String, Props>;
pub type Snapshot = BTreeMap<String, Interfaces>;

fn bytes_to_string(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Typed accessors with UDisks-style defaults for missing properties.
pub trait PropsExt {
    fn prop(&self, key: &str) -> Option<&Prop>;

    fn str(&self, key: &str) -> String {
        match self.prop(key) {
            Some(Prop::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }
    fn u64(&self, key: &str) -> u64 {
        match self.prop(key) {
            Some(Prop::U64(v)) => *v,
            Some(Prop::I64(v)) if *v >= 0 => *v as u64,
            _ => 0,
        }
    }
    fn i64(&self, key: &str) -> i64 {
        match self.prop(key) {
            Some(Prop::I64(v)) => *v,
            Some(Prop::U64(v)) => *v as i64,
            _ => 0,
        }
    }
    fn f64(&self, key: &str) -> f64 {
        match self.prop(key) {
            Some(Prop::F64(v)) => *v,
            Some(Prop::U64(v)) => *v as f64,
            Some(Prop::I64(v)) => *v as f64,
            _ => 0.0,
        }
    }
    fn bool(&self, key: &str) -> bool {
        matches!(self.prop(key), Some(Prop::Bool(true)))
    }
    /// An object path, or `None` when missing or `/`.
    fn path(&self, key: &str) -> Option<String> {
        match self.prop(key) {
            Some(Prop::Path(p)) if p != "/" && !p.is_empty() => Some(p.clone()),
            _ => None,
        }
    }
    /// A NUL-terminated `ay` as a string.
    fn bytestring(&self, key: &str) -> String {
        match self.prop(key) {
            Some(Prop::Bytes(b)) => bytes_to_string(b),
            Some(Prop::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }
    fn bytestrings(&self, key: &str) -> Vec<String> {
        match self.prop(key) {
            Some(Prop::ByteStrings(v)) => v.iter().map(|b| bytes_to_string(b)).collect(),
            _ => Vec::new(),
        }
    }
}

impl PropsExt for Props {
    fn prop(&self, key: &str) -> Option<&Prop> {
        self.get(key)
    }
}

/// Builds a `Props` from `(key, value)` pairs (used by fixtures).
#[cfg(test)]
pub fn props<const N: usize>(pairs: [(&str, Prop); N]) -> Props {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// `Prop::Bytes` for a NUL-terminated string.
#[cfg(test)]
pub fn bytes(s: &str) -> Prop {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    Prop::Bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors() {
        let p = props([
            ("Device", bytes("/dev/sda")),
            ("Size", Prop::U64(42)),
            ("Drive", Prop::Path("/".into())),
            ("Table", Prop::Path("/org/x".into())),
            ("MountPoints", Prop::ByteStrings(vec![b"/mnt\0".to_vec()])),
            ("ReadOnly", Prop::Bool(true)),
        ]);
        assert_eq!(p.bytestring("Device"), "/dev/sda");
        assert_eq!(p.u64("Size"), 42);
        assert_eq!(p.path("Drive"), None);
        assert_eq!(p.path("Table").as_deref(), Some("/org/x"));
        assert_eq!(p.bytestrings("MountPoints"), vec!["/mnt".to_string()]);
        assert!(p.bool("ReadOnly"));
        assert!(!p.bool("Missing"));
        assert_eq!(p.str("Missing"), "");
    }
}
